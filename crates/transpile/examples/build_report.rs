//! Runs `transpile::build` on a MIR directory and a placement index
//! (`map-placements.json`, schema 2) and prints, per report section,
//! what was compiled, folded, dropped and approximated, with the top
//! fallback reasons.
//!
//! ```text
//! cargo run -p transpile --example build_report -- <mir dir> <map-placements.json>
//!     [--speaker EDID]... [--topic EDID]... [--quest EDID]... [--json out.json]
//!     [--team-faction EDID]...
//! ```
//!
//! `--speaker`, `--topic` and `--quest` print what the build made of those
//! records. `--team-faction` adds to `SimConfig::team_factions` (spec 7.5).
//! Reads only; prints counts and editor ids, never source text.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde_json::Value;
use transpile::diag::summarize;
use transpile::ir::{
    FormKey, Module, Pick, PlacedGroup, PlacedKind, PlacedLock, PlacedObject, Placement, SimConfig,
    SimSlice, SkillCheck, Stmt,
};

fn usage() -> ! {
    eprintln!(
        "usage: build_report <mir dir> <map-placements.json> [--speaker EDID]... \
         [--topic EDID]... [--quest EDID]... [--json out.json] [--team-faction EDID]..."
    );
    std::process::exit(2)
}

fn key(v: Option<&Value>) -> Option<FormKey> {
    v.and_then(Value::as_str).map(|s| FormKey(s.to_owned()))
}

fn kind(text: &str) -> PlacedKind {
    match text {
        "critter" => PlacedKind::Critter,
        "door" => PlacedKind::Door,
        "container" => PlacedKind::Container,
        "item" => PlacedKind::Item,
        "terminal" => PlacedKind::Terminal,
        _ => PlacedKind::Scenery,
    }
}

/// The placement index as the build's input (what the `sim` stage does).
fn placement(index: &Value) -> Placement {
    let mut objects = Vec::new();
    for p in index["placed"].as_array().into_iter().flatten() {
        let int = |name: &str| p[name].as_u64();
        let presence = p["presence_of"].as_str().is_some();
        let reference = p["reference"]
            .as_str()
            .map(|r| FormKey(r.split('@').next().unwrap_or(r).to_owned()));
        objects.push(PlacedObject {
            map: p["map"].as_str().unwrap_or_default().to_owned(),
            map_number: int("map_number").unwrap_or(0) as u32,
            elevation: int("elevation").unwrap_or(0) as u8,
            hex: p["hex"].as_i64().unwrap_or(0).max(0) as u32,
            kind: kind(p["kind"].as_str().unwrap_or_default()),
            object_id: int("object_id").map(|v| v as u32),
            pid: int("pid").map(|v| v as u32),
            reference,
            base: key(p.get("base")),
            hidden: p["hidden"].as_bool().unwrap_or(false),
            group: p
                .get("group")
                .filter(|g| g.is_object())
                .map(|g| PlacedGroup {
                    root: FormKey(g["root"].as_str().unwrap_or_default().to_owned()),
                    opposite: g["opposite"].as_bool().unwrap_or(false),
                    initially_enabled: g["initially_enabled"].as_bool().unwrap_or(true),
                }),
            lock: p.get("lock").filter(|l| l.is_object()).map(|l| PlacedLock {
                locked: l["locked"].as_bool().unwrap_or(false),
                level: l["level"].as_u64().unwrap_or(0).min(255) as u8,
                key: key(l.get("key")),
                key_pid: l["key_pid"].as_u64().map(|v| v as u32),
            }),
            linked_ref: key(p.get("linked_ref")),
            hostile: p["hostile"].as_bool().unwrap_or(false),
            presence,
            // Markers are in the index's unplaced list without a hex; the
            // `sim` stage adds them with their projected hex.
            marker: false,
        });
    }
    Placement { objects }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let (Some(mir_dir), Some(index_path)) = (args.next(), args.next()) else {
        usage()
    };
    let mut speakers = Vec::new();
    let mut topics = Vec::new();
    let mut quests = Vec::new();
    let mut json_out: Option<PathBuf> = None;
    let mut config = SimConfig::default();
    while let Some(flag) = args.next() {
        let value = args.next().unwrap_or_else(|| usage());
        match flag.as_str() {
            "--speaker" => speakers.push(value),
            "--topic" => topics.push(value),
            "--quest" => quests.push(value),
            "--json" => json_out = Some(PathBuf::from(value)),
            "--team-faction" => config.team_factions.push(value),
            _ => usage(),
        }
    }
    let index: Value =
        serde_json::from_str(&std::fs::read_to_string(&index_path).unwrap_or_else(|e| {
            eprintln!("reading {index_path}: {e}");
            std::process::exit(1)
        }))
        .unwrap_or_else(|e| {
            eprintln!("parsing {index_path}: {e}");
            std::process::exit(1)
        });
    let slice = SimSlice {
        name: index["layer"].as_str().unwrap_or_default().to_owned(),
        placement: placement(&index),
        ..SimSlice::default()
    };
    let mir = mir::Mir::open(std::path::Path::new(&mir_dir)).unwrap_or_else(|e| {
        eprintln!("opening {mir_dir}: {e}");
        std::process::exit(1)
    });
    let started = std::time::Instant::now();
    let module = match transpile::build(&mir, &slice, &config) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("build failed: {e}");
            std::process::exit(1)
        }
    };
    println!(
        "{}: {} placed objects, built in {:.1} s",
        slice.name,
        slice.placement.objects.len(),
        started.elapsed().as_secs_f64()
    );
    println!(
        "module: {} texts, {} topics, {} infos, {} scripts, {} quests, {} globals, {} packages, \
         {} groups, {} terminals, {} notes, {} refs, {} actors, {} diagnostics",
        module.texts.len(),
        module.topics.len(),
        module.infos.len(),
        module.scripts.len(),
        module.quests.len(),
        module.globals.len(),
        module.packages.len(),
        module.groups.len(),
        module.terminals.len(),
        module.notes.len(),
        module.refs.len(),
        module.actors.len(),
        module.diagnostics.len()
    );
    for summary in summarize(&module) {
        let Some(section) = summary.section else {
            continue;
        };
        let compiled: Vec<String> = summary
            .compiled
            .iter()
            .map(|(k, v)| format!("{k} {v}"))
            .collect();
        println!(
            "\n## {}: compiled [{}]; folded {}, dropped {}, fallback {}, notes {}",
            section.name(),
            compiled.join(", "),
            summary.folded,
            summary.dropped,
            summary.fallback,
            summary.notes
        );
        for (reason, count) in summary.top_reasons(8) {
            println!("    {count:>6}  {reason}");
        }
    }
    for name in &speakers {
        speaker(&module, name);
    }
    for name in &topics {
        topic(&module, name);
    }
    for name in &quests {
        quest(&module, name);
    }
    if let Some(path) = json_out {
        std::fs::write(&path, module.to_json()).unwrap_or_else(|e| {
            eprintln!("writing {}: {e}", path.display());
            std::process::exit(1)
        });
    }
}

fn topic_name(module: &Module, id: &transpile::ir::SimId) -> String {
    module
        .topics
        .get(id)
        .map_or_else(|| id.to_string(), |t| t.editor_id.clone())
}

fn speaker(module: &Module, name: &str) {
    let Some(d) = module
        .dialogues
        .values()
        .find(|d| d.editor_id.eq_ignore_ascii_case(name))
    else {
        println!("\nspeaker {name}: no dialogue");
        return;
    };
    println!(
        "\nspeaker {}: {} nodes, {} topics, greeting {}, counts {:?}",
        d.editor_id,
        d.nodes.len(),
        d.topics.len(),
        d.greeting
            .as_ref()
            .map_or("none".into(), |g| topic_name(module, g)),
        d.counts
    );
    let hub: Vec<String> = d
        .hub
        .iter()
        .map(|h| {
            format!(
                "{}{}",
                topic_name(module, &h.topic),
                if h.given.is_some() { " (given)" } else { "" }
            )
        })
        .collect();
    println!("  hub: {}", hub.join(", "));
    if let Some(g) = &d.greeting {
        let picks = &d.topics[g].picks;
        println!(
            "  greeting picks: {} ({} random groups)",
            picks.len(),
            picks
                .iter()
                .filter(|p| matches!(p, Pick::Random(_)))
                .count()
        );
    }
    let mut by_quest: BTreeMap<String, usize> = BTreeMap::new();
    for n in &d.nodes {
        let info = &module.infos[&n.info];
        let quest = module
            .quests
            .get(&info.quest)
            .map_or_else(|| info.quest.to_string(), |q| q.editor_id.clone());
        *by_quest.entry(quest).or_default() += 1;
    }
    println!("  nodes by quest: {by_quest:?}");
}

fn stage_sets(stmts: &[Stmt], out: &mut Vec<String>, module: &Module) {
    for call in transpile::ir::visit::calls_in_stmts(stmts) {
        if let transpile::ir::Func::Known(id) = &call.func
            && id.name() == "SetStage"
        {
            let quest = match call.args.first() {
                Some(transpile::ir::Expr::Form(q)) => module
                    .quests
                    .get(&transpile::ir::SimId::of(transpile::ir::SimKind::Quest, q))
                    .map_or_else(|| q.to_string(), |q| q.editor_id.clone()),
                _ => "?".into(),
            };
            let stage = match call.args.get(1) {
                Some(transpile::ir::Expr::Int(s)) => s.to_string(),
                _ => "?".into(),
            };
            out.push(format!("{quest} {stage}"));
        }
    }
}

fn topic(module: &Module, name: &str) {
    let Some(t) = module
        .topics
        .values()
        .find(|t| t.editor_id.eq_ignore_ascii_case(name))
    else {
        println!("\ntopic {name}: not in the module");
        return;
    };
    println!(
        "\ntopic {}: {} infos, top level {}",
        t.editor_id,
        t.infos.len(),
        t.top_level
    );
    for info_id in &t.infos {
        let info = &module.infos[info_id];
        let mut sets = Vec::new();
        for block in info.begin.iter().chain(info.end.iter()) {
            stage_sets(&block.body, &mut sets, module);
        }
        let check = match &info.check {
            Some(SkillCheck::Percent {
                difficulty,
                failure,
                ..
            }) => format!("percent difficulty {difficulty}, failure {failure:?}"),
            Some(SkillCheck::Threshold {
                stat, op, value, ..
            }) => {
                format!("threshold {stat:?} {} {value}", op.as_str())
            }
            None => "none".into(),
        };
        let said_by: Vec<String> = module
            .dialogues
            .values()
            .filter_map(|d| {
                let n = d.nodes.iter().find(|n| &n.info == info_id)?;
                Some(format!(
                    "{} (node {}, failure {})",
                    d.editor_id,
                    n.ordinal,
                    n.failure.as_ref().map_or("none".into(), |f| {
                        let picks = d.topics.get(f).map_or(0, |t| t.picks.len());
                        format!("{} with {picks} picks", topic_name(module, f))
                    })
                ))
            })
            .collect();
        println!(
            "  {}: check {check}; sets stages {sets:?}; said by {}",
            info_id,
            if said_by.is_empty() {
                "nobody".into()
            } else {
                said_by.join(", ")
            }
        );
    }
}

fn quest(module: &Module, name: &str) {
    let Some(q) = module
        .quests
        .values()
        .find(|q| q.editor_id.eq_ignore_ascii_case(name))
    else {
        println!("\nquest {name}: not in the module");
        return;
    };
    let stages: Vec<i16> = q.stages.iter().map(|s| s.index).collect();
    let journal = module.journal.get(&q.id);
    println!(
        "\nquest {}: stages {stages:?}, {} objectives, {} Pip-Boy rows, script {}",
        q.editor_id,
        q.objectives.len(),
        journal.map_or(0, |j| j.rows.len()),
        q.script.is_some()
    );
    // Stages set in dialogue, and the objectives their stage scripts show.
    let mut in_dialogue: BTreeMap<i16, Vec<String>> = BTreeMap::new();
    for d in module.dialogues.values() {
        for n in &d.nodes {
            let info = &module.infos[&n.info];
            let mut sets = Vec::new();
            for block in info.begin.iter().chain(info.end.iter()) {
                stage_sets(&block.body, &mut sets, module);
            }
            for set in sets {
                if let Some(stage) = set.strip_prefix(&format!("{} ", q.editor_id))
                    && let Ok(stage) = stage.parse::<i16>()
                {
                    in_dialogue
                        .entry(stage)
                        .or_default()
                        .push(d.editor_id.clone());
                }
            }
        }
    }
    for (stage, who) in &in_dialogue {
        let mut shows = Vec::new();
        if let Some(s) = q.stages.iter().find(|s| s.index == *stage) {
            objectives_shown(module, q, s, journal, None, &mut shows);
        }
        let mut who = who.clone();
        who.sort();
        who.dedup();
        println!("  stage {stage} set in dialogue by {who:?}; its script shows {shows:?}");
    }
}

/// The objectives of `quest` a stage's scripts display, directly or
/// through one stage of another quest that they set (the source's
/// objective quests).
fn objectives_shown(
    module: &Module,
    quest: &transpile::ir::Quest,
    stage: &transpile::ir::Stage,
    journal: Option<&transpile::ir::Journal>,
    via: Option<String>,
    shows: &mut Vec<String>,
) {
    use transpile::ir::{Expr, Func, SimId, SimKind};
    for entry in &stage.entries {
        let Some(script) = entry.script.as_ref().and_then(|id| module.scripts.get(id)) else {
            continue;
        };
        for block in &script.blocks {
            for call in transpile::ir::visit::calls_in_stmts(&block.body) {
                let Func::Known(id) = &call.func else {
                    continue;
                };
                match (
                    id.name(),
                    call.args.first(),
                    call.args.get(1),
                    call.args.get(2),
                ) {
                    ("SetObjectiveDisplayed", Some(Expr::Form(q)), Some(Expr::Int(n)), flag)
                        if *q == quest.id.form && !matches!(flag, Some(Expr::Int(0))) =>
                    {
                        let row = journal
                            .is_some_and(|j| j.rows.iter().any(|r| r.objective.sub == *n as u32));
                        shows.push(match &via {
                            Some(v) => format!("objective {n} via {v} (Pip-Boy row {row})"),
                            None => format!("objective {n} (Pip-Boy row {row})"),
                        });
                    }
                    ("SetStage", Some(Expr::Form(other)), Some(Expr::Int(s)), _)
                        if via.is_none() && *other != quest.id.form =>
                    {
                        if let Some(o) = module.quests.get(&SimId::of(SimKind::Quest, other))
                            && let Some(next) = o.stages.iter().find(|st| i32::from(st.index) == *s)
                        {
                            objectives_shown(
                                module,
                                quest,
                                next,
                                journal,
                                Some(format!("{} stage {s}", o.editor_id)),
                                shows,
                            );
                        }
                    }
                    _ => {}
                }
            }
        }
    }
}
