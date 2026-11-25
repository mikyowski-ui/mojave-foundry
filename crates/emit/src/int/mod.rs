//! A local assembler for compiled scripts (`.int`): the text format of
//! `docs/vault-engine-briefs/int-assembler.md`, producing the bytes the
//! pinned engine's loader (`vault_vm::IntProgram::parse_in`) accepts. It is
//! the stand-in for the engine's own assembler (gap A of the simulation
//! layer design); the source text is the stable interface.
//!
//! ```text
//! .procedure main                 ; record 0, its body is the label `main`
//! .procedure sumxy args=2         ; record 1
//! .literal greeting "hi"
//! .entry init                     ; where the bootstrap jumps
//! init:
//!     global_area
//!     push.i 0
//!     push.i 0
//!     critical_off
//!     push.a main
//!     jmp
//! main:
//!     ...
//! ```
//!
//! # Source format
//!
//! One statement per line; `;` starts a comment (not inside a quoted
//! literal); blank lines are ignored; names and labels are case sensitive,
//! made of printable ASCII without `:`, `;`, `"` or `=`. A label may share
//! its line with the statement it addresses.
//!
//! | Statement | Meaning |
//! |---|---|
//! | `.procedure <name> [args=<n>] [flags=<f>] [deadline=<n>] [condition=<label>] [body=<label>]` | the next procedure record, in order; its body is the label `<name>` unless `body=` names another (two records may share a body) |
//! | `.import <name> [args=<n>]` | a record with the import flag and body 0, resolved by name in the export registry at run time |
//! | `.name <name>` | an identifier record without a procedure (external variables) |
//! | `.literal <label> "<text>"` | a string record; `\"`, `\\` and `\xHH` are the escapes; ASCII only |
//! | `.entry <label>` | where the bootstrap jumps (required) |
//! | `<label>:` | an address in the code |
//! | `push.i <n>` | `C001`, a 32-bit integer (decimal, or `0x` hex as a bit pattern) |
//! | `push.f <decimal>` | `A001`, nearest binary32; must be finite |
//! | `push.s <literal>` | `9001`, the literal's offset |
//! | `push.n <name>` | `C001`, an identifier's offset (for the external variable words) |
//! | `push.a <label>` | `C001`, an absolute code address |
//! | `push.p <procedure>` | `C001`, a procedure's record index |
//! | `push.raw <tag> <u32>` | any of the 32 push words, payload as given |
//! | `<mnemonic>` | a core word (table in [`CORE_WORDS`]) or a host word of the word set |
//! | `op <word>` | any accepted non-push word, by number |
//!
//! `flags=` is a number or names joined by `|`: `timed`, `conditional`,
//! `import`, `export`, `critical`. `condition=` is only stored: the
//! conditional flag is the caller's to set (`flags=conditional`).
//!
//! # Layout produced
//!
//! The canonical form of the brief's section 3: the 42-byte bootstrap
//! (SPEC-004 section 2.2), the procedure records, one identifier record per
//! distinct name in first-declaration order, one literal record per
//! distinct text in declaration order (no literal table, `FF FF FF FF`,
//! when there are none), then the code. Equal names and equal literals
//! share a record. Output depends on the source text alone.
//!
//! # Word sets
//!
//! [`Words`] says which instruction words the program may use, as the
//! loader's `WordSet` does: the Fallout 1 set, the Fallout 2 set, with the
//! manifest scripting words (SPEC-024a) and the enhanced range when asked.
//! A word outside it is an error at the line that uses it. The sfall word
//! set is not supported.

mod words;

use std::collections::BTreeMap;

/// The loader's limit on the procedure count.
pub const MAX_PROCEDURES: usize = 65_536;
/// The loader's limit on a procedure's argument count.
pub const MAX_ARGUMENTS: u32 = 341;
/// The most text bytes of one identifier or literal (a record's stored
/// length is a 16-bit even number counting the NUL).
pub const MAX_STRING_BYTES: usize = 65_533;
/// The loader's limit on a program image.
pub const MAX_IMAGE_BYTES: usize = 64 * 1024 * 1024;
/// The size of the bootstrap and return-trampoline area (SPEC-004 2.2).
pub const BOOTSTRAP_BYTES: usize = 42;

/// Procedure flag bits (SPEC-004 section 2.3).
pub mod flags {
    pub const TIMED: u32 = 1;
    pub const CONDITIONAL: u32 = 2;
    pub const IMPORT: u32 = 4;
    pub const EXPORT: u32 = 8;
    pub const CRITICAL: u32 = 16;
}

/// The core words the source can name (SPEC-004 sections 3.2 to 3.5); the
/// rest are written with `op`.
pub const CORE_WORDS: &[(&str, u16)] = &[
    ("nop", 0x8000),
    ("critical_on", 0x8002),
    ("critical_off", 0x8003),
    ("jmp", 0x8004),
    ("call", 0x8005),
    ("schedule_at", 0x8006),
    ("schedule_when", 0x8007),
    ("r_to_v", 0x800c),
    ("v_to_r", 0x800d),
    ("end", 0x8010),
    ("stop", 0x8011),
    ("fetch_global", 0x8012),
    ("store_global", 0x8013),
    ("fetch_external", 0x8014),
    ("store_external", 0x8015),
    ("export_var", 0x8016),
    ("export_proc", 0x8017),
    ("swap", 0x8018),
    ("swap_r", 0x8019),
    ("pop", 0x801a),
    ("dup", 0x801b),
    ("ret", 0x801c),
    ("ret_yield", 0x801d),
    ("pop_r", 0x801e),
    ("restore_base", 0x8029),
    ("drop_frame", 0x802a),
    ("frame", 0x802b),
    ("global_area", 0x802c),
    ("proc_addr", 0x802d),
    ("bz", 0x802f),
    ("loop_bnz", 0x8030),
    ("store_local", 0x8031),
    ("fetch_local", 0x8032),
    ("eq", 0x8033),
    ("ne", 0x8034),
    ("le", 0x8035),
    ("ge", 0x8036),
    ("lt", 0x8037),
    ("gt", 0x8038),
    ("add", 0x8039),
    ("sub", 0x803a),
    ("mul", 0x803b),
    ("div", 0x803c),
    ("mod", 0x803d),
    ("and", 0x803e),
    ("or", 0x803f),
    ("band", 0x8040),
    ("bor", 0x8041),
    ("bxor", 0x8042),
    ("bnot", 0x8043),
    ("ftoi", 0x8044),
    ("not", 0x8045),
    ("neg", 0x8046),
    ("wait", 0x8047),
    ("cancel", 0x8048),
    ("cancel_all", 0x8049),
];

/// The enhanced range's named words (`vault_vm::extensions::ENHANCED`).
const ENHANCED_WORDS: &[(&str, u16)] = &[
    ("engine_api_version", 0x8200),
    ("op_cam_pan_to", 0x8210),
    ("op_cam_set_zoom", 0x8211),
    ("op_cam_shake", 0x8212),
    ("op_cam_reset", 0x8213),
];

/// What the assembler refuses, with the one-based source line (0 for a
/// fault of the whole program, such as its size).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("line {line}: {message}")]
pub struct AsmError {
    pub line: usize,
    pub message: String,
}

fn fail<T>(line: usize, message: impl Into<String>) -> Result<T, AsmError> {
    Err(AsmError {
        line,
        message: message.into(),
    })
}

/// The game whose host words a program may use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Game {
    /// `0x8000..=0x8155` (SPEC-004, SPEC-005).
    Fallout1,
    /// `0x8000..=0x8183` (SPEC-017).
    Fallout2,
}

/// The instruction words a program may use, as the engine's `WordSet` plus
/// its enhanced flag decide them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Words {
    pub game: Game,
    /// The manifest scripting words `0x8300..=0x8309` (SPEC-024a).
    pub mods: bool,
    /// The enhanced range `0x8200..=0x82FF`.
    pub enhanced: bool,
}

impl Words {
    pub const FALLOUT1: Self = Self {
        game: Game::Fallout1,
        mods: false,
        enhanced: false,
    };
    pub const FALLOUT2: Self = Self {
        game: Game::Fallout2,
        mods: false,
        enhanced: false,
    };

    pub const fn with_mods(self, mods: bool) -> Self {
        Self { mods, ..self }
    }

    pub const fn with_enhanced(self, enhanced: bool) -> Self {
        Self { enhanced, ..self }
    }

    /// Whether the loader accepts `word` as an instruction: a push word
    /// (low ten bits 1, bit 15 set), or a word of the set.
    pub fn accepts(&self, word: u16) -> bool {
        if word & 0x3ff == 1 {
            return word & 0x8000 != 0;
        }
        let last = match self.game {
            Game::Fallout1 => 0x8155,
            Game::Fallout2 => 0x8183,
        };
        (0x8000..=last).contains(&word)
            || (self.mods && (0x8300..=0x8309).contains(&word))
            || (self.enhanced && (0x8200..=0x82ff).contains(&word))
    }

    /// The word a mnemonic names in this set: core words, then the host
    /// words (Fallout 2 renames replace the Fallout 1 names of their
    /// words), the manifest scripting words and the enhanced words.
    pub fn word_named(&self, name: &str) -> Option<u16> {
        if let Some((_, word)) = CORE_WORDS.iter().find(|(core, _)| *core == name) {
            return Some(*word);
        }
        if self.game == Game::Fallout2
            && let Some((word, _)) = words::FALLOUT2_HOST
                .iter()
                .find(|(_, own)| !own.is_empty() && *own == name)
        {
            return Some(*word);
        }
        if let Some((word, _)) = words::FALLOUT1_HOST.iter().find(|(_, own)| *own == name) {
            let renamed = self.game == Game::Fallout2
                && words::FALLOUT2_HOST.iter().any(|(other, _)| other == word);
            if !renamed {
                return Some(*word);
            }
        }
        if self.mods
            && let Some((word, _)) = words::MOD_HOST.iter().find(|(_, own)| *own == name)
        {
            return Some(*word);
        }
        if self.enhanced
            && let Some((_, word)) = ENHANCED_WORDS.iter().find(|(own, _)| *own == name)
        {
            return Some(*word);
        }
        None
    }

    /// The mnemonic of `word` in this set, if it has one.
    pub fn name_of(&self, word: u16) -> Option<&'static str> {
        if let Some((name, _)) = CORE_WORDS.iter().find(|(_, core)| *core == word) {
            return Some(name);
        }
        if !self.accepts(word) {
            return None;
        }
        if self.game == Game::Fallout2
            && let Some((_, name)) = words::FALLOUT2_HOST.iter().find(|(own, _)| *own == word)
        {
            return (!name.is_empty()).then_some(*name);
        }
        words::FALLOUT1_HOST
            .iter()
            .chain(words::MOD_HOST)
            .find(|(own, _)| *own == word)
            .map(|(_, name)| *name)
            .or_else(|| {
                ENHANCED_WORDS
                    .iter()
                    .find(|(_, own)| *own == word)
                    .map(|(name, _)| *name)
            })
    }
}

// ---- Lexing --------------------------------------------------------------

enum Token {
    Word(String),
    Text(Vec<u8>),
}

fn lex(line: &str, number: usize) -> Result<Vec<Token>, AsmError> {
    let mut tokens = Vec::new();
    let mut chars = line.chars().peekable();
    while let Some(&ch) = chars.peek() {
        if ch == ';' {
            break;
        }
        if ch.is_whitespace() {
            chars.next();
            continue;
        }
        if ch == '"' {
            chars.next();
            let mut bytes = Vec::new();
            loop {
                match chars.next() {
                    None => return fail(number, "a literal is not closed by a `\"`"),
                    Some('"') => break,
                    Some('\\') => match chars.next() {
                        Some('"') => bytes.push(b'"'),
                        Some('\\') => bytes.push(b'\\'),
                        Some('x') => {
                            let high = chars.next().and_then(|c| c.to_digit(16));
                            let low = chars.next().and_then(|c| c.to_digit(16));
                            match (high, low) {
                                (Some(high), Some(low)) => bytes.push((high * 16 + low) as u8),
                                _ => return fail(number, "`\\x` needs two hex digits"),
                            }
                        }
                        other => {
                            return fail(
                                number,
                                format!(
                                    "unknown escape `\\{}`; the escapes are \\\", \\\\ and \\xHH",
                                    other.map(String::from).unwrap_or_default()
                                ),
                            );
                        }
                    },
                    Some(ch) if ch.is_ascii() => bytes.push(ch as u8),
                    Some(ch) => {
                        return fail(
                            number,
                            format!("{ch:?} is not ASCII; write the byte as \\xHH"),
                        );
                    }
                }
            }
            tokens.push(Token::Text(bytes));
            continue;
        }
        let mut word = String::new();
        while let Some(&ch) = chars.peek() {
            if ch.is_whitespace() || ch == ';' || ch == '"' {
                break;
            }
            word.push(ch);
            chars.next();
        }
        tokens.push(Token::Word(word));
    }
    Ok(tokens)
}

/// A name, label or word: printable ASCII without the characters the
/// syntax uses.
fn check_name(line: usize, what: &str, name: &str) -> Result<(), AsmError> {
    if name.len() > MAX_STRING_BYTES {
        return fail(
            line,
            format!(
                "{what} of {} bytes; a record holds at most {MAX_STRING_BYTES}",
                name.len()
            ),
        );
    }
    let valid = !name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_graphic() && !matches!(byte, b':' | b';' | b'"' | b'='));
    if valid {
        Ok(())
    } else {
        fail(
            line,
            format!("{what} {name:?} is not printable ASCII without `:`, `;`, `\"` and `=`"),
        )
    }
}

fn number(line: usize, text: &str) -> Result<i64, AsmError> {
    let (negative, digits) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text.strip_prefix('+').unwrap_or(text)),
    };
    let parsed = match digits
        .strip_prefix("0x")
        .or_else(|| digits.strip_prefix("0X"))
    {
        Some(hex) => i64::from_str_radix(hex, 16),
        None => digits.parse::<i64>(),
    };
    match parsed {
        Ok(value) if digits.bytes().all(|b| b.is_ascii_alphanumeric()) => {
            Ok(if negative { -value } else { value })
        }
        _ => fail(line, format!("{text:?} is not a number")),
    }
}

fn unsigned(line: usize, what: &str, text: &str, max: u64) -> Result<u32, AsmError> {
    let value = number(line, text)?;
    match u64::try_from(value) {
        Ok(value) if value <= max => Ok(value as u32),
        _ => fail(line, format!("{what} {text} is outside 0 to {max}")),
    }
}

fn flag_value(line: usize, text: &str) -> Result<u32, AsmError> {
    let mut value = 0;
    for part in text.split('|') {
        value |= match part {
            "timed" => flags::TIMED,
            "conditional" => flags::CONDITIONAL,
            "import" => flags::IMPORT,
            "export" => flags::EXPORT,
            "critical" => flags::CRITICAL,
            other => unsigned(line, "flags", other, u64::from(u32::MAX))?,
        };
    }
    Ok(value)
}

// ---- Pass 1: statements ----------------------------------------------------

struct ProcedureDecl {
    line: usize,
    name: String,
    arguments: u32,
    flags: u32,
    deadline: u32,
    condition: Option<String>,
    /// `None` for an import (body 0); else the label.
    body: Option<String>,
}

struct LiteralDecl {
    line: usize,
    label: String,
    text: Vec<u8>,
}

/// An operand that resolves once the layout is known.
enum Operand {
    Value(u32),
    Address(String),
    Name(String),
    Literal(String),
    Procedure(String),
}

enum Code {
    Word(u16),
    Push { tag: u16, operand: Operand },
}

impl Code {
    fn len(&self) -> usize {
        match self {
            Self::Word(_) => 2,
            Self::Push { .. } => 6,
        }
    }
}

struct Statement {
    line: usize,
    code: Code,
}

#[derive(Default)]
struct Source {
    procedures: Vec<ProcedureDecl>,
    procedure_index: BTreeMap<String, usize>,
    names: Vec<String>,
    name_index: BTreeMap<String, usize>,
    literals: Vec<LiteralDecl>,
    entry: Option<(usize, String)>,
    /// Label to (line, byte offset in the code).
    labels: BTreeMap<String, (usize, usize)>,
    code: Vec<Statement>,
    code_len: usize,
}

impl Source {
    fn intern(&mut self, name: &str) {
        if !self.name_index.contains_key(name) {
            self.name_index.insert(name.to_owned(), self.names.len());
            self.names.push(name.to_owned());
        }
    }

    fn define_label(&mut self, line: usize, label: &str) -> Result<(), AsmError> {
        check_name(line, "label", label)?;
        if let Some((first, _)) = self.labels.get(label) {
            return fail(
                line,
                format!("label `{label}` is already defined at line {first}"),
            );
        }
        self.labels.insert(label.to_owned(), (line, self.code_len));
        Ok(())
    }

    fn push_code(&mut self, line: usize, code: Code) {
        self.code_len += code.len();
        self.code.push(Statement { line, code });
    }

    fn statement(
        &mut self,
        line: usize,
        tokens: Vec<Token>,
        words: &Words,
    ) -> Result<(), AsmError> {
        let Some(Token::Word(head)) = tokens.first() else {
            return fail(line, "a statement starts with a word, not a quoted text");
        };
        if head.starts_with('.') {
            return self.directive(line, head, &tokens);
        }
        let mut operands = Vec::with_capacity(tokens.len() - 1);
        for token in &tokens[1..] {
            match token {
                Token::Word(word) => operands.push(word.as_str()),
                Token::Text(_) => {
                    return fail(line, "a quoted text is only allowed in `.literal`");
                }
            }
        }
        self.instruction(line, head, &operands, words)
    }

    fn directive(&mut self, line: usize, head: &str, tokens: &[Token]) -> Result<(), AsmError> {
        let word = |index: usize, what: &str| match tokens.get(index) {
            Some(Token::Word(word)) => Ok(word.as_str()),
            _ => fail(line, format!("`{head}` needs {what}")),
        };
        match head {
            ".procedure" | ".import" => {
                let import = head == ".import";
                let name = word(1, "a procedure name")?;
                check_name(line, "procedure name", name)?;
                let mut declaration = ProcedureDecl {
                    line,
                    name: name.to_owned(),
                    arguments: 0,
                    flags: if import { flags::IMPORT } else { 0 },
                    deadline: 0,
                    condition: None,
                    body: (!import).then(|| name.to_owned()),
                };
                for token in &tokens[2..] {
                    let Token::Word(option) = token else {
                        return fail(line, format!("`{head}` takes key=value options"));
                    };
                    let Some((key, value)) = option.split_once('=') else {
                        return fail(line, format!("{option:?} is not a key=value option"));
                    };
                    match (key, import) {
                        ("args", _) => {
                            declaration.arguments =
                                unsigned(line, "args", value, u64::from(MAX_ARGUMENTS))?;
                        }
                        ("flags", false) => declaration.flags = flag_value(line, value)?,
                        ("deadline", false) => {
                            declaration.deadline =
                                unsigned(line, "deadline", value, u64::from(u32::MAX))?;
                        }
                        ("condition", false) => {
                            check_name(line, "condition label", value)?;
                            declaration.condition = Some(value.to_owned());
                        }
                        ("body", false) => {
                            check_name(line, "body label", value)?;
                            declaration.body = Some(value.to_owned());
                        }
                        _ => {
                            return fail(line, format!("unknown option `{key}` for `{head}`"));
                        }
                    }
                }
                if self.procedure_index.contains_key(name) {
                    return fail(line, format!("procedure `{name}` is declared twice"));
                }
                if self.procedures.len() == MAX_PROCEDURES {
                    return fail(
                        line,
                        format!("more than {MAX_PROCEDURES} procedures; the loader's limit"),
                    );
                }
                self.intern(name);
                self.procedure_index
                    .insert(name.to_owned(), self.procedures.len());
                self.procedures.push(declaration);
                Ok(())
            }
            ".name" => {
                let name = word(1, "a name")?;
                check_name(line, "name", name)?;
                if tokens.len() > 2 {
                    return fail(line, "`.name` takes one name");
                }
                self.intern(name);
                Ok(())
            }
            ".literal" => {
                let label = word(1, "a label and a quoted text")?;
                check_name(line, "literal label", label)?;
                let Some(Token::Text(text)) = tokens.get(2) else {
                    return fail(line, "`.literal` needs a quoted text after its label");
                };
                if tokens.len() > 3 {
                    return fail(line, "`.literal` takes a label and one quoted text");
                }
                if text.contains(&0) {
                    return fail(line, "a literal cannot hold a NUL byte");
                }
                if text.len() > MAX_STRING_BYTES {
                    return fail(
                        line,
                        format!(
                            "a literal of {} bytes; a record holds at most {MAX_STRING_BYTES}",
                            text.len()
                        ),
                    );
                }
                if let Some(other) = self.literals.iter().find(|other| other.label == label) {
                    return fail(
                        line,
                        format!(
                            "literal `{label}` is already defined at line {}",
                            other.line
                        ),
                    );
                }
                self.literals.push(LiteralDecl {
                    line,
                    label: label.to_owned(),
                    text: text.clone(),
                });
                Ok(())
            }
            ".entry" => {
                let label = word(1, "a label")?;
                check_name(line, "entry label", label)?;
                if tokens.len() > 2 {
                    return fail(line, "`.entry` takes one label");
                }
                if let Some((first, _)) = &self.entry {
                    return fail(line, format!("`.entry` is already given at line {first}"));
                }
                self.entry = Some((line, label.to_owned()));
                Ok(())
            }
            other => fail(line, format!("unknown directive `{other}`")),
        }
    }

    fn instruction(
        &mut self,
        line: usize,
        head: &str,
        operands: &[&str],
        words: &Words,
    ) -> Result<(), AsmError> {
        let exactly = |count: usize| {
            if operands.len() == count {
                Ok(())
            } else {
                fail(
                    line,
                    format!("`{head}` takes {count} operand(s), not {}", operands.len()),
                )
            }
        };
        match head {
            "push.i" => {
                exactly(1)?;
                let value = number(line, operands[0])?;
                let bits = match (i32::try_from(value), u32::try_from(value)) {
                    (Ok(signed), _) => signed as u32,
                    (_, Ok(unsigned)) if operands[0].contains(['x', 'X']) => unsigned,
                    _ => {
                        return fail(
                            line,
                            format!("{} does not fit a 32-bit integer", operands[0]),
                        );
                    }
                };
                self.push_code(
                    line,
                    Code::Push {
                        tag: 0xc001,
                        operand: Operand::Value(bits),
                    },
                );
            }
            "push.f" => {
                exactly(1)?;
                let value: f32 = operands[0].parse().map_err(|_| AsmError {
                    line,
                    message: format!("{:?} is not a decimal number", operands[0]),
                })?;
                if !value.is_finite() {
                    return fail(line, format!("{:?} is not a finite float", operands[0]));
                }
                self.push_code(
                    line,
                    Code::Push {
                        tag: 0xa001,
                        operand: Operand::Value(value.to_bits()),
                    },
                );
            }
            "push.s" | "push.n" | "push.a" | "push.p" => {
                exactly(1)?;
                check_name(line, "operand", operands[0])?;
                let (tag, operand) = match head {
                    "push.s" => (0x9001, Operand::Literal(operands[0].to_owned())),
                    "push.n" => (0xc001, Operand::Name(operands[0].to_owned())),
                    "push.a" => (0xc001, Operand::Address(operands[0].to_owned())),
                    _ => (0xc001, Operand::Procedure(operands[0].to_owned())),
                };
                self.push_code(line, Code::Push { tag, operand });
            }
            "push.raw" => {
                exactly(2)?;
                let tag = unsigned(line, "tag", operands[0], 0xffff)? as u16;
                if tag & 0x3ff != 1 || tag & 0x8000 == 0 {
                    return fail(
                        line,
                        format!("{tag:#06x} is not a push word (bit 15 set, low ten bits 1)"),
                    );
                }
                let payload = number(line, operands[1])?;
                let bits = match (i32::try_from(payload), u32::try_from(payload)) {
                    (Ok(signed), _) => signed as u32,
                    (_, Ok(unsigned)) => unsigned,
                    _ => {
                        return fail(line, format!("{} does not fit 32 bits", operands[1]));
                    }
                };
                self.push_code(
                    line,
                    Code::Push {
                        tag,
                        operand: Operand::Value(bits),
                    },
                );
            }
            "op" => {
                exactly(1)?;
                let word = unsigned(line, "word", operands[0], 0xffff)? as u16;
                if word & 0x3ff == 1 {
                    return fail(line, "a push word is written with `push.raw`");
                }
                if !words.accepts(word) {
                    return fail(
                        line,
                        format!("word {word:#06x} is not accepted by the selected word set"),
                    );
                }
                self.push_code(line, Code::Word(word));
            }
            mnemonic => {
                let Some(word) = words.word_named(mnemonic) else {
                    return fail(
                        line,
                        format!("unknown word `{mnemonic}` for the selected word set"),
                    );
                };
                if !words.accepts(word) {
                    return fail(
                        line,
                        format!("`{mnemonic}` ({word:#06x}) is not accepted by the word set"),
                    );
                }
                exactly(0)?;
                self.push_code(line, Code::Word(word));
            }
        }
        Ok(())
    }
}

// ---- Layout and pass 2 -----------------------------------------------------

/// A string table: records in order, the offset of each text, the payload.
struct Table {
    /// Offsets of the first text byte of each record, from the table's
    /// origin (which holds its 4-byte length word).
    offsets: Vec<u32>,
    payload: Vec<u8>,
}

fn string_table<'a>(texts: impl Iterator<Item = &'a [u8]>) -> Table {
    let mut offsets = Vec::new();
    let mut payload = Vec::new();
    for text in texts {
        let stored = 2 * (text.len() + 1).div_ceil(2);
        payload.extend_from_slice(&(stored as u16).to_be_bytes());
        offsets.push((4 + payload.len()) as u32);
        payload.extend_from_slice(text);
        payload.resize(payload.len() + stored - text.len(), 0);
    }
    Table { offsets, payload }
}

fn put(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_be_bytes());
}

fn push(out: &mut Vec<u8>, tag: u16, value: u32) {
    out.extend_from_slice(&tag.to_be_bytes());
    put(out, value);
}

/// Assemble `source` for the word set `words`. Errors name the source line;
/// no input panics. The output is the same for the same source.
pub fn assemble(source: &str, words: &Words) -> Result<Vec<u8>, AsmError> {
    let mut parsed = Source::default();
    for (index, text) in source.lines().enumerate() {
        let line = index + 1;
        let mut tokens = lex(text, line)?;
        // A leading `label:` word defines a label.
        if let Some(Token::Word(first)) = tokens.first()
            && let Some(label) = first.strip_suffix(':')
        {
            let label = label.to_owned();
            parsed.define_label(line, &label)?;
            tokens.remove(0);
        }
        if tokens.is_empty() {
            continue;
        }
        parsed.statement(line, tokens, words)?;
    }

    let Some((entry_line, entry)) = parsed.entry.clone() else {
        return fail(0, "no `.entry`: the bootstrap needs an address to jump to");
    };

    // Tables.
    let names = string_table(parsed.names.iter().map(|name| name.as_bytes()));
    let mut distinct: Vec<&[u8]> = Vec::new();
    let mut seen: BTreeMap<&[u8], usize> = BTreeMap::new();
    let mut literal_slot: BTreeMap<&str, usize> = BTreeMap::new();
    for literal in &parsed.literals {
        let slot = *seen.entry(literal.text.as_slice()).or_insert_with(|| {
            distinct.push(&literal.text);
            distinct.len() - 1
        });
        literal_slot.insert(&literal.label, slot);
    }
    let literals = string_table(distinct.iter().copied());

    let count = parsed.procedures.len();
    let identifier_origin = BOOTSTRAP_BYTES + 4 + 24 * count;
    let literal_origin = identifier_origin + 4 + names.payload.len() + 4;
    let code_start = if distinct.is_empty() {
        literal_origin + 4
    } else {
        literal_origin + 4 + literals.payload.len() + 4
    };
    let total = code_start + parsed.code_len;
    if total > MAX_IMAGE_BYTES {
        return fail(
            0,
            format!("the program would be {total} bytes; the loader's limit is {MAX_IMAGE_BYTES}"),
        );
    }
    let address = |line: usize, label: &str| -> Result<u32, AsmError> {
        match parsed.labels.get(label) {
            Some((_, offset)) => Ok((code_start + offset) as u32),
            None => fail(line, format!("undefined label `{label}`")),
        }
    };
    let entry_address = address(entry_line, &entry)?;

    let mut out = Vec::with_capacity(total);
    // The bootstrap (SPEC-004 section 2.2).
    out.extend_from_slice(&0x8002u16.to_be_bytes());
    push(&mut out, 0xc001, 18);
    out.extend_from_slice(&0x800du16.to_be_bytes());
    push(&mut out, 0xc001, entry_address);
    out.extend_from_slice(&0x8004u16.to_be_bytes());
    out.extend_from_slice(&0x8010u16.to_be_bytes());
    // The four return trampolines (each discards a result first) and the
    // three value-preserving returns.
    for restore in [0x8020u16, 0x8021, 0x8022, 0x8023] {
        out.extend_from_slice(&0x801au16.to_be_bytes());
        out.extend_from_slice(&restore.to_be_bytes());
    }
    for word in [0x8024u16, 0x8025, 0x8026] {
        out.extend_from_slice(&word.to_be_bytes());
    }
    debug_assert_eq!(out.len(), BOOTSTRAP_BYTES);

    put(&mut out, count as u32);
    for procedure in &parsed.procedures {
        let name = names.offsets[parsed.name_index[&procedure.name]];
        let condition = match &procedure.condition {
            Some(label) => {
                let at = address(procedure.line, label)?;
                if at == 0 {
                    return fail(procedure.line, "a condition at address 0 reads as none");
                }
                at
            }
            None => 0,
        };
        let body = match &procedure.body {
            Some(label) => address(procedure.line, label)?,
            None => 0,
        };
        for value in [
            name,
            procedure.flags,
            procedure.deadline,
            condition,
            body,
            procedure.arguments,
        ] {
            put(&mut out, value);
        }
    }

    put(&mut out, names.payload.len() as u32);
    out.extend_from_slice(&names.payload);
    put(&mut out, u32::MAX);
    if distinct.is_empty() {
        put(&mut out, u32::MAX);
    } else {
        put(&mut out, literals.payload.len() as u32);
        out.extend_from_slice(&literals.payload);
        put(&mut out, u32::MAX);
    }
    debug_assert_eq!(out.len(), code_start);

    for statement in &parsed.code {
        match &statement.code {
            Code::Word(word) => out.extend_from_slice(&word.to_be_bytes()),
            Code::Push { tag, operand } => {
                let line = statement.line;
                let value = match operand {
                    Operand::Value(value) => *value,
                    Operand::Address(label) => address(line, label)?,
                    Operand::Name(name) => match parsed.name_index.get(name) {
                        Some(index) => names.offsets[*index],
                        None => {
                            return fail(
                                line,
                                format!("`{name}` is not a declared name (`.procedure`, `.name`)"),
                            );
                        }
                    },
                    Operand::Literal(label) => match literal_slot.get(label.as_str()) {
                        Some(slot) => literals.offsets[*slot],
                        None => return fail(line, format!("undefined literal `{label}`")),
                    },
                    Operand::Procedure(name) => match parsed.procedure_index.get(name) {
                        Some(index) => *index as u32,
                        None => return fail(line, format!("undefined procedure `{name}`")),
                    },
                };
                push(&mut out, *tag, value);
            }
        }
    }
    debug_assert_eq!(out.len(), total);
    Ok(out)
}

/// The procedure names of an assembled program, in record order: a light
/// read of the procedure records and the identifier table (SPEC-004
/// sections 2.3 and 2.4), without decoding the code. For a layer's
/// checks that a hook or global script names a procedure the program
/// has; the engine's loader does the full validation.
pub fn procedure_names(image: &[u8]) -> Result<Vec<Vec<u8>>, AsmError> {
    let word = |at: usize| -> Result<usize, AsmError> {
        image
            .get(at..at + 4)
            .map(|bytes| u32::from_be_bytes(bytes.try_into().expect("four bytes")) as usize)
            .ok_or_else(|| AsmError {
                line: 0,
                message: format!("the program ends before byte {at:#x}"),
            })
    };
    let count = word(BOOTSTRAP_BYTES)?;
    if count > MAX_PROCEDURES {
        return fail(
            0,
            format!("{count} procedures; the loader's limit is {MAX_PROCEDURES}"),
        );
    }
    let origin = BOOTSTRAP_BYTES + 4 + 24 * count;
    let length = word(origin)?;
    let end = origin + 4 + length;
    if end > image.len() {
        return fail(0, "the identifier table runs past the program");
    }
    // Text by the offset of its first byte from the table origin.
    let mut texts: BTreeMap<usize, &[u8]> = BTreeMap::new();
    let mut at = origin + 4;
    while at < end {
        let size = image
            .get(at..at + 2)
            .map(|bytes| usize::from(u16::from_be_bytes([bytes[0], bytes[1]])))
            .filter(|size| *size > 0 && at + 2 + size <= end);
        let Some(size) = size else {
            return fail(0, format!("a bad identifier record at byte {at:#x}"));
        };
        let record = &image[at + 2..at + 2 + size];
        let Some(nul) = record.iter().position(|byte| *byte == 0) else {
            return fail(0, format!("an identifier at byte {at:#x} has no NUL"));
        };
        texts.insert(at + 2 - origin, &record[..nul]);
        at += 2 + size;
    }
    (0..count)
        .map(|index| {
            let offset = word(BOOTSTRAP_BYTES + 4 + 24 * index)?;
            texts
                .get(&offset)
                .map(|name| name.to_vec())
                .ok_or_else(|| AsmError {
                    line: 0,
                    message: format!("procedure {index}'s name is not an identifier record"),
                })
        })
        .collect()
}
