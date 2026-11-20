//! GAM variable initializers, as `vault_formats::gam::GamFile::parse`
//! reads them (SPEC-006 §2.1): a section marker line, then one
//! `name = value;` line per variable. Only the order of the values
//! matters to the engine; the names are documentation.

use crate::EmitError;
use vault_formats::GamSection;

/// Bytes per bounded read, including the LF.
pub const GAM_LINE_BYTES: usize = 257;
/// The most variables this writer emits (the parser's limit is one
/// million).
pub const GAM_MAX_VARS: usize = 1_000_000;

/// One variable: a documentary name and its initial value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GamVar {
    pub name: String,
    pub value: i32,
}

impl GamVar {
    pub fn new(name: impl Into<String>, value: i32) -> Self {
        Self {
            name: name.into(),
            value,
        }
    }
}

/// The marker line the parser looks for. The campaign marker compares 16
/// bytes, so its conventional colon is written but not matched; the map
/// marker includes its colon.
pub fn marker_line(section: GamSection) -> &'static str {
    match section {
        GamSection::Campaign => "GAME_GLOBAL_VARS:\n",
        GamSection::Map => "MAP_GLOBAL_VARS:\n",
    }
}

fn check_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("the name is empty".into());
    }
    if !name
        .bytes()
        .all(|byte| byte.is_ascii_graphic() || byte == b' ')
    {
        return Err(format!("{name:?} is not printable ASCII"));
    }
    // `;` ends the record, `=` starts its value, and a line that begins
    // with `//` is a comment that allocates no variable.
    if name.contains([';', '=']) || name.starts_with("//") {
        return Err(format!(
            "{name:?} holds `;` or `=`, or starts with `//`, which the parser reads as syntax"
        ));
    }
    if name.starts_with("GAME_GLOBAL_VARS") || name.starts_with("MAP_GLOBAL_VARS:") {
        return Err(format!("{name:?} reads as a section marker"));
    }
    Ok(())
}

/// Serialize `vars` under `section`'s marker. The parser reads back the
/// values in order, `found` set.
pub fn write_gam(section: GamSection, vars: &[GamVar]) -> Result<Vec<u8>, EmitError> {
    if vars.len() > GAM_MAX_VARS {
        return Err(EmitError::Gam {
            index: GAM_MAX_VARS,
            message: format!("more than {GAM_MAX_VARS} variables"),
        });
    }
    let mut out = marker_line(section).as_bytes().to_vec();
    for (index, var) in vars.iter().enumerate() {
        let error = |message: String| EmitError::Gam { index, message };
        check_name(&var.name).map_err(error)?;
        let line = format!("{} = {};\n", var.name, var.value);
        if line.len() > GAM_LINE_BYTES {
            return Err(error(format!(
                "the line has {} bytes with its LF; one read holds {GAM_LINE_BYTES}",
                line.len()
            )));
        }
        out.extend_from_slice(line.as_bytes());
    }
    Ok(out)
}
