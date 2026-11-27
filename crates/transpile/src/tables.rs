//! The non-program files of the layer (spec 10.1), through the `emit`
//! appenders: the whole-copy `scripts.lst` with our rows appended
//! (owner decision 7), the per-program dialogue `.msg` files, the
//! `vault13.gam` additions, `quests.txt` and `holodisk.txt`, and the
//! shared `quests.msg`, `map.msg` and `pipboy.msg` additions.
//!
//! Text goes through [`msg_text`]: Windows-1252, braces as parentheses,
//! line breaks and tabs as spaces, other control characters dropped,
//! characters outside Windows-1252 as `?`; every change is counted.

use std::collections::BTreeMap;

use emit::{
    GamVar, HolodiskRow, MsgEntry, QuestRow, ScriptRow, append_globals, append_holodisks,
    append_message_entries, append_quests, append_script_rows,
};

use crate::codegen::Program;
use crate::diag::{Category, Diag};
use crate::ir::{BaseFiles, GlobalPurpose, Module, NoteBody, SimId};
use crate::numbering::{HOLODISK_TEXT, Numbering, SHARED_FIRST};

/// The most text bytes of one message field.
const FIELD_BYTES: usize = 1023;
/// Characters per holodisk line: font 101 over 370 pixels at about seven
/// pixels a character (an approximation until the font's widths are
/// read).
pub const HOLODISK_LINE: usize = 50;
/// The most lines of one holodisk text window, before `**END-DISK**`.
pub const HOLODISK_LINES: usize = 499;

/// The Windows-1252 byte of a character, if it has one.
fn cp1252(c: char) -> Option<u8> {
    let code = c as u32;
    if code < 0x80 || (0xa0..=0xff).contains(&code) {
        return Some(code as u8);
    }
    Some(match c {
        '\u{20ac}' => 0x80,
        '\u{201a}' => 0x82,
        '\u{0192}' => 0x83,
        '\u{201e}' => 0x84,
        '\u{2026}' => 0x85,
        '\u{2020}' => 0x86,
        '\u{2021}' => 0x87,
        '\u{02c6}' => 0x88,
        '\u{2030}' => 0x89,
        '\u{0160}' => 0x8a,
        '\u{2039}' => 0x8b,
        '\u{0152}' => 0x8c,
        '\u{017d}' => 0x8e,
        '\u{2018}' => 0x91,
        '\u{2019}' => 0x92,
        '\u{201c}' => 0x93,
        '\u{201d}' => 0x94,
        '\u{2022}' => 0x95,
        '\u{2013}' => 0x96,
        '\u{2014}' => 0x97,
        '\u{02dc}' => 0x98,
        '\u{2122}' => 0x99,
        '\u{0161}' => 0x9a,
        '\u{203a}' => 0x9b,
        '\u{0153}' => 0x9c,
        '\u{017e}' => 0x9e,
        '\u{0178}' => 0x9f,
        _ => return None,
    })
}

/// A text as message bytes (see the module notes); `changes` counts every
/// character changed.
pub fn msg_text(text: &str, changes: &mut u32) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '{' => {
                out.push(b'(');
                *changes += 1;
            }
            '}' => {
                out.push(b')');
                *changes += 1;
            }
            '\n' | '\r' | '\t' => {
                out.push(b' ');
                *changes += 1;
            }
            c if (c as u32) < 0x20 || c == '\u{7f}' => *changes += 1,
            c => match cp1252(c) {
                Some(byte) => out.push(byte),
                None => {
                    out.push(b'?');
                    *changes += 1;
                }
            },
        }
        if out.len() >= FIELD_BYTES {
            out.truncate(FIELD_BYTES);
            *changes += 1;
            break;
        }
    }
    out
}

/// Wraps a note's text into holodisk lines (spec 9.2): words to
/// [`HOLODISK_LINE`] characters, `**END-PAR**` between paragraphs, then
/// `**END-DISK**`; cut at [`HOLODISK_LINES`] lines.
pub fn holodisk_lines(text: &str) -> (Vec<String>, bool) {
    let mut lines = Vec::new();
    let normal = text.replace("\r\n", "\n").replace('\r', "\n");
    for (i, paragraph) in normal.split('\n').enumerate() {
        if i > 0 {
            lines.push("**END-PAR**".to_owned());
        }
        let mut line = String::new();
        for word in paragraph.split_whitespace() {
            if !line.is_empty() && line.chars().count() + 1 + word.chars().count() > HOLODISK_LINE {
                lines.push(std::mem::take(&mut line));
            }
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(word);
        }
        if !line.is_empty() {
            lines.push(line);
        }
    }
    let cut = lines.len() > HOLODISK_LINES;
    lines.truncate(HOLODISK_LINES);
    lines.push("**END-DISK**".to_owned());
    (lines, cut)
}

/// What [`build`] made.
#[derive(Debug, Default)]
pub struct Tables {
    /// Layer path to bytes.
    pub files: BTreeMap<String, Vec<u8>>,
    /// Message records written.
    pub messages: u32,
    /// Holodisk rows.
    pub holodisks: u32,
    /// Quest rows.
    pub quest_rows: u32,
    /// Text changes.
    pub text_changes: u32,
    /// Notes whose body was cut, and other findings.
    pub diagnostics: Vec<Diag>,
}

fn bytes(text: &Option<String>) -> &[u8] {
    text.as_deref().map_or(&[], str::as_bytes)
}

fn text_of(module: &Module, id: crate::ir::TextId) -> String {
    module
        .texts
        .get(id.0 as usize)
        .map(|t| t.source.clone())
        .unwrap_or_default()
}

fn mismatch(what: &str) -> String {
    format!("{what}: the appender's numbers differ from the allocation")
}

/// Builds every table file.
pub fn build(
    module: &Module,
    numbers: &Numbering,
    programs: &[Program],
    base: &BaseFiles,
) -> Result<Tables, String> {
    let mut out = Tables::default();
    let mut changes = 0;

    // scripts.lst
    if base.scripts_lst.is_none() {
        out.diagnostics.push(Diag::warning(
            Category::MissingRecord,
            "no base scripts.lst: the layer's list holds only its own rows",
        ));
    }
    let rows: Vec<(String, ScriptRow)> = programs
        .iter()
        .map(|p| {
            (
                p.stem.clone(),
                ScriptRow {
                    stem: p.stem.clone(),
                    editor_id: Some(p.editor_id.clone()),
                    local_vars: p.local_vars,
                },
            )
        })
        .collect();
    let lst = append_script_rows(bytes(&base.scripts_lst), &rows).map_err(|e| e.to_string())?;
    for ((_, row), program) in lst.numbers.iter().zip(programs) {
        if *row != program.row {
            return Err(mismatch("scripts.lst"));
        }
    }
    out.files
        .insert("scripts/scripts.lst".to_owned(), lst.bytes);

    // vault13.gam
    let rows: Vec<((GlobalPurpose, SimId), GamVar)> = numbers
        .globals
        .values()
        .map(|g| {
            (
                (g.global.purpose, g.global.id.clone()),
                GamVar::new(g.gam_name.clone(), g.global.initial),
            )
        })
        .collect();
    let gam = append_globals(bytes(&base.vault13_gam), &rows).map_err(|e| e.to_string())?;
    for ((_, id), index) in &gam.numbers {
        if numbers.global(id) != Some(*index as u32) {
            return Err(mismatch("vault13.gam"));
        }
    }
    out.files.insert("data/vault13.gam".to_owned(), gam.bytes);

    // Dialogue message files.
    for program in programs {
        if program.messages.is_empty() {
            continue;
        }
        let entries: Vec<MsgEntry> = program
            .messages
            .iter()
            .map(|(id, text)| MsgEntry::new(*id, msg_text(text, &mut changes)))
            .collect();
        out.messages += entries.len() as u32;
        let file = append_message_entries(&[], &entries).map_err(|e| e.to_string())?;
        out.files
            .insert(format!("text/english/dialog/{}.msg", program.stem), file);
    }

    // Quests: headings in map.msg, descriptions in quests.msg, rows.
    let mut map_entries = Vec::new();
    let mut quest_entries = Vec::new();
    let mut quest_rows = Vec::new();
    for (quest, journal) in &module.journal {
        let Some(&heading) = numbers.quest_headings.get(quest) else {
            continue;
        };
        map_entries.push(MsgEntry::new(
            heading,
            msg_text(&text_of(module, journal.heading), &mut changes),
        ));
        for row in &journal.rows {
            let Some(&id) = numbers.objective_messages.get(&row.objective) else {
                continue;
            };
            let text = text_of(module, row.text);
            if text.chars().count() > HOLODISK_LINE {
                out.diagnostics.push(
                    Diag::note(
                        Category::MissingRecord,
                        "objective line may run past the Pip-Boy list (about 350 pixels)",
                    )
                    .owned_by(&row.objective),
                );
            }
            quest_entries.push(MsgEntry::new(id, msg_text(&text, &mut changes)));
            let Some(global) = numbers.global(&row.global) else {
                continue;
            };
            quest_rows.push((
                row.objective.clone(),
                QuestRow {
                    location: heading,
                    description: id,
                    global: global as i32,
                    display: row.display_at,
                    completed: row.completed_at,
                },
            ));
        }
    }
    let quests = append_quests(bytes(&base.quests_txt), &quest_rows).map_err(|e| e.to_string())?;
    out.quest_rows = quests.numbers.len() as u32;
    out.files.insert("data/quests.txt".to_owned(), quests.bytes);
    out.messages += (map_entries.len() + quest_entries.len()) as u32;
    let map_msg =
        append_message_entries(bytes(&base.map_msg), &map_entries).map_err(|e| e.to_string())?;
    out.files
        .insert("text/english/game/map.msg".to_owned(), map_msg);
    let quests_msg = append_message_entries(bytes(&base.quests_msg), &quest_entries)
        .map_err(|e| e.to_string())?;
    out.files
        .insert("text/english/game/quests.msg".to_owned(), quests_msg);

    // Holodisks.
    let mut pipboy = Vec::new();
    let mut disks = Vec::new();
    for (id, &k) in &numbers.notes {
        let k = k as i32;
        let (global, title, body) = match module.holodisks.get(id) {
            Some(disk) => {
                let title = disk.title.map(|t| text_of(module, t)).unwrap_or_default();
                let body: Vec<String> = disk
                    .paragraphs
                    .iter()
                    .map(|t| text_of(module, *t))
                    .collect();
                if body.is_empty() {
                    out.diagnostics.push(
                        Diag::note(
                            Category::MissingRecord,
                            "image or sound note: the holodisk shows its title only",
                        )
                        .owned_by(id),
                    );
                }
                let body = if body.is_empty() {
                    title.clone()
                } else {
                    body.join("\n")
                };
                (numbers.global(&disk.global), title, body)
            }
            None => {
                let Some(note) = module.notes.get(id) else {
                    continue;
                };
                let title = note.title.map(|t| text_of(module, t)).unwrap_or_default();
                let body = match &note.body {
                    NoteBody::Text(t) => text_of(module, *t),
                    _ => title.clone(),
                };
                (numbers.global(id), title, body)
            }
        };
        let Some(global) = global else {
            continue;
        };
        pipboy.push(MsgEntry::new(
            SHARED_FIRST + k,
            msg_text(&title, &mut changes),
        ));
        let (lines, cut) = holodisk_lines(&body);
        if cut {
            out.diagnostics.push(
                Diag::warning(Category::MissingRecord, "note body over 499 lines; cut")
                    .owned_by(id),
            );
        }
        let first = HOLODISK_TEXT + 500 * k;
        for (i, line) in lines.iter().enumerate() {
            pipboy.push(MsgEntry::new(
                first + i as i32,
                msg_text(line, &mut changes),
            ));
        }
        disks.push((
            id.clone(),
            HolodiskRow {
                global: global as i32,
                title: SHARED_FIRST + k,
                text: first,
            },
        ));
    }
    let holodisk =
        append_holodisks(bytes(&base.holodisk_txt), &disks).map_err(|e| e.to_string())?;
    out.holodisks = holodisk.numbers.len() as u32;
    out.files
        .insert("data/holodisk.txt".to_owned(), holodisk.bytes);
    out.messages += pipboy.len() as u32;
    let pipboy_msg =
        append_message_entries(bytes(&base.pipboy_msg), &pipboy).map_err(|e| e.to_string())?;
    out.files
        .insert("text/english/game/pipboy.msg".to_owned(), pipboy_msg);

    out.text_changes = changes;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_goes_to_windows_1252_with_counted_changes() {
        let mut changes = 0;
        assert_eq!(
            msg_text("a{b}c\u{2019}\u{3042}", &mut changes),
            b"a(b)c\x92?"
        );
        assert_eq!(changes, 3);
    }

    #[test]
    fn holodisk_lines_wrap_and_mark_paragraphs() {
        let (lines, cut) = holodisk_lines("one two\nthree");
        assert!(!cut);
        assert_eq!(lines, ["one two", "**END-PAR**", "three", "**END-DISK**"]);
    }
}
