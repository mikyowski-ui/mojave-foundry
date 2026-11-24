//! The rebalance tables of a run: `emit::rebalance::Rebalance` for the
//! game (weapons, armour, ammunition, aid, fighters) plus the converter's
//! own tables under `[convert]` (how a weapon record is classified, which
//! side a person is on, how loot lists are drawn).
//!
//! Where they come from, first found:
//!
//! 1. `--rebalance <file>`;
//! 2. `rebalance/<game>.toml` (`fo3.toml`, `fnv.toml`) in the slice file's
//!    directory or any directory above it (so `slices/fo3/x.toml` finds the
//!    repository's `rebalance/fo3.toml`); for a built-in slice, in the
//!    working directory or above;
//! 3. the built-in defaults ([`Rebalance::for_game`] and
//!    [`ConvertTables::for_game`]).
//!
//! A file is an overlay: every key it gives replaces that value of the
//! game's default, every key it leaves out keeps the default, an unknown
//! key is an error. The repository's two files are full copies of the
//! defaults (a unit test keeps them equal), with a header saying how to
//! tune. The effective tables, not the file bytes, are hashed into the map
//! stage's key when loot is on ([`LoadedRebalance::digest`]), so a file
//! that equals the built-in default changes nothing.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use emit::rebalance::{Rebalance, SourceGame, WeaponClass};
use mir::GameVariant;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// The top-level key of the converter's own tables in a rebalance file.
pub const CONVERT_KEY: &str = "convert";

/// The repository's rebalance files, as shipped (for the tests and for
/// `--rebalance` users who want a starting point).
pub const FALLOUT3_FILE: &str = include_str!("../../../rebalance/fo3.toml");
pub const NEW_VEGAS_FILE: &str = include_str!("../../../rebalance/fnv.toml");

/// `emit`'s game for a `mir` game.
pub fn source_game(game: GameVariant) -> SourceGame {
    match game {
        GameVariant::Fallout3 => SourceGame::Fallout3,
        GameVariant::FalloutNewVegas => SourceGame::NewVegas,
    }
}

/// The file name a game's rebalance file has under `rebalance/`.
pub fn file_name(game: GameVariant) -> &'static str {
    match game {
        GameVariant::Fallout3 => "fo3.toml",
        GameVariant::FalloutNewVegas => "fnv.toml",
    }
}

/// Which side a placed person is on, and the AI packet and team each side
/// gets (`[convert.hostility]`).
///
/// The rule, in order:
///
/// 1. an editor id (of the actor or of one of its factions) matching
///    `friendly_patterns` is friendly;
/// 2. with the actor's AI data (`AIDT`, after its templates): aggression
///    at or above `hostile_aggression` (2, very aggressive: attacks
///    neutrals too) is hostile unless one of its factions and the
///    player's faction are friends or allies; aggression at or above
///    `enemy_aggression` (1, aggressive: attacks enemies) is hostile when
///    one of its factions and the player's faction are enemies (either
///    side's relation, `XNAM`); anything else is friendly;
/// 3. without AI data: `hostile_patterns` on the editor ids, else
///    friendly (reported: the proper data was not found).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Hostility {
    /// The player's faction, by editor id.
    pub player_faction: String,
    pub hostile_aggression: u8,
    pub enemy_aggression: u8,
    /// Editor-id patterns (`*` matches anything; case does not matter)
    /// that make an actor hostile when it has no AI data.
    pub hostile_patterns: Vec<String>,
    /// Editor-id patterns that always make an actor friendly.
    pub friendly_patterns: Vec<String>,
    /// The team every hostile person shares (above 0, the player's, and
    /// apart from the friendly team).
    pub hostile_team: i32,
    /// The hostile AI packet, by its name in the base `data/ai.txt`
    /// (case does not matter) ...
    pub hostile_packet: String,
    /// ... or this number when the base table has no such name.
    pub hostile_packet_number: i32,
    /// The friendly team; 0 or less takes the builder's default (99).
    pub friendly_team: i32,
    /// The friendly packet by name; empty takes the builder's rule
    /// (`map_builder::critters::choose_ai_packet`: the most careful packet
    /// of the base table).
    pub friendly_packet: String,
}

/// One refinement of a weapon's class (`[[convert.weapons.rules]]`): when
/// every given condition holds (an empty list is no condition), the class
/// and the damage kind it names replace those of the animation type.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct WeaponRule {
    /// Editor-id or name patterns, any of which matches.
    pub patterns: Vec<String>,
    /// Animation types (the weapon record's `DNAM`), any of which matches.
    pub animations: Vec<u32>,
    /// Skills (actor value numbers), any of which matches.
    pub skills: Vec<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub class: Option<WeaponClass>,
    /// `ballistic`, `laser`, `plasma`, `fire`, `electric`, `pulse` or
    /// `explosive`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub damage: Option<String>,
}

/// How a weapon record becomes a [`WeaponClass`] (`[convert.weapons]`): by
/// its animation type, then the first matching rule of each kind (class,
/// damage) wins.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WeaponTable {
    /// Animation type (as a string key, `"0"` to `"13"`) to class.
    pub by_animation: BTreeMap<String, WeaponClass>,
    /// The skill (actor value) of energy weapons: a ballistic animation
    /// type with this skill takes the energy class of its kind.
    pub energy_skill: u32,
    pub rules: Vec<WeaponRule>,
}

/// How containers and inventories are filled (`[convert.loot]`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LootRules {
    /// The player level a leveled item list is drawn at: its entries at or
    /// below it may be picked (the lowest-level ones when none is).
    pub level: i32,
    /// At most this many stacks a person carries besides hands and armour;
    /// the rest are dropped and reported.
    pub max_carried: usize,
    /// At most this many stacks in one container.
    pub max_contents: usize,
}

/// The converter's own tables.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ConvertTables {
    pub hostility: Hostility,
    pub weapons: WeaponTable,
    pub loot: LootRules,
}

impl Default for ConvertTables {
    fn default() -> Self {
        Self::for_game(GameVariant::Fallout3)
    }
}
impl Default for Hostility {
    fn default() -> Self {
        Self::for_game(GameVariant::Fallout3)
    }
}
impl Default for WeaponTable {
    fn default() -> Self {
        Self::for_game(GameVariant::Fallout3)
    }
}
impl Default for LootRules {
    fn default() -> Self {
        LootRules {
            level: 1,
            max_carried: 12,
            max_contents: 24,
        }
    }
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| (*s).to_owned()).collect()
}

impl Hostility {
    pub fn for_game(game: GameVariant) -> Self {
        let hostile_patterns = match game {
            GameVariant::Fallout3 => strings(&[
                "*raider*",
                "*talon*",
                "*supermutant*",
                "*slaver*",
                "*feral*",
            ]),
            GameVariant::FalloutNewVegas => {
                strings(&["*fiend*", "*viper*", "*jackal*", "*feral*", "*raider*"])
            }
        };
        Hostility {
            player_faction: "PlayerFaction".to_owned(),
            hostile_aggression: 2,
            enemy_aggression: 1,
            hostile_patterns,
            friendly_patterns: Vec::new(),
            hostile_team: 66,
            hostile_packet: "Merc Raider".to_owned(),
            hostile_packet_number: 136,
            friendly_team: 0,
            friendly_packet: String::new(),
        }
    }
}

impl WeaponTable {
    /// The same in both games: their weapon records share the animation
    /// types (0 hand to hand, 1 one-hand melee, 2 two-hand melee, 3 and 4
    /// ballistic and energy pistols, 5 rifles, 6 automatic rifles, 7 energy
    /// rifles, 8 two-hand handles (miniguns, flamers, gatlings), 9
    /// launchers, 10 grenades, 11 and 12 mines, 13 thrown) and the energy
    /// weapons skill (actor value 34).
    pub fn for_game(_game: GameVariant) -> Self {
        use WeaponClass as C;
        let by_animation: BTreeMap<String, WeaponClass> = [
            (0, C::Unarmed),
            (1, C::Blunt),
            (2, C::HeavyMelee),
            (3, C::Pistol),
            (4, C::EnergyPistol),
            (5, C::Rifle),
            (6, C::AssaultRifle),
            (7, C::EnergyRifle),
            (8, C::Minigun),
            (9, C::Launcher),
            (10, C::Grenade),
            (11, C::Grenade),
            (12, C::Grenade),
            (13, C::Thrown),
        ]
        .into_iter()
        .map(|(n, c)| (n.to_string(), c))
        .collect();
        let rule =
            |patterns: &[&str], animations: &[u32], class: Option<C>, damage: Option<&str>| {
                WeaponRule {
                    patterns: strings(patterns),
                    animations: animations.to_vec(),
                    skills: Vec::new(),
                    class,
                    damage: damage.map(str::to_owned),
                }
            };
        WeaponTable {
            by_animation,
            energy_skill: 34,
            rules: vec![
                rule(
                    &[
                        "*knife*",
                        "*blade*",
                        "*sword*",
                        "*machete*",
                        "*ripper*",
                        "*cleaver*",
                        "*shishkebab*",
                        "*razor*",
                        "*katana*",
                        "*axe*",
                        "*scalpel*",
                    ],
                    &[1, 2],
                    Some(C::Blade),
                    None,
                ),
                rule(&["*spear*"], &[1, 2], Some(C::Spear), None),
                rule(
                    &["*shotgun*", "*combatshotgun*"],
                    &[5, 6],
                    Some(C::Shotgun),
                    None,
                ),
                rule(&["*smg*", "*submachine*"], &[5, 6], Some(C::Smg), None),
                rule(
                    &["*flamer*", "*flame*", "*incinerator*"],
                    &[8],
                    Some(C::Flamer),
                    Some("fire"),
                ),
                rule(&["*plasma*"], &[], None, Some("plasma")),
                rule(&["*tesla*", "*arcwelder*"], &[], None, Some("electric")),
                rule(&["*pulse*"], &[], None, Some("pulse")),
                rule(&[], &[9, 10, 11, 12], None, Some("explosive")),
            ],
        }
    }
}

impl ConvertTables {
    pub fn for_game(game: GameVariant) -> Self {
        ConvertTables {
            hostility: Hostility::for_game(game),
            weapons: WeaponTable::for_game(game),
            loot: LootRules::default(),
        }
    }
}

/// The tables a run uses, and where they came from.
#[derive(Debug, Clone, PartialEq)]
pub struct LoadedRebalance {
    pub game: GameVariant,
    pub table: Rebalance,
    pub convert: ConvertTables,
    /// `built-in`, or the file's path as given.
    pub source: String,
}

impl LoadedRebalance {
    /// The built-in defaults of a game.
    pub fn built_in(game: GameVariant) -> Self {
        LoadedRebalance {
            game,
            table: Rebalance::for_game(source_game(game)),
            convert: ConvertTables::for_game(game),
            source: "built-in".to_owned(),
        }
    }

    /// The defaults with a file's text laid over them.
    pub fn from_text(game: GameVariant, text: &str, source: &str) -> Result<Self, String> {
        let mut document: toml::Table =
            toml::from_str(text).map_err(|e| format!("{source}: {e}"))?;
        let mut loaded = Self::built_in(game);
        loaded.source = source.to_owned();
        if let Some(convert) = document.remove(CONVERT_KEY) {
            // Overlay the converter's tables key by key: a table it names
            // replaces only the keys it gives.
            let mut base = toml::Value::try_from(&loaded.convert).map_err(|e| e.to_string())?;
            merge(&mut base, convert);
            loaded.convert = base
                .try_into()
                .map_err(|e| format!("{source}: [{CONVERT_KEY}]: {e}"))?;
        }
        let rest = toml::to_string(&document).map_err(|e| e.to_string())?;
        loaded
            .table
            .overlay(toml::Deserializer::parse(&rest).map_err(|e| format!("{source}: {e}"))?)
            .map_err(|e| format!("{source}: {e}"))?;
        if loaded.table.game != source_game(game) {
            return Err(format!(
                "{source}: game = {:?}, but this run converts {}",
                loaded.table.game,
                game.display_name()
            ));
        }
        loaded.validate().map_err(|e| format!("{source}: {e}"))?;
        Ok(loaded)
    }

    /// Reads and overlays a file.
    pub fn from_file(game: GameVariant, path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        Self::from_text(game, &text, &path.display().to_string())
    }

    fn validate(&self) -> Result<(), String> {
        let h = &self.convert.hostility;
        if h.hostile_team <= 0 {
            return Err(
                "convert.hostility.hostile_team must be above 0 (0 is the player's)".into(),
            );
        }
        if h.friendly_team > 0 && h.friendly_team == h.hostile_team {
            return Err("convert.hostility: the hostile and friendly teams are the same".into());
        }
        if h.hostile_packet_number < 0 {
            return Err("convert.hostility.hostile_packet_number is negative".into());
        }
        for key in self.convert.weapons.by_animation.keys() {
            if key.parse::<u32>().is_err() {
                return Err(format!(
                    "convert.weapons.by_animation: {key:?} is not an animation type number"
                ));
            }
        }
        for rule in &self.convert.weapons.rules {
            if let Some(damage) = &rule.damage
                && crate::catalogue::damage_kind(damage).is_none()
            {
                return Err(format!(
                    "convert.weapons.rules: damage {damage:?}: expected ballistic, laser, \
                     plasma, fire, electric, pulse or explosive"
                ));
            }
        }
        if self.convert.loot.max_carried == 0 || self.convert.loot.max_contents == 0 {
            return Err("convert.loot: max_carried and max_contents must be at least 1".into());
        }
        Ok(())
    }

    /// A content hash of the effective tables (not of the file): equal
    /// tables from any source give the same digest.
    pub fn digest(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(format!("{:?}", self.table).as_bytes());
        hasher.update(format!("{:?}", self.convert).as_bytes());
        hasher
            .finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }
}

/// `overlay` into `base`: tables key by key, anything else replaced.
fn merge(base: &mut toml::Value, overlay: toml::Value) {
    match (base, overlay) {
        (toml::Value::Table(base), toml::Value::Table(overlay)) => {
            for (key, value) in overlay {
                match base.get_mut(&key) {
                    Some(slot) => merge(slot, value),
                    None => {
                        base.insert(key, value);
                    }
                }
            }
        }
        (slot, value) => *slot = value,
    }
}

/// The default file for a run: `rebalance/<game>.toml` in `start` or any
/// directory above it.
pub fn find_default(game: GameVariant, start: &Path) -> Option<PathBuf> {
    let start = if start.is_absolute() {
        start.to_path_buf()
    } else {
        std::env::current_dir().ok()?.join(start)
    };
    start
        .ancestors()
        .map(|dir| dir.join("rebalance").join(file_name(game)))
        .find(|path| path.is_file())
}

/// Whether an editor id or name matches a pattern: `*` matches any run of
/// characters, case does not matter, and a pattern without `*` must match
/// whole.
pub fn glob(pattern: &str, text: &str) -> bool {
    let pattern = pattern.to_ascii_lowercase();
    let text = text.to_ascii_lowercase();
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        return pattern == text;
    }
    let mut rest = text.as_str();
    for (i, part) in parts.iter().enumerate() {
        if part.is_empty() {
            continue;
        }
        if i == 0 {
            match rest.strip_prefix(part) {
                Some(r) => rest = r,
                None => return false,
            }
        } else if i == parts.len() - 1 {
            return rest.ends_with(part);
        } else {
            match rest.find(part) {
                Some(at) => rest = &rest[at + part.len()..],
                None => return false,
            }
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    const GAMES: [GameVariant; 2] = [GameVariant::Fallout3, GameVariant::FalloutNewVegas];

    /// The full default of a game as a rebalance file, after `header`.
    fn file_text(game: GameVariant, header: &str) -> String {
        let loaded = LoadedRebalance::built_in(game);
        let emit_text = toml::to_string(&loaded.table).expect("serialises");
        let mut convert = toml::Table::new();
        convert.insert(
            CONVERT_KEY.to_owned(),
            toml::Value::try_from(&loaded.convert).expect("serialises"),
        );
        format!(
            "{header}{emit_text}\n# The converter's own tables: weapon classes, sides, loot draws.\n{}",
            toml::to_string(&convert).expect("serialises")
        )
    }

    #[test]
    fn the_repository_files_are_the_built_in_defaults() {
        // `MOJAVE_BLESS=1` rewrites them from the defaults, keeping the
        // header comment block of each.
        for (game, shipped) in [
            (GameVariant::Fallout3, FALLOUT3_FILE),
            (GameVariant::FalloutNewVegas, NEW_VEGAS_FILE),
        ] {
            if std::env::var_os("MOJAVE_BLESS").is_some() {
                let header: String = shipped
                    .lines()
                    .take_while(|l| l.starts_with('#') || l.is_empty())
                    .map(|l| format!("{l}\n"))
                    .collect();
                let path = Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../rebalance")
                    .join(file_name(game));
                std::fs::write(path, file_text(game, &header)).expect("write");
                continue;
            }
            let loaded = LoadedRebalance::from_text(game, shipped, "shipped").expect("parses");
            let built_in = LoadedRebalance::built_in(game);
            assert_eq!(loaded.table, built_in.table, "{game:?}");
            assert_eq!(loaded.convert, built_in.convert, "{game:?}");
            assert_eq!(loaded.digest(), built_in.digest());
            assert!(shipped.starts_with("# "), "a header says how to tune");
        }
    }

    #[test]
    fn a_partial_file_overlays_key_by_key() {
        for game in GAMES {
            let text = "[damage]\nscale = 2.0\n\n[convert.hostility]\nhostile_team = 7\n\n\
                        [convert.loot]\nlevel = 5\n";
            let loaded = LoadedRebalance::from_text(game, text, "t").expect("parses");
            let base = LoadedRebalance::built_in(game);
            assert_eq!(loaded.table.damage.scale, 2.0);
            assert_eq!(loaded.table.damage.spread, base.table.damage.spread);
            assert_eq!(loaded.table.armor, base.table.armor);
            assert_eq!(loaded.convert.hostility.hostile_team, 7);
            assert_eq!(
                loaded.convert.hostility.hostile_patterns,
                base.convert.hostility.hostile_patterns
            );
            assert_eq!(loaded.convert.loot.level, 5);
            assert_eq!(loaded.convert.weapons, base.convert.weapons);
            assert_ne!(loaded.digest(), base.digest());
        }
    }

    #[test]
    fn bad_files_are_refused() {
        let game = GameVariant::Fallout3;
        for (text, expected) in [
            ("colour = 1\n", "colour"),
            ("[convert.hostility]\nhostile_team = 0\n", "hostile_team"),
            ("[convert.wobble]\nx = 1\n", "wobble"),
            ("game = \"new_vegas\"\n", "Fallout 3"),
            (
                "[[convert.weapons.rules]]\npatterns = [\"*x*\"]\ndamage = \"soggy\"\n",
                "soggy",
            ),
        ] {
            let error = LoadedRebalance::from_text(game, text, "t").expect_err(text);
            assert!(error.contains(expected), "{text}: {error}");
        }
    }

    #[test]
    fn globs_match_by_star() {
        assert!(glob("*raider*", "LvlRaiderGun"));
        assert!(glob("raider*", "RaiderFaction"));
        assert!(!glob("raider*", "LvlRaider"));
        assert!(glob("*faction", "RaiderFaction"));
        assert!(glob("PlayerFaction", "playerfaction"));
        assert!(!glob("Player", "PlayerFaction"));
        assert!(glob("*a*b*c", "xxaYbZc"));
        assert!(!glob("*a*b*c", "xxaYcZb"));
    }

    #[test]
    fn the_default_file_is_found_above_the_slice() {
        let dir = tempfile::tempdir().expect("temp dir");
        let slices = dir.path().join("slices").join("fo3");
        std::fs::create_dir_all(&slices).expect("dirs");
        assert_eq!(find_default(GameVariant::Fallout3, &slices), None);
        let file = dir.path().join("rebalance").join("fo3.toml");
        std::fs::create_dir_all(file.parent().unwrap()).expect("dirs");
        std::fs::write(&file, "").expect("write");
        assert_eq!(find_default(GameVariant::Fallout3, &slices), Some(file));
        assert_eq!(find_default(GameVariant::FalloutNewVegas, &slices), None);
    }
}
