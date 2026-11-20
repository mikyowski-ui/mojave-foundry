//! Inputs every stage shares: the two installs, the output directory and
//! the slice being converted.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use vault_formats::{Dat2Archive, Palette, Vfs};

/// Everything that can stop a stage.
#[derive(Debug, thiserror::Error)]
pub enum ConvertError {
    #[error("--fnv {path}: {reason}")]
    Fnv { path: PathBuf, reason: String },
    #[error("--f2 {path}: {reason}")]
    F2 { path: PathBuf, reason: String },
    #[error("--slice {name}: {reason}")]
    Slice { name: String, reason: String },
    #[error("{stage}: {reason}")]
    Stage { stage: &'static str, reason: String },
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

impl ConvertError {
    pub fn stage(stage: &'static str, reason: impl std::fmt::Display) -> Self {
        Self::Stage {
            stage,
            reason: reason.to_string(),
        }
    }
}

/// What to convert: a block of exterior cells of one worldspace, and the
/// window of it that becomes one Fallout 2 map.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SliceSpec {
    pub name: String,
    /// Editor id of the worldspace.
    pub worldspace: String,
    /// Plugins in load order.
    pub plugins: Vec<String>,
    /// Inclusive exterior cell range, west to east.
    pub x: [i32; 2],
    /// Inclusive exterior cell range, south to north.
    pub y: [i32; 2],
    /// Teleport hops into interiors to extract.
    #[serde(default = "one")]
    pub interior_depth: u32,
    /// Centre of the map window in source units (`docs/contracts.md`
    /// section 4). Defaults to the centre of the cell range.
    #[serde(default)]
    pub map_center: Option<[f32; 2]>,
}

fn one() -> u32 {
    1
}

impl SliceSpec {
    /// The built-in slices, by name.
    pub fn preset(name: &str) -> Option<Self> {
        match name {
            "goodsprings" => Some(Self {
                name: "goodsprings".to_owned(),
                worldspace: "WastelandNV".to_owned(),
                plugins: vec!["FalloutNV.esm".to_owned()],
                x: [-21, -14],
                y: [-4, 3],
                interior_depth: 1,
                // The centre of exterior cell (-18, 0).
                map_center: Some([-71_680.0, 2_048.0]),
            }),
            _ => None,
        }
    }

    /// A preset name, or the path of a TOML file with the same fields.
    pub fn resolve(name: &str) -> Result<Self, ConvertError> {
        if let Some(preset) = Self::preset(name) {
            return Ok(preset);
        }
        let path = Path::new(name);
        if !path.is_file() {
            return Err(ConvertError::Slice {
                name: name.to_owned(),
                reason: "not a built-in slice (known: goodsprings) and not a file".to_owned(),
            });
        }
        let text = std::fs::read_to_string(path).map_err(|source| ConvertError::Io {
            path: path.to_owned(),
            source,
        })?;
        Self::from_toml(&text).map_err(|reason| ConvertError::Slice {
            name: name.to_owned(),
            reason,
        })
    }

    pub fn from_toml(text: &str) -> Result<Self, String> {
        let spec: Self = toml::from_str(text).map_err(|e| e.to_string())?;
        spec.validate()?;
        Ok(spec)
    }

    pub fn to_toml(&self) -> String {
        toml::to_string(self).expect("a slice spec is plain data")
    }

    fn validate(&self) -> Result<(), String> {
        let safe = |s: &str| {
            !s.is_empty()
                && s.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        };
        if !safe(&self.name) {
            return Err("name must be letters, digits, '-' or '_'".to_owned());
        }
        if self.worldspace.is_empty() {
            return Err("worldspace is empty".to_owned());
        }
        if self.plugins.is_empty() {
            return Err("plugins is empty".to_owned());
        }
        for (axis, range) in [("x", self.x), ("y", self.y)] {
            if range[0] > range[1] {
                return Err(format!("{axis} range is reversed"));
            }
            if i64::from(range[1]) - i64::from(range[0]) >= 64 {
                return Err(format!("{axis} range spans more than 64 cells"));
            }
        }
        if let Some(center) = self.map_center
            && !center.iter().all(|v| v.is_finite())
        {
            return Err("map_center is not finite".to_owned());
        }
        Ok(())
    }

    /// The map window's centre in source units.
    pub fn center(&self) -> [f32; 2] {
        self.map_center.unwrap_or_else(|| {
            let mid = |r: [i32; 2]| (r[0] + r[1] + 1) as f32 * 0.5 * 4096.0;
            [mid(self.x), mid(self.y)]
        })
    }

    pub fn mir_slice(&self) -> mir::Slice {
        mir::Slice {
            name: self.name.clone(),
            worldspace: self.worldspace.clone(),
            x: self.x[0]..=self.x[1],
            y: self.y[0]..=self.y[1],
            interior_depth: self.interior_depth,
        }
    }
}

/// The validated inputs of a run. Both installs are required for every
/// stage: the source game for content, Fallout 2 for the palette, fonts,
/// interface art and sound effects the output relies on.
#[derive(Debug, Clone)]
pub struct Inputs {
    /// The source install's `Data` directory.
    pub fnv_data: PathBuf,
    /// The Fallout 2 install directory.
    pub f2: PathBuf,
    /// Its main archive. `None` for an install that keeps everything as
    /// loose files under `data/` (the synthetic test install does).
    pub f2_master: Option<PathBuf>,
    pub out: PathBuf,
    pub slice: SliceSpec,
    pub jobs: usize,
    /// Scale, supersampling and lighting (`docs/contracts.md` section 4).
    pub calibration: foundry_render::Calibration,
    /// Which render path to use. The CPU path is the default because only
    /// it gives the same bytes on every machine.
    pub backend: foundry_render::Backend,
    /// Draw a one-pixel dark inner edge on sprites.
    pub outline: bool,
}

impl Inputs {
    pub fn new(
        fnv: &Path,
        f2: &Path,
        out: &Path,
        slice: SliceSpec,
        jobs: usize,
    ) -> Result<Self, ConvertError> {
        let fnv_data = find_data_dir(fnv, &slice.plugins)?;
        let f2_master = find_master(f2)?;
        Ok(Self {
            fnv_data,
            f2: f2.to_owned(),
            f2_master,
            out: out.to_owned(),
            slice,
            jobs: jobs.max(1),
            calibration: foundry_render::Calibration::default(),
            backend: foundry_render::Backend::Cpu,
            outline: false,
        })
    }

    /// Replaces the default calibration with the one in a TOML file.
    pub fn with_calibration_file(mut self, path: &Path) -> Result<Self, ConvertError> {
        let text = std::fs::read_to_string(path).map_err(|source| ConvertError::Io {
            path: path.to_owned(),
            source,
        })?;
        self.calibration = foundry_render::Calibration::from_toml(&text)
            .map_err(|e| ConvertError::stage("config", format!("{}: {e}", path.display())))?;
        Ok(self)
    }

    pub fn mir_dir(&self) -> PathBuf {
        self.out.join("mir")
    }
    pub fn sprites_dir(&self) -> PathBuf {
        self.out.join("sprites")
    }
    pub fn critters_dir(&self) -> PathBuf {
        self.out.join("critters")
    }
    pub fn gallery_dir(&self) -> PathBuf {
        self.out.join("gallery")
    }
    pub fn mod_dir(&self) -> PathBuf {
        self.out.join("mods").join("mojave")
    }

    /// Reads and parses the Fallout 2 palette.
    pub fn palette(&self) -> Result<Palette, ConvertError> {
        let f2_error = |reason: String| ConvertError::F2 {
            path: self.f2.clone(),
            reason,
        };
        let bytes = self.f2_file("color.pal")?;
        Palette::parse(&bytes).map_err(|e| f2_error(format!("color.pal: {e}")))
    }

    /// Reads one file of the Fallout 2 install: a loose file under `data/`
    /// wins over the main archive.
    pub fn f2_file(&self, path: &str) -> Result<Vec<u8>, ConvertError> {
        let f2_error = |reason: String| ConvertError::F2 {
            path: self.f2.clone(),
            reason,
        };
        let loose = self.f2.join("data").join(path.replace('\\', "/"));
        if loose.is_file() {
            return std::fs::read(&loose).map_err(|source| ConvertError::Io {
                path: loose,
                source,
            });
        }
        let Some(master) = &self.f2_master else {
            return Err(f2_error(format!("{path}: not found under data/")));
        };
        // Later archives override earlier ones, as the game mounts them.
        let mut archives = vec![master.clone()];
        for name in ["critter.dat", "patch000.dat"] {
            let extra = self.f2.join(name);
            if extra.is_file() {
                archives.push(extra);
            }
        }
        let mut last_error = format!("{path}: not found");
        for archive_path in archives.iter().rev() {
            let archive = Dat2Archive::open(archive_path)
                .map_err(|e| f2_error(format!("{}: {e}", archive_path.display())))?;
            match archive.read(path) {
                Ok(bytes) => return Ok(bytes),
                Err(error) => last_error = format!("{path}: {error}"),
            }
        }
        Err(f2_error(last_error))
    }
}

/// Accepts the install root (with a `Data` directory) or the data directory
/// itself, and checks that every plugin of the slice is there.
fn find_data_dir(fnv: &Path, plugins: &[String]) -> Result<PathBuf, ConvertError> {
    let error = |reason: String| ConvertError::Fnv {
        path: fnv.to_owned(),
        reason,
    };
    if !fnv.is_dir() {
        return Err(error("not a directory".to_owned()));
    }
    let candidates = [fnv.join("Data"), fnv.join("data"), fnv.to_owned()];
    for dir in &candidates {
        if dir.is_dir() && plugins.iter().all(|p| dir.join(p).is_file()) {
            return Ok(dir.clone());
        }
    }
    Err(error(format!(
        "no data directory here holds {}",
        plugins.join(", ")
    )))
}

/// The main archive, or `None` when the install is loose files only.
fn find_master(f2: &Path) -> Result<Option<PathBuf>, ConvertError> {
    let error = |reason: &str| ConvertError::F2 {
        path: f2.to_owned(),
        reason: reason.to_owned(),
    };
    if !f2.is_dir() {
        return Err(error("not a directory"));
    }
    let master = ["master.dat", "MASTER.DAT", "Master.dat"]
        .iter()
        .map(|name| f2.join(name))
        .find(|path| path.is_file());
    if master.is_none() && !f2.join("data").join("color.pal").is_file() {
        return Err(error("neither master.dat nor data/color.pal is here"));
    }
    Ok(master)
}
