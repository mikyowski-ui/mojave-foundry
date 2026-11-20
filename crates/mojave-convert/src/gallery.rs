//! The gallery stage: contact sheets of the sprite stage's output, so a
//! person can judge scale, palette and silhouette at a glance.
//!
//! When the Fallout 2 install has it, an original standing human figure is
//! drawn beside every sprite at the same pixel scale.

use std::path::Path;

use foundry_gallery::{Anchor, Columns, SheetOptions, Tile, contact_sheets, frm_frame_rgba};
use vault_formats::frm::FrmArchive;

use crate::config::{ConvertError, Inputs};
use crate::sprites::{read_index, read_sprite};

/// The original art used as a scale reference, in order of preference.
const SCALE_REFERENCES: [&str; 2] = ["art/critters/hmjmpsaa.frm", "art/critters/hmwarraa.frm"];

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GallerySummary {
    pub sprites: usize,
    pub pages: usize,
    pub with_reference: bool,
    /// Pages of `critters-NN.png`.
    pub critter_pages: usize,
}

struct Entry {
    label: String,
    metrics: Vec<(String, String)>,
    rgba: Vec<u8>,
    width: u32,
    height: u32,
}

fn io(path: &Path) -> impl FnOnce(std::io::Error) -> ConvertError + '_ {
    move |source| ConvertError::Io {
        path: path.to_owned(),
        source,
    }
}

/// Writes `<out>/gallery/sprites-NN.png`. `filter` keeps only sprites whose
/// model path contains it (case-insensitive); empty keeps all.
pub fn gallery(inputs: &Inputs, filter: &str) -> Result<GallerySummary, ConvertError> {
    let stage = |e: &dyn std::fmt::Display| ConvertError::stage("gallery", e);
    let palette = inputs.palette()?;
    let filter = filter.to_ascii_lowercase();
    let mut index = read_index(inputs)?;
    index.retain(|entry| {
        entry.error.is_none() && entry.model.to_ascii_lowercase().contains(&filter)
    });
    index.sort_by(|a, b| (&a.model, &a.mesh).cmp(&(&b.model, &b.mesh)));

    let reference = SCALE_REFERENCES.iter().find_map(|path| {
        let bytes = inputs.f2_file(path).ok()?;
        let archive = FrmArchive::parse(&bytes).ok()?;
        // Direction 2 faces the viewer's right-front, the usual idle pose.
        frm_frame_rgba(&archive, 2, 0, &palette)
            .or_else(|| frm_frame_rgba(&archive, 0, 0, &palette))
    });

    let mut entries = Vec::with_capacity(index.len());
    for item in &index {
        let (meta, bytes) = read_sprite(inputs, &item.key)?;
        let archive =
            FrmArchive::parse(&bytes).map_err(|e| stage(&format!("{}: {e}", item.key)))?;
        let Some((rgba, width, height)) = frm_frame_rgba(&archive, 0, 0, &palette) else {
            continue;
        };
        let name = item
            .model
            .rsplit('\\')
            .next()
            .unwrap_or(&item.model)
            .trim_end_matches(".nif");
        let mut metrics = vec![
            ("size".to_owned(), format!("{}x{}", meta.width, meta.height)),
            ("colours".to_owned(), meta.colours.to_string()),
            ("error".to_owned(), format!("{:.3}", meta.mean_error)),
            ("refs".to_owned(), item.references.to_string()),
        ];
        if !meta.missing_textures.is_empty() {
            metrics.push((
                "missing tex".to_owned(),
                meta.missing_textures.len().to_string(),
            ));
        }
        entries.push(Entry {
            label: name.to_owned(),
            metrics,
            rgba,
            width,
            height,
        });
    }

    let tiles: Vec<Tile<'_>> = entries
        .iter()
        .map(|entry| Tile {
            label: &entry.label,
            metrics: &entry.metrics,
            rgba: &entry.rgba,
            width: entry.width,
            height: entry.height,
            compare: reference
                .as_ref()
                .map(|(rgba, w, h)| (rgba.as_slice(), *w, *h)),
        })
        .collect();
    let options = SheetOptions {
        columns: Columns::Auto { sheet_width: 1800 },
        anchor: Anchor::Bottom,
        max_height: 2400,
        title: Some(format!("{} sprites", inputs.slice.name)),
        ..SheetOptions::default()
    };
    let sheets = contact_sheets(&tiles, &options).map_err(|e| stage(&e))?;

    let dir = inputs.gallery_dir();
    std::fs::create_dir_all(&dir).map_err(io(&dir))?;
    // Drop pages a previous, longer run left behind.
    if let Ok(read) = std::fs::read_dir(&dir) {
        for file in read.flatten() {
            let name = file.file_name();
            let name = name.to_string_lossy();
            if name.starts_with("sprites-") && name.ends_with(".png") {
                let _ = std::fs::remove_file(file.path());
            }
        }
    }
    for (page, sheet) in sheets.iter().enumerate() {
        let path = dir.join(format!("sprites-{:02}.png", page + 1));
        let png = sheet.to_png().map_err(|e| stage(&e))?;
        std::fs::write(&path, png).map_err(io(&path))?;
    }
    let critter_pages = critter_sheets(inputs, &palette, reference.as_ref())?;
    Ok(GallerySummary {
        sprites: entries.len(),
        pages: sheets.len(),
        with_reference: reference.is_some(),
        critter_pages,
    })
}

/// Writes `<out>/gallery/critters-NN.png`: for each sprite set of the
/// critter stage, the first stand frame in the six hex directions and three
/// frames of the walk, beside the scale reference. Returns the page count.
fn critter_sheets(
    inputs: &Inputs,
    palette: &vault_formats::Palette,
    reference: Option<&(Vec<u8>, u32, u32)>,
) -> Result<usize, ConvertError> {
    const FACINGS: [&str; 6] = ["NE", "E", "SE", "SW", "W", "NW"];
    let stage = |e: &dyn std::fmt::Display| ConvertError::stage("gallery", e);
    let mut seen = std::collections::BTreeSet::new();
    let mut entries = Vec::new();
    for item in crate::critters::read_index(inputs)? {
        let Some(key) = item.set else { continue };
        if !seen.insert(key.clone()) {
            continue;
        }
        let name = item
            .editor_id
            .or(item.name)
            .unwrap_or_else(|| item.actor.clone());
        let files = crate::critters::read_set(inputs, &key)?;
        let parse = |suffix: &str| {
            files
                .get(suffix)
                .ok_or_else(|| stage(&format!("{key}: no {suffix}")))
                .and_then(|bytes| {
                    FrmArchive::parse(bytes).map_err(|e| stage(&format!("{key} {suffix}: {e}")))
                })
        };
        let stand = parse(crate::critters::STAND)?;
        let walk = parse(crate::critters::WALK)?;
        let mut push = |archive: &FrmArchive, direction: usize, frame: usize, label: String| {
            if let Some((rgba, width, height)) = frm_frame_rgba(archive, direction, frame, palette)
            {
                entries.push(Entry {
                    label,
                    metrics: vec![("size".to_owned(), format!("{width}x{height}"))],
                    rgba,
                    width,
                    height,
                });
            }
        };
        for (direction, facing) in FACINGS.iter().enumerate() {
            push(&stand, direction, 0, format!("{name} {facing}"));
        }
        for frame in [0, 3, 6] {
            push(&walk, 2, frame, format!("{name} walk {frame}"));
        }
    }
    let tiles: Vec<Tile<'_>> = entries
        .iter()
        .map(|entry| Tile {
            label: &entry.label,
            metrics: &entry.metrics,
            rgba: &entry.rgba,
            width: entry.width,
            height: entry.height,
            compare: reference.map(|(rgba, w, h)| (rgba.as_slice(), *w, *h)),
        })
        .collect();
    let dir = inputs.gallery_dir();
    std::fs::create_dir_all(&dir).map_err(io(&dir))?;
    if let Ok(read) = std::fs::read_dir(&dir) {
        for file in read.flatten() {
            let name = file.file_name();
            let name = name.to_string_lossy();
            if name.starts_with("critters-") && name.ends_with(".png") {
                let _ = std::fs::remove_file(file.path());
            }
        }
    }
    if tiles.is_empty() {
        return Ok(0);
    }
    let options = SheetOptions {
        columns: Columns::Fixed(9),
        anchor: Anchor::Bottom,
        max_height: 2400,
        title: Some(format!("{} characters", inputs.slice.name)),
        ..SheetOptions::default()
    };
    let sheets = contact_sheets(&tiles, &options).map_err(|e| stage(&e))?;
    for (page, sheet) in sheets.iter().enumerate() {
        let path = dir.join(format!("critters-{:02}.png", page + 1));
        let png = sheet.to_png().map_err(|e| stage(&e))?;
        std::fs::write(&path, png).map_err(io(&path))?;
    }
    Ok(sheets.len())
}
