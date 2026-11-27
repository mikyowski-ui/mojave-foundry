//! Filling a [`SimSource`] from a MIR.
//!
//! Typed queries first (wave S1, records revision 2): dialogue infos with
//! their links and flags (`Records::topic_info_rows`), embedded script
//! bodies with their reference and variable lists
//! (`Records::script_contexts`), globals, game settings, messages,
//! factions, the actors' AI data, factions and voice through templates
//! (`actor_ai`, `actor_factions`), their packages (`actor_packages`) and the
//! package rows with their behaviour and type flags. What has no typed
//! query is read here and only here: the selection of infos by speaker
//! (`infos` and `conditions`), topics and quests (decoded from the raw
//! store: `DIAL` and `QUST` have no typed row with every field the build
//! needs, the quest delay among them), the race, class and flags of an NPC
//! or creature (decoded), and the placed references' enable parents and
//! locks (`placed_refs`).
//!
//! Raw form ids in condition parameters still resolve through
//! [`FormResolver`]'s assumption that a plugin's masters are the plugins
//! before it: the MIR keeps no master list (the typed queries resolve their
//! own forms).

use std::collections::{BTreeMap, BTreeSet, HashMap};

use fnv_esm::{FormId, Record};

use super::{
    CondValue, Corpus, EmbeddedScript, FormResolver, SimSource, SourceActor, SourceBase,
    SourceBody, SourceCondition, SourceFaction, SourceGlobal, SourceInfo, SourceLocation,
    SourceLogEntry, SourceMenuItem, SourceMessage, SourceName, SourceNote, SourceObjective,
    SourcePackage, SourceQuest, SourceRef, SourceRelation, SourceResponse, SourceScript,
    SourceStage, SourceTarget, SourceTerminal, SourceTopic,
};
use crate::error::TranspileError;
use crate::ir::{AiData, FormKey, PackageEvent, SimSlice};
use crate::obscript::DeclaredVar;

type Result<T> = std::result::Result<T, TranspileError>;

/// The editor id of the shared failure topic of Fallout 3 speech
/// challenges (spec 4.7).
pub const FAILURE_TOPIC: &str = "SpeechChallengeFailure";

fn mir_key(key: &FormKey) -> Option<mir::FormKey> {
    key.as_str().parse().ok()
}

fn key(text: String) -> Option<FormKey> {
    text.parse::<mir::FormKey>().ok().map(FormKey::from)
}

fn opt(key: Option<&mir::FormKey>) -> Option<FormKey> {
    key.map(FormKey::from)
}

/// A body from a typed script context; `None` without source text.
fn context_body(context: &mir::ScriptContextRow) -> Option<SourceBody> {
    let source = context.source.as_deref()?;
    if source.trim().is_empty() {
        return None;
    }
    Some(SourceBody {
        source: source.to_owned(),
        vars: context
            .locals
            .iter()
            .map(|l| DeclaredVar {
                index: l.index,
                name: l.name.clone().unwrap_or_default(),
                flags: l.flags,
            })
            .collect(),
        references: context
            .references
            .iter()
            .filter_map(|r| r.form().map(FormKey::from))
            .collect(),
    })
}

struct Loader<'r> {
    records: &'r mir::Records,
    resolver: FormResolver,
    /// Editor ids by form, for the `KNAM` labels.
    names: HashMap<FormKey, String>,
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
            reference: c
                .reference
                .as_ref()
                .or(c.param1_ref.as_ref().filter(|_| c.run_on == 2))
                .map(FormKey::from),
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

    /// The embedded script bodies of a record, by context.
    fn contexts(&self, owner: &FormKey) -> Result<BTreeMap<String, SourceBody>> {
        let Some(k) = mir_key(owner) else {
            return Ok(BTreeMap::new());
        };
        Ok(self
            .records
            .script_contexts(&k)?
            .iter()
            .filter_map(|c| Some((c.context.clone(), context_body(c)?)))
            .collect())
    }

    fn row_conditions(
        &mut self,
        owner: usize,
        list: &[mir::ConditionRow],
    ) -> Result<Vec<SourceCondition>> {
        list.iter().map(|c| self.row_condition(owner, c)).collect()
    }
}

/// Reads what the build needs for a slice. The selection: the slice's
/// references (with their enable parents and linked references), their
/// bases and actors, the actors' packages, the dialogue (spec 4.2: the
/// topics with an INFO that names a slice actor as speaker, `ANAM`,
/// `GetIsID` or `GetIsVoiceType` on the subject, then every topic those
/// topics' INFOs lead to, and the shared failure topic; within each topic
/// the INFOs whose speaker terms allow a slice actor or that have none),
/// the slice's terminals and their sub-terminals, and every script, quest,
/// global, game setting, message, faction, form list and note.
///
/// # Errors
///
/// When the MIR cannot be read, or was written before records revision 2
/// ([`mir::MirError::MissingTable`]: extract again).
pub fn load(mir: &mir::Mir, slice: &SimSlice) -> Result<SimSource> {
    let records = mir.records()?;
    let (mut l, mut source) = common(&records)?;
    load_scripts(&mut l, &mut source)?;
    load_refs(&mut l, &mut source, slice)?;
    load_actors(&mut l, &mut source)?;
    load_quests(&mut l, &mut source)?;
    load_globals(&mut l, &mut source)?;
    load_settings(&mut l, &mut source)?;
    load_factions(&mut l, &mut source)?;
    load_form_lists(&mut l, &mut source)?;
    load_messages(&mut l, &mut source)?;
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
                conditions: Vec::new(),
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
        names: HashMap::new(),
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
         initially_disabled, persistent, lock_level, lock_key, cell FROM placed_refs \
         WHERE form = '{}'",
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
                lock: r[8]
                    .as_deref()
                    .and_then(|v| v.parse::<i64>().ok())
                    .map(|level| (level.clamp(0, 255) as u8, r[9].clone().and_then(key))),
                cell: r[10].clone().and_then(key),
            })
        },
        11,
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
        // Race, class and the base flags have no typed query: decoded.
        match l.decode(&form, &sig)? {
            Some(Record::Npc(npc)) => {
                actor.race = npc.race.and_then(|v| l.resolve(owner, v));
                actor.class = npc.class.and_then(|v| l.resolve(owner, v));
                actor.voice = npc.voice.and_then(|v| l.resolve(owner, v));
                actor.actor_flags = npc.base.map_or(0, |b| b.flags);
            }
            Some(Record::Creature(creature)) => {
                actor.voice = creature.voice.and_then(|v| l.resolve(owner, v));
                actor.actor_flags = creature.base.map_or(0, |b| b.flags);
            }
            _ => {}
        }
        actor.female = actor.actor_flags & 1 != 0;
        if let Some(k) = mir_key(&form) {
            actor.packages = l
                .records
                .actor_packages(&k)?
                .iter()
                .map(FormKey::from)
                .collect();
            if let Some(ai) = l.records.actor_ai(&k)? {
                actor.ai = ai.ai.map(|a| AiData {
                    aggression: a.aggression,
                    confidence: a.confidence,
                    energy: a.energy_level,
                    responsibility: a.responsibility,
                    mood: a.mood,
                    services: a.services,
                    assistance: a.assistance,
                });
                if let Some(voice) = &ai.voice {
                    actor.voice = Some(FormKey::from(voice));
                }
            }
            actor.factions = l
                .records
                .actor_factions(&k)?
                .iter()
                .filter_map(|f| Some((FormKey::from(f.faction.as_ref()?), f.rank as i8)))
                .collect();
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
    let conditions = l.row_conditions(owner, &row.conditions)?;
    let mut bodies = l.contexts(&form)?;
    let events = [
        ("on begin", PackageEvent::Begin),
        ("on end", PackageEvent::End),
        ("on change", PackageEvent::Change),
    ]
    .into_iter()
    .filter(|(context, _)| row.events.iter().any(|e| e.event == *context))
    .map(|(context, event)| (event, bodies.remove(context)))
    .collect();
    let data = row.data();
    Ok(SourcePackage {
        editor_id: row.editor_id.clone(),
        package_type: row.package_type.unwrap_or(0),
        flags: row.flags.unwrap_or(0),
        behaviour_flags: data
            .as_ref()
            .and_then(fnv_esm::PackageData::behaviour_flags)
            .unwrap_or(0),
        type_flags: data
            .as_ref()
            .and_then(fnv_esm::PackageData::type_flags)
            .unwrap_or(0),
        schedule: row
            .schedule
            .map(|s| (s.month, s.day_of_week, s.date, s.hour, s.duration)),
        location: row.location.as_ref().map(|loc| SourceLocation {
            location_type: loc.location_type,
            form: opt(loc.form.as_ref()),
            value: loc.value,
            radius: loc.radius,
        }),
        target: row.target.as_ref().map(|t| SourceTarget {
            target_type: t.target_type,
            form: opt(t.form.as_ref()),
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
        let mut bodies = l.contexts(&form)?;
        let mut stages = Vec::new();
        for stage in &quest.stages {
            let mut entries = Vec::new();
            for (number, entry) in stage.entries.iter().enumerate() {
                let context = format!("stage {} entry {number}", stage.index);
                let script = match bodies.remove(&context) {
                    Some(body) => Some(body),
                    None => l.body(owner, &entry.script),
                };
                entries.push(SourceLogEntry {
                    flags: entry.flags.unwrap_or(0),
                    conditions: l.conditions(owner, &entry.conditions)?,
                    text: entry.text.clone(),
                    script,
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
                conditions: l.conditions(owner, &quest.conditions)?,
            },
        );
    }
    Ok(())
}

fn load_globals(l: &mut Loader<'_>, source: &mut SimSource) -> Result<()> {
    for row in l.records.globals()? {
        source.globals.insert(
            FormKey::from(&row.form),
            SourceGlobal {
                editor_id: row.editor_id,
                kind: row.kind.map_or('s', |k| char::from(k.code())),
                value: row.value.unwrap_or(0.0),
            },
        );
    }
    Ok(())
}

fn load_settings(l: &mut Loader<'_>, source: &mut SimSource) -> Result<()> {
    for row in l.records.game_settings()? {
        if let Some(value) = row
            .value
            .as_ref()
            .and_then(fnv_esm::GameSettingValue::as_f32)
        {
            source.settings.insert(row.name, f64::from(value));
        }
    }
    Ok(())
}

fn load_factions(l: &mut Loader<'_>, source: &mut SimSource) -> Result<()> {
    for row in l.records.factions()? {
        source.factions.insert(
            FormKey::from(&row.form),
            SourceFaction {
                editor_id: row.editor_id,
                flags: row.flags.unwrap_or(0),
                flags2: row.flags2.unwrap_or(0),
                relations: row
                    .relations
                    .iter()
                    .filter_map(|r| {
                        Some(SourceRelation {
                            other: FormKey::from(r.other.as_ref()?),
                            modifier: r.modifier,
                            reaction: r.combat_reaction,
                        })
                    })
                    .collect(),
            },
        );
    }
    Ok(())
}

/// Every form list's members (`LNAM`, a form id each), resolved, in order.
fn load_form_lists(l: &mut Loader<'_>, source: &mut SimSource) -> Result<()> {
    let lists = l.forms_of(
        "form lists",
        "SELECT form FROM records WHERE sig = 'FLST' ORDER BY form",
    )?;
    for list in lists {
        let Some(k) = mir_key(&list) else { continue };
        let owner = l.plugin_of(&list)?.unwrap_or(0);
        let members: Vec<FormKey> = l
            .records
            .raw_subrecords(&k)?
            .into_iter()
            .filter(|(sig, _)| sig == "LNAM")
            .filter_map(|(_, data)| {
                let raw = u32::from_le_bytes(data.get(..4)?.try_into().ok()?);
                l.resolve(owner, FormId(raw))
            })
            .collect();
        source.form_lists.insert(list, members);
    }
    Ok(())
}

fn load_messages(l: &mut Loader<'_>, source: &mut SimSource) -> Result<()> {
    for row in l.records.messages()? {
        let form = FormKey::from(&row.form);
        let owner = l.plugin_of(&form)?.unwrap_or(0);
        let mut buttons = Vec::new();
        for button in &row.buttons {
            buttons.push((
                button.text.clone().unwrap_or_default(),
                l.row_conditions(owner, &button.conditions)?,
            ));
        }
        source.messages.insert(
            form,
            SourceMessage {
                editor_id: row.editor_id,
                title: row.title,
                text: row.text,
                flags: row.flags.unwrap_or(0),
                buttons,
            },
        );
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
    // A voice note reads as its topic's responses (spec 9.2, decision 4).
    let topics: BTreeSet<FormKey> = source
        .notes
        .values()
        .filter(|n| n.note_type == 3)
        .filter_map(|n| n.topic.clone())
        .collect();
    for topic in topics {
        let Some(k) = mir_key(&topic) else { continue };
        let lines: Vec<String> = l
            .records
            .topic_info_rows(&k)?
            .iter()
            .flat_map(|info| info.responses.iter())
            .filter_map(|r| r.text.clone())
            .filter(|t| !t.trim().is_empty())
            .collect();
        source.voice_lines.insert(topic, lines);
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
        let mut bodies = l.contexts(&form)?;
        let mut items = Vec::new();
        for item in &row.items {
            let context = format!("item {}", item.seq);
            let conditions = l.row_conditions(owner, &item.conditions)?;
            let sub_terminal = item.sub_terminal.as_ref().map(FormKey::from);
            if let Some(sub) = &sub_terminal {
                pending.push(sub.clone());
            }
            let script = bodies.remove(&context).or_else(|| {
                item.script_source
                    .clone()
                    .filter(|s| !s.trim().is_empty())
                    .map(|s| SourceBody {
                        source: s,
                        vars: Vec::new(),
                        references: Vec::new(),
                    })
            });
            items.push(SourceMenuItem {
                text: item.text.clone(),
                result: item.result_text.clone(),
                flags: item.flags.unwrap_or(0),
                display_note: item.display_note.as_ref().map(FormKey::from),
                sub_terminal,
                script,
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

/// The speaker terms of an INFO: `ANAM` and the bases of `GetIsID == 1` on
/// the subject; and the voice types of `GetIsVoiceType == 1` on the
/// subject.
fn speaker_terms(
    l: &Loader<'_>,
    owner: usize,
    row: &mir::InfoRow,
) -> (BTreeSet<FormKey>, BTreeSet<FormKey>) {
    let mut bases = BTreeSet::new();
    let mut voices = BTreeSet::new();
    if let Some(speaker) = &row.speaker {
        bases.insert(FormKey::from(speaker));
    }
    for c in &row.conditions {
        let is_one = match (c.comparison.as_str(), c.value) {
            ("==" | ">=", Some(v)) => v == 1.0,
            ("!=", Some(v)) => v == 0.0,
            _ => false,
        };
        if c.run_on != 0 || !is_one || c.global.is_some() {
            continue;
        }
        let Some(form) = l.resolver.resolve(owner, c.param1) else {
            continue;
        };
        match c.function {
            72 => {
                bases.insert(form);
            }
            427 => {
                voices.insert(form);
            }
            _ => {}
        }
    }
    (bases, voices)
}

fn info_from_row(
    l: &mut Loader<'_>,
    owner: usize,
    topic: &FormKey,
    position: usize,
    row: &mir::InfoRow,
) -> Result<SourceInfo> {
    let form = FormKey::from(&row.form);
    let mut bodies = l.contexts(&form)?;
    let flags = match row.flag_bytes {
        0 => 0,
        1 => row.flags.unwrap_or(0) & 0x00FF,
        _ => row.flags.unwrap_or(0),
    };
    let label = match (&row.knam, row.knam_sig.as_deref()) {
        (Some(k), Some("AVIF")) => l
            .names
            .get(&FormKey::from(k))
            .and_then(|edid| crate::ir::ActorValue::named(edid.strip_prefix("AV").unwrap_or(edid)))
            .map(|av| av.0),
        _ => None,
    };
    let topics = |list: &[mir::FormKey]| list.iter().map(FormKey::from).collect::<Vec<_>>();
    Ok(SourceInfo {
        topic: topic.clone(),
        quest: opt(row.quest.as_ref()),
        order: (0, position as u64),
        info_type: row.info_type.unwrap_or(0),
        flags,
        speaker: opt(row.speaker.as_ref()),
        prompt: row.prompt.clone(),
        responses: row
            .responses
            .iter()
            .map(|r| SourceResponse {
                number: r.number.unwrap_or(0),
                emotion_type: r.emotion_type.unwrap_or(0),
                emotion_value: r.emotion_value.unwrap_or(0),
                text: r.text.clone().unwrap_or_default(),
            })
            .collect(),
        conditions: l.row_conditions(owner, &row.conditions)?,
        choices: topics(&row.choices),
        link_from: topics(&row.link_from),
        add_topics: topics(&row.add_topics),
        begin: bodies.remove("result begin"),
        end: bodies.remove("result end"),
        challenge: row.speech_challenge,
        label,
        knam: opt(row.knam.as_ref()),
        knam_sig: row.knam_sig.clone(),
    })
}

fn load_dialogue(l: &mut Loader<'_>, source: &mut SimSource) -> Result<()> {
    let actors: BTreeSet<FormKey> = source.actors.keys().cloned().collect();
    let voices: BTreeSet<FormKey> = source
        .actors
        .values()
        .filter_map(|a| a.voice.clone())
        .collect();
    for name in &source.names {
        l.names.insert(name.form.clone(), name.editor_id.clone());
    }
    // Seeds: the topics of the INFOs that name a slice actor or voice.
    let info_topics: HashMap<FormKey, FormKey> = l
        .rows(
            "info topics",
            "SELECT form, topic FROM infos WHERE topic IS NOT NULL",
            |r| Some((key(r[0].clone()?)?, key(r[1].clone()?)?)),
            2,
        )?
        .into_iter()
        .collect();
    let mut seeds: BTreeSet<FormKey> = BTreeSet::new();
    for (info, speaker) in l.rows(
        "info speakers",
        "SELECT form, speaker FROM infos WHERE speaker IS NOT NULL",
        |r| Some((key(r[0].clone()?)?, key(r[1].clone()?)?)),
        2,
    )? {
        if actors.contains(&speaker)
            && let Some(topic) = info_topics.get(&info)
        {
            seeds.insert(topic.clone());
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
        let Some(topic) = info_topics.get(&info) else {
            continue;
        };
        if seeds.contains(topic) {
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
            seeds.insert(topic.clone());
        }
    }
    for found in l.records.find_editor_id(FAILURE_TOPIC)? {
        seeds.insert(FormKey::from(&found));
    }
    // Reverse links: topic T is offered after topic X when an INFO of T
    // names X in `TCLF`.
    let mut linked_from: BTreeMap<FormKey, BTreeSet<FormKey>> = BTreeMap::new();
    for (info, from) in l.rows(
        "reverse links",
        "SELECT info, topic FROM info_links WHERE kind = 'link_from'",
        |r| Some((key(r[0].clone()?)?, key(r[1].clone()?)?)),
        2,
    )? {
        if let Some(topic) = info_topics.get(&info) {
            linked_from.entry(from).or_default().insert(topic.clone());
        }
    }

    let mut pending: Vec<FormKey> = seeds.into_iter().rev().collect();
    let mut seen: BTreeSet<FormKey> = BTreeSet::new();
    let mut topics: BTreeSet<FormKey> = BTreeSet::new();
    while let Some(topic) = pending.pop() {
        if !seen.insert(topic.clone()) {
            continue;
        }
        let Some(k) = mir_key(&topic) else { continue };
        let rows = l.records.topic_info_rows(&k)?;
        if rows.is_empty() {
            continue;
        }
        topics.insert(topic.clone());
        for (position, row) in rows.iter().enumerate() {
            let form = FormKey::from(&row.form);
            let owner = l.plugin_of(&form)?.unwrap_or(0);
            let (bases, info_voices) = speaker_terms(l, owner, row);
            let relevant = (bases.is_empty() && info_voices.is_empty())
                || bases.iter().any(|b| actors.contains(b))
                || info_voices.iter().any(|v| voices.contains(v));
            if !relevant {
                continue;
            }
            let loaded = info_from_row(l, owner, &topic, position, row)?;
            for next in loaded.choices.iter().chain(&loaded.add_topics) {
                pending.push(next.clone());
            }
            topics.extend(loaded.choices.iter().cloned());
            topics.extend(loaded.link_from.iter().cloned());
            topics.extend(loaded.add_topics.iter().cloned());
            source.infos.insert(form, loaded);
        }
        if let Some(after) = linked_from.get(&topic) {
            pending.extend(after.iter().cloned());
        }
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
