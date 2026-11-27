//! Enable-parent world state (spec 8, task T5).
//!
//! The front end ([`crate::build`]) makes the groups from the MIR's enable
//! parents. [`compile`] then:
//!
//! - **checks them against the placement index** (schema 2 `group`): the
//!   map stage flattened the same chains; where it names a root or an
//!   opposite flag the MIR does not, its reading is kept (it is what was
//!   placed) and the difference is a [`Category::GroupMismatch`] note;
//!   children the map stage did not place are a [`Category::ChildNotPlaced`]
//!   note per group;
//! - **writes the per-map tables** of spec 8.3 (`Module::state`): every
//!   placed child, and the parent itself when it is placed, with its
//!   group, opposite flag, whether it is a critter (its own program applies
//!   the state) and whether it was written hidden. The map stage places
//!   hidden children (spec 8.3: a script shows or hides them); the
//!   lowering joins each row with the placement's hex, elevation and PID;
//! - **records which scripts switch which groups** (`Module::group_users`),
//!   so that the calling program applies a change at once;
//! - **adds the actor globals** conditions and scripts read: a
//!   reference's dead flag for `GetDead` on a named reference
//!   ([`actor_sub::DEAD`]) and a base's kill counter for `GetDeadCount`
//!   ([`actor_sub::KILLS`]).

use std::collections::{BTreeMap, BTreeSet};

use crate::build::Pass;
use crate::diag::{Category, Diag};
use crate::ir::{
    EnableGroup, Expr, FormKey, Func, Global, GlobalPurpose, PlacedKind, RefExpr, SimId, SimKind,
    StateChild, VarTy, actor_sub, global_name, visit,
};

/// Adds the per-map tables, the group users and the actor globals.
pub fn compile(pass: &mut Pass<'_>) {
    reconcile(pass);
    tables(pass);
    users(pass);
    actor_globals(pass);
}

fn editor_id(pass: &Pass<'_>, reference: &FormKey) -> String {
    pass.source
        .refs
        .get(reference)
        .and_then(|r| r.editor_id.clone())
        .unwrap_or_else(|| reference.as_str().to_owned())
}

/// Takes the placement index's group facts where it states them.
fn reconcile(pass: &mut Pass<'_>) {
    let mut stated: Vec<(FormKey, FormKey, bool, bool)> = Vec::new();
    for (reference, objects) in pass.placed {
        if let Some(group) = objects.iter().find_map(|o| o.group.as_ref()) {
            stated.push((
                reference.clone(),
                group.root.clone(),
                group.opposite,
                group.initially_enabled,
            ));
        }
    }
    for (reference, root, opposite, enabled) in stated {
        let id = SimId::of(SimKind::Group, &root);
        // The root's own start state: a child's flag is the root's flipped.
        let root_enabled = if reference == root {
            enabled
        } else {
            enabled ^ opposite
        };
        let existing = pass.module.groups.get(&id).cloned();
        match existing {
            None => {
                pass.diag(
                    Diag::note(
                        Category::GroupMismatch,
                        format!(
                            "the placement index puts {reference} in a group of {root} that the \
                             MIR does not make; taken from the index"
                        ),
                    )
                    .owned_by(&id)
                    .about(editor_id(pass, &root)),
                );
                let children = if reference == root {
                    Vec::new()
                } else {
                    vec![(reference.clone(), opposite)]
                };
                let name = global_name(&editor_id(pass, &root));
                pass.module.globals.insert(
                    id.clone(),
                    Global {
                        id: id.clone(),
                        purpose: GlobalPurpose::Group,
                        name,
                        ty: VarTy::Int,
                        initial: i32::from(root_enabled),
                    },
                );
                pass.module.groups.insert(
                    id.clone(),
                    EnableGroup {
                        id,
                        initially_enabled: root_enabled,
                        children,
                    },
                );
            }
            Some(group) => {
                let mut group = group;
                let mut changed = false;
                if reference != root {
                    match group.children.iter_mut().find(|(c, _)| *c == reference) {
                        Some((_, flag)) if *flag != opposite => {
                            *flag = opposite;
                            changed = true;
                        }
                        Some(_) => {}
                        None => {
                            group.children.push((reference.clone(), opposite));
                            group.children.sort();
                            changed = true;
                        }
                    }
                }
                if group.initially_enabled != root_enabled {
                    group.initially_enabled = root_enabled;
                    if let Some(g) = pass.module.globals.get_mut(&id) {
                        g.initial = i32::from(root_enabled);
                    }
                    changed = true;
                }
                if changed {
                    pass.diag(
                        Diag::note(
                            Category::GroupMismatch,
                            format!(
                                "the placement index and the MIR differ on {reference} in the \
                                 group of {root}; taken from the index"
                            ),
                        )
                        .owned_by(&id)
                        .about(editor_id(pass, &root)),
                    );
                    pass.module.groups.insert(id, group);
                }
            }
        }
    }
    // A child in two groups (the MIR's and the index's): keep it in the
    // index's group only.
    let mut owner_of: BTreeMap<FormKey, SimId> = BTreeMap::new();
    for (reference, objects) in pass.placed {
        if let Some(group) = objects.iter().find_map(|o| o.group.as_ref()) {
            owner_of.insert(reference.clone(), SimId::of(SimKind::Group, &group.root));
        }
    }
    for group in pass.module.groups.values_mut() {
        group
            .children
            .retain(|(c, _)| owner_of.get(c).is_none_or(|g| *g == group.id));
    }
}

fn tables(pass: &mut Pass<'_>) {
    let mut by_map: BTreeMap<String, BTreeSet<StateChild>> = BTreeMap::new();
    let mut missing: Vec<(SimId, usize, usize)> = Vec::new();
    for group in pass.module.groups.values() {
        let mut rows: Vec<(FormKey, bool)> = group.children.clone();
        if !rows.iter().any(|(c, _)| *c == group.id.form) {
            rows.push((group.id.form.clone(), false));
        }
        let mut unplaced = 0;
        for (reference, opposite) in &rows {
            let Some(objects) = pass.placed.get(reference) else {
                if *reference != group.id.form {
                    unplaced += 1;
                }
                continue;
            };
            for object in objects {
                by_map
                    .entry(object.map.clone())
                    .or_default()
                    .insert(StateChild {
                        reference: reference.clone(),
                        group: group.id.clone(),
                        opposite: *opposite,
                        mobile: object.kind == PlacedKind::Critter,
                        hidden: object.hidden,
                    });
            }
        }
        if unplaced > 0 {
            missing.push((group.id.clone(), unplaced, group.children.len()));
        }
    }
    for (id, unplaced, total) in missing {
        let name = editor_id(pass, &id.form);
        pass.diag(
            Diag::note(
                Category::ChildNotPlaced,
                format!("{unplaced} of {total} children are not placed in the slice"),
            )
            .owned_by(&id)
            .about(name),
        );
    }
    pass.module.state = by_map
        .into_iter()
        .map(|(map, rows)| (map, rows.into_iter().collect()))
        .collect();
}

fn users(pass: &mut Pass<'_>) {
    // The references whose base runs each script.
    let mut runs: BTreeMap<FormKey, Vec<FormKey>> = BTreeMap::new();
    for reference in pass.placed.keys() {
        if let Some(script) = pass.names.ref_script(reference) {
            runs.entry(script.clone())
                .or_default()
                .push(reference.clone());
        }
    }
    let mut users: BTreeMap<SimId, BTreeSet<SimId>> = BTreeMap::new();
    let mut strays: BTreeSet<(SimId, FormKey)> = BTreeSet::new();
    for (script, body) in visit::module_bodies(pass.module) {
        for call in visit::calls_in_stmts(body) {
            let Func::Known(id) = &call.func else {
                continue;
            };
            if !matches!(id.name(), "Enable" | "Disable") {
                continue;
            }
            let targets: Vec<FormKey> = match &call.on {
                Some(RefExpr::Placed(r)) => vec![r.clone()],
                None | Some(RefExpr::SelfRef) => {
                    runs.get(&script.form).cloned().unwrap_or_default()
                }
                _ => Vec::new(),
            };
            for target in targets {
                let group = SimId::of(SimKind::Group, &target);
                if pass.module.groups.contains_key(&group) {
                    users.entry(script.clone()).or_default().insert(group);
                } else {
                    strays.insert((script.clone(), target));
                }
            }
        }
    }
    pass.module.group_users = users
        .into_iter()
        .map(|(script, groups)| (script, groups.into_iter().collect()))
        .collect();
    for (script, target) in strays {
        let name = editor_id(pass, &target);
        pass.diag(
            Diag::note(
                Category::NotAGroup,
                format!("Enable or Disable of {name}, which parents no group: its own visibility"),
            )
            .owned_by(&script)
            .about(name),
        );
    }
}

fn actor_globals(pass: &mut Pass<'_>) {
    let mut dead: BTreeSet<FormKey> = BTreeSet::new();
    let mut kills: BTreeSet<FormKey> = BTreeSet::new();
    let mut look = |e: &Expr| {
        let Expr::Call(call) = e else { return };
        let Func::Known(id) = &call.func else { return };
        match id.name() {
            "GetDead" => {
                if let Some(RefExpr::Placed(r)) = &call.on {
                    dead.insert(r.clone());
                }
            }
            "GetDeadCount" => {
                if let Some(Expr::Form(base)) = call.args.first() {
                    kills.insert(base.clone());
                }
            }
            _ => {}
        }
    };
    for (_, body) in visit::module_bodies(pass.module) {
        visit::exprs_in_stmts(body, &mut look);
        for call in visit::calls_in_stmts(body) {
            look(&Expr::Call(Box::new(call.clone())));
        }
    }
    for cond in visit::module_conditions(pass.module) {
        visit::exprs_in_cond(cond, &mut look);
    }
    for reference in dead {
        let name = global_name(&format!("{}_DEAD", editor_id(pass, &reference)));
        let id = SimId::new(SimKind::Ref, reference, actor_sub::DEAD);
        pass.module.globals.entry(id.clone()).or_insert(Global {
            id,
            purpose: GlobalPurpose::Actor,
            name,
            ty: VarTy::Int,
            initial: 0,
        });
    }
    for base in kills {
        let edid = pass
            .module
            .actors
            .get(&base)
            .and_then(|a| a.editor_id.clone())
            .or_else(|| {
                pass.source
                    .names
                    .iter()
                    .find(|n| n.form == base)
                    .map(|n| n.editor_id.clone())
            })
            .unwrap_or_else(|| base.as_str().to_owned());
        let id = SimId::new(SimKind::Actor, base, actor_sub::KILLS);
        pass.module.globals.entry(id.clone()).or_insert(Global {
            id,
            purpose: GlobalPurpose::Actor,
            name: global_name(&format!("{edid}_KILLS")),
            ty: VarTy::Int,
            initial: 0,
        });
    }
}
