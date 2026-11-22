//! The engine's default land texture: what a land layer with no usable
//! land texture is drawn with.
//!
//! A `LAND` record may leave a quadrant without a base layer (`BTXT`), or
//! give a layer a null land texture. The engine then draws the default
//! land texture, a diffuse file name under `textures\landscape\` that the
//! game's settings file holds: `sDefaultLandDiffuseTexture` in the
//! `[Landscape]` section of `Fallout_default.ini`, beside the install's
//! `Data` directory. The extractor reads it there; when the file or the
//! setting is absent or unusable, it uses the value a stock install ships,
//! [`DEFAULT_LAND_TEXTURE`].
//!
//! Not read: the per-user copy of the settings (under the user's documents
//! folder), which a stock install never changes for this setting.

use std::fs;
use std::path::Path;

use crate::paths::normalize_texture_path;

/// The default land texture of a stock install, normalised.
pub const DEFAULT_LAND_TEXTURE: &str = "textures\\landscape\\dirtwasteland01.dds";

/// The settings file, in the install's root, beside `Data`.
const SETTINGS_FILE: &str = "fallout_default.ini";
/// Its section and key, compared without case.
const SECTION: &str = "landscape";
const KEY: &str = "sdefaultlanddiffusetexture";
/// Largest settings file read.
const MAX_SETTINGS_BYTES: u64 = 1 << 20;
/// Longest value accepted.
const MAX_VALUE_LEN: usize = 200;

/// The default land texture an extract uses, and where it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefaultLandTexture {
    /// Normalised texture path.
    pub path: String,
    /// Whether the install's settings file gave it; `false` is
    /// [`DEFAULT_LAND_TEXTURE`].
    pub from_settings: bool,
}

/// The default land texture of the install whose `Data` directory is
/// `data_dir`: the settings file's value when it holds a usable one, else
/// [`DEFAULT_LAND_TEXTURE`]. Never fails.
pub fn default_land_texture(data_dir: &Path) -> DefaultLandTexture {
    let from_file = data_dir
        .parent()
        .and_then(find_settings)
        .and_then(|path| {
            let meta = fs::metadata(&path).ok()?;
            (meta.len() <= MAX_SETTINGS_BYTES).then_some(())?;
            fs::read(&path).ok()
        })
        .and_then(|bytes| setting_value(&String::from_utf8_lossy(&bytes)))
        .and_then(|value| texture_path(&value));
    match from_file {
        Some(path) => DefaultLandTexture {
            path,
            from_settings: true,
        },
        None => DefaultLandTexture {
            path: DEFAULT_LAND_TEXTURE.to_owned(),
            from_settings: false,
        },
    }
}

/// The settings file in `root`, matched without case; the first in name
/// order when several match.
fn find_settings(root: &Path) -> Option<std::path::PathBuf> {
    let mut names: Vec<std::path::PathBuf> = fs::read_dir(root)
        .ok()?
        .filter_map(Result::ok)
        .filter(|e| {
            e.file_name()
                .to_str()
                .is_some_and(|n| n.eq_ignore_ascii_case(SETTINGS_FILE))
        })
        .map(|e| e.path())
        .filter(|p| p.is_file())
        .collect();
    names.sort();
    names.into_iter().next()
}

/// The value of `sDefaultLandDiffuseTexture` in `[Landscape]`: the last
/// one when it is set twice. Section and key names are compared without
/// case; `;` and `#` start a comment line.
fn setting_value(text: &str) -> Option<String> {
    let mut in_section = false;
    let mut value = None;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
            continue;
        }
        if let Some(rest) = line.strip_prefix('[') {
            in_section = rest
                .strip_suffix(']')
                .is_some_and(|name| name.trim().eq_ignore_ascii_case(SECTION));
            continue;
        }
        if !in_section {
            continue;
        }
        if let Some((key, v)) = line.split_once('=')
            && key.trim().eq_ignore_ascii_case(KEY)
        {
            value = Some(v.trim().to_owned());
        }
    }
    value
}

/// The normalised path of a settings value, a file name under
/// `textures\landscape\` (a value that already starts with `textures\` or
/// `landscape\` keeps its folders). `None` for an empty value, one that is
/// not printable ASCII, too long, or climbs out of its folder, or one that
/// is not a `.dds` file.
fn texture_path(value: &str) -> Option<String> {
    let value = value.trim_matches(|c: char| c == '"' || c.is_ascii_whitespace());
    if value.is_empty()
        || value.len() > MAX_VALUE_LEN
        || !value.bytes().all(|b| (0x20..0x7f).contains(&b))
    {
        return None;
    }
    let path = normalize_texture_path(value);
    let relative = path.strip_prefix("textures\\")?;
    let path = if relative.starts_with("landscape\\") {
        path.clone()
    } else {
        format!("textures\\landscape\\{relative}")
    };
    let climbs = path.split('\\').any(|part| part == ".." || part.is_empty());
    (!climbs && path.ends_with(".dds")).then_some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_setting_is_read_from_its_section_only() {
        let text = "[General]\nsDefaultLandDiffuseTexture=wrong.dds\n\n\
                    [ LANDSCAPE ]\n; a comment\nfLandTextureTilingMult=2.0\n\
                    SDefaultLandDiffuseTexture = Synthetic01.dds \n\
                    sDefaultLandNormalTexture=Synthetic01_N.dds\n[Display]\n";
        assert_eq!(setting_value(text).as_deref(), Some("Synthetic01.dds"));
        assert_eq!(setting_value("[Landscape]\nfOther=1\n"), None);
        assert_eq!(setting_value("sDefaultLandDiffuseTexture=a.dds\n"), None);
    }

    #[test]
    fn values_become_paths_under_the_landscape_folder() {
        assert_eq!(
            texture_path("Synthetic01.dds").as_deref(),
            Some("textures\\landscape\\synthetic01.dds")
        );
        assert_eq!(
            texture_path("Landscape/Sub/A.DDS").as_deref(),
            Some("textures\\landscape\\sub\\a.dds")
        );
        assert_eq!(
            texture_path("Data\\Textures\\Landscape\\b.dds").as_deref(),
            Some("textures\\landscape\\b.dds")
        );
        assert_eq!(
            texture_path("\"q.dds\"").as_deref(),
            Some("textures\\landscape\\q.dds")
        );
        for bad in [
            "",
            "  ",
            "..\\x.dds",
            "a\\..\\b.dds",
            "x.png",
            "é.dds",
            "a\\\\b.dds",
        ] {
            assert_eq!(texture_path(bad), None, "{bad:?}");
        }
        assert_eq!(texture_path(&"a".repeat(300)), None);
    }

    #[test]
    fn the_install_settings_win_and_their_absence_falls_back() {
        let dir = tempfile::tempdir().expect("temp dir");
        let root = dir.path();
        let data = root.join("Data");
        fs::create_dir_all(&data).expect("temp dir");
        let fallback = default_land_texture(&data);
        assert_eq!(fallback.path, DEFAULT_LAND_TEXTURE);
        assert!(!fallback.from_settings);
        fs::write(
            root.join("FALLOUT_default.INI"),
            "[Landscape]\r\nsDefaultLandDiffuseTexture=Synthetic02.dds\r\n",
        )
        .expect("settings");
        let found = default_land_texture(&data);
        assert_eq!(found.path, "textures\\landscape\\synthetic02.dds");
        assert!(found.from_settings);
        // An unusable value falls back.
        fs::write(
            root.join("FALLOUT_default.INI"),
            "[Landscape]\nsDefaultLandDiffuseTexture=\n",
        )
        .expect("settings");
        assert_eq!(default_land_texture(&data).path, DEFAULT_LAND_TEXTURE);
    }
}
