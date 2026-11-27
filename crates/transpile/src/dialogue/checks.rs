//! Skill checks (spec 4.7): the Fallout 3 percentage challenge and the
//! threshold gates, and the plugin settings they use.
//!
//! - [`fo3_chance`] is the one place the challenge formula lives
//!   (calibration C-2).
//! - [`read_settings`] reads [`CheckSettings`] from the plugin's game
//!   settings (owner decision 2: the plugin's own values).
//! - [`percent`] and [`stat_terms`] find the checks of an INFO; the
//!   dialogue compiler pairs New Vegas thresholds.

use std::collections::BTreeMap;

use crate::diag::{Category, Diag};
use crate::ir::{
    ActorValue, CheckSettings, CmpOp, FailureRoute, Remember, SimId, Skill, SkillCheck, Stat,
};
use crate::source::{CondValue, SourceCondition, SourceInfo};

/// `INFO` flag `0x0080`: a speech challenge (spec 1.3).
pub const SPEECH_CHALLENGE: u16 = 0x0080;

/// The difficulty names of `DNAM` 1 to 5, as the settings spell them.
pub const DIFFICULTIES: [&str; 5] = ["VeryEasy", "Easy", "Average", "Hard", "VeryHard"];

/// The chance of a Fallout 3 speech challenge, in percent (spec 4.7,
/// calibration C-2): `clamp(round(S * M) - D[difficulty], 0, 100)` with
/// `S` the player's Speech (Fallout 2 skill 14, compared 1:1, owner
/// decision 2), `M` the plugin's `fSpeechChallengeMultiplier` and `D` its
/// `iSpeechChallengeDifficulty<level>`; a chance at or above
/// `100 * fSpeechAutoSuccessThreshold` is 100.
///
/// The spec writes the auto-success test as `S >= threshold`; the
/// plugin's threshold is a fraction (0.9), so it is read as a fraction of
/// the chance. Difficulty is `DNAM`, 1 to 5; other values are clamped.
pub fn fo3_chance(speech: i32, difficulty: u8, settings: &CheckSettings) -> i32 {
    let level = usize::from(difficulty.clamp(1, 5) - 1);
    let raw = (f64::from(speech) * f64::from(settings.speech_multiplier)).round() as i64
        - i64::from(settings.speech_difficulty[level]);
    let chance = raw.clamp(0, 100) as i32;
    if f64::from(chance) >= f64::from(settings.speech_auto_success) * 100.0 {
        100
    } else {
        chance
    }
}

/// Reads the check, hacking and lock-picking settings; a missing setting
/// keeps [`CheckSettings::default`]'s value and writes one diagnostic.
pub fn read_settings(settings: &BTreeMap<String, f64>, diags: &mut Vec<Diag>) -> CheckSettings {
    let mut out = CheckSettings::default();
    let mut missing: Vec<String> = Vec::new();
    let mut read = |name: String, slot: &mut f64| match settings.get(&name) {
        Some(v) => *slot = *v,
        None => missing.push(name),
    };
    let mut m = f64::from(out.speech_multiplier);
    read("fSpeechChallengeMultiplier".into(), &mut m);
    out.speech_multiplier = m as f32;
    let mut a = f64::from(out.speech_auto_success);
    read("fSpeechAutoSuccessThreshold".into(), &mut a);
    out.speech_auto_success = a as f32;
    for (i, level) in DIFFICULTIES.iter().enumerate() {
        let mut rows: [(String, &mut i32); 4] = [
            (
                format!("iSpeechChallengeDifficulty{level}"),
                &mut out.speech_difficulty[i],
            ),
            (
                format!("iXPRewardSpeechChallenge{level}"),
                &mut out.speech_xp[i],
            ),
            (format!("iXPRewardHackComputer{level}"), &mut out.hack_xp[i]),
            (format!("iXPRewardPickLock{level}"), &mut out.lockpick_xp[i]),
        ];
        for (name, slot) in &mut rows {
            let mut v = f64::from(**slot);
            read(std::mem::take(name), &mut v);
            **slot = v.round() as i32;
        }
    }
    // The speech XP of a missing level is the nearest lower level's.
    for i in 1..5 {
        let name = format!("iXPRewardSpeechChallenge{}", DIFFICULTIES[i]);
        if missing.contains(&name) {
            out.speech_xp[i] = out.speech_xp[i - 1];
        }
    }
    if !missing.is_empty() {
        missing.sort();
        diags.push(
            Diag::note(
                Category::MissingSetting,
                format!(
                    "{} game settings missing, defaults used (speech XP: the nearest lower \
                     level's): {}",
                    missing.len(),
                    missing.join(", ")
                ),
            )
            .about(missing.join(",")),
        );
    }
    out
}

/// The Fallout 3 percentage challenge of an INFO: the challenge flag and a
/// difficulty from 1 to 5 (spec 4.7). `failure` is the shared failure
/// topic, when the build has it.
pub fn percent(info: &SourceInfo, failure: Option<&SimId>) -> Option<SkillCheck> {
    if info.flags & SPEECH_CHALLENGE == 0 {
        return None;
    }
    let difficulty = u8::try_from(info.challenge?).ok()?;
    if !(1..=5).contains(&difficulty) {
        return None;
    }
    Some(SkillCheck::Percent {
        skill: Skill::Speech,
        difficulty,
        label: info.label.map(ActorValue),
        failure: match failure {
            Some(topic) => FailureRoute::SharedTopic(topic.clone()),
            None => FailureRoute::NextInfo,
        },
        remember: Remember::Forever,
    })
}

/// A comparison of one of the player's attributes or skills against a
/// literal, from a condition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StatTerm {
    /// The source actor value.
    pub value_of: ActorValue,
    /// What it reads.
    pub stat: Stat,
    /// The comparison.
    pub op: CmpOp,
    /// The literal, rounded.
    pub value: i32,
}

/// Whether a condition runs on the player: run on the target (in dialogue
/// the player), or on the reference `PlayerRef` (local id `0x14` of the
/// first plugin).
fn on_player(c: &SourceCondition, first_plugin: &str) -> bool {
    match c.run_on {
        1 => true,
        2 => c.reference.as_ref().is_some_and(|r| {
            r.as_str()
                .rsplit_once(':')
                .is_some_and(|(p, id)| p == first_plugin && id == "000014")
        }),
        _ => false,
    }
}

/// The player-stat terms of a condition list (spec 4.7, "Threshold"):
/// `GetActorValue`, `GetBaseActorValue` or `GetPermanentActorValue` of an
/// actor value that maps to a [`Stat`], on the player, against a literal.
pub fn stat_terms(conditions: &[SourceCondition], first_plugin: &str) -> Vec<StatTerm> {
    conditions
        .iter()
        .filter(|c| matches!(c.function, 14 | 277 | 495) && on_player(c, first_plugin))
        .filter_map(|c| {
            let CondValue::Literal(v) = c.value else {
                return None;
            };
            let value_of = ActorValue(c.param1);
            Some(StatTerm {
                value_of,
                stat: value_of.stat()?,
                op: CmpOp::parse(&c.comparison)?,
                value: v.round() as i32,
            })
        })
        .collect()
}

/// A threshold check from one stat term.
pub fn threshold(term: StatTerm, label: Option<ActorValue>, partner: Option<SimId>) -> SkillCheck {
    SkillCheck::Threshold {
        stat: term.stat,
        op: term.op,
        value: term.value,
        label: label.or(Some(term.value_of)),
        shown_when_failed: partner.is_some(),
        partner,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fo3_formula_is_pinned() {
        let s = CheckSettings::default();
        // M = 2, D = 10 40 55 70 80, auto success at 90.
        assert_eq!(fo3_chance(0, 1, &s), 0);
        assert_eq!(fo3_chance(30, 1, &s), 50);
        assert_eq!(fo3_chance(50, 2, &s), 60);
        assert_eq!(fo3_chance(50, 5, &s), 20);
        assert_eq!(fo3_chance(100, 5, &s), 100);
        assert_eq!(fo3_chance(49, 1, &s), 88);
        assert_eq!(fo3_chance(50, 1, &s), 100, "90 or more is a sure thing");
        assert_eq!(fo3_chance(20, 9, &s), 0, "difficulty clamps to 5");
    }

    #[test]
    fn missing_settings_are_reported_once() {
        let mut settings = BTreeMap::new();
        settings.insert("fSpeechChallengeMultiplier".to_owned(), 3.0);
        settings.insert("iXPRewardSpeechChallengeVeryEasy".to_owned(), 7.0);
        settings.insert("iXPRewardPickLockHard".to_owned(), 99.0);
        let mut diags = Vec::new();
        let read = read_settings(&settings, &mut diags);
        assert_eq!(read.speech_multiplier, 3.0);
        assert_eq!(read.speech_xp, [7; 5]);
        assert_eq!(read.lockpick_xp[3], 99);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].category, Category::MissingSetting);
    }
}
