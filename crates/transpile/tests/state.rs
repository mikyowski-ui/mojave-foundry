//! Enable-parent groups, the per-map tables and the group users on the
//! testkit's town (spec 8), and terminals, locks, notes and messages
//! (spec 9).

use transpile::diag::Category;
use transpile::ir::{GlobalPurpose, HolodiskSource, PlacedKind, RefUses, SimId, SimKind, TextRole};
use transpile::testkit::{self, MAP, forms::*, key};

#[test]
fn groups_and_their_table() {
    let m = testkit::town();
    let group = SimId::of(SimKind::Group, &key(R_PARENT));
    assert!(!m.groups[&group].initially_enabled);
    assert_eq!(m.globals[&group].initial, 0);
    assert_eq!(m.globals[&group].purpose, GlobalPurpose::Group);
    let rows: Vec<(u32, bool, bool)> = m.state[MAP]
        .iter()
        .filter(|r| r.group == group)
        .map(|r| {
            let id =
                u32::from_str_radix(r.reference.as_str().rsplit_once(':').unwrap().1, 16).unwrap();
            (id, r.opposite, r.hidden)
        })
        .collect();
    // The parent itself, its child (hidden, as the map stage wrote it) and
    // the opposite child (shown while the group is off).
    assert_eq!(
        rows,
        [
            (R_PARENT, false, true),
            (R_CHILD, false, true),
            (R_OPPOSITE, true, false)
        ]
    );
    // The group of one a script enables.
    let lonely = SimId::of(SimKind::Group, &key(R_LONELY));
    assert!(m.state[MAP].iter().any(|r| r.group == lonely));
    // The door script switches both.
    let door = SimId::of(SimKind::Script, &key(DOOR_SCRIPT));
    assert_eq!(m.group_users[&door], vec![group, lonely]);
    assert!(
        m.diagnostics
            .iter()
            .all(|d| d.category != Category::GroupMismatch),
        "the index and the MIR agree"
    );
}

#[test]
fn terminals_locks_notes_messages() {
    let m = testkit::town();
    let terminal = SimId::of(SimKind::Terminal, &key(TERMINAL));
    let program = &m.terminal_programs[&terminal];
    assert_eq!(program.references, vec![key(R_TERMINAL)]);
    assert_eq!(
        program.menus,
        vec![
            terminal.clone(),
            SimId::of(SimKind::Terminal, &key(SUB_TERMINAL))
        ]
    );
    let hack = program.hack.as_ref().expect("locked");
    assert_eq!(hack.science, Some(25));
    assert_eq!(hack.xp, 30, "iXPRewardHackComputerEasy");
    assert_eq!(hack.password, Some(SimId::of(SimKind::Note, &key(NOTE))));
    assert!(m.refs[&key(R_TERMINAL)].why.contains(RefUses::TERMINAL));

    let lock = &m.locks[&key(R_DOOR)];
    assert_eq!(lock.kind, PlacedKind::Door);
    assert_eq!((lock.level, lock.lockpick), (50, Some(50)));
    assert_eq!(lock.key, Some(key(KEY)));
    assert_eq!(lock.key_pid, Some(0x700));
    assert_eq!(lock.xp, 40, "iXPRewardPickLockAverage");
    assert!(lock.locked_bit);
    assert!(m.refs[&key(R_DOOR)].why.contains(RefUses::LOCK));

    let note = SimId::of(SimKind::Note, &key(NOTE));
    let disk = &m.holodisks[&note];
    assert_eq!(disk.source, HolodiskSource::Text);
    let lines: Vec<&str> = disk
        .paragraphs
        .iter()
        .map(|t| m.texts[t.0 as usize].source.as_str())
        .collect();
    assert_eq!(lines, ["Day one. The door sticks.", "The password is OWL."]);
    assert_eq!(m.globals[&disk.global].purpose, GlobalPurpose::Note);

    let message = &m.messages[&SimId::of(SimKind::Message, &key(MESSAGE))];
    assert!(message.message_box);
    assert_eq!(message.buttons.len(), 2);
    assert_eq!(
        m.texts[message.text.unwrap().0 as usize].role,
        TextRole::MessageBox
    );
    assert!(
        m.diagnostics
            .iter()
            .any(|d| d.category == Category::MessageFormat)
    );
}
