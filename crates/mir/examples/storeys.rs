//! Prints the storeys of cell documents.
//!
//! ```text
//! cargo run -p mir --example storeys -- [--gap UNITS] <cell.json>...
//! ```

use std::process::ExitCode;

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let mut gap = mir::DEFAULT_STOREY_GAP;
    let mut paths = Vec::new();
    while let Some(arg) = args.next() {
        if arg == "--gap" {
            match args.next().and_then(|v| v.parse().ok()) {
                Some(v) => gap = v,
                None => {
                    eprintln!("--gap needs a number");
                    return ExitCode::FAILURE;
                }
            }
        } else {
            paths.push(arg);
        }
    }
    if paths.is_empty() {
        eprintln!("usage: storeys [--gap UNITS] <cell.json>...");
        return ExitCode::FAILURE;
    }
    for path in paths {
        let doc: mir::CellDoc = match std::fs::read(&path)
            .map_err(|e| e.to_string())
            .and_then(|bytes| serde_json::from_slice(&bytes).map_err(|e| e.to_string()))
        {
            Ok(doc) => doc,
            Err(e) => {
                eprintln!("{path}: {e}");
                return ExitCode::FAILURE;
            }
        };
        let triangles: usize = doc.navmeshes.iter().map(|m| m.triangles.len()).sum();
        println!(
            "{path}: interior {}, {} navmeshes, {triangles} triangles, gap {gap}",
            doc.is_interior(),
            doc.navmeshes.len()
        );
        for (i, s) in mir::storeys_with_gap(&doc, gap).iter().enumerate() {
            println!(
                "  {i}: floor z {:.1}, z {:.1}..{:.1}, area {:.0}, {} triangles, \
                 x {:.0}..{:.0}, y {:.0}..{:.0}",
                s.floor_z,
                s.z_range[0],
                s.z_range[1],
                s.area,
                s.triangles,
                s.min[0],
                s.max[0],
                s.min[1],
                s.max[1]
            );
        }
    }
    ExitCode::SUCCESS
}
