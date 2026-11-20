//! Deterministic names: art files, map stems and prototype message text.
//!
//! - Art files are `mf` + a category letter + five decimal digits +
//!   `.frm` (12 bytes, the most an art list keeps): `mfw` walls, `mfs`
//!   scenery, `mfi` items, `mft` tiles, `mfm` misc. The digits run from 0
//!   in the order the files are made; a name the base list already holds
//!   is skipped, so a new entry never aliases a base one.
//! - The exterior map is `<stem>.map`; stub map `n` (interiors sorted by
//!   form key) is the stem's first four bytes, `_` and `n` in three
//!   digits, such as `good_000.map`.

use emit::ArtList;

use crate::BuildError;

/// Hands out unused art file names for one list.
pub(crate) struct ArtNames {
    prefix: &'static str,
    next: u32,
}

impl ArtNames {
    pub(crate) fn new(prefix: &'static str) -> Self {
        Self { prefix, next: 0 }
    }

    /// The next name `list` does not hold.
    pub(crate) fn next(&mut self, list: &ArtList) -> Result<String, BuildError> {
        while self.next <= 99_999 {
            let name = format!("{}{:05}.frm", self.prefix, self.next);
            self.next += 1;
            if list.find(&name).is_none() {
                return Ok(name);
            }
        }
        Err(BuildError::Input(format!(
            "no unused {}NNNNN.frm name is left",
            self.prefix
        )))
    }
}

/// A map stem as `maps.txt` and the map header accept it: 1 to 8 ASCII
/// letters, digits, `_` and `-`; returned lowercase.
pub(crate) fn map_stem(name: &str) -> Result<String, BuildError> {
    let valid = (1..=8).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
    if !valid {
        return Err(BuildError::Input(format!(
            "map name {name:?} is not 1 to 8 letters, digits, `_` or `-`"
        )));
    }
    Ok(name.to_ascii_lowercase())
}

/// The stem of stub map `index`.
pub(crate) fn stub_stem(stem: &str, index: usize) -> Result<String, BuildError> {
    if index > 999 {
        return Err(BuildError::Input(
            "more than 1,000 doors into interiors; stub names have three digits".into(),
        ));
    }
    let head: String = stem.chars().take(4).collect();
    let stub = format!("{head}_{index:03}");
    if stub == stem {
        return Err(BuildError::Input(format!(
            "map name {stem:?} collides with its own stub map names"
        )));
    }
    Ok(stub)
}

/// Text a MSG field can carry: printable ASCII without braces, at most
/// `limit` bytes; anything else becomes `?`.
pub(crate) fn message_text(text: &str, limit: usize) -> String {
    let mut out: String = text
        .chars()
        .map(|c| {
            if (' '..='~').contains(&c) && c != '{' && c != '}' {
                c
            } else {
                '?'
            }
        })
        .take(limit)
        .collect();
    if out.trim().is_empty() {
        out = "unnamed".into();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn art_names_skip_base_entries_and_fit_the_list() {
        let list = ArtList::from_base(2, b"mfs00000.frm\nMFS00002.FRM\n").unwrap();
        let mut names = ArtNames::new("mfs");
        assert_eq!(names.next(&list).unwrap(), "mfs00001.frm");
        assert_eq!(names.next(&list).unwrap(), "mfs00003.frm");
        assert!(emit::lst::check_art_name("mfs99999.frm").is_ok());
    }

    #[test]
    fn stems_and_messages() {
        assert_eq!(map_stem("GoodSp").unwrap(), "goodsp");
        assert!(map_stem("toolongname").is_err());
        assert!(map_stem("a.b").is_err());
        assert_eq!(stub_stem("goodsp", 7).unwrap(), "good_007");
        assert!(stub_stem("good_000", 0).is_err());
        assert_eq!(message_text("a{b}\nc", 10), "a?b??c");
        assert_eq!(message_text("", 10), "unnamed");
        assert_eq!(message_text("abcdef", 3), "abc");
    }
}
