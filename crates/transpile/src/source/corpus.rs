//! Every script of a plugin list, for whole-corpus checks of the front end
//! (`examples/parse_all.rs`): not a build input.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::{SimSource, SourceBody};
use crate::ir::FormKey;

/// A result script embedded in another record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmbeddedScript {
    /// The record it is embedded in.
    pub owner: FormKey,
    /// That record's signature (`INFO`, `QUST`, `TERM`, `PACK`).
    pub owner_sig: String,
    /// Where in it: `result begin`, `result end`, `stage 10 entry 0`,
    /// `item 2`, `on begin`, ...
    pub context: String,
    /// The script.
    pub body: SourceBody,
}

/// Every script record and embedded script, with what resolves their
/// names.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Corpus {
    /// Plugins, names, reference bases, base scripts, every script record,
    /// and every quest with its script (stages and objectives left empty).
    pub source: SimSource,
    /// Every embedded script, by owner then file order.
    pub embedded: Vec<EmbeddedScript>,
    /// The speaker of each INFO that names one (`ANAM`, else the first
    /// `GetIsID` on the subject).
    pub info_speakers: BTreeMap<FormKey, FormKey>,
}
