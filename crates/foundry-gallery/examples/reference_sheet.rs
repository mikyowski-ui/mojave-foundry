//! Contact sheets of original Fallout 2 scenery and wall art from the
//! owner's own install, to check the reference-art path on real files.
//!
//! ```text
//! FALLOUT2_DIR=/path/to/fallout2 cargo run -p foundry-gallery --example reference_sheet
//! ```
//!
//! `FALLOUT2_DIR` holds `master.dat`. Sheets go to `GALLERY_OUT` (default
//! `out/gallery` at the workspace root), which must sit under the
//! workspace's ignored `out/` directory. `GALLERY_COUNT` (default 24) is the
//! number of FRMs sampled per category. The program prints only size
//! statistics; it writes no art anywhere else.

use std::path::{Path, PathBuf};

use foundry_gallery::{
    Columns, SheetOptions, Tile, contact_sheets, frm_frame_rgba, sprite_metrics,
};
use vault_formats::{Dat2Archive, FrmArchive, Palette, Vfs};

fn main() {
    if let Err(message) = run() {
        eprintln!("reference_sheet: {message}");
        std::process::exit(1);
    }
}

struct Sprite {
    name: String,
    rgba: Vec<u8>,
    width: u32,
    height: u32,
    metrics: Vec<(String, String)>,
}

fn run() -> Result<(), String> {
    let install = std::env::var_os("FALLOUT2_DIR")
        .map(PathBuf::from)
        .ok_or("set FALLOUT2_DIR to the Fallout 2 install directory")?;
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let workspace = workspace
        .canonicalize()
        .map_err(|e| format!("workspace: {e}"))?;
    let out_root = workspace.join("out");
    let out = std::env::var_os("GALLERY_OUT")
        .map(PathBuf::from)
        .unwrap_or_else(|| out_root.join("gallery"));
    std::fs::create_dir_all(&out).map_err(|e| format!("create output directory: {e}"))?;
    let out = out.canonicalize().map_err(|e| format!("output: {e}"))?;
    if !out.starts_with(&out_root) {
        return Err("GALLERY_OUT must sit under the workspace's out/ directory".to_owned());
    }
    let count: usize = std::env::var("GALLERY_COUNT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(24);

    let dat =
        Dat2Archive::open(install.join("master.dat")).map_err(|e| format!("master.dat: {e}"))?;
    let palette = Palette::parse(&dat.read("color.pal").map_err(|e| e.to_string())?)
        .map_err(|e| format!("color.pal: {e}"))?;

    for category in ["scenery", "walls"] {
        let prefix = format!("art/{category}/");
        let names: Vec<String> = dat
            .files()
            .map(|path| path.as_str().to_owned())
            .filter(|path| path.starts_with(&prefix) && path.ends_with(".frm"))
            .collect();
        let mut widths = Vec::new();
        let mut heights = Vec::new();
        let mut frames = Vec::new();
        let mut sampled = Vec::new();
        let step = (names.len() / count.max(1)).max(1);
        for (i, name) in names.iter().enumerate() {
            let Ok(bytes) = dat.read(name) else { continue };
            let Ok(frm) = FrmArchive::parse(&bytes) else {
                continue;
            };
            let Some((rgba, width, height)) = frm_frame_rgba(&frm, 0, 0, &palette) else {
                continue;
            };
            widths.push(width);
            heights.push(height);
            frames.push(frm.frame_count() as u32);
            if i % step == 0 && sampled.len() < count {
                let mut metrics = sprite_metrics(&rgba, width, height)
                    .map_err(|e| e.to_string())?
                    .lines();
                metrics.push(("frames".to_owned(), frm.frame_count().to_string()));
                sampled.push(Sprite {
                    name: name[prefix.len()..].to_owned(),
                    rgba,
                    width,
                    height,
                    metrics,
                });
            }
        }
        println!(
            "{category}: {} FRMs parsed of {}",
            widths.len(),
            names.len()
        );
        print_stats("  width ", &mut widths);
        print_stats("  height", &mut heights);
        print_stats("  frames", &mut frames);

        let tiles: Vec<Tile> = sampled
            .iter()
            .map(|sprite| Tile {
                label: &sprite.name,
                metrics: &sprite.metrics,
                rgba: &sprite.rgba,
                width: sprite.width,
                height: sprite.height,
                compare: None,
            })
            .collect();
        let options = SheetOptions {
            columns: Columns::Auto { sheet_width: 1800 },
            title: Some(format!("Fallout 2 {category}, frame 0")),
            max_cell: [360, 360],
            ..SheetOptions::default()
        };
        let pages = contact_sheets(&tiles, &options).map_err(|e| e.to_string())?;
        for (page, sheet) in pages.iter().enumerate() {
            let path = out.join(format!("{category}-{page:02}.png"));
            let png = sheet.to_png().map_err(|e| e.to_string())?;
            std::fs::write(&path, png).map_err(|e| format!("{}: {e}", path.display()))?;
            println!(
                "  wrote {category}-{page:02}.png ({}x{})",
                sheet.width, sheet.height
            );
        }
    }
    Ok(())
}

fn print_stats(label: &str, values: &mut [u32]) {
    if values.is_empty() {
        println!("{label}: none");
        return;
    }
    values.sort_unstable();
    let pick = |q: usize| values[(values.len() - 1) * q / 100];
    println!(
        "{label}: min {} p10 {} median {} p90 {} max {}",
        values[0],
        pick(10),
        pick(50),
        pick(90),
        values[values.len() - 1]
    );
}
