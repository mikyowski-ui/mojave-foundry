//! Rebalance tables as TOML: the defaults round-trip, the example files
//! are exactly the defaults, a partial file overlays a game's default key
//! by key, and an unknown key is an error.
//!
//! `EMIT_BLESS=1 cargo test -p emit --test rebalance_toml` rewrites the
//! example files from the defaults.

use emit::rebalance::{
    FALLOUT3_TOML, NEW_VEGAS_TOML, Rebalance, SourceGame, WeaponClass, normalize_key,
};
use std::path::Path;

const HEADER_FO3: &str = "# Rebalance table: the full Fallout 3 default \
(emit::rebalance::Rebalance::fallout3_default).\n\
# Load with Rebalance::for_game(Fallout3) then Rebalance::overlay; keys left out keep\n\
# this default, unknown keys are errors.\n\n";
const HEADER_FNV: &str = "# Rebalance table: the full New Vegas default \
(emit::rebalance::Rebalance::new_vegas_default).\n\
# Load with Rebalance::for_game(NewVegas) then Rebalance::overlay; keys left out keep\n\
# this default, unknown keys are errors.\n\n";

fn text(game: SourceGame) -> String {
    let (header, table) = match game {
        SourceGame::Fallout3 => (HEADER_FO3, Rebalance::fallout3_default()),
        SourceGame::NewVegas => (HEADER_FNV, Rebalance::new_vegas_default()),
    };
    format!("{header}{}", toml::to_string(&table).unwrap())
}

fn overlaid(game: SourceGame, document: &str) -> Result<Rebalance, toml::de::Error> {
    let mut table = Rebalance::for_game(game);
    table.overlay(toml::Deserializer::parse(document)?)?;
    Ok(table)
}

#[test]
fn the_example_files_are_the_defaults() {
    if std::env::var_os("EMIT_BLESS").is_some() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples");
        std::fs::write(dir.join("rebalance-fo3.toml"), text(SourceGame::Fallout3)).unwrap();
        std::fs::write(dir.join("rebalance-fnv.toml"), text(SourceGame::NewVegas)).unwrap();
        return;
    }
    assert_eq!(FALLOUT3_TOML, text(SourceGame::Fallout3));
    assert_eq!(NEW_VEGAS_TOML, text(SourceGame::NewVegas));
}

#[test]
fn defaults_round_trip_through_toml() {
    for (game, file) in [
        (SourceGame::Fallout3, FALLOUT3_TOML),
        (SourceGame::NewVegas, NEW_VEGAS_TOML),
    ] {
        let default = Rebalance::for_game(game);
        let parsed: Rebalance = toml::from_str(&toml::to_string(&default).unwrap()).unwrap();
        assert_eq!(parsed, default);
        assert_eq!(toml::from_str::<Rebalance>(file).unwrap(), default);
        // Overlaying a whole file over the other game gives this game,
        // except map entries the file does not name, which an overlay
        // keeps (Fallout 3 has no `guns` skill; New Vegas keeps its own).
        let other = match game {
            SourceGame::Fallout3 => SourceGame::NewVegas,
            SourceGame::NewVegas => SourceGame::Fallout3,
        };
        let mut over = overlaid(other, file).unwrap();
        for (skill, set) in &default.skills {
            assert_eq!(over.skills.get(skill), Some(set));
        }
        over.skills
            .retain(|skill, _| default.skills.contains_key(skill));
        assert_eq!(over, default);
        // An empty file changes nothing.
        assert_eq!(overlaid(game, "").unwrap(), default);
    }
    // Plain deserializing fills missing keys from the Fallout 3 default.
    assert_eq!(
        toml::from_str::<Rebalance>("").unwrap(),
        Rebalance::fallout3_default()
    );
}

#[test]
fn a_partial_file_overrides_only_its_keys() {
    let document = r#"
accurate_spread_below = 0.5

[damage]
scale = 0.75

[calibers]
10mm = 150
"7.62mm" = 160

[classes.smg]
burst = 12

[skills]
guns = ["small_guns"]
"#;
    for game in [SourceGame::Fallout3, SourceGame::NewVegas] {
        let base = Rebalance::for_game(game);
        let table = overlaid(game, document).unwrap();
        let mut expected = base.clone();
        expected.accurate_spread_below = Some(0.5);
        expected.damage.scale = 0.75;
        expected.calibers.insert("10mm".into(), 150);
        expected.calibers.insert("7.62mm".into(), 160);
        expected.classes.get_mut(&WeaponClass::Smg).unwrap().burst = 12;
        expected.skills.insert(
            emit::rebalance::SourceSkill::Guns,
            vec![emit::Skill::SmallGuns],
        );
        assert_eq!(table, expected);
        // The game's own values survive: New Vegas keeps its armour rule.
        assert_eq!(table.armor, base.armor);
        assert_eq!(table.damage.spread, base.damage.spread);
        assert_eq!(table.caliber("10 mm"), Some(150));
        assert_eq!(table.caliber("7.62 mm"), Some(160));
        assert_eq!(normalize_key("7.62mm"), "762mm");
    }
    // Nested factor tables overlay key by key too.
    let table = overlaid(SourceGame::NewVegas, "[armor.dr_factors]\nlaser = 0.5\n").unwrap();
    let mut expected = Rebalance::new_vegas_default();
    expected.armor.dr_factors.laser = 0.5;
    assert_eq!(table, expected);
}

#[test]
fn unknown_keys_are_errors() {
    for document in [
        "damgae = 1\n",
        "[damage]\nscael = 0.5\n",
        "[classes.laser_cannon]\nburst = 3\n",
        "[classes.smg]\nbursts = 3\n",
        "[armor.dr_factors]\nlazer = 0.5\n",
        "[skills]\ngunz = [\"small_guns\"]\n",
        "[skills]\nguns = [\"small_gunz\"]\n",
        "game = \"fallout_4\"\n",
    ] {
        assert!(
            overlaid(SourceGame::Fallout3, document).is_err(),
            "{document}"
        );
        assert!(toml::from_str::<Rebalance>(document).is_err(), "{document}");
    }
    let error = overlaid(SourceGame::Fallout3, "[damage]\nscael = 0.5\n").unwrap_err();
    assert!(error.to_string().contains("scael"), "{error}");
}
