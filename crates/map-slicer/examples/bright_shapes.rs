//! Lists the placed references of a worldspace whose primitives draw
//! nearly white: the mean of their diffuse texture (and vertex colour),
//! to find blank planes. Prints references, models and figures only.
//!
//! ```text
//! cargo run --release -p map-slicer --example bright_shapes -- <mir dir> <worldspace> [threshold 0-255]
//! ```

use std::collections::BTreeMap;
use std::path::Path;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mir = mir::Mir::open(Path::new(&args[1])).expect("mir");
    let worldspace = args[2].to_ascii_lowercase();
    let threshold: f64 = args.get(3).map_or(225.0, |t| t.parse().unwrap());
    let mut means: BTreeMap<String, Option<f64>> = BTreeMap::new();
    let mut seen: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for key in mir.cells() {
        if !key.worldspace.eq_ignore_ascii_case(&worldspace) {
            continue;
        }
        let Some(cell) = mir.cell(&worldspace, key.x, key.y).unwrap() else {
            continue;
        };
        for r in &cell.references {
            let (Some(hash), Some(model)) = (&r.mesh, &r.model) else {
                continue;
            };
            let lines = seen.entry(hash.clone()).or_insert_with(|| {
                let Ok(mesh) = mir.mesh(hash) else {
                    return Vec::new();
                };
                let mut out = Vec::new();
                for (i, p) in mesh.primitives.iter().enumerate() {
                    let tex = p.material.textures.first().cloned().unwrap_or_default();
                    let mean = means
                        .entry(tex.clone())
                        .or_insert_with(|| {
                            let h = mir.texture_hash(&tex)?;
                            let img = mir.texture(h).ok()?;
                            let n = (img.rgba.len() / 4).max(1);
                            let sum: f64 = img
                                .rgba
                                .chunks(4)
                                .map(|c| {
                                    (f64::from(c[0]) + f64::from(c[1]) + f64::from(c[2])) / 3.0
                                })
                                .sum();
                            Some(sum / n as f64)
                        })
                        .unwrap_or(255.0);
                    if mean >= threshold {
                        out.push(format!(
                            "prim {i} {:?} tex {tex:?} mean {mean:.0} alpha {:?} {} vertices",
                            p.name,
                            p.material.alpha_flags,
                            p.positions.len()
                        ));
                    }
                }
                out
            });
            if !lines.is_empty() {
                let p = r.transform.position;
                println!(
                    "{} {} {model} at ({:.0}, {:.0}, {:.0})",
                    r.form,
                    r.base_kind.as_deref().unwrap_or("?"),
                    p[0],
                    p[1],
                    p[2]
                );
                for l in lines.iter() {
                    println!("    {l}");
                }
            }
        }
    }
}
