//! Globals, references, enable groups and actors.

use std::collections::{BTreeMap, BTreeSet};

use super::Builder;
use crate::diag::{Category, Diag};
use crate::ir::{
    Actor, ActorKind, EnableGroup, FormKey, Global, GlobalPurpose, PlacedKind, RefEntry, RefUses,
    SimId, SimKind, VarTy, fixed_point, global_name,
};

impl Builder<'_> {
    /// One campaign global per `GLOB` the slice's code or conditions use
    /// (spec 5.4), initialised from `FLTV`.
    pub(super) fn globals(&mut self) {
        let source = self.source;
        for form in self.uses.globals.clone() {
            let id = SimId::of(SimKind::Global, &form);
            let Some(global) = source.globals.get(&form) else {
                let diag = Diag::warning(
                    Category::MissingRecord,
                    format!("global {form} is not in the MIR"),
                )
                .about(form.as_str());
                self.diag(diag);
                continue;
            };
            let (ty, initial) = if global.kind == 'f' {
                (VarTy::Float, fixed_point(f64::from(global.value)))
            } else {
                let rounded = f64::from(global.value).round();
                (
                    VarTy::Int,
                    rounded.clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32,
                )
            };
            let name = global_name(global.editor_id.as_deref().unwrap_or(form.as_str()));
            self.module.globals.insert(
                id.clone(),
                Global {
                    id,
                    purpose: GlobalPurpose::Glob,
                    name,
                    ty,
                    initial,
                },
            );
        }
    }

    /// The slice's references something needs (spec 5.8), and a note for
    /// each reference the slice's code names that it does not place.
    pub(super) fn refs(&mut self) {
        let source = self.source;
        let placed: BTreeSet<FormKey> = self.placed.keys().cloned().collect();
        // Scripted bases, enable parents and children.
        for reference in &placed {
            let Some(row) = source.refs.get(reference) else {
                continue;
            };
            let scripted = row
                .base
                .as_ref()
                .and_then(|b| source.bases.get(b))
                .is_some_and(|b| b.script.is_some());
            if scripted {
                self.uses.mark(reference, RefUses::SCRIPT);
            }
            if let Some((parent, _)) = &row.enable_parent {
                self.uses.mark(reference, RefUses::ENABLE_PARENT);
                self.uses.mark(parent, RefUses::ENABLE_PARENT);
            }
        }
        // Linked references of the references needed so far.
        let needed: Vec<FormKey> = self
            .uses
            .refs
            .iter()
            .filter(|(r, why)| placed.contains(*r) && !why.is_empty())
            .map(|(r, _)| r.clone())
            .collect();
        for reference in needed {
            if let Some(linked) = source
                .refs
                .get(&reference)
                .and_then(|r| r.linked_ref.clone())
            {
                self.uses.mark(&linked, RefUses::LINKED);
            }
        }
        let uses = std::mem::take(&mut self.uses.refs);
        for (reference, why) in &uses {
            let Some(objects) = self.placed.get(reference) else {
                let diag = Diag::note(
                    Category::RefOutsideSlice,
                    format!("reference {reference} is named but not placed in the slice"),
                )
                .about(reference.as_str());
                self.diag(diag);
                continue;
            };
            let first = &objects[0];
            let row = source.refs.get(reference);
            let base = first
                .base
                .clone()
                .or_else(|| row.and_then(|r| r.base.clone()))
                .unwrap_or_else(FormKey::converter);
            self.module.refs.insert(
                reference.clone(),
                RefEntry {
                    form: reference.clone(),
                    editor_id: row.and_then(|r| r.editor_id.clone()),
                    base,
                    area: first.map.clone(),
                    mobile: first.kind == PlacedKind::Critter,
                    why: *why,
                },
            );
        }
        self.uses.refs = uses;
    }

    /// Enable-parent groups (spec 8.1): each parent with a child in the
    /// slice, flattened to the root of a chain with the opposite flags
    /// multiplied; and a group of one for a reference that starts disabled
    /// with no parent and that some script enables or disables.
    pub(super) fn groups(&mut self) {
        let source = self.source;
        let mut groups: BTreeMap<FormKey, Vec<(FormKey, bool)>> = BTreeMap::new();
        let mut lonely: Vec<FormKey> = Vec::new();
        for reference in self.placed.keys() {
            let Some(row) = source.refs.get(reference) else {
                continue;
            };
            let Some((parent, opposite)) = &row.enable_parent else {
                if row.initially_disabled && self.uses.enabled.contains(reference) {
                    lonely.push(reference.clone());
                }
                continue;
            };
            let mut root = parent.clone();
            let mut flipped = *opposite;
            let mut seen = BTreeSet::from([reference.clone()]);
            while let Some((next, next_opposite)) =
                source.refs.get(&root).and_then(|r| r.enable_parent.clone())
            {
                if !seen.insert(root.clone()) {
                    break;
                }
                flipped ^= next_opposite;
                root = next;
            }
            groups
                .entry(root)
                .or_default()
                .push((reference.clone(), flipped));
        }
        // A parent of other references is its group's state already.
        for reference in lonely {
            groups
                .entry(reference.clone())
                .or_insert_with(|| vec![(reference.clone(), false)]);
        }
        for (parent, mut children) in groups {
            children.sort();
            children.dedup();
            let id = SimId::of(SimKind::Group, &parent);
            let initially_enabled = !source
                .refs
                .get(&parent)
                .is_some_and(|r| r.initially_disabled);
            let name = source
                .refs
                .get(&parent)
                .and_then(|r| r.editor_id.clone())
                .unwrap_or_else(|| parent.as_str().to_owned());
            self.module.globals.insert(
                id.clone(),
                Global {
                    id: id.clone(),
                    purpose: GlobalPurpose::Group,
                    name: global_name(&name),
                    ty: VarTy::Int,
                    initial: i32::from(initially_enabled),
                },
            );
            self.module.groups.insert(
                id.clone(),
                EnableGroup {
                    id,
                    initially_enabled,
                    children,
                },
            );
        }
    }

    /// The actor bases placed in the slice.
    pub(super) fn actors(&mut self) {
        let source = self.source;
        let mut placements: BTreeMap<FormKey, Vec<FormKey>> = BTreeMap::new();
        for (reference, objects) in &self.placed {
            let base = objects[0]
                .base
                .clone()
                .or_else(|| source.refs.get(reference).and_then(|r| r.base.clone()));
            if let Some(base) = base {
                placements.entry(base).or_default().push(reference.clone());
            }
        }
        for (form, actor) in &source.actors {
            let kind = match actor.sig.as_str() {
                "NPC_" => ActorKind::Npc,
                "CREA" => ActorKind::Creature,
                _ => ActorKind::Leveled,
            };
            if actor.essential() {
                let diag = Diag::note(
                    Category::IgnoredFlag,
                    "essential flag ignored (owner decision 12)",
                )
                .owned_by(&SimId::of(SimKind::Actor, form))
                .about(actor.editor_id.clone().unwrap_or_default());
                self.diag(diag);
            }
            let mut placed = placements.remove(form).unwrap_or_default();
            placed.sort();
            self.module.actors.insert(
                form.clone(),
                Actor {
                    base: form.clone(),
                    editor_id: actor.editor_id.clone(),
                    kind,
                    script: actor
                        .script
                        .as_ref()
                        .map(|s| SimId::of(SimKind::Script, s))
                        .filter(|s| self.module.scripts.contains_key(s)),
                    packages: actor
                        .packages
                        .iter()
                        .map(|p| SimId::of(SimKind::Package, p))
                        .filter(|p| self.module.packages.contains_key(p))
                        .collect(),
                    factions: actor.factions.clone(),
                    voice: actor.voice.clone(),
                    race: actor.race.clone(),
                    class: actor.class.clone(),
                    female: actor.female,
                    essential: actor.essential(),
                    ai: actor.ai,
                    placements: placed,
                },
            );
        }
    }
}
