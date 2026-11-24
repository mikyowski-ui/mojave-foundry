//! Prints the primitives of the meshes of a real MIR directory whose model
//! path contains a text: material, triangles, surface, and how much of the
//! diffuse texture passes the alpha test, to see why a prop renders as it
//! does.
//!
//! ```text
//! SLICER_MIR=out/<slice>/mir SLICER_MODEL=shoppingcart \
//!     cargo run --release -p map-slicer --example mesh_dump
//! ```

use std::collections::BTreeSet;
use std::path::PathBuf;

use map_slicer::{Assets, MirAssets};
use mir::Mir;

fn main() {
    if let Err(message) = run() {
        eprintln!("mesh_dump: {message}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mir_dir = PathBuf::from(std::env::var("SLICER_MIR").map_err(|_| "set SLICER_MIR")?);
    let text = std::env::var("SLICER_MODEL")
        .map_err(|_| "set SLICER_MODEL")?
        .to_ascii_lowercase();
    let mir = Mir::open(&mir_dir).map_err(|e| e.to_string())?;
    let assets = MirAssets::new(&mir, 64 << 20);
    let mut docs = Vec::new();
    for key in mir.cells().collect::<Vec<_>>() {
        if let Ok(Some(doc)) = mir.cell(&key.worldspace, key.x, key.y) {
            docs.push(doc);
        }
    }
    let ids: Vec<String> = mir.interiors().map(|e| e.id.clone()).collect();
    for id in ids {
        if let Ok(Some(doc)) = mir.interior(&id) {
            docs.push(doc);
        }
    }
    let mut seen = BTreeSet::new();
    for doc in &docs {
        for r in &doc.references {
            let model = r.model.clone().unwrap_or_default().to_ascii_lowercase();
            let Some(hash) = &r.mesh else { continue };
            if !model.contains(&text) || !seen.insert(hash.clone()) {
                continue;
            }
            let mesh = assets.mesh(hash)?;
            println!("{model} ({} primitives)", mesh.primitives.len());
            for p in &mesh.primitives {
                let tris = p.indices.len() / 3;
                let mut area = 0.0f64;
                for t in p.indices.as_chunks::<3>().0 {
                    let [a, b, c] = t.map(|i| p.positions[i as usize].map(f64::from));
                    let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
                    let w = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
                    let n = [
                        u[1] * w[2] - u[2] * w[1],
                        u[2] * w[0] - u[0] * w[2],
                        u[0] * w[1] - u[1] * w[0],
                    ];
                    area += (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt() / 2.0;
                }
                let m = &p.material;
                if let Some(uv) = &p.uvs {
                    let (mut lo, mut hi) = ([f32::MAX; 2], [f32::MIN; 2]);
                    let mut texels_per_unit = Vec::new();
                    for t in p.indices.as_chunks::<3>().0 {
                        let [a, b, c] = t.map(|i| uv[i as usize]);
                        let [pa, pb, pc] = t.map(|i| p.positions[i as usize]);
                        let uv_area =
                            ((b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])).abs()
                                / 2.0;
                        let u = [pb[0] - pa[0], pb[1] - pa[1], pb[2] - pa[2]];
                        let w = [pc[0] - pa[0], pc[1] - pa[1], pc[2] - pa[2]];
                        let n = [
                            u[1] * w[2] - u[2] * w[1],
                            u[2] * w[0] - u[0] * w[2],
                            u[0] * w[1] - u[1] * w[0],
                        ];
                        let area = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt() / 2.0;
                        if area > 0.0 {
                            texels_per_unit.push((uv_area / area).sqrt());
                        }
                    }
                    for v in uv {
                        for k in 0..2 {
                            lo[k] = lo[k].min(v[k]);
                            hi[k] = hi[k].max(v[k]);
                        }
                    }
                    texels_per_unit.sort_by(f32::total_cmp);
                    println!(
                        "    uv {lo:?}..{hi:?}, texture side per unit (median) {:.4}",
                        texels_per_unit
                            .get(texels_per_unit.len() / 2)
                            .copied()
                            .unwrap_or(0.0)
                    );
                }
                let diffuse = m.textures.first().cloned().unwrap_or_default();
                let pass = assets.textures().texture(&diffuse).map(|t| {
                    let (w, h) = (t.width as usize, t.height as usize);
                    let shares: Vec<String> = [1usize, 2, 4, 8, 16, 32]
                        .iter()
                        .map(|&b| {
                            let (bw, bh) = (w.div_ceil(b), h.div_ceil(b));
                            let mut pass = 0;
                            for by in 0..bh {
                                for bx in 0..bw {
                                    let (mut sum, mut n) = (0u64, 0u64);
                                    for y in by * b..((by + 1) * b).min(h) {
                                        for x in bx * b..((bx + 1) * b).min(w) {
                                            sum += u64::from(t.rgba[(y * w + x) * 4 + 3]);
                                            n += 1;
                                        }
                                    }
                                    if sum >= 128 * n {
                                        pass += 1;
                                    }
                                }
                            }
                            format!("{b}:{:.0}%", 100.0 * pass as f64 / (bw * bh) as f64)
                        })
                        .collect();
                    format!(
                        "{}x{}, texels passing 0.5 by block {}",
                        t.width,
                        t.height,
                        shares.join(" ")
                    )
                });
                println!(
                    "  {tris:>5} tris, area {area:>9.0}, alpha {:?}, two-sided {}, opacity {}, \
                     flags {:?}, shader {:?}\n        {diffuse} ({})",
                    m.alpha,
                    m.two_sided,
                    m.opacity,
                    m.alpha_flags,
                    m.shader.as_ref().map(|s| format!("{s:?}")),
                    pass.unwrap_or_else(|| "no texture".into())
                );
            }
        }
    }
    Ok(())
}
