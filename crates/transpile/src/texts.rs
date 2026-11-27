//! The text table behind `Module::texts`, and the converter's own labels
//! (spec 3.2, 4.10): the only text this repository writes.

use std::collections::HashMap;

use crate::ir::{SimId, Text, TextId, TextRole};

/// The converter labels: number and English text. Numbers 1 to 99 are the
/// dialogue message ids of spec 4.10; they are stable, and a later pass
/// adds labels after the last one.
pub const LABELS: &[(u32, &str)] = &[
    (1, "[Continue]"),
    (2, "Goodbye."),
    (3, "[Done]"),
    (4, "Back"),
    (5, "Exit"),
    (6, "[Speech "),
    (7, "%] "),
    (8, "["),
    (9, "] "),
    (10, "/"),
    (11, "[Password]"),
    (12, "[Science "),
    (13, "] Hack"),
    (14, "Note added: "),
    (15, "Unlocked with the key."),
    (16, "The lock is too hard."),
    (17, "Speech"),
    (18, "Barter"),
    (19, "Medicine"),
    (20, "Science"),
    (21, "Explosives"),
    (22, "Lockpick"),
    (23, "Repair"),
    (24, "Sneak"),
    (25, "Strength"),
    (26, "Perception"),
    (27, "Endurance"),
    (28, "Charisma"),
    (29, "Intelligence"),
    (30, "Agility"),
    (31, "Luck"),
    (32, "(Waits for you to speak.)"),
];

/// The label said when no greeting INFO passes (spec 4.9): the reply
/// above the hub.
pub const GREETING_LABEL: u32 = 32;

/// Builds `Module::texts`: interns each (owner, role, text) once, in the
/// order of the calls. The build calls it in [`SimId`] order, so the
/// numbering is stable under reordering of the input records.
#[derive(Debug, Default)]
pub struct TextTable {
    texts: Vec<Text>,
    seen: HashMap<(SimId, TextRole, String), TextId>,
}

impl TextTable {
    /// An empty table.
    pub fn new() -> Self {
        Self::default()
    }

    /// A table holding the converter labels first.
    pub fn with_labels() -> Self {
        let mut table = Self::new();
        for &(number, text) in LABELS {
            table.intern(&SimId::label(number), TextRole::Label, text);
        }
        table
    }

    /// The id of a text, added if new.
    pub fn intern(&mut self, owner: &SimId, role: TextRole, source: &str) -> TextId {
        let key = (owner.clone(), role, source.to_owned());
        if let Some(&id) = self.seen.get(&key) {
            return id;
        }
        let id = TextId(u32::try_from(self.texts.len()).unwrap_or(u32::MAX));
        self.texts.push(Text {
            owner: owner.clone(),
            source: source.to_owned(),
            role,
        });
        self.seen.insert(key, id);
        id
    }

    /// The id of an optional text; `None` for an absent or empty one.
    pub fn intern_opt(
        &mut self,
        owner: &SimId,
        role: TextRole,
        source: Option<&str>,
    ) -> Option<TextId> {
        source
            .filter(|s| !s.is_empty())
            .map(|s| self.intern(owner, role, s))
    }

    /// The id of a label by number.
    pub fn label(&mut self, number: u32) -> Option<TextId> {
        LABELS
            .iter()
            .find(|(n, _)| *n == number)
            .map(|&(n, text)| self.intern(&SimId::label(n), TextRole::Label, text))
    }

    /// How many texts it holds.
    pub fn len(&self) -> usize {
        self.texts.len()
    }

    /// Whether it holds none.
    pub fn is_empty(&self) -> bool {
        self.texts.is_empty()
    }

    /// The texts, by id.
    pub fn into_texts(self) -> Vec<Text> {
        self.texts
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{FormKey, SimKind};

    #[test]
    fn interning_dedupes_per_owner_and_role() {
        let mut table = TextTable::with_labels();
        let base = table.len();
        let a = SimId::of(SimKind::Info, &FormKey::new("x.esm", 1));
        let first = table.intern(&a, TextRole::Response, "Hello.");
        assert_eq!(first, TextId(base as u32));
        assert_eq!(table.intern(&a, TextRole::Response, "Hello."), first);
        assert_ne!(table.intern(&a, TextRole::Prompt, "Hello."), first);
        assert_eq!(table.intern_opt(&a, TextRole::Prompt, Some("")), None);
        assert_eq!(table.label(1), Some(TextId(0)));
        let numbers: Vec<u32> = LABELS.iter().map(|(n, _)| *n).collect();
        let mut sorted = numbers.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(numbers, sorted);
        assert!(numbers.iter().all(|n| (1..=99).contains(n)));
    }
}
