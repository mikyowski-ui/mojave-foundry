//! The condition function table of spec 4.8, with the package-condition
//! additions of the same section, the speaker terms of 4.2 and the
//! threshold reads of 4.7. One row per function index; the lowering says
//! what [`super::lower`] makes of a term.

use std::collections::HashMap;
use std::sync::OnceLock;

/// How a condition function lowers.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Lowering {
    /// A runtime call of the signature table's function (spec 4.8's
    /// engine column is task T8's).
    Runtime,
    /// Folded at build time on the subject when the facts are known
    /// (spec 4.2, [`super::fold`]); a runtime call otherwise.
    Static,
    /// A variable read: `GetGlobalValue`, `GetQuestVariable`,
    /// `GetScriptVariable`.
    Variable,
    /// An actor value read (spec 4.7): a runtime call for the values with
    /// an engine read, 100 for the body-part conditions, else a reported
    /// fallback of 0.
    ActorValue,
    /// Not evaluable: the term takes `value`.
    Fallback {
        /// The value the function is taken to return.
        value: f32,
        /// Whether the report lists it as a loss.
        report: bool,
    },
}

/// One row.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CondFunction {
    /// The function index.
    pub index: u16,
    /// The public name, or `"(index n)"`.
    pub name: &'static str,
    /// How it lowers.
    pub lowering: Lowering,
}

const fn row(index: u16, name: &'static str, lowering: Lowering) -> CondFunction {
    CondFunction {
        index,
        name,
        lowering,
    }
}

const fn fallback(index: u16, name: &'static str, value: f32, report: bool) -> CondFunction {
    row(index, name, Lowering::Fallback { value, report })
}

use Lowering::{ActorValue, Runtime, Static, Variable};

/// The table, in the order of spec 4.8 (by count in the slice), then the
/// package additions, the speaker terms and the threshold reads.
pub static TABLE: &[CondFunction] = &[
    row(72, "GetIsID", Static),
    row(79, "GetQuestVariable", Variable),
    row(58, "GetStage", Runtime),
    row(14, "GetActorValue", ActorValue),
    row(47, "GetItemCount", Runtime),
    row(67, "GetInCell", Runtime),
    row(50, "GetTalkedToPC", Runtime),
    row(59, "GetStageDone", Runtime),
    row(131, "GetPCIsSex", Runtime),
    row(53, "GetScriptVariable", Variable),
    row(70, "GetIsSex", Static),
    row(182, "GetEquipped", Runtime),
    row(74, "GetGlobalValue", Variable),
    row(71, "GetInFaction", Static),
    row(161, "GetIsCurrentPackage", Runtime),
    row(431, "GetHealthPercentage", Runtime),
    row(1, "GetDistance", Runtime),
    row(35, "GetDisabled", Runtime),
    fallback(446, "GetInZone", 0.0, true),
    row(300, "IsInInterior", Static),
    row(84, "GetDeadCount", Runtime),
    fallback(117, "(index 117)", 0.0, true),
    fallback(255, "GetOffersServicesNow", 1.0, false),
    fallback(449, "HasPerk", 0.0, true),
    fallback(492, "GetMapMarkerVisible", 0.0, true),
    row(56, "GetQuestRunning", Runtime),
    row(289, "IsInCombat", Runtime),
    row(46, "GetDead", Runtime),
    fallback(76, "GetDisposition", 50.0, false),
    fallback(415, "Exists", 1.0, false),
    fallback(403, "HasFriendDisposition", 1.0, false),
    row(18, "GetCurrentTime", Runtime),
    row(77, "GetRandomPercent", Runtime),
    row(382, "GetHasNote", Runtime),
    fallback(515, "IsCombatTarget", 0.0, true),
    fallback(49, "GetSleeping", 0.0, true),
    fallback(323, "WhichServiceMenu", 0.0, true),
    // Package conditions (spec 4.8, last paragraph).
    fallback(62, "IsRaining", 0.0, true),
    row(310, "GetInWorldspace", Runtime),
    row(32, "GetInSameCell", Runtime),
    row(136, "GetIsReference", Static),
    row(45, "GetDetected", Runtime),
    // Speaker terms (spec 4.2).
    row(427, "GetIsVoiceType", Static),
    row(69, "GetIsRace", Static),
    row(68, "GetIsClass", Static),
    row(64, "GetIsCreature", Static),
    // Threshold reads (spec 4.7).
    row(277, "GetBaseActorValue", ActorValue),
    row(495, "GetPermanentActorValue", ActorValue),
];

/// The row of an index.
pub fn by_index(index: u16) -> Option<&'static CondFunction> {
    static INDEX: OnceLock<HashMap<u16, &'static CondFunction>> = OnceLock::new();
    INDEX
        .get_or_init(|| TABLE.iter().map(|f| (f.index, f)).collect())
        .get(&index)
        .copied()
}

/// The name of an index: the table's, or `"(index n)"`.
pub fn name_of(index: u16) -> String {
    by_index(index).map_or_else(|| format!("(index {index})"), |f| f.name.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::obscript::functions;

    #[test]
    fn indices_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for f in TABLE {
            assert!(seen.insert(f.index), "index {}", f.index);
        }
    }

    /// Every evaluable row is a function of the signature table with the
    /// same index, so that its call has a `FuncId`.
    #[test]
    fn evaluable_rows_are_in_the_signature_table() {
        for f in TABLE {
            if matches!(f.lowering, Lowering::Fallback { .. }) && f.name.starts_with('(') {
                continue;
            }
            let sig = functions::lookup(f.name).unwrap_or_else(|| panic!("{} missing", f.name));
            assert_eq!(sig.index, Some(f.index), "{}", f.name);
            assert_eq!(functions::by_index(f.index).map(|s| s.name), Some(sig.name));
        }
    }
}
