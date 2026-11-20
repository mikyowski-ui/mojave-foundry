//! LST index files, as `vault_formats::lst::LstTable::parse` reads them for
//! each `LstKind` (SPEC-001 §2.4 and §3.6).
//!
//! A position is one bounded line read, and blank lines keep their
//! positions, so the writer emits exactly one LF-terminated line per
//! [`LstLine`] and refuses any line that would be read as more or fewer
//! positions, or with a different name or metadata.

use crate::EmitError;
use vault_formats::{LstKind, VfsPath};

/// An art list name is kept to this many bytes.
pub const ART_NAME_BYTES: usize = 12;
/// Bytes per bounded read, including the LF, for each list kind.
pub const ART_LINE_BYTES: usize = 199;
pub const SCRIPT_LINE_BYTES: usize = 259;
pub const PROTOTYPE_LINE_BYTES: usize = 255;
/// Art lines carry at most three comma-separated numbers: a critter
/// alias, or a head's good, neutral and bad fidget counts.
pub const ART_NUMBERS: usize = 3;

/// One line of a list, by the kind of list it belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LstLine {
    /// `art/<category>/<category>.lst`: a file name (or a critter or head
    /// stem) of 1 to 12 bytes, then up to three numbers written
    /// `name,a,b,c`. The first is read as the critter alias
    /// (`LstEntry::alias`), all of them as head counts
    /// (`LstEntry::head_columns`).
    Art { name: String, numbers: Vec<i32> },
    /// `scripts/scripts.lst`: `stem.int`, an optional `; comment`, and an
    /// optional `# local_vars=N` annotation.
    Script {
        stem: String,
        local_vars: Option<i32>,
        comment: Option<String>,
    },
    /// `proto/<category>/<category>.lst`: a file name, and an optional
    /// comment after one space.
    Prototype {
        file: String,
        comment: Option<String>,
    },
    /// A line of any other list, kept byte for byte.
    Raw(Vec<u8>),
    /// An empty line: it keeps a position in any kind of list.
    Blank,
}

impl LstLine {
    /// An art line with no numbers.
    pub fn art(name: impl Into<String>) -> Self {
        Self::Art {
            name: name.into(),
            numbers: Vec::new(),
        }
    }
    /// A prototype line with no comment.
    pub fn prototype(file: impl Into<String>) -> Self {
        Self::Prototype {
            file: file.into(),
            comment: None,
        }
    }
    /// The list kind this line belongs to; `None` for a blank line.
    pub fn kind(&self) -> Option<LstKind> {
        match self {
            Self::Art { .. } => Some(LstKind::Art),
            Self::Script { .. } => Some(LstKind::Script),
            Self::Prototype { .. } => Some(LstKind::Prototype),
            Self::Raw(_) => Some(LstKind::Raw),
            Self::Blank => None,
        }
    }
}

/// Bytes that end or corrupt a line in every list kind.
fn line_breaking(byte: u8) -> bool {
    matches!(byte, b'\n' | b'\r' | 0)
}

/// A file name the engine can open: a canonical single VFS component.
fn file_name(name: &str) -> Result<(), String> {
    if name.contains(['/', '\\']) {
        return Err(format!("{name:?} names a folder"));
    }
    VfsPath::new(name)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

/// SPEC-001 §2.4: an art name ends at the first space, comma, semicolon,
/// TAB or LF and is kept to 12 bytes, so a name the engine reads back
/// unchanged has 1 to 12 bytes and none of those delimiters.
pub fn check_art_name(name: &str) -> Result<(), String> {
    if name.is_empty() || name.len() > ART_NAME_BYTES {
        return Err(format!(
            "art name {name:?} has {} bytes; the list keeps 1 to {ART_NAME_BYTES}",
            name.len()
        ));
    }
    if name
        .bytes()
        .any(|byte| b" ,;\t".contains(&byte) || line_breaking(byte))
    {
        return Err(format!(
            "art name {name:?} holds a space, comma, semicolon, TAB or line break"
        ));
    }
    file_name(name)
}

fn text(field: &str, value: &str, forbidden: &[u8]) -> Result<(), String> {
    if !value.is_ascii() {
        return Err(format!("{field} {value:?} is not ASCII"));
    }
    if value
        .bytes()
        .any(|byte| line_breaking(byte) || forbidden.contains(&byte))
    {
        return Err(format!(
            "{field} {value:?} holds a line break, NUL or one of {:?}",
            String::from_utf8_lossy(forbidden)
        ));
    }
    Ok(())
}

fn bounded(mut line: Vec<u8>, limit: usize) -> Result<Vec<u8>, String> {
    line.push(b'\n');
    if line.len() > limit {
        return Err(format!(
            "{} bytes with its LF; one read of this list holds {limit}",
            line.len()
        ));
    }
    Ok(line)
}

/// One line's bytes, LF included.
pub(crate) fn encode_line(line: &LstLine) -> Result<Vec<u8>, String> {
    match line {
        LstLine::Blank => Ok(b"\n".to_vec()),
        LstLine::Art { name, numbers } => {
            check_art_name(name)?;
            if numbers.len() > ART_NUMBERS {
                return Err(format!(
                    "{} numbers; an art line carries at most {ART_NUMBERS}",
                    numbers.len()
                ));
            }
            let mut out = name.clone().into_bytes();
            for number in numbers {
                out.extend_from_slice(format!(",{number}").as_bytes());
            }
            bounded(out, ART_LINE_BYTES)
        }
        LstLine::Script {
            stem,
            local_vars,
            comment,
        } => {
            let valid = !stem.is_empty()
                && stem
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'));
            if !valid {
                return Err(format!(
                    "script stem {stem:?} is not letters, digits, `_` and `-`"
                ));
            }
            file_name(&format!("{stem}.int"))?;
            let mut out = format!("{stem}.int").into_bytes();
            if let Some(comment) = comment {
                // `#` would start the annotation the engine searches.
                text("comment", comment, b"#")?;
                out.extend_from_slice(format!(" ; {comment}").as_bytes());
            }
            if let Some(count) = local_vars {
                if *count < 0 {
                    return Err(format!("local_vars {count} is negative"));
                }
                out.extend_from_slice(format!(" # local_vars={count}").as_bytes());
            }
            bounded(out, SCRIPT_LINE_BYTES)
        }
        LstLine::Prototype { file, comment } => {
            if file.is_empty() || file.bytes().any(|byte| byte == b' ' || line_breaking(byte)) {
                return Err(format!(
                    "prototype file {file:?} is empty or holds a space or line break"
                ));
            }
            file_name(file)?;
            let mut out = file.clone().into_bytes();
            if let Some(comment) = comment {
                text("comment", comment, b"")?;
                out.push(b' ');
                out.extend_from_slice(comment.as_bytes());
            }
            bounded(out, PROTOTYPE_LINE_BYTES)
        }
        LstLine::Raw(bytes) => {
            if bytes.iter().any(|byte| line_breaking(*byte)) {
                return Err("a raw line holds a line break or NUL".into());
            }
            Ok([bytes.as_slice(), b"\n"].concat())
        }
    }
}

/// `base` ready for appended lines: an LF ends an unterminated last line,
/// unless that line already filled a whole read (then the LF would be a
/// position of its own and the next line starts a fresh read anyway).
/// `entries` is the base's position count.
pub(crate) fn terminate_list(
    base: &[u8],
    kind: LstKind,
    entries: usize,
) -> Result<Vec<u8>, EmitError> {
    let mut bytes = base.to_vec();
    if !bytes.is_empty() && !bytes.ends_with(b"\n") {
        bytes.push(b'\n');
        if vault_formats::LstTable::parse(&bytes, kind)?
            .entries()
            .len()
            != entries
        {
            bytes.pop();
        }
    }
    Ok(bytes)
}

/// Serialize `lines`, one LF-terminated line each, as `LstTable::parse`
/// reads them. All lines but blank ones must be of one kind.
pub fn write_lst(lines: &[LstLine]) -> Result<Vec<u8>, EmitError> {
    let mut kind = None;
    let mut out = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        let error = |message: String| EmitError::Lst {
            line: index + 1,
            message,
        };
        if let Some(this) = line.kind() {
            match kind {
                None => kind = Some(this),
                Some(first) if first != this => {
                    return Err(error(format!(
                        "a {this:?} line in a {first:?} list; one list has one kind"
                    )));
                }
                Some(_) => {}
            }
        }
        out.extend_from_slice(&encode_line(line).map_err(error)?);
    }
    Ok(out)
}
