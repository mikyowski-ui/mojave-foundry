//! Puts crops of several pictures side by side with a label over each, to
//! compare a run before and after a change.
//!
//! ```text
//! cargo run --release -p map-slicer --example pairs -- <out.png> <x0,y0,x1,y1> <zoom> \
//!     <label>=<picture.png> ...
//! ```
//!
//! The crop is in fractions of each picture; `zoom` draws every pixel
//! `n x n`. The output must sit under the workspace's ignored `out/`.

use std::path::{Path, PathBuf};

use foundry_gallery::{Canvas, decode_png, draw_text, encode_png};

fn main() {
    if let Err(message) = run() {
        eprintln!("pairs: {message}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 4 {
        return Err("pairs <out.png> <x0,y0,x1,y1> <zoom> <label>=<png> ...".into());
    }
    let out = PathBuf::from(&args[0]);
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .map_err(|e| e.to_string())?;
    let parent = out
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
        .canonicalize()
        .map_err(|e| e.to_string())?;
    if !parent.starts_with(workspace.join("out")) {
        return Err("the output must sit under the workspace's out/".into());
    }
    let crop: Vec<f64> = args[1]
        .split(',')
        .map(|v| v.trim().parse::<f64>().map_err(|e| e.to_string()))
        .collect::<Result<_, _>>()?;
    let [x0, y0, x1, y1] = <[f64; 4]>::try_from(crop).map_err(|_| "crop is x0,y0,x1,y1")?;
    let zoom: u32 = args[2].parse().map_err(|e| format!("zoom: {e}"))?;
    let zoom = zoom.clamp(1, 8);
    let mut tiles = Vec::new();
    for arg in &args[3..] {
        let (label, path) = arg.split_once('=').ok_or("each picture is label=path")?;
        let bytes = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
        let image = decode_png(&bytes).map_err(|e| format!("{path}: {e}"))?;
        let (w, h) = (f64::from(image.width), f64::from(image.height));
        let (cx, cy) = ((w * x0) as u32, (h * y0) as u32);
        let cw = ((w * (x1 - x0)) as u32).min(image.width - cx).max(1);
        let ch = ((h * (y1 - y0)) as u32).min(image.height - cy).max(1);
        tiles.push((label.to_owned(), image, [cx, cy, cw, ch]));
    }
    let label_h = 16u32;
    let width: u32 = tiles.iter().map(|t| t.2[2] * zoom + 8).sum::<u32>() + 8;
    let height: u32 = tiles.iter().map(|t| t.2[3] * zoom).max().unwrap_or(1) + label_h + 16;
    let mut rgba = [16u8, 18, 22, 255].repeat((width * height) as usize);
    let mut x = 8u32;
    for (label, image, [cx, cy, cw, ch]) in &tiles {
        for j in 0..ch * zoom {
            for i in 0..cw * zoom {
                let s = (((cy + j / zoom) * image.width + cx + i / zoom) * 4) as usize;
                let d = (((label_h + 8 + j) * width + x + i) * 4) as usize;
                rgba[d..d + 4].copy_from_slice(&image.rgba[s..s + 4]);
            }
        }
        let mut canvas = Canvas::new(&mut rgba, width, height).map_err(|e| e.to_string())?;
        draw_text(&mut canvas, x as i32, 4, label, [230, 230, 230, 255], 2);
        x += cw * zoom + 8;
    }
    let png = encode_png(&rgba, width, height).map_err(|e| e.to_string())?;
    std::fs::write(&out, png).map_err(|e| format!("{}: {e}", out.display()))
}
