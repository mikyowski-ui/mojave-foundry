//! Trigger-volume probe: counts the placed references that carry a
//! primitive (`XPRM`), by the base form's record type, by shape, and by
//! whether the base names a model. It prints tallies only (and the most
//! common stand-in model file names, which are paths, not contents), and it
//! writes nothing.
//!
//! ```text
//! FNV_ESM_PLUGIN=/path/to/plugin.esm cargo run -p fnv-esm --example triggers
//! ```

use std::collections::BTreeMap;

use fnv_esm::*;

type Tally<K> = BTreeMap<K, u64>;

fn bump<K: Ord>(tally: &mut Tally<K>, key: K) {
    *tally.entry(key).or_default() += 1;
}

/// What the probe keeps of a base form.
struct Base {
    sig: Sig,
    editor_id: Option<String>,
    model: Option<String>,
}

/// What the probe keeps of a reference with a primitive.
struct Volume {
    record: Sig,
    base: Option<FormId>,
    primitive: Option<Primitive>,
    others: Vec<String>,
}

fn model_name(data: &[u8]) -> String {
    let text: String = data
        .iter()
        .take_while(|&&b| b != 0)
        .map(|&b| char::from(b))
        .collect();
    text.to_ascii_lowercase()
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or_default()
        .to_owned()
}

fn main() {
    let path = std::env::var("FNV_ESM_PLUGIN").expect("set FNV_ESM_PLUGIN to a plugin path");
    let bytes = std::fs::read(path).expect("read plugin");
    let plugin = Plugin::parse(&bytes).expect("plugin");
    let mut walker = plugin.walk();
    let mut bases: BTreeMap<FormId, Base> = BTreeMap::new();
    let mut volumes = Vec::new();
    let mut placed_refs = 0u64;
    let mut pipe_refs: Tally<String> = Tally::new();
    let mut all_refs: Vec<(Option<FormId>, bool)> = Vec::new();
    let mut errors: Tally<String> = Tally::new();
    loop {
        let record = match walker.next_event() {
            Ok(Some(Event::Record(record))) => record,
            Ok(Some(_)) => continue,
            Ok(None) => break,
            Err(error) => {
                println!("walk error: {error}");
                break;
            }
        };
        let sig = record.sig();
        let data = match record.data() {
            Ok(data) => data,
            Err(_) => {
                bump(&mut errors, format!("{sig}: data"));
                continue;
            }
        };
        let Ok(subs) = data.collect() else {
            bump(&mut errors, format!("{sig}: subrecord framing"));
            continue;
        };
        let placed = [sigs::REFR, sigs::ACHR, sigs::ACRE].contains(&sig);
        if !placed {
            let model = subs
                .iter()
                .find(|s| &s.sig.0 == b"MODL")
                .map(|s| model_name(s.data))
                .filter(|m| !m.is_empty());
            let editor_id = subs
                .iter()
                .find(|s| &s.sig.0 == b"EDID")
                .map(|s| model_name(s.data));
            bases.insert(
                record.form_id(),
                Base {
                    sig,
                    editor_id,
                    model,
                },
            );
            continue;
        }
        placed_refs += 1;
        let Record::Placed(reference) = decode_record(sig, record.header.meta, &subs) else {
            bump(&mut errors, format!("{sig}: not decoded as placed"));
            continue;
        };
        let has_xprm = subs.iter().any(|s| &s.sig.0 == b"XPRM");
        all_refs.push((reference.base, has_xprm));
        if let Some(b) = reference.base.and_then(|b| bases.get(&b))
            && b.model.as_deref() == Some("watertowerpipes01.nif")
        {
            bump(
                &mut pipe_refs,
                format!(
                    "{} xprm {has_xprm} disabled {} parent {} persistent {}",
                    b.editor_id.clone().unwrap_or_default(),
                    reference.is_initially_disabled(),
                    reference.enable_parent.is_some(),
                    reference.is_persistent()
                ),
            );
        }
        if !has_xprm {
            continue;
        }
        let mut others: Vec<String> = subs
            .iter()
            .map(|s| s.sig.to_string())
            .filter(|s| s.starts_with('X') && s != "XPRM")
            .collect();
        others.sort();
        others.dedup();
        volumes.push(Volume {
            record: sig,
            base: reference.base,
            primitive: reference.primitive,
            others,
        });
    }

    println!("placed references {placed_refs}");
    println!("decode errors {errors:?}");
    println!("references with XPRM {}", volumes.len());
    let mut by_kind: Tally<String> = Tally::new();
    let mut by_shape: Tally<String> = Tally::new();
    let mut models: Tally<String> = Tally::new();
    let mut extras: Tally<String> = Tally::new();
    let mut marker_ids: Tally<String> = Tally::new();
    let mut model_bases: BTreeMap<String, Vec<FormId>> = BTreeMap::new();
    let mut with_model = 0u64;
    let mut undecoded = 0u64;
    for volume in &volumes {
        let base = volume.base.and_then(|b| bases.get(&b));
        let base_sig = base.map_or("not in plugin".to_owned(), |b| b.sig.to_string());
        let model = base.and_then(|b| b.model.clone());
        if model.is_some() {
            with_model += 1;
        }
        let Some(p) = volume.primitive else {
            undecoded += 1;
            continue;
        };
        bump(
            &mut by_kind,
            format!(
                "{} base {base_sig} model {}",
                volume.record,
                model.is_some()
            ),
        );
        bump(
            &mut by_shape,
            format!(
                "shape {} base {base_sig} bounds>0 {} colour0..1 {} unknown {}",
                p.shape,
                p.bounds.iter().all(|b| b.is_finite() && *b > 0.0),
                p.color.iter().all(|c| (0.0..=1.0).contains(c)),
                if p.unknown == 0.0 {
                    "0".to_owned()
                } else if (0.0..=1.0).contains(&p.unknown) {
                    "0..=1".to_owned()
                } else {
                    "other".to_owned()
                }
            ),
        );
        for other in &volume.others {
            bump(&mut extras, format!("{other} base {base_sig}"));
        }
        if let Some(b) = base.filter(|b| &b.sig.0 != b"ACTI") {
            bump(
                &mut marker_ids,
                format!("{} {}", b.sig, b.editor_id.clone().unwrap_or_default()),
            );
        }
        if let Some(model) = model {
            bump(&mut models, model.clone());
            model_bases.entry(model).or_default().extend(volume.base);
        }
    }
    println!("  of which the base has a model {with_model}");
    println!("  of which XPRM did not decode {undecoded}");
    println!("\n== by record, base type and model ==");
    for (what, count) in &by_kind {
        println!("  {count:>6}  {what}");
    }
    println!("\n== by shape ==");
    for (what, count) in &by_shape {
        println!("  {count:>6}  {what}");
    }
    println!("\n== other X subrecords on those references ==");
    for (what, count) in &extras {
        println!("  {count:>6}  {what}");
    }
    println!("\n== stand-in model file names ==");
    let mut ranked: Vec<_> = models.iter().collect();
    ranked.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
    for (model, count) in ranked {
        let mut distinct = model_bases.get(model).cloned().unwrap_or_default();
        distinct.sort();
        distinct.dedup();
        println!("  {count:>6} refs {:>4} bases  {model}", distinct.len());
    }
    println!("\n== every base and reference naming a stand-in model ==");
    for model in models.keys() {
        let mut by_sig: Tally<String> = Tally::new();
        for base in bases.values() {
            if base.model.as_ref() == Some(model) {
                bump(&mut by_sig, format!("{} bases", base.sig));
            }
        }
        for (base, has_xprm) in &all_refs {
            if let Some(b) = base.and_then(|b| bases.get(&b))
                && b.model.as_ref() == Some(model)
            {
                bump(&mut by_sig, format!("{} refs with XPRM {has_xprm}", b.sig));
            }
        }
        println!("  {model}: {by_sig:?}");
    }
    println!("\n== references to stand-in pipe bases ==");
    for (what, count) in &pipe_refs {
        println!("  {count:>6}  {what}");
    }
    println!("\n== non-activator bases with XPRM (editor ids) ==");
    for (what, count) in &marker_ids {
        println!("  {count:>6}  {what}");
    }
}
