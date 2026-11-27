//! Terminals, notes, locks and messages (spec 9, 5.9 `ShowMessage`; task
//! T5).
//!
//! [`compile`] adds to the module:
//!
//! - **Terminal programs** (spec 9.1, `Module::terminal_programs`): one per
//!   terminal base placed in the slice, listing its menus (the terminal,
//!   then each sub-terminal it reaches, depth first in item order, once:
//!   every menu gets a converter "Back" and the top one "Exit" when
//!   lowered) and its lock: a terminal not flagged unlocked (`DNAM` flag
//!   [`UNLOCKED`]) opens with "[Science R] Hack" at R = 0, 25, 50, 75, 100
//!   for difficulty 0 to 4 and with "[Password]" when the player holds the
//!   password note; difficulty 5 needs the password. Owner decision 3:
//!   automatic at the required skill, no minigame, no lockout; the XP is
//!   the plugin's `iXPRewardHackComputer<level>`.
//! - **Locks** (spec 9.3, `Module::locks`): every placed door or container
//!   the placement index (or the MIR) says is locked, with the Lockpick
//!   skill needed (the level rounded up to 0, 25, 50, 75 or 100; 255 is key
//!   only), the key and the plugin's `iXPRewardPickLock<tier>`.
//! - **Holodisks** (spec 9.2, owner decision 4, `Module::holodisks`): every
//!   note of the module, one paragraph per source line; a voice note reads
//!   as its topic's responses; an image or sound note shows its title only
//!   (reported). Each note's global ([`GlobalPurpose::Note`]) is its own id.
//! - **Messages** (`Module::messages`): every `MESG` a script names, with
//!   its title, text and buttons.

use std::collections::BTreeSet;

use crate::build::Pass;
use crate::cond::{self, CondUses};
use crate::diag::{Category, Diag};
use crate::ir::{
    FormKey, Global, GlobalPurpose, Hack, Holodisk, HolodiskSource, Lock, MessageBox, NoteBody,
    PlacedKind, RefEntry, RefUses, SimId, SimKind, TerminalProgram, TextRole, VarTy, global_name,
};

/// `TERM` `DNAM` flag: the terminal is unlocked. Checked on the Fallout 3
/// master: of 386 terminals the 231 with this bit all have difficulty 0.
pub const UNLOCKED: u8 = 0x02;

/// The Science skill a terminal difficulty (0 to 4) needs; `None` for 5,
/// password only.
pub fn hack_skill(difficulty: u8) -> Option<u8> {
    (difficulty <= 4).then(|| difficulty * 25)
}

/// The Lockpick skill a lock level needs: the level rounded up to 0, 25,
/// 50, 75 or 100; `None` for a key-only lock (255). Levels above 100 need
/// 100.
pub fn lockpick_skill(level: u8) -> Option<u8> {
    match level {
        255 => None,
        0 => Some(0),
        l => Some((l.div_ceil(25) * 25).min(100)),
    }
}

/// Adds terminal programs, locks, holodisks and messages.
pub fn compile(pass: &mut Pass<'_>) {
    terminal_programs(pass);
    locks(pass);
    holodisks(pass);
    messages(pass);
}

fn base_of(pass: &Pass<'_>, reference: &FormKey) -> Option<FormKey> {
    pass.placement(reference)
        .and_then(|o| o.base.clone())
        .or_else(|| pass.source.refs.get(reference).and_then(|r| r.base.clone()))
}

fn need_ref(pass: &mut Pass<'_>, reference: &FormKey, why: RefUses) {
    if let Some(entry) = pass.module.refs.get_mut(reference) {
        entry.why.insert(why);
        return;
    }
    let Some(first) = pass.placement(reference).cloned() else {
        return;
    };
    let row = pass.source.refs.get(reference);
    let base = base_of(pass, reference).unwrap_or_else(FormKey::converter);
    pass.module.refs.insert(
        reference.clone(),
        RefEntry {
            form: reference.clone(),
            editor_id: row.and_then(|r| r.editor_id.clone()),
            base,
            area: first.map.clone(),
            mobile: first.kind == PlacedKind::Critter,
            why,
        },
    );
}

fn terminal_programs(pass: &mut Pass<'_>) {
    let mut placed_terminals: Vec<(FormKey, FormKey)> = Vec::new();
    for reference in pass.placed.keys() {
        if let Some(base) = base_of(pass, reference)
            && pass
                .source
                .bases
                .get(&base)
                .is_some_and(|b| b.sig == "TERM")
        {
            placed_terminals.push((base, reference.clone()));
        }
    }
    placed_terminals.sort();
    let bases: BTreeSet<FormKey> = placed_terminals.iter().map(|(b, _)| b.clone()).collect();
    for base in bases {
        let id = SimId::of(SimKind::Terminal, &base);
        let references: Vec<FormKey> = placed_terminals
            .iter()
            .filter(|(b, _)| *b == base)
            .map(|(_, r)| r.clone())
            .collect();
        for reference in &references {
            need_ref(pass, reference, RefUses::TERMINAL);
        }
        let Some(top) = pass.module.terminals.get(&id).cloned() else {
            pass.diag(
                Diag::warning(
                    Category::MissingRecord,
                    "a placed terminal whose record is not in the MIR",
                )
                .owned_by(&id),
            );
            continue;
        };
        let mut menus = Vec::new();
        let mut missing = Vec::new();
        let mut stack = vec![id.clone()];
        while let Some(menu) = stack.pop() {
            if menus.contains(&menu) {
                continue;
            }
            let Some(terminal) = pass.module.terminals.get(&menu) else {
                continue;
            };
            menus.push(menu.clone());
            for item in terminal.items.iter().rev() {
                if let Some(sub) = &item.submenu {
                    if pass.module.terminals.contains_key(sub) {
                        stack.push(sub.clone());
                    } else if !missing.contains(&item.id) {
                        missing.push(item.id.clone());
                    }
                }
            }
        }
        for item in &missing {
            pass.diag(
                Diag::warning(
                    Category::MissingSubTerminal,
                    "the item's sub-terminal is not in the MIR; the item is dropped",
                )
                .owned_by(item),
            );
        }
        let hack = if top.flags & UNLOCKED != 0 {
            None
        } else {
            let level = usize::from(top.difficulty.min(4));
            Some(Hack {
                science: hack_skill(top.difficulty),
                xp: pass.module.settings.hack_xp[level],
                password: top.password.clone(),
            })
        };
        if hack
            .as_ref()
            .is_some_and(|h| h.science.is_none() && h.password.is_none())
        {
            pass.diag(
                Diag::warning(
                    Category::MissingRecord,
                    "a password-only terminal with no password note: it never opens",
                )
                .owned_by(&id),
            );
        }
        pass.module.terminal_programs.insert(
            id.clone(),
            TerminalProgram {
                terminal: id,
                references,
                menus,
                missing,
                hack,
            },
        );
    }
}

fn locks(pass: &mut Pass<'_>) {
    type Found = (
        FormKey,
        PlacedKind,
        u8,
        Option<FormKey>,
        Option<u32>,
        bool,
        bool,
    );
    let mut found: Vec<Found> = Vec::new();
    for (reference, objects) in pass.placed {
        let Some(object) = objects.first() else {
            continue;
        };
        if !matches!(object.kind, PlacedKind::Door | PlacedKind::Container) {
            continue;
        }
        let (level, key, key_pid, bit, indexed) =
            match (&object.lock, pass.source.refs.get(reference)) {
                (Some(lock), _) => (
                    lock.level,
                    lock.key.clone(),
                    lock.key_pid,
                    lock.locked,
                    true,
                ),
                (None, Some(row)) => match &row.lock {
                    Some((level, key)) => (*level, key.clone(), None, false, false),
                    None => continue,
                },
                (None, None) => continue,
            };
        found.push((
            reference.clone(),
            object.kind,
            level,
            key,
            key_pid,
            bit,
            indexed,
        ));
    }
    for (reference, kind, level, key, key_pid, locked_bit, indexed) in found {
        if indexed && key.is_some() && key_pid.is_none() {
            pass.diag(
                Diag::warning(
                    Category::KeyNotConverted,
                    "the lock's key did not convert: the lock is left to the skill",
                )
                .owned_by(&SimId::of(SimKind::Ref, &reference))
                .about(key.as_ref().map_or("", FormKey::as_str).to_owned()),
            );
        }
        let lockpick = lockpick_skill(level);
        let tier = usize::from(lockpick.unwrap_or(100) / 25).min(4);
        if let Some(skill) = lockpick
            && skill != level
        {
            pass.diag(
                Diag::note(
                    Category::LockLevelRounded,
                    format!("lock level {level} needs Lockpick {skill}"),
                )
                .owned_by(&SimId::of(SimKind::Ref, &reference))
                .about(level.to_string()),
            );
        }
        if lockpick.is_none() && key.is_none() {
            pass.diag(
                Diag::warning(
                    Category::KeyNotConverted,
                    "a key-only lock that names no key: it never opens",
                )
                .owned_by(&SimId::of(SimKind::Ref, &reference)),
            );
        }
        need_ref(pass, &reference, RefUses::LOCK);
        let xp = pass.module.settings.lockpick_xp[tier];
        pass.module.locks.insert(
            reference.clone(),
            Lock {
                reference,
                kind,
                level,
                lockpick,
                key,
                key_pid,
                xp,
                locked_bit,
            },
        );
    }
}

fn paragraphs(text: &str) -> Vec<&str> {
    text.lines()
        .map(|l| l.trim_end_matches('\r').trim_end())
        .filter(|l| !l.trim().is_empty())
        .collect()
}

fn holodisks(pass: &mut Pass<'_>) {
    let notes: Vec<_> = pass.module.notes.values().cloned().collect();
    for note in notes {
        let form = note.id.form.clone();
        let source = pass.source.notes.get(&form);
        let editor = source
            .and_then(|n| n.editor_id.clone())
            .unwrap_or_else(|| form.as_str().to_owned());
        let (kind, lines): (HolodiskSource, Vec<String>) = match &note.body {
            NoteBody::Text(_) => (
                HolodiskSource::Text,
                source
                    .and_then(|n| n.text.as_deref())
                    .map(|t| paragraphs(t).into_iter().map(str::to_owned).collect())
                    .unwrap_or_default(),
            ),
            NoteBody::Voice { topic } => (
                HolodiskSource::Voice,
                topic
                    .as_ref()
                    .and_then(|t| pass.source.voice_lines.get(&t.form))
                    .cloned()
                    .unwrap_or_default(),
            ),
            NoteBody::Image(_) => (HolodiskSource::Image, Vec::new()),
            NoteBody::Sound(_) => (HolodiskSource::Sound, Vec::new()),
        };
        match kind {
            HolodiskSource::Image | HolodiskSource::Sound => pass.diag(
                Diag::note(
                    Category::NoteTitleOnly,
                    "an image or sound note: the holodisk shows its title only",
                )
                .owned_by(&note.id)
                .about(editor.clone()),
            ),
            _ if lines.is_empty() => pass.diag(
                Diag::warning(Category::EmptyNote, "the note has no text")
                    .owned_by(&note.id)
                    .about(editor.clone()),
            ),
            _ => {}
        }
        let paragraphs = lines
            .iter()
            .map(|l| pass.texts.intern(&note.id, TextRole::NoteBody, l))
            .collect();
        pass.module
            .globals
            .entry(note.id.clone())
            .or_insert(Global {
                id: note.id.clone(),
                purpose: GlobalPurpose::Note,
                name: global_name(&format!("NOTE_{editor}")),
                ty: VarTy::Int,
                initial: 0,
            });
        pass.module.holodisks.insert(
            note.id.clone(),
            Holodisk {
                note: note.id.clone(),
                global: note.id.clone(),
                title: note.title,
                paragraphs,
                source: kind,
            },
        );
    }
}

fn messages(pass: &mut Pass<'_>) {
    let wanted: Vec<FormKey> = pass.messages.iter().cloned().collect();
    for form in wanted {
        let id = SimId::of(SimKind::Message, &form);
        let Some(message) = pass.source.messages.get(&form) else {
            pass.diag(
                Diag::warning(
                    Category::MissingMessage,
                    format!("message {form} is not in the MIR; ShowMessage shows nothing"),
                )
                .owned_by(&id)
                .about(form.as_str()),
            );
            continue;
        };
        let editor_id = message.editor_id.clone().unwrap_or_default();
        if message.text.as_deref().is_some_and(|t| t.contains('%')) {
            pass.diag(
                Diag::note(
                    Category::MessageFormat,
                    "the message text has format codes, kept as written",
                )
                .owned_by(&id)
                .about(editor_id.clone()),
            );
        }
        let title = pass
            .texts
            .intern_opt(&id, TextRole::MessageBox, message.title.as_deref());
        let text = pass
            .texts
            .intern_opt(&id, TextRole::MessageBox, message.text.as_deref());
        let mut buttons = Vec::new();
        for (label, conditions) in &message.buttons {
            let mut uses = CondUses::default();
            let condition = cond::lower(conditions, &id, pass.names, &mut uses);
            pass.module.diagnostics.extend(uses.diags);
            buttons.push((
                pass.texts.intern(&id, TextRole::MessageBox, label),
                condition,
            ));
        }
        pass.module.messages.insert(
            id.clone(),
            MessageBox {
                id,
                editor_id,
                title,
                text,
                message_box: message.flags & 0x01 != 0,
                buttons,
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skills_needed() {
        assert_eq!(hack_skill(0), Some(0));
        assert_eq!(hack_skill(4), Some(100));
        assert_eq!(hack_skill(5), None);
        assert_eq!(lockpick_skill(0), Some(0));
        assert_eq!(lockpick_skill(7), Some(25));
        assert_eq!(lockpick_skill(25), Some(25));
        assert_eq!(lockpick_skill(80), Some(100));
        assert_eq!(lockpick_skill(100), Some(100));
        assert_eq!(lockpick_skill(200), Some(100));
        assert_eq!(lockpick_skill(255), None);
    }
}
