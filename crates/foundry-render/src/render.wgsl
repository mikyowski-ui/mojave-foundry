// The GPU path of foundry-render: the shading model of the CPU reference,
// one supersampled tile at a time. Positions arrive in sub-sample units
// relative to the window; depth arrives in metres and is written out
// linearly to the second colour target.

struct Globals {
    // x, y: this tile's offset in sub-samples; z, w: its size.
    tile: vec4<f32>,
    // x: depth minimum, y: depth scale into [0.25, 0.75], z: ambient,
    // w: key light intensity.
    depth: vec4<f32>,
    // xyz: unit direction towards the key light, map axes.
    light: vec4<f32>,
}

@group(0) @binding(0) var<uniform> g: Globals;
@group(1) @binding(0) var tex: texture_2d<f32>;
@group(1) @binding(1) var samp: sampler;

struct VsIn {
    @location(0) pos: vec2<f32>,
    @location(1) depth: f32,
    @location(2) level: f32,
    @location(3) uv: vec2<f32>,
    @location(4) cutoff: f32,
    @location(5) lit: f32,
    @location(6) color: vec4<f32>,
    @location(7) normal: vec3<f32>,
}

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) depth: f32,
    @location(1) @interpolate(flat) level: f32,
    @location(2) uv: vec2<f32>,
    @location(3) @interpolate(flat) cutoff: f32,
    @location(4) @interpolate(flat) lit: f32,
    @location(5) color: vec4<f32>,
    @location(6) normal: vec3<f32>,
}

@vertex
fn vs_main(v: VsIn) -> VsOut {
    var out: VsOut;
    let p = v.pos - g.tile.xy;
    let ndc = vec2<f32>(p.x / g.tile.z * 2.0 - 1.0, 1.0 - p.y / g.tile.w * 2.0);
    let z = (v.depth - g.depth.x) * g.depth.y + 0.25;
    out.pos = vec4<f32>(ndc, z, 1.0);
    out.depth = v.depth;
    out.level = v.level;
    out.uv = v.uv;
    out.cutoff = v.cutoff;
    out.lit = v.lit;
    out.color = v.color;
    out.normal = v.normal;
    return out;
}

struct FsOut {
    @location(0) color: vec4<f32>,
    @location(1) depth: f32,
}

@fragment
fn fs_main(in: VsOut) -> FsOut {
    let texel = textureSampleLevel(tex, samp, in.uv, in.level);
    let c = texel * in.color;
    if (in.cutoff >= 0.0 && c.a < in.cutoff) {
        discard;
    }
    var factor = 1.0;
    if (in.lit > 0.5) {
        let n = in.normal;
        let inv = inverseSqrt(max(dot(n, n), 1e-20));
        factor = g.depth.z + g.depth.w * max(dot(n, g.light.xyz) * inv, 0.0);
    }
    var out: FsOut;
    out.color = vec4<f32>(clamp(c.rgb * factor, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
    out.depth = in.depth;
    return out;
}
