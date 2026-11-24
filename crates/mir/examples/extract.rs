//! Runs an extract on a real install, configured by environment variables,
//! and prints the summary and timings.
//!
//! ```text
//! MIR_DATA_DIR=<install>/Data      the install's data directory
//! MIR_PLUGINS=A.esm,B.esm          plugin list, in load order
//! MIR_OUT=<dir>                    output; keep it outside git or under out/
//! MIR_WORLDSPACE=<editor id>       the slice's worldspace
//! MIR_NAME=<name>                  the slice's name (default: the worldspace, lowercase)
//! MIR_X=-3..3  MIR_Y=-2..2         inclusive cell ranges
//! MIR_DEPTH=1                      teleport hops into interiors (default 1)
//! MIR_JOBS=4                       threads (default 4)
//! MIR_RAW=all|typed_only|none      raw store (default all)
//! MIR_GAME=fallout3|falloutnv       the game (default: detected from the first plugin)
//! MIR_REFERENCED=1                  also time and count `Records::referenced_refs`
//! ```
//!
//! `cargo run --release -p mir --example extract`

use std::env;
use std::ops::RangeInclusive;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

use mir::{ExtractOptions, GameVariant, Mir, RawStore, Slice, extract_for_game};

fn var(name: &str) -> Result<String, String> {
    env::var(name).map_err(|_| format!("{name} is not set"))
}

fn range(name: &str) -> Result<RangeInclusive<i32>, String> {
    let text = var(name)?;
    let (a, b) = text
        .split_once("..")
        .ok_or_else(|| format!("{name}: expected <from>..<to>"))?;
    let parse = |s: &str| {
        s.trim_start_matches('=')
            .parse::<i32>()
            .map_err(|e| format!("{name}: {e}"))
    };
    Ok(parse(a)?..=parse(b)?)
}

fn options() -> Result<ExtractOptions, String> {
    let raw_store = match env::var("MIR_RAW").as_deref() {
        Ok("typed_only") => RawStore::TypedOnly,
        Ok("none") => RawStore::None,
        _ => RawStore::All,
    };
    let number = |name: &str, default: u32| {
        env::var(name)
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(default)
    };
    let worldspace = var("MIR_WORLDSPACE")?;
    Ok(ExtractOptions {
        data_dir: PathBuf::from(var("MIR_DATA_DIR")?),
        plugins: var("MIR_PLUGINS")?
            .split(',')
            .map(|p| p.trim().to_owned())
            .filter(|p| !p.is_empty())
            .collect(),
        out: PathBuf::from(var("MIR_OUT")?),
        slice: Slice {
            name: env::var("MIR_NAME")
                .ok()
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| worldspace.to_ascii_lowercase()),
            worldspace,
            x: range("MIR_X")?,
            y: range("MIR_Y")?,
            interior_depth: number("MIR_DEPTH", 1),
        },
        jobs: number("MIR_JOBS", 4) as usize,
        raw_store,
    })
}

/// Times the referenced-reference queries and prints their counts.
fn referenced(out: &std::path::Path) {
    let records = match Mir::open(out).and_then(|m| m.records()) {
        Ok(records) => records,
        Err(error) => {
            eprintln!("referenced: {error}");
            return;
        }
    };
    let start = Instant::now();
    match records.referenced_ref_reasons() {
        Ok(reasons) => {
            let elapsed = start.elapsed().as_secs_f64();
            let mut by_reason = std::collections::BTreeMap::new();
            for set in reasons.values() {
                for reason in set {
                    *by_reason.entry(format!("{reason:?}")).or_insert(0usize) += 1;
                }
            }
            println!(
                "referenced refs: {} in {elapsed:.2} s; by reason: {by_reason:?}",
                reasons.len()
            );
        }
        Err(error) => eprintln!("referenced: {error}"),
    }
}

fn main() -> ExitCode {
    let options = match options() {
        Ok(options) => options,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::FAILURE;
        }
    };
    let game = match env::var("MIR_GAME").ok().filter(|g| !g.is_empty()) {
        Some(text) => match text.parse::<GameVariant>() {
            Ok(game) => Some(game),
            Err(error) => {
                eprintln!("MIR_GAME: {error}");
                return ExitCode::FAILURE;
            }
        },
        None => None,
    };
    let start = Instant::now();
    match extract_for_game(&options, game) {
        Ok(summary) => {
            for (step, time) in &summary.timings {
                println!("{step:>22}: {:>8.2} s", time.as_secs_f64());
            }
            println!("{:>22}: {:>8.2} s", "total", start.elapsed().as_secs_f64());
            println!(
                "records rebuilt: {} ({} records, {} raw subrecords, {} bad)",
                summary.records_rebuilt,
                summary.records,
                summary.raw_subrecords,
                summary.bad_records
            );
            println!(
                "cells: {} exterior, {} interior, {} references, {} issues",
                summary.exterior_cells, summary.interior_cells, summary.references, summary.issues
            );
            println!("meshes: {:?}", summary.meshes);
            println!("textures: {:?}", summary.textures);
            println!("actors: {}", summary.actors);
            println!("skeletons: {:?}", summary.skeletons);
            println!("animations: {:?}", summary.animations);
            println!(
                "game: {}; notes: {}",
                summary.game.map_or("?", GameVariant::id),
                summary.notes
            );
            if env::var("MIR_REFERENCED").is_ok_and(|v| v == "1") {
                referenced(&options.out);
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("extract failed: {error}");
            ExitCode::FAILURE
        }
    }
}
