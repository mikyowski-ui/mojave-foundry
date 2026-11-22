//! Model path patterns.
//!
//! A model path is normalised (lowercase, `\` separators) and read as
//! segments split at `\`: its directories, then its file name. A pattern
//! list holds patterns and exclusions; the list matches a path when one of
//! its patterns has an occurrence that no exclusion covers. The forms:
//!
//! - `name\` (ends with a separator): a **directory** named exactly `name`.
//!   `trees\` matches `meshes\landscape\trees\joshua.nif`, not
//!   `meshes\nv_trees\…` and never a file name.
//! - `\name` (starts with a separator): a **segment**, directory or file,
//!   that begins with `name`. `\fx` matches `…\fxdust01.nif`.
//! - `*name`: `name` **anywhere**, the substring rule of the first pass,
//!   for names that are glued onto anything (`wallmarker`, `barrelcactus`).
//! - `name` (plain): `name` at a **word start**. A word starts at a segment
//!   start, after any character that is not a letter (`_`, `-`, `.`, a
//!   digit, a space), and right after a leading name prefix of the segment
//!   (`SlicerConfig::name_prefixes`: `nv` and `scol` by default, so
//!   `scolroadstraight01.nif` is a road and `nvrockpile.nif` a rock).
//!   `rock` matches `nv_rocks\`, `tst_rockcanyon06.nif` and `rock01.nif`,
//!   not `shamrock.nif`; it would still match `rocketcrane.nif`, which is
//!   what exclusions are for.
//! - `!text`: an **exclusion**. `text` is found anywhere in the path; an
//!   occurrence of a pattern of the same list that lies inside an
//!   occurrence of `text` does not count. `!rocket` keeps `rock` from
//!   matching `rocketcrane.nif` while `rocket\rockpile01.nif` still matches
//!   through its file name; `!bouldercity` keeps every building under a
//!   `bouldercity\` folder from being a boulder, while a
//!   `bouldercity\boulder01.nif` still is one.
//!
//! An empty pattern (or a lone `\`, `*` or `!`) matches nothing.

/// Where a pattern may occur.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Form {
    Directory,
    SegmentStart,
    Anywhere,
    WordStart,
}

/// The text a pattern looks for and where.
fn parse(pattern: &str) -> Option<(Form, &str)> {
    if let Some(rest) = pattern.strip_prefix('*') {
        Some((Form::Anywhere, rest))
    } else if pattern.len() > 1 && pattern.ends_with('\\') {
        // The separator is part of the text: the directory must end there.
        Some((Form::Directory, pattern))
    } else if let Some(rest) = pattern.strip_prefix('\\') {
        Some((Form::SegmentStart, rest))
    } else {
        Some((Form::WordStart, pattern))
    }
    .filter(|(_, text)| !text.is_empty() && *text != "\\")
}

/// Whether byte `i` of `path` starts a segment.
fn segment_start(path: &[u8], i: usize) -> bool {
    i == 0 || path[i - 1] == b'\\'
}

/// Whether byte `i` of `path` starts a word (see the module documentation).
fn word_start(path: &[u8], i: usize, prefixes: &[String]) -> bool {
    if i == 0 || !path[i - 1].is_ascii_alphabetic() {
        return true;
    }
    // Right after a leading prefix of this segment.
    let start = path[..i]
        .iter()
        .rposition(|&b| b == b'\\')
        .map_or(0, |p| p + 1);
    prefixes
        .iter()
        .any(|p| !p.is_empty() && i - start == p.len() && path[start..i] == *p.as_bytes())
}

/// The byte ranges where `text` occurs in `path`.
fn occurrences<'a>(path: &'a str, text: &'a str) -> impl Iterator<Item = usize> + 'a {
    path.match_indices(text).map(|(i, _)| i)
}

/// The first pattern of `list` that matches the normalised `model`, by the
/// rules of this module.
pub(crate) fn first_match(list: &[String], model: &str, prefixes: &[String]) -> Option<String> {
    let bytes = model.as_bytes();
    let excluded: Vec<(usize, usize)> = list
        .iter()
        .filter_map(|p| p.strip_prefix('!'))
        .filter(|text| !text.is_empty())
        .flat_map(|text| occurrences(model, text).map(move |i| (i, i + text.len())))
        .collect();
    let covered = |start: usize, end: usize| excluded.iter().any(|&(a, b)| a <= start && end <= b);
    list.iter()
        .filter(|p| !p.starts_with('!'))
        .find(|pattern| {
            let Some((form, text)) = parse(pattern) else {
                return false;
            };
            occurrences(model, text).any(|i| {
                let placed = match form {
                    Form::Anywhere => true,
                    Form::Directory | Form::SegmentStart => segment_start(bytes, i),
                    Form::WordStart => word_start(bytes, i, prefixes),
                };
                placed && !covered(i, i + text.len())
            })
        })
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::first_match;

    fn list(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_owned()).collect()
    }

    fn prefixes() -> Vec<String> {
        list(&["nv", "scol"])
    }

    fn hit(patterns: &[&str], model: &str) -> Option<String> {
        first_match(&list(patterns), model, &prefixes())
    }

    #[test]
    fn directories_match_whole_segments_only() {
        assert!(hit(&["trees\\"], "meshes\\landscape\\trees\\a.nif").is_some());
        assert!(hit(&["trees\\"], "meshes\\nv_trees\\a.nif").is_none());
        assert!(hit(&["trees\\"], "meshes\\landscape\\trees.nif").is_none());
        assert!(hit(&["trees\\"], "trees\\a.nif").is_some());
    }

    #[test]
    fn segment_starts_and_anywhere() {
        assert!(hit(&["\\fx"], "meshes\\effects\\fxdust.nif").is_some());
        assert!(hit(&["\\fx"], "meshes\\effects\\nvfxdust.nif").is_none());
        assert!(hit(&["*marker"], "meshes\\test\\testwallmarker.nif").is_some());
        assert!(hit(&["marker"], "meshes\\test\\testwallmarker.nif").is_none());
    }

    #[test]
    fn words_start_after_separators_digits_and_prefixes() {
        for model in [
            "meshes\\landscape\\nv_rocks\\x.nif",
            "meshes\\a\\tst_rockcanyon06.nif",
            "meshes\\a\\rock01.nif",
            "meshes\\a\\nvrockpile.nif",
            "meshes\\a\\big-rock.nif",
            "meshes\\a\\dlc03rock.nif",
        ] {
            assert!(hit(&["rock"], model).is_some(), "{model}");
        }
        assert!(hit(&["rock"], "meshes\\a\\shamrock.nif").is_none());
        assert!(hit(&["road"], "meshes\\scol\\scolroadstr01.nif").is_some());
        assert!(hit(&["road"], "meshes\\a\\railroad.nif").is_none());
        // A prefix counts only at the start of its segment.
        assert!(hit(&["road"], "meshes\\a\\xscolroad.nif").is_none());
    }

    #[test]
    fn exclusions_cover_only_their_own_text() {
        let relief = ["rock", "boulder", "!rocket", "!bouldercity"];
        assert!(hit(&relief, "meshes\\clutter\\rocketcrane01.nif").is_none());
        assert!(hit(&relief, "meshes\\a\\rocket\\rockpile01.nif").is_some());
        assert!(hit(&relief, "meshes\\dungeons\\bouldercity\\bcruin01.nif").is_none());
        assert!(hit(&relief, "meshes\\dungeons\\bouldercity\\boulder01.nif").is_some());
        assert!(hit(&["*bush", "!ambush"], "meshes\\a\\ambushsite.nif").is_none());
        assert!(hit(&["*bush", "!ambush"], "meshes\\a\\creosotebush.nif").is_some());
    }

    #[test]
    fn degenerate_patterns_match_nothing() {
        for p in ["", "\\", "*", "!", "!\\"] {
            assert!(hit(&[p], "meshes\\a\\b.nif").is_none(), "{p:?}");
        }
    }

    #[test]
    fn the_first_pattern_in_list_order_is_named() {
        assert_eq!(
            hit(&["cliff", "rock"], "meshes\\rocks\\cliffs\\cliff01.nif"),
            Some("cliff".to_owned())
        );
    }
}
