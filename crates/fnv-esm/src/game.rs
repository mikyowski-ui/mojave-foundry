//! Which game a plugin list belongs to.
//!
//! Fallout 3 and Fallout: New Vegas share the plugin layout this crate
//! reads: the same 24-byte headers, groups, compression and subrecord
//! framing. They differ in the `HEDR` version of their plugins, in a few
//! subrecord sizes (which the decoders tell apart by size, not by game),
//! and in record types that only New Vegas has.
//!
//! Measured on the retail masters: every Fallout 3 plugin (the master and
//! its five add-ons) declares `HEDR` version 0.94, every New Vegas master
//! 1.34.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::error::EsmError;
use crate::sig::Sig;
use crate::walker::Tes4Header;

/// The game a plugin list was made for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum GameVariant {
    /// Fallout 3: `Fallout3.esm`, `HEDR` version 0.94.
    #[serde(rename = "fallout3")]
    Fallout3,
    /// Fallout: New Vegas: `FalloutNV.esm`, `HEDR` version 1.34.
    #[serde(rename = "falloutnv")]
    FalloutNewVegas,
}

/// Record types that only New Vegas plugins have: reputations, recipes
/// and their categories, weapon mods, challenges, casinos and Caravan
/// cards, decks and money, the hardcore needs, ammunition effects, the
/// load screen types and recipe categories. A Fallout 3 master has none.
pub const NEW_VEGAS_ONLY: &[Sig] = &[
    Sig::new(b"AMEF"),
    Sig::new(b"CCRD"),
    Sig::new(b"CDCK"),
    Sig::new(b"CHAL"),
    Sig::new(b"CMNY"),
    Sig::new(b"CSNO"),
    Sig::new(b"DEHY"),
    Sig::new(b"HUNG"),
    Sig::new(b"IMOD"),
    Sig::new(b"LSCT"),
    Sig::new(b"RCCT"),
    Sig::new(b"RCPE"),
    Sig::new(b"REPU"),
    Sig::new(b"SLPD"),
];

/// Whether a record type is one that only New Vegas has
/// ([`NEW_VEGAS_ONLY`]).
pub fn is_new_vegas_only(sig: Sig) -> bool {
    NEW_VEGAS_ONLY.contains(&sig)
}

impl GameVariant {
    /// Both games, Fallout 3 first.
    pub const ALL: [Self; 2] = [Self::Fallout3, Self::FalloutNewVegas];

    /// How far a `HEDR` version may be from a game's own and still count
    /// as that game. The stored value is a 32-bit float, so 0.94 and 1.34
    /// are not exact; 0.01 is far tighter than the 0.4 between them.
    pub const VERSION_TOLERANCE: f32 = 0.01;

    /// The `HEDR` version this game's plugins declare.
    pub const fn hedr_version(self) -> f32 {
        match self {
            Self::Fallout3 => 0.94,
            Self::FalloutNewVegas => 1.34,
        }
    }

    /// The file name of the game's master, as shipped.
    pub const fn master_name(self) -> &'static str {
        match self {
            Self::Fallout3 => "Fallout3.esm",
            Self::FalloutNewVegas => "FalloutNV.esm",
        }
    }

    /// A short stable identifier: `"fallout3"` or `"falloutnv"`. It is the
    /// JSON form too.
    pub const fn id(self) -> &'static str {
        match self {
            Self::Fallout3 => "fallout3",
            Self::FalloutNewVegas => "falloutnv",
        }
    }

    /// The game's name for people.
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Fallout3 => "Fallout 3",
            Self::FalloutNewVegas => "Fallout: New Vegas",
        }
    }

    /// The game whose `HEDR` version is within [`Self::VERSION_TOLERANCE`]
    /// of `version`.
    pub fn from_hedr_version(version: f32) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|game| (version - game.hedr_version()).abs() <= Self::VERSION_TOLERANCE)
    }

    /// The game whose master has this file name (case does not matter).
    /// Any other name, an add-on's included, gives `None`.
    pub fn from_master_name(file_name: &str) -> Option<Self> {
        let name = file_name.rsplit(['/', '\\']).next().unwrap_or(file_name);
        Self::ALL
            .into_iter()
            .find(|game| game.master_name().eq_ignore_ascii_case(name))
    }

    /// Decides the game of a plugin list from its first plugin: its `HEDR`
    /// version, cross-checked against its file name when that is one of
    /// the two masters.
    ///
    /// `overridden` wins outright when given. Otherwise the version must
    /// be one of the two games' (within [`Self::VERSION_TOLERANCE`]); a
    /// master named for one game whose version is the other's is refused
    /// too.
    ///
    /// # Errors
    ///
    /// [`EsmError::UnknownGame`] for a version that is neither game's, and
    /// [`EsmError::GameMismatch`] when the name and the version disagree.
    pub fn detect(
        header: &Tes4Header,
        file_name: &str,
        overridden: Option<Self>,
    ) -> Result<Self, EsmError> {
        if let Some(game) = overridden {
            return Ok(game);
        }
        let Some(by_version) = Self::from_hedr_version(header.version) else {
            return Err(EsmError::UnknownGame {
                plugin: file_name.to_owned(),
                version: header.version.to_string(),
            });
        };
        match Self::from_master_name(file_name) {
            Some(by_name) if by_name != by_version => Err(EsmError::GameMismatch {
                plugin: file_name.to_owned(),
                version: header.version.to_string(),
                version_game: by_version,
                name_game: by_name,
            }),
            _ => Ok(by_version),
        }
    }
}

impl fmt::Display for GameVariant {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.id())
    }
}

impl FromStr for GameVariant {
    type Err = String;

    /// Reads [`GameVariant::id`], ignoring ASCII case; `"fo3"`, `"fnv"`
    /// and `"newvegas"` are accepted too.
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        match text.trim().to_ascii_lowercase().as_str() {
            "fallout3" | "fo3" => Ok(Self::Fallout3),
            "falloutnv" | "fnv" | "newvegas" | "new_vegas" => Ok(Self::FalloutNewVegas),
            other => Err(format!(
                "{other:?} is not a game: expected \"fallout3\" or \"falloutnv\""
            )),
        }
    }
}
