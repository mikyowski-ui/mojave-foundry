//! The `wgpu` path: same preparation, same shading model, same resolve.
//!
//! The scene is drawn at supersampled resolution into an `Rgba8Unorm`
//! colour target and an `R32Float` target that receives linear depth, with
//! a `Depth32Float` attachment for the depth test. Large windows are drawn
//! in tiles of at most [`TILE`] sub-samples per side. The sub-samples are
//! read back and resolved by the CPU resolve code.
//!
//! Vertex positions are the CPU path's fixed-point positions, so both paths
//! rasterize the same snapped triangles; they differ in the hardware's fill
//! rule at exact ties, its texture filter precision, its depth-test
//! precision, and in draw order, which the GPU groups by texture.

use std::sync::{Arc, Mutex, mpsc};

use crate::cpu::{Shade, finish};
use crate::prepare::{Prepared, SUBSTEPS, prepare};
use crate::resolve::Samples;
use crate::{Backend, Calibration, Projection, RenderError, RenderOutput, Renderer, Scene, Window};

/// Largest tile side, in sub-samples.
const TILE: u32 = 2048;
/// Floats per vertex.
const VERTEX_FLOATS: usize = 16;
const COLOR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const DEPTH_OUT_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R32Float;
const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

/// How closely the GPU path matches the CPU reference.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GpuTolerance {
    /// Largest fraction of pixels whose opacity differs.
    pub silhouette_fraction: f64,
    /// Colour channel difference, out of 255, allowed on pixels opaque in
    /// both images.
    pub colour_levels: u8,
    /// Largest fraction of pixels opaque in both whose colour differs by
    /// more than `colour_levels` (texture-filter and edge outliers).
    pub colour_outlier_fraction: f64,
}

/// The agreement this crate asserts in its tests (`tests/gpu.rs`): opacity
/// differs on at most 0.5 % of pixels, and on at most 1 % of pixels opaque
/// in both images a colour channel differs by more than 3 / 255.
///
/// Measured on llvmpipe (software Vulkan, `wgpu` 27) at the default
/// calibration: a textured multi-part building with lit, unlit and
/// missing-texture instances (455 × 368 px) and a two-tile 700 × 520 px
/// window of a 19 000-triangle textured box agree on every pixel's opacity,
/// with colour within 1 / 255 and identical depth; a rotated two-sided
/// alpha-tested cut-out plus a mip-mapped one-texel checker (238 × 220 px)
/// differs in opacity on 0.004 % of pixels (two pixels at the cut-out
/// edge), colour within 1 / 255, depth within 0.002 m. The margin is for
/// hardware GPUs with coarser texture-filter weights.
pub const GPU_TOLERANCE: GpuTolerance = GpuTolerance {
    silhouette_fraction: 0.005,
    colour_levels: 3,
    colour_outlier_fraction: 0.01,
};

/// Measured differences between two renders of one window.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Agreement {
    /// Fraction of pixels whose opacity differs.
    pub silhouette_fraction: f64,
    /// Largest channel difference on pixels opaque in both.
    pub max_colour_difference: u8,
    /// Fraction of pixels opaque in both that differ by more than the
    /// tolerance's `colour_levels`.
    pub colour_outlier_fraction: f64,
    /// Largest depth difference on pixels opaque in both, in metres.
    pub max_depth_difference: f32,
}

impl GpuTolerance {
    /// Compares two renders of the same window; `None` when their windows
    /// differ.
    pub fn compare(&self, a: &RenderOutput, b: &RenderOutput) -> Option<Agreement> {
        if a.origin != b.origin || a.width != b.width || a.height != b.height {
            return None;
        }
        let pixels = (a.width as usize * a.height as usize).max(1);
        let (mut silhouette, mut both, mut outliers, mut max_colour) =
            (0usize, 0usize, 0usize, 0u8);
        let mut max_depth = 0.0f32;
        for (i, (p, q)) in a
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .zip(b.rgba.as_chunks::<4>().0)
            .enumerate()
        {
            match (p[3] != 0, q[3] != 0) {
                (true, true) => {
                    both += 1;
                    let d = (0..3).map(|k| p[k].abs_diff(q[k])).max().unwrap_or(0);
                    max_colour = max_colour.max(d);
                    if d > self.colour_levels {
                        outliers += 1;
                    }
                    max_depth = max_depth.max((a.depth[i] - b.depth[i]).abs());
                }
                (false, false) => {}
                _ => silhouette += 1,
            }
        }
        Some(Agreement {
            silhouette_fraction: silhouette as f64 / pixels as f64,
            max_colour_difference: max_colour,
            colour_outlier_fraction: outliers as f64 / both.max(1) as f64,
            max_depth_difference: max_depth,
        })
    }

    /// Whether an agreement is within this tolerance.
    pub fn accepts(&self, agreement: &Agreement) -> bool {
        agreement.silhouette_fraction <= self.silhouette_fraction
            && agreement.colour_outlier_fraction <= self.colour_outlier_fraction
    }
}

/// The GPU path. Construction fails cleanly when no adapter or device is
/// available.
pub struct GpuRenderer {
    calibration: Calibration,
    projection: Projection,
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::RenderPipeline,
    globals_layout: wgpu::BindGroupLayout,
    texture_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    adapter: String,
    tile: u32,
    errors: Arc<Mutex<Vec<String>>>,
}

impl std::fmt::Debug for GpuRenderer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GpuRenderer")
            .field("adapter", &self.adapter)
            .finish_non_exhaustive()
    }
}

impl GpuRenderer {
    /// Opens the first available adapter (any backend `wgpu` was built
    /// with, or those named by `WGPU_BACKEND`) and builds the pipeline.
    pub fn new(calibration: &Calibration) -> Result<Self, RenderError> {
        let projection = Projection::new(calibration)?;
        let backends = wgpu::Backends::from_env().unwrap_or(wgpu::Backends::PRIMARY);
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends,
            ..wgpu::InstanceDescriptor::default()
        });
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: None,
        }))
        .map_err(|e| RenderError::NoAdapter(e.to_string()))?;
        let info = adapter.get_info();
        let adapter_name = format!("{} ({:?})", info.name, info.backend);
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("foundry-render"),
            required_features: wgpu::Features::empty(),
            required_limits: adapter.limits(),
            ..wgpu::DeviceDescriptor::default()
        }))
        .map_err(|e| RenderError::NoAdapter(format!("{adapter_name}: {e}")))?;
        let errors: Arc<Mutex<Vec<String>>> = Arc::default();
        let sink = Arc::clone(&errors);
        device.on_uncaptured_error(Arc::new(move |e: wgpu::Error| {
            if let Ok(mut list) = sink.lock() {
                list.push(e.to_string());
            }
        }));
        let tile = TILE.min(device.limits().max_texture_dimension_2d);

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("foundry-render shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("render.wgsl").into()),
        });
        let globals_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("globals"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let texture_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("texture"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("foundry-render layout"),
            bind_group_layouts: &[&globals_layout, &texture_layout],
            push_constant_ranges: &[],
        });
        let attributes = wgpu::vertex_attr_array![
            0 => Float32x2, 1 => Float32, 2 => Float32, 3 => Float32x2,
            4 => Float32, 5 => Float32, 6 => Float32x4, 7 => Float32x3
        ];
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("foundry-render pipeline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: (VERTEX_FLOATS * 4) as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &attributes,
                }],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..wgpu::PrimitiveState::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::Less,
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[
                    Some(wgpu::ColorTargetState {
                        format: COLOR_FORMAT,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    }),
                    Some(wgpu::ColorTargetState {
                        format: DEPTH_OUT_FORMAT,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    }),
                ],
            }),
            multiview: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("repeat"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Nearest,
            ..wgpu::SamplerDescriptor::default()
        });
        let renderer = GpuRenderer {
            calibration: calibration.clone(),
            projection,
            device,
            queue,
            pipeline,
            globals_layout,
            texture_layout,
            sampler,
            adapter: adapter_name,
            tile,
            errors,
        };
        renderer.take_error()?;
        Ok(renderer)
    }

    /// The adapter's name and backend.
    pub fn adapter(&self) -> &str {
        &self.adapter
    }

    /// The projection in use.
    pub fn projection(&self) -> &Projection {
        &self.projection
    }

    fn take_error(&self) -> Result<(), RenderError> {
        let mut list = self
            .errors
            .lock()
            .map_err(|_| RenderError::Gpu("error sink poisoned".into()))?;
        if list.is_empty() {
            Ok(())
        } else {
            let message = list.join("; ");
            list.clear();
            Err(RenderError::Gpu(message))
        }
    }

    fn wait(&self) -> Result<(), RenderError> {
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .map(|_| ())
            .map_err(|e| RenderError::Gpu(e.to_string()))
    }

    fn draw(&self, prepared: &Prepared) -> Result<Samples, RenderError> {
        let ss = self.calibration.supersample;
        let rect = prepared.rect;
        let (ws, hs) = (rect.width * ss, rect.height * ss);
        let mut samples = Samples::new(ws as usize, hs as usize);
        if prepared.tris.is_empty() || ws == 0 || hs == 0 {
            return Ok(samples);
        }

        // Vertices grouped by texture, triangles in scene order within a
        // group.
        let mut order: Vec<u32> = (0..prepared.tris.len() as u32).collect();
        order.sort_by_key(|&t| prepared.tris[t as usize].texture);
        let origin = [
            i64::from(rect.x) * i64::from(ss) * SUBSTEPS,
            i64::from(rect.y) * i64::from(ss) * SUBSTEPS,
        ];
        let (mut dmin, mut dmax) = (f32::INFINITY, f32::NEG_INFINITY);
        for v in &prepared.vertices {
            dmin = dmin.min(v.depth);
            dmax = dmax.max(v.depth);
        }
        let range = (dmax - dmin).max(1.0e-6);
        let mut bytes = Vec::with_capacity(prepared.vertices.len() * VERTEX_FLOATS * 4);
        let mut groups: Vec<(u32, u32, u32)> = Vec::new(); // texture, first vertex, count
        for &t in &order {
            let tri = &prepared.tris[t as usize];
            let first = (bytes.len() / (VERTEX_FLOATS * 4)) as u32;
            match groups.last_mut() {
                Some(g) if g.0 == tri.texture => g.2 += 3,
                _ => groups.push((tri.texture, first, 3)),
            }
            for v in &prepared.vertices[3 * t as usize..3 * t as usize + 3] {
                let x = (v.fixed[0] - origin[0]) as f64 / SUBSTEPS as f64;
                let y = (v.fixed[1] - origin[1]) as f64 / SUBSTEPS as f64;
                let floats: [f32; VERTEX_FLOATS] = [
                    x as f32,
                    y as f32,
                    v.depth,
                    tri.level as f32,
                    v.uv[0],
                    v.uv[1],
                    tri.cutoff,
                    if tri.lit { 1.0 } else { 0.0 },
                    v.color[0],
                    v.color[1],
                    v.color[2],
                    v.color[3],
                    v.normal[0],
                    v.normal[1],
                    v.normal[2],
                    0.0,
                ];
                for f in floats {
                    bytes.extend_from_slice(&f.to_ne_bytes());
                }
            }
        }
        if bytes.len() as u64 > self.device.limits().max_buffer_size {
            return Err(RenderError::Gpu(format!(
                "vertex data of {} bytes exceeds the device limit",
                bytes.len()
            )));
        }
        let vertex_buffer = self.buffer(&bytes, wgpu::BufferUsages::VERTEX);

        // Textures actually used.
        let max_side = self.device.limits().max_texture_dimension_2d;
        if let Some(chain) = groups
            .iter()
            .map(|g| &prepared.textures[g.0 as usize])
            .find(|c| c.levels[0].width > max_side || c.levels[0].height > max_side)
        {
            let base = &chain.levels[0];
            return Err(RenderError::Gpu(format!(
                "texture {} x {} exceeds the device limit of {max_side}",
                base.width, base.height
            )));
        }
        let mut bind_groups: Vec<Option<wgpu::BindGroup>> = vec![None; prepared.textures.len()];
        for &(texture, _, _) in &groups {
            let slot = &mut bind_groups[texture as usize];
            if slot.is_none() {
                *slot = Some(self.upload_texture(&prepared.textures[texture as usize]));
            }
        }

        let shade = Shade::new(&self.calibration);
        let globals = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("globals"),
            size: 48,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let globals_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("globals"),
            layout: &self.globals_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: globals.as_entire_binding(),
            }],
        });

        let (tw, th) = (ws.min(self.tile), hs.min(self.tile));
        let target = |format, label| {
            self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: tw,
                    height: th,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            })
        };
        let color = target(COLOR_FORMAT, "colour");
        let depth_out = target(DEPTH_OUT_FORMAT, "linear depth");
        let depth = target(DEPTH_FORMAT, "depth test");
        let (color_view, depth_out_view, depth_view) = (
            color.create_view(&wgpu::TextureViewDescriptor::default()),
            depth_out.create_view(&wgpu::TextureViewDescriptor::default()),
            depth.create_view(&wgpu::TextureViewDescriptor::default()),
        );
        let row_bytes = (tw * 4).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
            * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let readback = |label| {
            self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: u64::from(row_bytes) * u64::from(th),
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            })
        };
        let color_read = readback("colour readback");
        let depth_read = readback("depth readback");

        for y0 in (0..hs).step_by(self.tile as usize) {
            for x0 in (0..ws).step_by(self.tile as usize) {
                let (w, h) = ((ws - x0).min(tw), (hs - y0).min(th));
                let uniforms: [f32; 12] = [
                    x0 as f32,
                    y0 as f32,
                    w as f32,
                    h as f32,
                    dmin,
                    0.5 / range,
                    shade.ambient,
                    shade.intensity,
                    shade.light[0],
                    shade.light[1],
                    shade.light[2],
                    0.0,
                ];
                let uniform_bytes: Vec<u8> =
                    uniforms.iter().flat_map(|f| f.to_ne_bytes()).collect();
                self.queue.write_buffer(&globals, 0, &uniform_bytes);
                let mut encoder =
                    self.device
                        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                            label: Some("tile"),
                        });
                {
                    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("tile"),
                        color_attachments: &[
                            Some(wgpu::RenderPassColorAttachment {
                                view: &color_view,
                                depth_slice: None,
                                resolve_target: None,
                                ops: wgpu::Operations {
                                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                                    store: wgpu::StoreOp::Store,
                                },
                            }),
                            Some(wgpu::RenderPassColorAttachment {
                                view: &depth_out_view,
                                depth_slice: None,
                                resolve_target: None,
                                ops: wgpu::Operations {
                                    load: wgpu::LoadOp::Clear(wgpu::Color {
                                        r: f64::from(f32::MAX),
                                        g: 0.0,
                                        b: 0.0,
                                        a: 0.0,
                                    }),
                                    store: wgpu::StoreOp::Store,
                                },
                            }),
                        ],
                        depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                            view: &depth_view,
                            depth_ops: Some(wgpu::Operations {
                                load: wgpu::LoadOp::Clear(1.0),
                                store: wgpu::StoreOp::Store,
                            }),
                            stencil_ops: None,
                        }),
                        timestamp_writes: None,
                        occlusion_query_set: None,
                    });
                    pass.set_viewport(0.0, 0.0, w as f32, h as f32, 0.0, 1.0);
                    pass.set_scissor_rect(0, 0, w, h);
                    pass.set_pipeline(&self.pipeline);
                    pass.set_bind_group(0, &globals_group, &[]);
                    pass.set_vertex_buffer(0, vertex_buffer.slice(..));
                    for &(texture, first, count) in &groups {
                        if let Some(group) = &bind_groups[texture as usize] {
                            pass.set_bind_group(1, group, &[]);
                            pass.draw(first..first + count, 0..1);
                        }
                    }
                }
                let extent = wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                };
                for (texture, buffer) in [(&color, &color_read), (&depth_out, &depth_read)] {
                    encoder.copy_texture_to_buffer(
                        wgpu::TexelCopyTextureInfo {
                            texture,
                            mip_level: 0,
                            origin: wgpu::Origin3d::ZERO,
                            aspect: wgpu::TextureAspect::All,
                        },
                        wgpu::TexelCopyBufferInfo {
                            buffer,
                            layout: wgpu::TexelCopyBufferLayout {
                                offset: 0,
                                bytes_per_row: Some(row_bytes),
                                rows_per_image: Some(h),
                            },
                        },
                        extent,
                    );
                }
                self.queue.submit([encoder.finish()]);
                let color_bytes = self.read(&color_read)?;
                let depth_bytes = self.read(&depth_read)?;
                for row in 0..h as usize {
                    let src = row * row_bytes as usize;
                    let dst = (y0 as usize + row) * ws as usize + x0 as usize;
                    for col in 0..w as usize {
                        let c = &color_bytes[src + col * 4..src + col * 4 + 4];
                        let covered = c[3] >= 128;
                        samples.color[dst + col] = [c[0], c[1], c[2], c[3]];
                        let d = &depth_bytes[src + col * 4..src + col * 4 + 4];
                        let d = f32::from_ne_bytes([d[0], d[1], d[2], d[3]]);
                        samples.depth[dst + col] = if covered { d } else { f32::INFINITY };
                    }
                }
                self.take_error()?;
            }
        }
        Ok(samples)
    }

    fn buffer(&self, bytes: &[u8], usage: wgpu::BufferUsages) -> wgpu::Buffer {
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("vertices"),
            size: (bytes.len() as u64)
                .max(4)
                .next_multiple_of(wgpu::COPY_BUFFER_ALIGNMENT),
            usage: usage | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let padded = bytes
            .len()
            .next_multiple_of(wgpu::COPY_BUFFER_ALIGNMENT as usize);
        if padded == bytes.len() {
            self.queue.write_buffer(&buffer, 0, bytes);
        } else {
            let mut copy = bytes.to_vec();
            copy.resize(padded, 0);
            self.queue.write_buffer(&buffer, 0, &copy);
        }
        buffer
    }

    fn upload_texture(&self, chain: &crate::texture::MipChain) -> wgpu::BindGroup {
        let base = &chain.levels[0];
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("diffuse"),
            size: wgpu::Extent3d {
                width: base.width,
                height: base.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: chain.levels.len() as u32,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: COLOR_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        for (level, data) in chain.levels.iter().enumerate() {
            self.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &texture,
                    mip_level: level as u32,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                &data.rgba,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(data.width * 4),
                    rows_per_image: Some(data.height),
                },
                wgpu::Extent3d {
                    width: data.width,
                    height: data.height,
                    depth_or_array_layers: 1,
                },
            );
        }
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("diffuse"),
            layout: &self.texture_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        })
    }

    fn read(&self, buffer: &wgpu::Buffer) -> Result<Vec<u8>, RenderError> {
        let slice = buffer.slice(..);
        let (tx, rx) = mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });
        self.wait()?;
        rx.recv()
            .map_err(|e| RenderError::Gpu(e.to_string()))?
            .map_err(|e| RenderError::Gpu(e.to_string()))?;
        let bytes = slice.get_mapped_range().to_vec();
        buffer.unmap();
        Ok(bytes)
    }
}

impl Renderer for GpuRenderer {
    fn render(&mut self, scene: &Scene, window: Window) -> Result<RenderOutput, RenderError> {
        let prepared = prepare(&self.calibration, &self.projection, scene, window)?;
        let samples = self.draw(&prepared)?;
        Ok(finish(&self.calibration, prepared, &samples, Backend::Gpu))
    }

    fn backend(&self) -> Backend {
        Backend::Gpu
    }
}
