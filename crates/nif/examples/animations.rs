//! Real-data survey of the human animation files of one game: every `.kf`
//! under a folder of the archives named by `MOJAVE_BSA` (comma separated,
//! searched in order, the first holding a path wins), with its length,
//! loop flag, text keys and root travel. Prints file names and timings
//! only; it writes nothing.
//!
//! ```text
//! MOJAVE_BSA="<data>/Fallout - Meshes.bsa" \
//! MOJAVE_PREFIX='meshes\characters\_male\' \
//!   cargo run -p nif --release --example animations
//! ```
//!
//! `MOJAVE_PREFIX` defaults to the human folder; `MOJAVE_KEYS=0` hides the
//! text keys; `MOJAVE_LIST=nif` only lists the files with that extension.

use std::collections::BTreeMap;
use std::path::Path;

use fnv_bsa::BsaArchive;
use nif::{Animation, parse_animation};

fn main() {
    let archives = std::env::var("MOJAVE_BSA").expect("MOJAVE_BSA names the archives");
    let prefix = std::env::var("MOJAVE_PREFIX")
        .unwrap_or_else(|_| "meshes\\characters\\_male\\".to_owned())
        .to_ascii_lowercase();
    let keys = std::env::var("MOJAVE_KEYS").map_or(true, |v| v != "0");
    // `MOJAVE_LIST=nif` lists the `.nif` files under the prefix instead
    // (names only), to find weapon and creature models.
    if let Ok(extension) = std::env::var("MOJAVE_LIST") {
        let suffix = format!(".{extension}");
        for path in archives.split(',').filter(|p| !p.trim().is_empty()) {
            let archive = BsaArchive::open(Path::new(path.trim())).expect("open archive");
            for entry in archive.entries() {
                let name = entry.path.to_ascii_lowercase();
                if name.starts_with(&prefix) && name.ends_with(&suffix) {
                    println!("{name}");
                }
            }
        }
        return;
    }
    let mut files: BTreeMap<String, (usize, u64)> = BTreeMap::new();
    let opened: Vec<BsaArchive> = archives
        .split(',')
        .filter(|p| !p.trim().is_empty())
        .map(|p| BsaArchive::open(Path::new(p.trim())).expect("open archive"))
        .collect();
    for (index, archive) in opened.iter().enumerate() {
        for entry in archive.entries() {
            let path = entry.path.to_ascii_lowercase();
            if path.starts_with(&prefix) && path.ends_with(".kf") {
                files
                    .entry(path)
                    .or_insert((index, u64::from(entry.stored_size)));
            }
        }
    }
    let mut by_folder: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for path in files.keys() {
        let (folder, name) = path.rsplit_once('\\').unwrap_or(("", path));
        by_folder
            .entry(folder.to_owned())
            .or_default()
            .push(name.to_owned());
    }
    println!("{} files under {prefix}", files.len());
    for (folder, names) in &by_folder {
        println!("\n[{folder}] {} files", names.len());
        for name in names {
            let path = format!("{folder}\\{name}");
            let (index, _) = files[&path];
            let archive = &opened[index];
            let Some(entry) = archive.find(&path) else {
                println!("  {name}: not found");
                continue;
            };
            let bytes = match archive.read(entry) {
                Ok(b) => b,
                Err(e) => {
                    println!("  {name}: read: {e}");
                    continue;
                }
            };
            match parse_animation(&bytes) {
                Ok(animation) => println!("  {name}: {}", describe(&animation, keys)),
                Err(e) => println!("  {name}: refused: {e}"),
            }
        }
    }
}

fn describe(animation: &Animation, keys: bool) -> String {
    let root = animation.track(&animation.accum_root);
    let travel = root
        .and_then(|t| {
            let a = t.translation_at(0.0)?;
            let b = t.translation_at(animation.duration)?;
            Some([b[0] - a[0], b[1] - a[1], b[2] - a[2]])
        })
        .map_or_else(
            || "-".to_owned(),
            |d| format!("{:.0},{:.0},{:.0}", d[0], d[1], d[2]),
        );
    let mut text = format!(
        "{:.2}s {} tracks={} root={} travel={travel}",
        animation.duration,
        if animation.looping { "loop" } else { "once" },
        animation.tracks.len(),
        if animation.accum_root.is_empty() {
            "-"
        } else {
            &animation.accum_root
        },
    );
    if keys && !animation.text_keys.is_empty() {
        let list: Vec<String> = animation
            .text_keys
            .iter()
            .map(|(t, k)| format!("{t:.2}:{}", k.replace('\n', " ").replace('\r', "")))
            .collect();
        text.push_str(&format!(" keys=[{}]", list.join(" ")));
    }
    text
}
