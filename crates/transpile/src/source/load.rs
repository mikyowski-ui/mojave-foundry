//! Filling a [`SimSource`] from a MIR.
//!
//! Today's MIR has typed queries for scripts, terminals, notes, packages
//! and conditions; the rest (topics, INFOs with their choices, quests with
//! their stage scripts, globals, actor factions) is decoded here from the
//! lossless raw store through `fnv_esm`'s decoders, and the few columns no
//! query returns are read through `Records::connection`. Every such read
//! is in this file, so that the typed queries of task T1 replace it in one
//! place.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use fnv_esm::{FormId, Record};

use super::{
    CondValue, Corpus, EmbeddedScript, FormResolver, SimSource, SourceActor, SourceBase,
    SourceBody, SourceCondition, SourceGlobal, SourceInfo, SourceLocation, SourceLogEntry,
    SourceMenuItem, SourceName, SourceNote, SourceObjective, SourcePackage, SourceQuest, SourceRef,
    SourceResponse, SourceScript, SourceStage, SourceTarget, SourceTerminal, SourceTopic,
};
use crate::error::TranspileError;
use crate::ir::{FormKey, PackageEvent, SimSlice};
use crate::obscript::DeclaredVar;

type Result<T> = std::result::Result<T, TranspileError>;

fn mir_key(key: &FormKey) -> Option<mir::FormKey> {
    key.as_str().parse().ok()
}

fn key(text: String) -> Option<FormKey> {
    text.parse::<mir::FormKey>().ok().map(FormKey::from)
}

struct Loader<'r> {
    records: &'r mir::Records,
    resolver: FormResolver,
    plugin_of: HashMap<FormKey, Option<usize>>,
    exists: HashMap<FormKey, bool>,
}

impl Loader<'_> {
    fn rows<T>(
        &self,
        query: &'static str,
        sql: &str,
        map: impl Fn(&[Option<String>]) -> Option<T>,
        columns: usize,
    ) -> Result<Vec<T>> {
        let conn = self.records.connection();
        let mut statement = conn
            .prepare(sql)
            .map_err(|e| TranspileError::query(query, e))?;
        let rows = statement
            .query_map([], |r| {
                let mut values = Vec::with_capacity(columns);
                for i in 0..columns {
                    let v = r.get_ref(i)?;
                    let value = if let Ok(text) = v.as_str() {
                        Some(text.to_owned())
                    } else if let Ok(int) = v.as_i64() {
                        Some(int.to_string())
                    } else if let Ok(real) = v.as_f64() {
                        Some(real.to_string())
                    } else {
                        None
                    };
                    values.push(value);
                }
                Ok(values)
            })
            .map_err(|e| TranspileError::query(query, e))?;
        let mut out = Vec::new();
        for row in rows {
            let row = row.map_err(|e| TranspileError::query(query, e))?;
            out.extend(map(&row));
        }
        Ok(out)
    }

    /// The load index of the plugin whose record of `form` wins.
    fn plugin_of(&mut self, form: &FormKey) -> Result<Option<usize>> {
        if let Some(found) = self.plugin_of.get(form) {
            return Ok(*found);
        }
        let found = match mir_key(form) {
            Some(k) => self
                .records
                .record(&k)?
                .and_then(|row| self.resolver.plugin_index(&row.plugin)),
            None => None,
        };
        self.plugin_of.insert(form.clone(), found);
        Ok(found)
    }

    fn exists(&mut self, form: &FormKey) -> Result<bool> {
        if let Some(found) = self.exists.get(form) {
            return Ok(*found);
        }
        let found = match mir_key(form) {
            Some(k) => self.records.record(&k)?.is_some(),
            None => false,
        };
        self.exists.insert(form.clone(), found);
        Ok(found)
    }

    fn resolve(&self, owner: usize, id: FormId) -> Option<FormKey> {
        self.resolver.resolve(owner, id.0)
    }

    fn resolve_all(&self, owner: usize, ids: &[FormId]) -> Vec<FormKey> {
        ids.iter()
            .filter_map(|id| self.resolve(owner, *id))
            .collect()
    }

    /// A raw parameter as a form, when it names a record of the plugin
    /// list.
    fn param_form(&mut self, owner: usize, raw: u32) -> Result<Option<FormKey>> {
        match self.resolver.resolve(owner, raw) {
            Some(form) if self.exists(&form)? => Ok(Some(form)),
            _ => Ok(None),
        }
    }

    fn condition(&mut self, owner: usize, c: &fnv_esm::Condition) -> Result<SourceCondition> {
        let comparison = match c.comparison {
            fnv_esm::Comparison::Equal => "==".to_owned(),
            fnv_esm::Comparison::NotEqual => "!=".to_owned(),
            fnv_esm::Comparison::Greater => ">".to_owned(),
            fnv_esm::Comparison::GreaterOrEqual => ">=".to_owned(),
            fnv_esm::Comparison::Less => "<".to_owned(),
            fnv_esm::Comparison::LessOrEqual => "<=".to_owned(),
            fnv_esm::Comparison::Reserved(code) => code.to_string(),
        };
        let value = match c.value {
            fnv_esm::ConditionValue::Float(v) => CondValue::Literal(f64::from(v)),
            fnv_esm::ConditionValue::Global(id) => match self.resolve(owner, id) {
                Some(global) => CondValue::Global(global),
                None => CondValue::Literal(0.0),
            },
        };
        Ok(SourceCondition {
            function: c.function_index(),
            comparison,
            value,
            param1: c.param1,
            param2: c.param2,
            param1_form: self.param_form(owner, c.param1)?,
            param2_form: self.param_form(owner, c.param2)?,
            run_on: c.run_on,
            reference: self.resolve(owner, c.reference),
            is_or: c.is_or(),
        })
    }

    fn conditions(
        &mut self,
        owner: usize,
        list: &[fnv_esm::Condition],
    ) -> Result<Vec<SourceCondition>> {
        list.iter().map(|c| self.condition(owner, c)).collect()
    }

    fn row_condition(&mut self, owner: usize, c: &mir::ConditionRow) -> Result<SourceCondition> {
        Ok(SourceCondition {
            function: (c.function & 0xFFFF) as u16,
            comparison: c.comparison.clone(),
            value: match (&c.global, c.value) {
                (Some(global), _) => CondValue::Global(FormKey::from(global)),
                (None, value) => CondValue::Literal(value.unwrap_or(0.0)),
            },
            param1: c.param1,
            param2: c.param2,
            param1_form: self.param_form(owner, c.param1)?,
            param2_form: self.param_form(owner, c.param2)?,
            run_on: c.run_on,
            reference: c.reference.as_ref().map(FormKey::from),
            is_or: c.is_or,
        })
    }

    fn body(&self, owner: usize, body: &fnv_esm::ScriptBody) -> Option<SourceBody> {
        let source = body.source.as_deref()?;
        if source.trim().is_empty() {
            return None;
        }
        Some(SourceBody {
            source: source.to_owned(),
            vars: body
                .locals
                .iter()
                .map(|l| DeclaredVar {
                    index: l.index,
                    name: l.name.clone().unwrap_or_default(),
                    flags: l.flags,
                })
                .collect(),
            references: body
                .references
                .iter()
                .filter_map(|r| match r {
                    fnv_esm::ScriptReference::Form(id) => self.resolve(owner, *id),
                    fnv_esm::ScriptReference::Local(_) => None,
                })
                .collect(),
        })
    }

    /// Decodes a record from the raw store.
    fn decode(&self, form: &FormKey, sig: &[u8; 4]) -> Result<Option<Record>> {
        let Some(k) = mir_key(form) else {
            return Ok(None);
        };
        let subs = self.records.raw_subrecords(&k)?;
        if subs.is_empty() {
            return Ok(None);
        }
        let mut raw = fnv_esm::RawRecord::new(sig, 0u32);
        for (s, data) in subs {
            if let [a, b, c, d] = *s.as_bytes() {
                raw = raw.with(&[a, b, c, d], data);
            }
        }
        Ok(Some(raw.decode()))
    }

    fn forms_of(&self, query: &'static str, sql: &str) -> Result<Vec<FormKey>> {
        self.rows(query, sql, |r| r[0].clone().and_then(key), 1)
    }
}

/// Reads what the build needs for a slice. The selection: the slice's
/// references (with their enable parents and linked references), their
/// bases and actors, the actors' packages, the INFOs that name an actor as
/// speaker (`ANAM`, `GetIsID` or `GetIsVoiceType` on the subject), their
/// topics and the topics they name, the slice's terminals and their
/// sub-terminals, and every script, quest, global and note.
///
/// # Errors
///
/// When the MIR cannot be read.
pub fn load(mir: &mir::Mir, slice: &SimSlice) -> Result<SimSource> {
    let records = mir.records()?;
    let (mut l, mut source) = common(&records)?;
    load_scripts(&mut l, &mut source)?;
    load_refs(&mut l, &mut source, slice)?;
    load_actors(&mut l, &mut source)?;
    load_quests(&mut l, &mut source)?;
    load_globals(&mut l, &mut source)?;
    load_notes(&mut l, &mut source)?;
    load_terminals(&mut l, &mut source)?;
    load_dialogue(&mut l, &mut source)?;
    Ok(source)
}

/// Reads every script record and every embedded script of a plugin list,
/// with what resolves their names: for whole-corpus checks of the front
/// end, not for a build.
///
/// # Errors
///
/// When the file cannot be read.
pub fn load_corpus(records: &mir::Records) -> Result<Corpus> {
    let (mut l, mut source) = common(records)?;
    load_scripts(&mut l, &mut source)?;
    for (form, script) in l.rows(
        "quest scripts",
        "SELECT form, script FROM records WHERE sig = 'QUST' ORDER BY form",
        |r| Some((key(r[0].clone()?)?, r[1].clone().and_then(key))),
        2,
    )? {
        source.quests.insert(
            form,
            SourceQuest {
                editor_id: None,
                name: None,
                script,
                flags: 0,
                priority: 0,
                delay: None,
                stages: Vec::new(),
                objectives: Vec::new(),
            },
        );
    }
    let mut refs: BTreeMap<(FormKey, String), Vec<FormKey>> = BTreeMap::new();
    for (owner, context, form) in l.rows(
        "embedded script references",
        "SELECT owner, context, form FROM script_refs WHERE context != 'script' ORDER BY owner, context, seq",
        |r| Some((key(r[0].clone()?)?, r[1].clone()?, key(r[2].clone()?)?)),
        3,
    )? {
        refs.entry((owner, context)).or_default().push(form);
    }
    let sigs: BTreeMap<FormKey, String> = l
        .rows(
            "embedded script owners",
            "SELECT DISTINCT e.owner, r.sig FROM embedded_scripts e JOIN records r ON r.form = e.owner",
            |r| Some((key(r[0].clone()?)?, r[1].clone()?)),
            2,
        )?
        .into_iter()
        .collect();
    let mut embedded = Vec::new();
    for (owner, context, text) in records.embedded_scripts()? {
        let owner = FormKey::from(&owner);
        let references = refs
            .remove(&(owner.clone(), context.clone()))
            .unwrap_or_default();
        embedded.push(EmbeddedScript {
            owner_sig: sigs.get(&owner).cloned().unwrap_or_default(),
            owner,
            context,
            body: SourceBody {
                source: text,
                vars: Vec::new(),
                references,
            },
        });
    }
    let mut info_speakers = BTreeMap::new();
    for (info, speaker) in l.rows(
        "info speakers",
        "SELECT form, speaker FROM infos WHERE speaker IS NOT NULL ORDER BY form",
        |r| Some((key(r[0].clone()?)?, key(r[1].clone()?)?)),
        2,
    )? {
        info_speakers.insert(info, speaker);
    }
    for (info, raw) in l.rows(
        "speaker conditions",
        "SELECT owner, param1 FROM conditions WHERE context = 'info' AND run_on = 0 AND function = 72 \
         ORDER BY owner, seq",
        |r| Some((key(r[0].clone()?)?, r[1].as_deref()?.parse::<i64>().ok()?)),
        2,
    )? {
        if info_speakers.contains_key(&info) {
            continue;
        }
        let owner = l.plugin_of(&info)?.unwrap_or(0);
        if let Some(base) = l.resolver.resolve(owner, raw as u32) {
            info_speakers.insert(info, base);
        }
    }
    Ok(Corpus {
        source,
        embedded,
        info_speakers,
    })
}

/// The loader and the tables every read starts from: plugins, names,
/// reference bases, base scripts.
fn common(records: &mir::Records) -> Result<(Loader<'_>, SimSource)> {
    let mut l = Loader {
        records,
        resolver: FormResolver::default(),
        plugin_of: HashMap::new(),
        exists: HashMap::new(),
    };
    let plugins: Vec<String> = l.rows(
        "plugins",
        "SELECT lower(name) FROM plugins ORDER BY load_order",
        |r| r[0].clone(),
        1,
    )?;
    l.resolver = FormResolver::new(plugins.clone());
    let mut source = SimSource {
        game: records.game()?.id().to_owned(),
        plugins,
        ..SimSource::default()
    };
    source.names = l.rows(
        "names",
        "SELECT form, sig, editor_id FROM records WHERE editor_id IS NOT NULL ORDER BY form",
        |r| {
            Some(SourceName {
                form: key(r[0].clone()?)?,
                sig: r[1].clone()?,
                editor_id: r[2].clone()?,
            })
        },
        3,
    )?;
    source.names.sort();
    for (reference, base) in l.rows(
        "ref bases",
        "SELECT form, base FROM placed_refs WHERE editor_id IS NOT NULL AND base IS NOT NULL",
        |r| Some((key(r[0].clone()?)?, key(r[1].clone()?)?)),
        2,
    )? {
        source.ref_bases.insert(reference, base);
    }
    for (form, script) in l.rows(
        "base scripts",
        "SELECT form, script FROM records WHERE script IS NOT NULL",
        |r| Some((key(r[0].clone()?)?, key(r[1].clone()?)?)),
        2,
    )? {
        source.base_scripts.insert(form, script);
    }
    for (base, reference) in l.rows(
        "unique actor references",
        "SELECT base, MIN(form) FROM placed_refs WHERE base_sig IN ('NPC_', 'CREA') \
         AND base IS NOT NULL GROUP BY base HAVING COUNT(*) = 1",
        |r| Some((key(r[0].clone()?)?, key(r[1].clone()?)?)),
        2,
    )? {
        source.unique_refs.insert(base, reference);
    }
    Ok((l, source))
}

fn load_scripts(l: &mut Loader<'_>, source: &mut SimSource) -> Result<()> {
    let mut vars: BTreeMap<FormKey, Vec<DeclaredVar>> = BTreeMap::new();
    for (script, var) in l.rows(
        "script variables",
        "SELECT script, idx, name, flags FROM script_variables ORDER BY script, idx",
        |r| {
            Some((
                key(r[0].clone()?)?,
                DeclaredVar {
                    index: r[1].as_deref()?.parse().ok()?,
                    name: r[2].clone().unwrap_or_default(),
                    flags: r[3].as_deref()?.parse().ok()?,
                },
            ))
        },
        4,
    )? {
        vars.entry(script).or_default().push(var);
    }
    let mut refs: BTreeMap<FormKey, Vec<FormKey>> = BTreeMap::new();
    for (owner, form) in l.rows(
        "script references",
        "SELECT owner, form FROM script_refs WHERE context = 'script' ORDER BY owner, seq",
        |r| Some((key(r[0].clone()?)?, key(r[1].clone()?)?)),
        2,
    )? {
        refs.entry(owner).or_default().push(form);
    }
    for row in l.records.scripts()? {
        let form = FormKey::from(&row.form);
        source.scripts.insert(
            form.clone(),
            SourceScript {
                editor_id: row.editor_id,
                script_type: row.script_type.unwrap_or(0) as u32,
                body: SourceBody {
                    source: row.source.unwrap_or_default(),
                    vars: vars.remove(&form).unwrap_or_default(),
                    references: refs.remove(&form).unwrap_or_default(),
                },
            },
        );
    }
    Ok(())
}

fn load_ref(l: &Loader<'_>, form: &FormKey) -> Result<Option<SourceRef>> {
    let sql = format!(
        "SELECT base_sig, editor_id, base, linked_ref, enable_parent, enable_opposite, \
         initially_disabled, persistent FROM placed_refs WHERE form = '{}'",
        form.as_str().replace('\'', "")
    );
    let rows = l.rows(
        "placed reference",
        &sql,
        |r| {
            Some(SourceRef {
                sig: match r[0].as_deref() {
                    Some("NPC_" | "LVLN") => "ACHR".to_owned(),
                    Some("CREA" | "LVLC") => "ACRE".to_owned(),
                    _ => "REFR".to_owned(),
                },
                editor_id: r[1].clone(),
                base: r[2].clone().and_then(key),
                linked_ref: r[3].clone().and_then(key),
                enable_parent: r[4]
                    .clone()
                    .and_then(key)
                    .map(|p| (p, r[5].as_deref().is_some_and(|v| v != "0"))),
                initially_disabled: r[6].as_deref().is_some_and(|v| v != "0"),
                persistent: r[7].as_deref().is_some_and(|v| v != "0"),
            })
        },
        8,
    )?;
    Ok(rows.into_iter().next())
}

fn load_refs(l: &mut Loader<'_>, source: &mut SimSource, slice: &SimSlice) -> Result<()> {
    let mut pending: Vec<FormKey> = slice
        .placement
        .objects
        .iter()
        .filter_map(|o| o.reference.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let slice_refs: BTreeSet<FormKey> = pending.iter().cloned().collect();
    while let Some(form) = pending.pop() {
        if source.refs.contains_key(&form) {
            continue;
        }
        let Some(row) = load_ref(l, &form)? else {
            continue;
        };
        // Enable parents are followed to their root; linked references one
        // step from the slice's own references.
        if let Some((parent, _)) = &row.enable_parent {
            pending.push(parent.clone());
        }
        if slice_refs.contains(&form)
            && let Some(linked) = &row.linked_ref
        {
            pending.push(linked.clone());
        }
        if let Some(base) = &row.base {
            source.ref_bases.insert(form.clone(), base.clone());
        }
        source.refs.insert(form, row);
    }
    for form in &slice_refs {
        let Some(base) = source.refs.get(form).and_then(|r| r.base.clone()) else {
            continue;
        };
        if source.bases.contains_key(&base) {
            continue;
        }
        if let Some(object) = mir_key(&base)
            .map(|k| l.records.base_object(&k))
            .transpose()?
            .flatten()
        {
            source.bases.insert(
                base,
                SourceBase {
                    sig: object.kind,
                    editor_id: object.editor_id,
                    script: object.script.as_ref().map(FormKey::from),
                },
            );
        }
    }
    Ok(())
}

fn load_actors(l: &mut Loader<'_>, source: &mut SimSource) -> Result<()> {
    let actor_bases: Vec<(FormKey, SourceBase)> = source
        .bases
        .iter()
        .filter(|(_, b)| matches!(b.sig.as_str(), "NPC_" | "CREA" | "LVLN" | "LVLC"))
        .map(|(k, b)| (k.clone(), b.clone()))
        .collect();
    for (form, base) in actor_bases {
        let owner = l.plugin_of(&form)?.unwrap_or(0);
        let mut actor = SourceActor {
            editor_id: base.editor_id.clone(),
            sig: base.sig.clone(),
            script: base.script.clone(),
            packages: Vec::new(),
            factions: Vec::new(),
            voice: None,
            race: None,
            class: None,
            female: false,
            actor_flags: 0,
            ai: None,
        };
        let sig: [u8; 4] = match base.sig.as_str() {
            "NPC_" => *b"NPC_",
            "CREA" => *b"CREA",
            _ => {
                source.actors.insert(form, actor);
                continue;
            }
        };
        match l.decode(&form, &sig)? {
            Some(Record::Npc(npc)) => {
                actor.packages = l.resolve_all(owner, &npc.packages);
                actor.factions = npc
                    .factions
                    .iter()
                    .filter_map(|f| l.resolve(owner, f.faction).map(|k| (k, f.rank as i8)))
                    .collect();
                actor.voice = npc.voice.and_then(|v| l.resolve(owner, v));
                actor.race = npc.race.and_then(|v| l.resolve(owner, v));
                actor.class = npc.class.and_then(|v| l.resolve(owner, v));
                actor.actor_flags = npc.base.map_or(0, |b| b.flags);
                actor.female = actor.actor_flags & 1 != 0;
            }
            Some(Record::Creature(creature)) => {
                actor.packages = l.resolve_all(owner, &creature.packages);
                actor.factions = creature
                    .factions
                    .iter()
                    .filter_map(|f| l.resolve(owner, f.faction).map(|k| (k, f.rank as i8)))
                    .collect();
                actor.voice = creature.voice.and_then(|v| l.resolve(owner, v));
                actor.actor_flags = creature.base.map_or(0, |b| b.flags);
                actor.female = actor.actor_flags & 1 != 0;
            }
            _ => {}
        }
        for package in actor.packages.clone() {
            if source.packages.contains_key(&package) {
                continue;
            }
            if let Some(row) = mir_key(&package)
                .map(|k| l.records.package(&k))
                .transpose()?
                .flatten()
            {
                let loaded = package_from_row(l, &row)?;
                source.packages.insert(package, loaded);
            }
        }
        source.actors.insert(form, actor);
    }
    Ok(())
}

fn package_from_row(l: &mut Loader<'_>, row: &mir::PackageRow) -> Result<SourcePackage> {
    let form = FormKey::from(&row.form);
    let owner = l.plugin_of(&form)?.unwrap_or(0);
    let conditions = row
        .conditions
        .iter()
        .map(|c| l.row_condition(owner, c))
        .collect::<Result<Vec<_>>>()?;
    let refs = l.records.script_references(&row.form).unwrap_or_default();
    let events = [
        ("on begin", PackageEvent::Begin),
        ("on end", PackageEvent::End),
        ("on change", PackageEvent::Change),
    ]
    .into_iter()
    .filter_map(|(context, event)| {
        let source = row.scripts.iter().find(|(c, _)| c == context)?;
        Some((
            event,
            Some(SourceBody {
                source: source.1.clone(),
                vars: Vec::new(),
                references: refs
                    .iter()
                    .filter(|(c, _)| c == context)
                    .map(|(_, f)| FormKey::from(f))
                    .collect(),
            }),
        ))
    })
    .collect();
    Ok(SourcePackage {
        editor_id: row.editor_id.clone(),
        package_type: row.package_type.unwrap_or(0),
        flags: row.flags.unwrap_or(0),
        behaviour_flags: 0,
        type_flags: 0,
        schedule: row
            .schedule
            .map(|s| (s.month, s.day_of_week, s.date, s.hour, s.duration)),
        location: row.location.as_ref().map(|loc| SourceLocation {
            location_type: loc.location_type,
            form: loc.form.as_ref().map(FormKey::from),
            value: loc.value,
            radius: loc.radius,
        }),
        target: row.target.as_ref().map(|t| SourceTarget {
            target_type: t.target_type,
            form: t.form.as_ref().map(FormKey::from),
            value: t.value,
            count: t.count,
        }),
        conditions,
        events,
    })
}

fn load_quests(l: &mut Loader<'_>, source: &mut SimSource) -> Result<()> {
    for form in l.forms_of(
        "quests",
        "SELECT form FROM records WHERE sig = 'QUST' ORDER BY form",
    )? {
        let owner = l.plugin_of(&form)?.unwrap_or(0);
        let Some(Record::Quest(quest)) = l.decode(&form, b"QUST")? else {
            continue;
        };
        let mut stages = Vec::new();
        for stage in &quest.stages {
            let mut entries = Vec::new();
            for entry in &stage.entries {
                entries.push(SourceLogEntry {
                    flags: entry.flags.unwrap_or(0),
                    conditions: l.conditions(owner, &entry.conditions)?,
                    text: entry.text.clone(),
                    script: l.body(owner, &entry.script),
                    next_quest: entry.next_quest.and_then(|q| l.resolve(owner, q)),
                });
            }
            stages.push(SourceStage {
                index: stage.index,
                entries,
            });
        }
        let mut objectives = Vec::new();
        for objective in &quest.objectives {
            let mut targets = Vec::new();
            for target in &objective.targets {
                if let Some(t) = l.resolve(owner, target.target) {
                    targets.push((t, l.conditions(owner, &target.conditions)?));
                }
            }
            objectives.push(SourceObjective {
                index: objective.index,
                text: objective.description.clone(),
                targets,
            });
        }
        source.quests.insert(
            form,
            SourceQuest {
                editor_id: quest.common.editor_id.clone(),
                name: quest.common.full_name.clone(),
                script: quest.common.script.and_then(|s| l.resolve(owner, s)),
                flags: quest.data.map_or(0, |d| d.flags),
                priority: quest.data.map_or(0, |d| d.priority),
                delay: quest.data.and_then(|d| d.delay),
                stages,
                objectives,
            },
        );
    }
    Ok(())
}

fn load_globals(l: &mut Loader<'_>, source: &mut SimSource) -> Result<()> {
    for form in l.forms_of(
        "globals",
        "SELECT form FROM records WHERE sig = 'GLOB' ORDER BY form",
    )? {
        let Some(k) = mir_key(&form) else { continue };
        let subs = l.records.raw_subrecords(&k)?;
        let mut global = SourceGlobal {
            editor_id: None,
            kind: 's',
            value: 0.0,
        };
        for (sig, data) in subs {
            match (sig.as_str(), data.as_slice()) {
                ("EDID", bytes) => {
                    global.editor_id = Some(
                        String::from_utf8_lossy(bytes)
                            .trim_end_matches('\0')
                            .to_owned(),
                    );
                }
                ("FNAM", [c, ..]) => global.kind = char::from(*c),
                ("FLTV", [a, b, c, d]) => global.value = f32::from_le_bytes([*a, *b, *c, *d]),
                _ => {}
            }
        }
        source.globals.insert(form, global);
    }
    Ok(())
}

fn load_notes(l: &mut Loader<'_>, source: &mut SimSource) -> Result<()> {
    let notes = match l.records.notes() {
        Ok(notes) => notes,
        Err(mir::MirError::MissingTable(_)) => Vec::new(),
        Err(e) => return Err(e.into()),
    };
    for note in notes {
        source.notes.insert(
            FormKey::from(&note.form),
            SourceNote {
                editor_id: note.editor_id,
                title: note.name,
                note_type: note.note_type.unwrap_or(1),
                text: note.text,
                topic: note.topic.as_ref().map(FormKey::from),
                sound: note.sound.as_ref().map(FormKey::from),
                texture: note.texture,
            },
        );
    }
    Ok(())
}

fn load_terminals(l: &mut Loader<'_>, source: &mut SimSource) -> Result<()> {
    let mut pending: Vec<FormKey> = source
        .bases
        .iter()
        .filter(|(_, b)| b.sig == "TERM")
        .map(|(k, _)| k.clone())
        .collect();
    while let Some(form) = pending.pop() {
        if source.terminals.contains_key(&form) {
            continue;
        }
        let Some(k) = mir_key(&form) else { continue };
        let row = match l.records.terminal(&k) {
            Ok(Some(row)) => row,
            Ok(None) | Err(mir::MirError::MissingTable(_)) => continue,
            Err(e) => return Err(e.into()),
        };
        let owner = l.plugin_of(&form)?.unwrap_or(0);
        let refs = l.records.script_references(&k).unwrap_or_default();
        let mut items = Vec::new();
        for item in &row.items {
            let context = format!("item {}", item.seq);
            let conditions = item
                .conditions
                .iter()
                .map(|c| l.row_condition(owner, c))
                .collect::<Result<Vec<_>>>()?;
            let sub_terminal = item.sub_terminal.as_ref().map(FormKey::from);
            if let Some(sub) = &sub_terminal {
                pending.push(sub.clone());
            }
            items.push(SourceMenuItem {
                text: item.text.clone(),
                result: item.result_text.clone(),
                flags: item.flags.unwrap_or(0),
                display_note: item.display_note.as_ref().map(FormKey::from),
                sub_terminal,
                script: item.script_source.clone().map(|s| SourceBody {
                    source: s,
                    vars: Vec::new(),
                    references: refs
                        .iter()
                        .filter(|(c, _)| *c == context)
                        .map(|(_, f)| FormKey::from(f))
                        .collect(),
                }),
                conditions,
            });
        }
        source.terminals.insert(
            form,
            SourceTerminal {
                editor_id: row.editor_id,
                header: row.header_text,
                difficulty: row.difficulty.unwrap_or(0),
                flags: row.flags.unwrap_or(0),
                server: row.server_type.unwrap_or(0),
                password_note: row.password_note.as_ref().map(FormKey::from),
                items,
            },
        );
    }
    Ok(())
}

fn load_dialogue(l: &mut Loader<'_>, source: &mut SimSource) -> Result<()> {
    let actors: BTreeSet<FormKey> = source.actors.keys().cloned().collect();
    let voices: BTreeSet<FormKey> = source
        .actors
        .values()
        .filter_map(|a| a.voice.clone())
        .collect();
    let mut selected: BTreeSet<FormKey> = BTreeSet::new();
    for (info, speaker) in l.rows(
        "info speakers",
        "SELECT form, speaker FROM infos WHERE speaker IS NOT NULL",
        |r| Some((key(r[0].clone()?)?, key(r[1].clone()?)?)),
        2,
    )? {
        if actors.contains(&speaker) {
            selected.insert(info);
        }
    }
    for (info, function, raw) in l.rows(
        "speaker conditions",
        "SELECT owner, function, param1 FROM conditions WHERE context = 'info' AND run_on = 0 \
         AND function IN (72, 427)",
        |r| {
            Some((
                key(r[0].clone()?)?,
                r[1].as_deref()?.parse::<u32>().ok()?,
                r[2].as_deref()?.parse::<i64>().ok()?,
            ))
        },
        3,
    )? {
        if selected.contains(&info) {
            continue;
        }
        let owner = l.plugin_of(&info)?.unwrap_or(0);
        let Some(named) = l.resolver.resolve(owner, raw as u32) else {
            continue;
        };
        let hit = match function {
            72 => actors.contains(&named),
            _ => voices.contains(&named),
        };
        if hit {
            selected.insert(info);
        }
    }

    let mut topics: BTreeSet<FormKey> = BTreeSet::new();
    for form in selected {
        let Some(k) = mir_key(&form) else { continue };
        let Some(row) = l.records.record(&k)? else {
            continue;
        };
        let owner = l.resolver.plugin_index(&row.plugin).unwrap_or(0);
        let Some(Record::Info(info)) = l.decode(&form, b"INFO")? else {
            continue;
        };
        let Some(topic) = row
            .parent
            .as_ref()
            .map(FormKey::from)
            .or_else(|| info.topic.and_then(|t| l.resolve(owner, t)))
        else {
            continue;
        };
        let loaded = SourceInfo {
            topic: topic.clone(),
            quest: info.quest.and_then(|q| l.resolve(owner, q)),
            order: (u32::try_from(owner).unwrap_or(u32::MAX), row.offset),
            info_type: info.data.map_or(0, |d| d.info_type),
            flags: info.data.and_then(|d| d.flags).unwrap_or(0),
            speaker: info.speaker.and_then(|s| l.resolve(owner, s)),
            prompt: info.prompt.clone(),
            responses: info
                .responses
                .iter()
                .map(|r| SourceResponse {
                    number: r.data.map_or(0, |d| d.response_number),
                    emotion_type: r.data.map_or(0, |d| d.emotion_type),
                    emotion_value: r.data.map_or(0, |d| d.emotion_value),
                    text: r.text.clone().unwrap_or_default(),
                })
                .collect(),
            conditions: l.conditions(owner, &info.conditions)?,
            choices: l.resolve_all(owner, &info.choices),
            link_from: l.resolve_all(owner, &info.link_from),
            add_topics: l.resolve_all(owner, &info.add_topics),
            begin: l.body(owner, &info.result_begin),
            end: l.body(owner, &info.result_end),
            challenge: info.speech_challenge,
            label: None,
        };
        topics.insert(topic);
        topics.extend(loaded.choices.iter().cloned());
        topics.extend(loaded.link_from.iter().cloned());
        topics.extend(loaded.add_topics.iter().cloned());
        source.infos.insert(form, loaded);
    }
    for form in topics {
        let Some(Record::Topic(topic)) = l.decode(&form, b"DIAL")? else {
            continue;
        };
        source.topics.insert(
            form,
            SourceTopic {
                editor_id: topic.common.editor_id.clone(),
                name: topic.common.full_name.clone(),
                topic_type: topic.data.map_or(0, |d| d.topic_type),
                flags: topic.data.and_then(|d| d.flags).unwrap_or(0),
                priority: topic.priority.unwrap_or(0.0),
            },
        );
    }
    Ok(())
}
