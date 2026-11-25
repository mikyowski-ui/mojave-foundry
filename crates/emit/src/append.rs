//! Appenders for the data files a simulation layer extends: the base
//! game's file kept byte for byte, new rows after it, and the numbers the
//! new rows got.
//!
//! | File | Function | A new row's number |
//! |---|---|---|
//! | `text/<lang>/dialog/*.msg`, `game/*.msg` | [`append_messages`], [`append_message_entries`] | message id |
//! | `data/vault13.gam` | [`append_globals`] | global index, from the base's count |
//! | `scripts/scripts.lst` | [`append_script_rows`] | row index, from the base's row count |
//! | `data/quests.txt` | [`append_quests`] | record ordinal among the valid lines |
//! | `data/holodisk.txt` | [`append_holodisks`] | record ordinal among the valid lines |
//!
//! **Numbering is by key.** Every appender takes rows as `(key, row)` and
//! numbers them in ascending key order, whatever order they come in, so a
//! caller that keys rows by a stable identifier (the simulation layer's
//! `SimId`) gets the same numbers and the same bytes from any input order
//! (`docs/simulation-layer-spec.md` section 10.2). Two rows with one key are
//! refused. The returned [`Appended::numbers`] list is in key order.
//!
//! Each result is checked by reading it back with the engine's own parser:
//! the base's records are still there, in place, and the new ones follow.
//! A base the engine's parser refuses is refused here, as the engine would
//! refuse it.

use crate::EmitError;
use crate::gam::{GamVar, write_gam_rows};
use crate::lst::{LstLine, encode_line, terminate_list};
use crate::msg::{MSG_MAX_RECORDS, MsgEntry, MsgFile};
use std::collections::BTreeSet;
use std::fmt::Debug;
use vault_formats::fallout2::lines;
use vault_formats::fallout2::text::{Diagnostics, TableError};
use vault_formats::{GamFile, GamSection, LstKind, LstTable};

/// A new file and the numbers its rows were given.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Appended<K, N> {
    /// The whole file: the base's bytes, then the new rows.
    pub bytes: Vec<u8>,
    /// `(key, number)` of every new row, in ascending key order.
    pub numbers: Vec<(K, N)>,
}

fn rows_by_key<'a, K: Ord + Debug, R>(
    file: &str,
    rows: &'a [(K, R)],
) -> Result<Vec<&'a (K, R)>, EmitError> {
    let mut sorted: Vec<&(K, R)> = rows.iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));
    if let Some(pair) = sorted.windows(2).find(|pair| pair[0].0 == pair[1].0) {
        return Err(EmitError::Append(format!(
            "{file}: two rows have the key {:?}",
            pair[0].0
        )));
    }
    Ok(sorted)
}

fn table_error(error: TableError) -> EmitError {
    EmitError::Append(error.to_string())
}

// ---- Messages ------------------------------------------------------------

/// Append records with chosen ids to the message file `base` (empty for a
/// new file), after its own records and in ascending id order. An id the
/// base holds, or two entries with one id, is refused.
///
/// The text rules are the writer's ([`crate::msg::write_msg`]): no `}`, LF,
/// NUL or `0x1A`, at most 1,023 bytes.
pub fn append_message_entries(base: &[u8], entries: &[MsgEntry]) -> Result<Vec<u8>, EmitError> {
    let mut file = MsgFile::from_base(base)?;
    let mut sorted: Vec<&MsgEntry> = entries.iter().collect();
    sorted.sort_by_key(|entry| entry.id);
    for entry in sorted {
        file.push(entry)?;
    }
    Ok(file.to_bytes())
}

/// Append one record per row to the message file `base`, numbering them
/// `first_id`, `first_id + 1`, ... in key order. The text is raw bytes in
/// the game's encoding with no sound. An id the base already holds is
/// refused (the base's ids are not skipped: a collision means two layers
/// claim one range).
pub fn append_messages<K: Ord + Clone + Debug>(
    base: &[u8],
    first_id: i32,
    rows: &[(K, Vec<u8>)],
) -> Result<Appended<K, i32>, EmitError> {
    let sorted = rows_by_key("messages", rows)?;
    if sorted.len() > MSG_MAX_RECORDS {
        return Err(EmitError::Append(format!(
            "messages: {} rows, more than {MSG_MAX_RECORDS}",
            sorted.len()
        )));
    }
    let mut file = MsgFile::from_base(base)?;
    let mut numbers = Vec::with_capacity(sorted.len());
    for (rank, (key, text)) in sorted.into_iter().enumerate() {
        let id = i32::try_from(rank)
            .ok()
            .and_then(|rank| first_id.checked_add(rank))
            .ok_or_else(|| {
                EmitError::Append(format!(
                    "messages: {first_id} and {} more ids overflow 32 bits",
                    rank
                ))
            })?;
        file.push(&MsgEntry::new(id, text.clone()))?;
        numbers.push((key.clone(), id));
    }
    Ok(Appended {
        bytes: file.to_bytes(),
        numbers,
    })
}

// ---- Campaign globals --------------------------------------------------------

/// Append global variables to `data/vault13.gam` (`base`; empty for a new
/// file, which gets the marker line). The base keeps its text and its
/// numbering; row `i` in key order is global `G + i`, where `G` is the
/// number of values the base allocates. The base must hold the
/// `GAME_GLOBAL_VARS` marker unless it is empty.
///
/// A layer that grows this file changes the campaign's global count, so
/// saves made without it do not load with it (spec 10.2).
pub fn append_globals<K: Ord + Clone + Debug>(
    base: &[u8],
    rows: &[(K, GamVar)],
) -> Result<Appended<K, usize>, EmitError> {
    let sorted = rows_by_key("globals", rows)?;
    let (mut bytes, existing) = if base.is_empty() {
        (
            crate::gam::marker_line(GamSection::Campaign)
                .as_bytes()
                .to_vec(),
            Vec::new(),
        )
    } else {
        let parsed = GamFile::parse(base, GamSection::Campaign)?;
        if !parsed.found {
            return Err(EmitError::Append(
                "globals: the base file has no GAME_GLOBAL_VARS marker".into(),
            ));
        }
        let mut bytes = base.to_vec();
        if !bytes.ends_with(b"\n") {
            bytes.push(b'\n');
        }
        (bytes, parsed.values)
    };
    let vars: Vec<GamVar> = sorted.iter().map(|(_, var)| var.clone()).collect();
    bytes.extend_from_slice(&write_gam_rows(existing.len(), &vars)?);
    // The engine reads the whole file back as the base's values, then ours.
    let back = GamFile::parse(&bytes, GamSection::Campaign)?;
    let expected: Vec<i32> = existing
        .iter()
        .copied()
        .chain(vars.iter().map(|var| var.value))
        .collect();
    if !back.found || back.values != expected {
        return Err(EmitError::Append(
            "globals: the appended file does not read back as the base's values then ours".into(),
        ));
    }
    let numbers = sorted
        .iter()
        .enumerate()
        .map(|(rank, (key, _))| (key.clone(), existing.len() + rank))
        .collect();
    Ok(Appended { bytes, numbers })
}

// ---- scripts.lst -----------------------------------------------------------

/// One `scripts.lst` row: `<stem>.int ; <editor id> # local_vars=<n>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptRow {
    /// The program's file stem (letters, digits, `_` and `-`); the row names
    /// `scripts/<stem>.int`.
    pub stem: String,
    /// The comment after `;`: ASCII, no `#`, no line break. It is read by
    /// nothing; it names the source form for a person.
    pub editor_id: Option<String>,
    /// The row's local variable count, which the engine allocates for each
    /// instance.
    pub local_vars: u32,
}

/// Append script rows to `scripts/scripts.lst` (`base`; the whole base
/// list, which is what a layer ships, spec decision 7). The base's lines
/// are kept byte for byte at their positions; row `i` in key order is
/// script index `rows_in_base + i`. A stem that the base or another new
/// row already uses (compared without case) is refused, as it would give
/// one program two indexes.
pub fn append_script_rows<K: Ord + Clone + Debug>(
    base: &[u8],
    rows: &[(K, ScriptRow)],
) -> Result<Appended<K, u32>, EmitError> {
    let sorted = rows_by_key("scripts.lst", rows)?;
    let table = LstTable::parse(base, LstKind::Script)?;
    let existing = table.entries().len();
    let mut stems: BTreeSet<Vec<u8>> = table
        .entries()
        .iter()
        .filter_map(|entry| entry.name.as_ref())
        .map(|name| name.to_ascii_lowercase())
        .collect();
    let mut bytes = terminate_list(base, LstKind::Script, existing)?;
    let mut numbers = Vec::with_capacity(sorted.len());
    for (rank, (key, row)) in sorted.into_iter().enumerate() {
        let position = existing + rank + 1;
        let line = LstLine::Script {
            stem: row.stem.clone(),
            local_vars: Some(i32::try_from(row.local_vars).map_err(|_| EmitError::Lst {
                line: position,
                message: format!("local_vars {} does not fit 31 bits", row.local_vars),
            })?),
            comment: row.editor_id.clone(),
        };
        bytes.extend_from_slice(&encode_line(&line).map_err(|message| EmitError::Lst {
            line: position,
            message,
        })?);
        let file = format!("{}.int", row.stem)
            .to_ascii_lowercase()
            .into_bytes();
        if !stems.insert(file) {
            return Err(EmitError::Lst {
                line: position,
                message: format!("{}.int is already a row of the list", row.stem),
            });
        }
        numbers.push((key.clone(), (existing + rank) as u32));
    }
    // Every old line is where it was, and ours follow. A last base line
    // that had no LF now has one: its `raw` differs by that byte only.
    let back = LstTable::parse(&bytes, LstKind::Script)?;
    let unterminated = |raw: &[u8]| raw.strip_suffix(b"\n").unwrap_or(raw).to_vec();
    let kept = back.entries().len() == existing + numbers.len()
        && bytes.starts_with(base)
        && back.entries()[..existing]
            .iter()
            .zip(table.entries())
            .all(|(new, old)| {
                unterminated(&new.raw) == unterminated(&old.raw)
                    && (&new.name, new.alias, new.head_columns, new.local_vars)
                        == (&old.name, old.alias, old.head_columns, old.local_vars)
            });
    if !kept {
        return Err(EmitError::Append(
            "scripts.lst: the appended list does not keep the base's lines in place".into(),
        ));
    }
    Ok(Appended { bytes, numbers })
}

// ---- Pip-Boy tables ----------------------------------------------------------

/// One `data/quests.txt` record (SPEC-018 section 4.6, 5.9).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuestRow {
    /// A `map.msg` message id: the location heading.
    pub location: i32,
    /// A `quests.msg` message id: the description.
    pub description: i32,
    /// The campaign global that holds the quest's state.
    pub global: i32,
    /// The quest is listed from this value of the global.
    pub display: i32,
    /// The quest is completed from this value.
    pub completed: i32,
}

/// One `data/holodisk.txt` record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HolodiskRow {
    /// The campaign global that, above 0, means the disk is held.
    pub global: i32,
    /// A `pipboy.msg` message id: the title.
    pub title: i32,
    /// The first of the 500 `pipboy.msg` ids of the text.
    pub text: i32,
}

/// `base` as a line table the engine accepts, ready for more lines: the
/// bytes with a final LF, and the count of valid records it holds.
fn line_table(
    base: &[u8],
    count: impl FnOnce(&[u8]) -> Result<usize, TableError>,
) -> Result<(Vec<u8>, usize), EmitError> {
    let records = count(base).map_err(table_error)?;
    let mut bytes = base.to_vec();
    if !bytes.is_empty() && !bytes.ends_with(b"\n") {
        bytes.push(b'\n');
    }
    // A line over the engine's limit, or a NUL, refuses the file; the new
    // lines are short, so only the base can do that, and `count` saw it.
    Ok((bytes, records))
}

/// Append quests to `data/quests.txt` (`base`, empty when the file is
/// absent): the base's lines kept, then one `a, b, c, d, e` line per row.
/// Row `i` in key order has ordinal `valid_base_records + i`.
pub fn append_quests<K: Ord + Clone + Debug>(
    base: &[u8],
    rows: &[(K, QuestRow)],
) -> Result<Appended<K, usize>, EmitError> {
    let sorted = rows_by_key("quests.txt", rows)?;
    let (mut bytes, existing) = line_table(base, |bytes| {
        lines::quests(bytes, &mut Diagnostics::default()).map(|quests| quests.len())
    })?;
    for (_, row) in &sorted {
        bytes.extend_from_slice(
            format!(
                "{}, {}, {}, {}, {}\n",
                row.location, row.description, row.global, row.display, row.completed
            )
            .as_bytes(),
        );
    }
    let back = lines::quests(&bytes, &mut Diagnostics::default()).map_err(table_error)?;
    let ours: Vec<_> = sorted.iter().map(|(_, row)| *row).collect();
    let read_back = back.len() == existing + ours.len()
        && back[existing..].iter().zip(&ours).all(|(read, row)| {
            (
                read.location,
                read.description,
                read.global,
                read.display,
                read.completed,
            ) == (
                row.location,
                row.description,
                row.global,
                row.display,
                row.completed,
            )
        });
    if !read_back {
        return Err(EmitError::Append(
            "quests.txt: the appended rows do not read back".into(),
        ));
    }
    let numbers = sorted
        .iter()
        .enumerate()
        .map(|(rank, (key, _))| (key.clone(), existing + rank))
        .collect();
    Ok(Appended { bytes, numbers })
}

/// Append holodisks to `data/holodisk.txt` (`base`, empty when the file is
/// absent): the base's lines kept, then one `a, b, c` line per row. Row `i`
/// in key order has ordinal `valid_base_records + i`.
pub fn append_holodisks<K: Ord + Clone + Debug>(
    base: &[u8],
    rows: &[(K, HolodiskRow)],
) -> Result<Appended<K, usize>, EmitError> {
    let sorted = rows_by_key("holodisk.txt", rows)?;
    let (mut bytes, existing) = line_table(base, |bytes| {
        lines::holodisks(bytes, &mut Diagnostics::default()).map(|disks| disks.len())
    })?;
    for (_, row) in &sorted {
        bytes
            .extend_from_slice(format!("{}, {}, {}\n", row.global, row.title, row.text).as_bytes());
    }
    let back = lines::holodisks(&bytes, &mut Diagnostics::default()).map_err(table_error)?;
    let ours: Vec<_> = sorted.iter().map(|(_, row)| *row).collect();
    let read_back = back.len() == existing + ours.len()
        && back[existing..].iter().zip(&ours).all(|(read, row)| {
            (read.global, read.title, read.text) == (row.global, row.title, row.text)
        });
    if !read_back {
        return Err(EmitError::Append(
            "holodisk.txt: the appended rows do not read back".into(),
        ));
    }
    let numbers = sorted
        .iter()
        .enumerate()
        .map(|(rank, (key, _))| (key.clone(), existing + rank))
        .collect();
    Ok(Appended { bytes, numbers })
}
