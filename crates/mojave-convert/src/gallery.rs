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
    /// Film-strip pages under `characters/`, one per sprite set.
    pub character_pages: usize,
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
    let character_pages = character_pages(inputs, &CharacterPages::default())?.len();
    Ok(GallerySummary {
        sprites: entries.len(),
        pages: sheets.len(),
        with_reference: reference.is_some(),
        critter_pages,
        character_pages,
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

/// Writes `<out>/gallery/items-NN.png`: one tile per item prototype of
/// the map's catalogue (`crate::catalogue`), its ground sprite with its
/// inventory icon beside it, its name, kind, where it was found and its
/// key figures. Returns the pages written.
pub fn items_page(
    inputs: &Inputs,
    items: &[crate::catalogue::ItemEntry],
    palette: &vault_formats::Palette,
) -> Result<usize, ConvertError> {
    let stage = |e: &dyn std::fmt::Display| ConvertError::stage("gallery", e);
    let dir = inputs.gallery_dir();
    std::fs::create_dir_all(&dir).map_err(io(&dir))?;
    if let Ok(read) = std::fs::read_dir(&dir) {
        for file in read.flatten() {
            let name = file.file_name();
            let name = name.to_string_lossy();
            if name.starts_with("items-") && name.ends_with(".png") {
                let _ = std::fs::remove_file(file.path());
            }
        }
    }
    struct Cell {
        label: String,
        metrics: Vec<(String, String)>,
        ground: (Vec<u8>, u32, u32),
        icon: Option<(Vec<u8>, u32, u32)>,
    }
    let rgba = |image: &(Vec<u8>, u32, u32)| {
        foundry_gallery::indexed_to_rgba(&image.0, image.1, image.2, palette)
            .ok()
            .map(|rgba| (rgba, image.1, image.2))
    };
    let mut cells = Vec::new();
    for item in items {
        let Some(ground) = item.ground.as_ref().and_then(rgba) else {
            continue;
        };
        cells.push(Cell {
            label: item.name.clone(),
            metrics: vec![
                ("kind".to_owned(), format!("{} ({})", item.kind, item.found)),
                ("stats".to_owned(), item.summary.clone()),
            ],
            ground,
            icon: item.icon.as_ref().and_then(rgba),
        });
    }
    if cells.is_empty() {
        return Ok(0);
    }
    let tiles: Vec<Tile<'_>> = cells
        .iter()
        .map(|c| Tile {
            label: &c.label,
            metrics: &c.metrics,
            rgba: &c.ground.0,
            width: c.ground.1,
            height: c.ground.2,
            compare: c
                .icon
                .as_ref()
                .map(|(rgba, w, h)| (rgba.as_slice(), *w, *h)),
        })
        .collect();
    let options = SheetOptions {
        columns: Columns::Auto { sheet_width: 1800 },
        anchor: Anchor::Bottom,
        scale: 2,
        max_height: 2400,
        title: Some(format!(
            "{} items: ground sprite, inventory icon beside it",
            inputs.slice.name
        )),
        ..SheetOptions::default()
    };
    let sheets = contact_sheets(&tiles, &options).map_err(|e| stage(&e))?;
    for (page, sheet) in sheets.iter().enumerate() {
        let path = dir.join(format!("items-{:02}.png", page + 1));
        let png = sheet.to_png().map_err(|e| stage(&e))?;
        std::fs::write(&path, png).map_err(io(&path))?;
    }
    Ok(sheets.len())
}

/// A page pairing the map previews (`gallery/map-<stem>.png`) of two `--out`
/// directories side by side, labelled: the declutter pass before and
/// after. Writes `dest`.
pub fn before_after_page(
    before: &Path,
    after: &Path,
    labels: [&str; 2],
    dest: &Path,
) -> Result<(), ConvertError> {
    let pick = |out: &Path| -> Result<std::path::PathBuf, ConvertError> {
        let dir = out.join("gallery");
        let mut found: Vec<std::path::PathBuf> = std::fs::read_dir(&dir)
            .map_err(io(&dir))?
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with("map-") && n.ends_with(".png"))
            })
            .collect();
        found.sort();
        found.into_iter().next().ok_or_else(|| {
            ConvertError::stage("gallery", format!("{}: no map preview", dir.display()))
        })
    };
    let entries = vec![
        (labels[0].to_owned(), pick(before)?),
        (labels[1].to_owned(), pick(after)?),
    ];
    crate::shots::contact_page(
        &entries,
        Some("declutter: before and after"),
        2,
        [960, 960],
        dest,
    )
}

/// What [`character_pages`] draws.
#[derive(Debug, Clone)]
pub struct CharacterPages {
    /// The facing of the strips: 2 (south-east) faces the viewer.
    pub direction: usize,
    /// At most this many frames a row; longer animations show every Nth
    /// frame (the action frame always).
    pub max_frames: usize,
    pub zoom: u32,
    /// Also write `<label>.gif`, the set's animations one after another.
    pub gif: bool,
    /// Only the sets of these editor ids (any case); empty: every set.
    pub only: Vec<String>,
    /// Only these animation suffixes (the six-facing stand row stays);
    /// empty: every one. A close look at a few rows at a high zoom.
    pub suffixes: Vec<String>,
    /// Appended to the page's file name (`<label>-strips<tag>.png`).
    pub tag: String,
}

impl Default for CharacterPages {
    fn default() -> Self {
        CharacterPages {
            direction: 2,
            max_frames: 16,
            zoom: 2,
            gif: true,
            only: Vec::new(),
            suffixes: Vec::new(),
            tag: String::new(),
        }
    }
}

/// The order animations are shown in: stand, moving, unarmed fighting,
/// falls, then everything else by suffix.
const SHOW_ORDER: [&str; 16] = [
    "aa", "ab", "at", "an", "ao", "ap", "aq", "ar", "ak", "al", "ba", "ra", "cj", "bb", "rb", "ch",
];

/// A file name made of an editor id.
fn file_label(label: &str) -> String {
    label
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect()
}

/// Writes `<out>/gallery/characters/<label>-strips.png` for every sprite
/// set: one row per animation (the facing of [`CharacterPages::direction`],
/// every Nth frame, the action frame marked in red, the hex centre a
/// yellow cross), the six-direction stand row first, each row labelled
/// `real` or `still`; and, with [`CharacterPages::gif`], `<label>.gif`
/// playing the set's animations at their own rates. Returns the pages
/// written.
pub fn character_pages(
    inputs: &Inputs,
    options: &CharacterPages,
) -> Result<Vec<std::path::PathBuf>, ConvertError> {
    let stage = |e: &dyn std::fmt::Display| ConvertError::stage("gallery", e);
    let palette = inputs.palette()?;
    let dir = inputs.gallery_dir().join("characters");
    let mut written = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for item in crate::critters::read_index(inputs)? {
        let Some(key) = item.set else { continue };
        if !options.only.is_empty()
            && !item
                .editor_id
                .as_deref()
                .is_some_and(|id| options.only.iter().any(|w| w.eq_ignore_ascii_case(id)))
        {
            continue;
        }
        if !seen.insert(key.clone()) {
            continue;
        }
        let meta = crate::critters::read_meta(inputs, &key)?;
        let files = crate::critters::read_set(inputs, &key)?;
        let mut archives = std::collections::BTreeMap::new();
        for (suffix, bytes) in &files {
            let archive =
                FrmArchive::parse(bytes).map_err(|e| stage(&format!("{key} {suffix}: {e}")))?;
            archives.insert(suffix.clone(), archive);
        }
        let mut order: Vec<String> = SHOW_ORDER
            .iter()
            .filter(|s| archives.contains_key(**s))
            .map(|s| (*s).to_owned())
            .collect();
        for suffix in archives.keys() {
            if !order.contains(suffix) {
                order.push(suffix.clone());
            }
        }
        // Every frame image the page needs, owned here; strips borrow.
        struct Cell {
            rgba: Vec<u8>,
            width: u32,
            height: u32,
            anchor: [i32; 2],
            marked: bool,
            caption: String,
        }
        let cell = |archive: &FrmArchive, direction: usize, frame: usize, marked: bool| {
            let (rgba, width, height) =
                foundry_gallery::frm_frame_rgba(archive, direction, frame, &palette)?;
            let shift = archive.shifts[direction];
            let moved = archive
                .accumulated_displacement(direction, frame)
                .unwrap_or([0, 0]);
            // The engine puts the frame's bottom centre at the hex centre
            // plus the shift plus the displacement so far.
            let anchor = [
                (width / 2) as i32 - i32::from(shift[0]) - moved[0],
                height as i32 - i32::from(shift[1]) - moved[1],
            ];
            Some(Cell {
                rgba,
                width,
                height,
                anchor,
                marked,
                caption: frame.to_string(),
            })
        };
        let mut rows: Vec<(String, Vec<Cell>)> = Vec::new();
        if let Some(stand) = archives.get(crate::critters::STAND) {
            const FACINGS: [&str; 6] = ["NE", "E", "SE", "SW", "W", "NW"];
            let cells = (0..6)
                .filter_map(|d| {
                    cell(stand, d, 0, false).map(|mut c| {
                        c.caption = FACINGS[d].to_owned();
                        c
                    })
                })
                .collect();
            rows.push(("aa six facings".to_owned(), cells));
        }
        for suffix in &order {
            if !options.suffixes.is_empty() && !options.suffixes.contains(suffix) {
                continue;
            }
            let archive = &archives[suffix];
            let count = archive.frame_count();
            let action = usize::from(archive.action_frame.max(0) as u16);
            let contact = (action > 0).then(|| action - 1);
            let every = count.div_ceil(options.max_frames.max(1)).max(1);
            let frames: Vec<usize> = (0..count)
                .filter(|k| k % every == 0 || Some(*k) == contact)
                .collect();
            let cells = frames
                .iter()
                .filter_map(|&k| cell(archive, options.direction, k, Some(k) == contact))
                .collect();
            let kind = match meta.real.get(suffix) {
                Some(note) => format!("real, {}", note.source),
                None if meta.placeholders.contains(suffix) => "still".to_owned(),
                None if meta.schema == 0 => "first wave".to_owned(),
                None => "?".to_owned(),
            };
            rows.push((
                format!("{suffix} {count}f a{} {kind}", archive.action_frame),
                cells,
            ));
        }
        let strips: Vec<foundry_gallery::Strip<'_>> = rows
            .iter()
            .map(|(label, cells)| foundry_gallery::Strip {
                label: label.clone(),
                frames: cells
                    .iter()
                    .map(|c| foundry_gallery::StripFrame {
                        rgba: &c.rgba,
                        width: c.width,
                        height: c.height,
                        anchor: c.anchor,
                        marked: c.marked,
                        caption: &c.caption,
                    })
                    .collect(),
            })
            .collect();
        let name = item
            .editor_id
            .clone()
            .or(item.name.clone())
            .unwrap_or_else(|| item.actor.clone());
        let page = foundry_gallery::film_strips_png(
            &strips,
            &foundry_gallery::StripOptions {
                title: Some(format!(
                    "{name}: {} real, {} still, {} bytes",
                    meta.real.len(),
                    meta.placeholders.len(),
                    meta.bytes
                )),
                zoom: options.zoom,
                label_width: 260,
                ..foundry_gallery::StripOptions::default()
            },
        )
        .map_err(|e| stage(&e))?;
        std::fs::create_dir_all(&dir).map_err(io(&dir))?;
        let path = dir.join(format!("{}-strips{}.png", file_label(&name), options.tag));
        std::fs::write(&path, page).map_err(io(&path))?;
        written.push(path);
        if options.gif && options.suffixes.is_empty() {
            let gif = animated_preview(&archives, &order, options.direction, &palette)
                .map_err(|e| stage(&e))?;
            let path = dir.join(format!("{}.gif", file_label(&name)));
            std::fs::write(&path, gif).map_err(io(&path))?;
        }
    }
    Ok(written)
}

/// Every animation of `order` one after another in one facing, each frame
/// placed by its hex anchor on a fixed grey canvas (walks in place), as a
/// looping GIF through the camera agent's encoder.
fn animated_preview(
    archives: &std::collections::BTreeMap<String, FrmArchive>,
    order: &[String],
    direction: usize,
    palette: &vault_formats::Palette,
) -> Result<Vec<u8>, String> {
    let mut placed = Vec::new();
    let mut bounds = [i32::MAX, i32::MAX, i32::MIN, i32::MIN];
    for suffix in order {
        let archive = &archives[suffix];
        let fps = archive.effective_fps().max(1) as u16;
        let delay = (100 / fps).max(2);
        // Show a still for half a second.
        let repeat = if archive.frame_count() == 1 { 5 } else { 1 };
        for frame in 0..archive.frame_count() {
            let Some((rgba, w, h)) =
                foundry_gallery::frm_frame_rgba(archive, direction, frame, palette)
            else {
                continue;
            };
            let shift = archive.shifts[direction];
            let left = i32::from(shift[0]) - (w / 2) as i32;
            let top = i32::from(shift[1]) - h as i32;
            bounds = [
                bounds[0].min(left),
                bounds[1].min(top),
                bounds[2].max(left + w as i32),
                bounds[3].max(top + h as i32),
            ];
            for _ in 0..repeat {
                placed.push((rgba.clone(), w, h, left, top, delay));
            }
        }
    }
    if placed.is_empty() {
        return Err("no frames".to_owned());
    }
    let pad = 4;
    let width = (bounds[2] - bounds[0] + 2 * pad) as u32;
    let height = (bounds[3] - bounds[1] + 2 * pad) as u32;
    let mut frames = Vec::with_capacity(placed.len());
    let mut delays = Vec::with_capacity(placed.len());
    for (rgba, w, h, left, top, delay) in &placed {
        let mut rgb = vec![96u8; width as usize * height as usize * 3];
        let ox = left - bounds[0] + pad;
        let oy = top - bounds[1] + pad;
        for y in 0..*h {
            for x in 0..*w {
                let from = ((y * w + x) * 4) as usize;
                if rgba[from + 3] == 0 {
                    continue;
                }
                let to = (((oy + y as i32) as u32 * width + (ox + x as i32) as u32) * 3) as usize;
                rgb[to..to + 3].copy_from_slice(&rgba[from..from + 3]);
            }
        }
        frames.push(crate::shots::Frame { width, height, rgb });
        delays.push(*delay);
    }
    let refs: Vec<&crate::shots::Frame> = frames.iter().collect();
    crate::sequence::gif::encode(&refs, &delays, 0).map(|(bytes, _)| bytes)
}
