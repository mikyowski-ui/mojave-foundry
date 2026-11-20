//! Art lists and the FID budget.
//!
//! An art identifier (FID) holds its list index in 12 bits, so one
//! `art/<category>/<category>.lst` addresses at most 4,096 entries. A mod
//! layer cannot append to a list: a layer's copy replaces the base copy
//! whole. So an [`ArtList`] starts from the base game's list bytes, keeps
//! them unchanged, and appends.

use crate::{EmitError, lst};
use vault_formats::fid::{ALIASED_ANIMATIONS, ART_DIRECTORIES, CRITTER_CATEGORY, critter_suffix};
use vault_formats::{ArtCatalog, Fid, LstKind, LstTable};

/// The most entries a FID can address: indexes 0 to 4,095.
pub const FID_INDEX_CAP: usize = 1 << 12;

/// One category's art list: the base bytes plus appended names.
#[derive(Debug, Clone)]
pub struct ArtList {
    category: u8,
    bytes: Vec<u8>,
    /// Each position's name, as the engine reads it (`None` for a line
    /// the parser withheld a name from).
    names: Vec<Option<Vec<u8>>>,
}

impl ArtList {
    /// An empty list for `category` (0 `items` to 10 `skilldex`).
    pub fn new(category: u8) -> Result<Self, EmitError> {
        Self::from_base(category, &[])
    }

    /// Start from the base game's list bytes, which are kept byte for
    /// byte. A base list already over the cap is an error.
    pub fn from_base(category: u8, base: &[u8]) -> Result<Self, EmitError> {
        let directory = directory(category)?;
        let table = LstTable::parse(base, LstKind::Art)?;
        let names: Vec<Option<Vec<u8>>> = table
            .entries()
            .iter()
            .map(|entry| entry.name.clone())
            .collect();
        if names.len() > FID_INDEX_CAP {
            return Err(EmitError::FidBudget {
                category: directory,
                cap: FID_INDEX_CAP,
                requested: names.len(),
            });
        }
        let bytes = lst::terminate_list(base, LstKind::Art, names.len())?;
        Ok(Self {
            category,
            bytes,
            names,
        })
    }

    pub fn category(&self) -> u8 {
        self.category
    }

    /// `art/<directory>/<directory>.lst`.
    pub fn list_path(&self) -> String {
        let directory = ART_DIRECTORIES[usize::from(self.category)];
        format!("art/{directory}/{directory}.lst")
    }

    /// The index of `name`, appending it when the list lacks it. Names
    /// compare without ASCII case, as the engine's file lookup does; when
    /// the base holds a name more than once, the last row is returned
    /// (`ArtCatalog::last_row`). Appending past 4,096 entries is
    /// [`EmitError::FidBudget`].
    pub fn push(&mut self, name: &str) -> Result<u16, EmitError> {
        if let Some(index) = self.find(name) {
            return Ok(index);
        }
        let line =
            lst::encode_line(&lst::LstLine::art(name)).map_err(|message| EmitError::Lst {
                line: self.names.len() + 1,
                message,
            })?;
        if self.names.len() >= FID_INDEX_CAP {
            return Err(EmitError::FidBudget {
                category: directory(self.category)?,
                cap: FID_INDEX_CAP,
                requested: self.names.len() + 1,
            });
        }
        self.bytes.extend_from_slice(&line);
        self.names.push(Some(name.as_bytes().to_vec()));
        Ok((self.names.len() - 1) as u16)
    }

    /// Check up front that `count` more names would fit, so a caller can
    /// fail before doing any work.
    pub fn reserve(&self, count: usize) -> Result<(), EmitError> {
        let requested = self.names.len().saturating_add(count);
        if requested > FID_INDEX_CAP {
            return Err(EmitError::FidBudget {
                category: directory(self.category)?,
                cap: FID_INDEX_CAP,
                requested,
            });
        }
        Ok(())
    }

    /// The last index whose name equals `name` without ASCII case.
    pub fn find(&self, name: &str) -> Option<u16> {
        self.names
            .iter()
            .rposition(|entry| {
                entry
                    .as_deref()
                    .is_some_and(|entry| entry.eq_ignore_ascii_case(name.as_bytes()))
            })
            .map(|index| index as u16)
    }

    /// Entries, base and appended.
    pub fn len(&self) -> usize {
        self.names.len()
    }

    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }

    /// How many more entries fit under the cap.
    pub fn remaining(&self) -> usize {
        FID_INDEX_CAP.saturating_sub(self.names.len())
    }

    /// The list file's bytes.
    pub fn to_bytes(&self) -> Vec<u8> {
        self.bytes.clone()
    }

    /// The plain FID of entry `index` (animation, weapon and selector 0),
    /// through `Fid::new`.
    pub fn fid(&self, index: u16) -> Fid {
        Fid::new(self.category, index, 0, 0, 0)
    }

    /// The art file's VFS path for `fid`, through `ArtCatalog::path` over
    /// this list.
    pub fn path(&self, fid: Fid) -> Result<String, EmitError> {
        let mut catalog = ArtCatalog::default();
        catalog.insert(self.category, LstTable::parse(&self.bytes, LstKind::Art)?);
        catalog.path(fid).map_err(|error| EmitError::Path {
            path: format!("{fid:?}"),
            message: error.to_string(),
        })
    }
}

/// `art/critters/critters.lst`.
pub const CRITTER_LIST_PATH: &str = "art/critters/critters.lst";
/// The longest stem [`CritterList::append`] accepts: with a two-letter
/// animation suffix and `.frm` the file name stays within 14 bytes.
pub const CRITTER_STEM_BYTES: usize = 8;

/// What a critter FID resolves to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CritterArt {
    pub fid: Fid,
    /// `<stem><two letters>.frm`, as `ArtCatalog::filename` gives it.
    pub file_name: String,
    /// `art/critters/<file_name>`, as `ArtCatalog::path` gives it.
    pub path: String,
}

/// The FID of critter art row `row` for `animation` (0 stand, 1 walk, ...)
/// with weapon code `weapon` (0 unarmed, up to 10), through `Fid::new`.
/// Refused: a row above 4,095, and an animation and weapon pair that
/// `critter_suffix` has no file name for.
pub fn critter_fid(row: u16, animation: u8, weapon: u8) -> Result<Fid, EmitError> {
    if usize::from(row) >= FID_INDEX_CAP {
        return Err(EmitError::Critter(format!(
            "art row {row} is past the last row a FID addresses, {}",
            FID_INDEX_CAP - 1
        )));
    }
    critter_suffix(animation, weapon).map_err(|error| EmitError::Critter(error.to_string()))?;
    let fid = Fid::new(CRITTER_CATEGORY, row, animation, weapon, 0);
    debug_assert_eq!(
        (fid.base(), fid.animation(), fid.weapon(), fid.category()),
        (row, animation, weapon, CRITTER_CATEGORY)
    );
    Ok(fid)
}

/// Why `stem` cannot be a new critter's stem, if it cannot.
fn check_stem(stem: &str) -> Result<(), String> {
    if stem.is_empty() || stem.len() > CRITTER_STEM_BYTES {
        return Err(format!(
            "critter stem {stem:?} has {} bytes; a stem has 1 to {CRITTER_STEM_BYTES}",
            stem.len()
        ));
    }
    if !stem
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        return Err(format!(
            "critter stem {stem:?} is not ASCII letters, digits and `_`"
        ));
    }
    Ok(())
}

/// `art/critters/critters.lst`: the base file's bytes, kept byte for byte,
/// then one `stem,alias` line per new critter, whose alias is its own row
/// (the engine borrows the alias row's art for a few special deaths, and a
/// line without one borrows row 0's).
#[derive(Debug, Clone, Default)]
pub struct CritterList {
    bytes: Vec<u8>,
    names: Vec<Option<Vec<u8>>>,
}

impl CritterList {
    /// An empty list.
    pub fn new() -> Self {
        Self::default()
    }

    /// Start from the base game's list. A base list over 4,096 rows is
    /// [`EmitError::FidBudget`].
    pub fn from_base(base: &[u8]) -> Result<Self, EmitError> {
        let table = LstTable::parse(base, LstKind::Art)?;
        let names: Vec<Option<Vec<u8>>> = table
            .entries()
            .iter()
            .map(|entry| entry.name.clone())
            .collect();
        if names.len() > FID_INDEX_CAP {
            return Err(EmitError::FidBudget {
                category: "critters",
                cap: FID_INDEX_CAP,
                requested: names.len(),
            });
        }
        let bytes = lst::terminate_list(base, LstKind::Art, names.len())?;
        Ok(Self { bytes, names })
    }

    /// [`CRITTER_LIST_PATH`].
    pub fn list_path(&self) -> &'static str {
        CRITTER_LIST_PATH
    }

    /// Append `stem` as a new critter; returns its row. Refused: a stem
    /// that is not 1 to 8 ASCII letters, digits or `_`, or that a row
    /// already names (without ASCII case) as [`EmitError::Critter`]; a row
    /// past 4,095 as [`EmitError::FidBudget`].
    pub fn append(&mut self, stem: &str) -> Result<u16, EmitError> {
        check_stem(stem).map_err(EmitError::Critter)?;
        if let Some(row) = self.find(stem) {
            return Err(EmitError::Critter(format!(
                "critter stem {stem:?} is already row {row}"
            )));
        }
        let row = self.names.len();
        if row >= FID_INDEX_CAP {
            return Err(EmitError::FidBudget {
                category: "critters",
                cap: FID_INDEX_CAP,
                requested: row + 1,
            });
        }
        let line = lst::encode_line(&lst::LstLine::Art {
            name: stem.into(),
            numbers: vec![row as i32],
        })
        .map_err(|message| EmitError::Lst {
            line: row + 1,
            message,
        })?;
        self.bytes.extend_from_slice(&line);
        self.names.push(Some(stem.as_bytes().to_vec()));
        Ok(row as u16)
    }

    /// The last row whose stem equals `stem` without ASCII case.
    pub fn find(&self, stem: &str) -> Option<u16> {
        self.names
            .iter()
            .rposition(|name| {
                name.as_deref()
                    .is_some_and(|name| name.eq_ignore_ascii_case(stem.as_bytes()))
            })
            .map(|row| row as u16)
    }

    /// Rows, base and appended.
    pub fn len(&self) -> usize {
        self.names.len()
    }

    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }

    /// How many more rows a FID can address.
    pub fn remaining(&self) -> usize {
        FID_INDEX_CAP.saturating_sub(self.names.len())
    }

    /// The list file's bytes.
    pub fn to_bytes(&self) -> Vec<u8> {
        self.bytes.clone()
    }

    /// The engine's art catalog over this list alone (default alias 0).
    pub fn catalog(&self) -> Result<ArtCatalog, EmitError> {
        let mut catalog = ArtCatalog::default();
        catalog.insert(
            CRITTER_CATEGORY,
            LstTable::parse(&self.bytes, LstKind::Art)?,
        );
        Ok(catalog)
    }

    /// The FID of `row`, `animation` and `weapon` ([`critter_fid`]) and the
    /// file the engine asks for, from `ArtCatalog::filename` and
    /// `ArtCatalog::path` over this list. That name is checked against the
    /// one the row's stem gives (the alias row's stem for the animations
    /// the engine aliases), so a row the engine would read under another
    /// name is an error rather than a missing file at run time.
    pub fn art(&self, row: u16, animation: u8, weapon: u8) -> Result<CritterArt, EmitError> {
        let fid = critter_fid(row, animation, weapon)?;
        let table = LstTable::parse(&self.bytes, LstKind::Art)?;
        let entry = |row: u16| {
            table.art_id(u32::from(row)).ok_or_else(|| {
                EmitError::Critter(format!("the list has {} rows, not row {row}", self.len()))
            })
        };
        let mut source = row;
        if ALIASED_ANIMATIONS.contains(&animation) {
            source = (entry(row)?.critter_alias(0) as u32 & 0xfff) as u16;
        }
        let stem = entry(source)?.name.clone().unwrap_or_default();
        let suffix = critter_suffix(animation, weapon)
            .map_err(|error| EmitError::Critter(error.to_string()))?;
        let mut expected = stem;
        expected.extend_from_slice(&suffix);
        expected.extend_from_slice(b".frm");
        let mut catalog = ArtCatalog::default();
        catalog.insert(CRITTER_CATEGORY, table.clone());
        let resolve = |result: Result<String, vault_formats::FidError>| {
            result.map_err(|error| EmitError::Path {
                path: format!("{fid:?}"),
                message: error.to_string(),
            })
        };
        let file_name = resolve(catalog.filename(fid))?;
        let path = resolve(catalog.path(fid))?;
        if file_name.as_bytes() != expected.as_slice() {
            return Err(EmitError::Critter(format!(
                "row {row} resolves to {file_name:?}, not {:?}",
                String::from_utf8_lossy(&expected)
            )));
        }
        Ok(CritterArt {
            fid,
            file_name,
            path,
        })
    }
}

fn directory(category: u8) -> Result<&'static str, EmitError> {
    ART_DIRECTORIES
        .get(usize::from(category))
        .copied()
        .ok_or_else(|| EmitError::Path {
            path: format!("art category {category}"),
            message: "art categories are 0 to 10".into(),
        })
}
