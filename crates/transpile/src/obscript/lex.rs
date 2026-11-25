//! The lexer (spec 5.2): lines, `;` comments outside strings,
//! case-insensitive keywords and identifiers.
//!
//! The spec lists no line continuation, and the source language has none:
//! a line ends a statement. A lexical error is a diagnostic and the lexer
//! goes on with the next character, so one bad character never loses a
//! line's other tokens.

use crate::diag::{Category, Diag, Span};

/// A token's kind.
#[derive(Debug, Clone, PartialEq)]
pub enum Tok {
    /// A name: a keyword, variable, form editor id or function. Keywords
    /// are recognised by the parser, case-insensitively.
    Ident(String),
    /// An integer literal.
    Int(i64),
    /// A literal with a decimal point.
    Float(f64),
    /// A string literal, without its quotes.
    Str(String),
    /// `(`
    LParen,
    /// `)`
    RParen,
    /// `,`
    Comma,
    /// `.` between two names.
    Dot,
    /// `+`
    Plus,
    /// `-`
    Minus,
    /// `*`
    Star,
    /// `/`
    Slash,
    /// `%`
    Percent,
    /// `==`
    EqEq,
    /// `=`, which some scripts use for `==`.
    Eq,
    /// `!=`
    NotEq,
    /// `!`
    Bang,
    /// `<`
    Lt,
    /// `<=`
    Le,
    /// `>`
    Gt,
    /// `>=`
    Ge,
    /// `&&`
    AndAnd,
    /// `||`
    OrOr,
}

/// A token and where it starts.
#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    /// The kind.
    pub tok: Tok,
    /// The position of its first character.
    pub span: Span,
}

/// One source line's tokens and lexical diagnostics; blank and comment-only
/// lines are left out.
#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    /// The line number, from 1.
    pub number: u32,
    /// The tokens; empty only when the line held nothing but bad
    /// characters.
    pub tokens: Vec<Token>,
    /// What the lexer found on the line. The parser decides whether they
    /// count: a line outside any block is never compiled by the source
    /// compiler, and neither is the text after a declaration's name.
    pub diags: Vec<Diag>,
}

/// Splits a source text into lines of tokens. Line breaks are `\n`,
/// `\r\n` or a lone `\r`.
pub fn lex(source: &str) -> Vec<Line> {
    let mut lines = Vec::new();
    let normalised = source.replace("\r\n", "\n").replace('\r', "\n");
    for (index, text) in normalised.split('\n').enumerate() {
        let number = u32::try_from(index + 1).unwrap_or(u32::MAX);
        let mut diags = Vec::new();
        let tokens = lex_line(text, number, &mut diags);
        // After a declaration's name the source compiler reads nothing.
        if let [
            Token {
                tok: Tok::Ident(keyword),
                ..
            },
            Token {
                tok: Tok::Ident(_),
                span,
            },
            ..,
        ] = tokens.as_slice()
            && is_declaration_keyword(keyword)
        {
            let name_line = span.line;
            let name_start = span.column;
            diags.retain(|d| {
                d.at.is_some_and(|a| a.line == name_line && a.column < name_start)
            });
        }
        if !tokens.is_empty() || !diags.is_empty() {
            lines.push(Line {
                number,
                tokens,
                diags,
            });
        }
    }
    lines
}

/// Whether a word starts a declaration.
pub fn is_declaration_keyword(word: &str) -> bool {
    ["short", "int", "long", "float", "ref", "reference"]
        .iter()
        .any(|k| k.eq_ignore_ascii_case(word))
}

fn lex_line(text: &str, line: u32, diags: &mut Vec<Diag>) -> Vec<Token> {
    let chars: Vec<char> = text.chars().collect();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let span = Span::new(line, u32::try_from(i + 1).unwrap_or(u32::MAX));
        let next = chars.get(i + 1).copied();
        let mut push = |tok: Tok, width: usize, i: &mut usize| {
            tokens.push(Token { tok, span });
            *i += width;
        };
        match c {
            ';' => break,
            c if c.is_whitespace() || c.is_control() => i += 1,
            '.' if i > 0 && is_name_char(chars[i - 1]) => push(Tok::Dot, 1, &mut i),
            '"' => {
                let start = i + 1;
                match chars[start..].iter().position(|&c| c == '"') {
                    Some(len) => {
                        let value: String = chars[start..start + len].iter().collect();
                        push(Tok::Str(value), len + 2, &mut i);
                    }
                    None => {
                        diags.push(
                            Diag::error(
                                Category::UnterminatedString,
                                "string without a closing quote",
                            )
                            .at(span),
                        );
                        let value: String = chars[start..].iter().collect();
                        push(Tok::Str(value), chars.len() - i, &mut i);
                    }
                }
            }
            c if c.is_ascii_digit() || (c == '.' && next.is_some_and(|n| n.is_ascii_digit())) => {
                let start = i;
                let mut end = i;
                let mut dot = false;
                while end < chars.len() {
                    let d = chars[end];
                    if d.is_ascii_digit() {
                        end += 1;
                    } else if d == '.' && !dot {
                        dot = true;
                        end += 1;
                    } else {
                        break;
                    }
                }
                // A number glued to letters (`10a`) is a name in the
                // source compiler's eyes; keep it whole as one.
                if end < chars.len() && (chars[end].is_alphabetic() || chars[end] == '_') {
                    while end < chars.len() && is_name_char(chars[end]) {
                        end += 1;
                    }
                    let word: String = chars[start..end].iter().collect();
                    push(Tok::Ident(word), end - start, &mut i);
                    continue;
                }
                let word: String = chars[start..end].iter().collect();
                let tok = if dot {
                    word.parse::<f64>().ok().map(Tok::Float)
                } else {
                    word.parse::<i64>().ok().map(Tok::Int)
                };
                match tok {
                    Some(tok) => push(tok, end - start, &mut i),
                    None => {
                        diags.push(
                            Diag::error(Category::BadNumber, format!("bad number {word:?}"))
                                .at(span)
                                .about(word.clone()),
                        );
                        push(Tok::Int(0), end - start, &mut i);
                    }
                }
            }
            c if c.is_alphabetic() || c == '_' => {
                let start = i;
                let mut end = i;
                while end < chars.len() && is_name_char(chars[end]) {
                    end += 1;
                }
                let word: String = chars[start..end].iter().collect();
                push(Tok::Ident(word), end - start, &mut i);
            }
            '(' => push(Tok::LParen, 1, &mut i),
            ')' => push(Tok::RParen, 1, &mut i),
            ',' => push(Tok::Comma, 1, &mut i),
            '.' => push(Tok::Dot, 1, &mut i),
            '+' => push(Tok::Plus, 1, &mut i),
            '-' => push(Tok::Minus, 1, &mut i),
            '*' => push(Tok::Star, 1, &mut i),
            '/' => push(Tok::Slash, 1, &mut i),
            '%' => push(Tok::Percent, 1, &mut i),
            '=' if next == Some('=') => push(Tok::EqEq, 2, &mut i),
            '=' => push(Tok::Eq, 1, &mut i),
            '!' if next == Some('=') => push(Tok::NotEq, 2, &mut i),
            '!' => push(Tok::Bang, 1, &mut i),
            '<' if next == Some('=') => push(Tok::Le, 2, &mut i),
            '<' => push(Tok::Lt, 1, &mut i),
            '>' if next == Some('=') => push(Tok::Ge, 2, &mut i),
            '>' => push(Tok::Gt, 1, &mut i),
            '&' if next == Some('&') => push(Tok::AndAnd, 2, &mut i),
            '|' if next == Some('|') => push(Tok::OrOr, 2, &mut i),
            other => {
                diags.push(
                    Diag::error(
                        Category::BadCharacter,
                        format!("unexpected character {other:?}"),
                    )
                    .at(span)
                    .about(other.to_string()),
                );
                i += 1;
            }
        }
    }
    tokens
}

fn is_name_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toks(source: &str) -> (Vec<Vec<Tok>>, Vec<Diag>) {
        let lines = lex(source);
        let diags: Vec<Diag> = lines.iter().flat_map(|l| l.diags.clone()).collect();
        (
            lines
                .into_iter()
                .map(|l| l.tokens.into_iter().map(|t| t.tok).collect())
                .collect(),
            diags,
        )
    }

    #[test]
    fn comments_and_blank_lines_go() {
        let (lines, diags) = toks("; header\r\n\r\nset x to 1 ; trailing\n   ;only\n");
        assert!(diags.is_empty());
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].len(), 4);
    }

    #[test]
    fn semicolons_inside_strings_are_text() {
        let (lines, _) = toks("MessageBox \"a; b\" x");
        assert_eq!(lines[0][1], Tok::Str("a; b".to_owned()));
        assert_eq!(lines[0][2], Tok::Ident("x".to_owned()));
    }

    #[test]
    fn operators_numbers_and_names() {
        let (lines, diags) = toks("if (a.b >= -1.5) && c != 2 || !d == .5");
        assert!(diags.is_empty());
        assert_eq!(
            lines[0],
            vec![
                Tok::Ident("if".into()),
                Tok::LParen,
                Tok::Ident("a".into()),
                Tok::Dot,
                Tok::Ident("b".into()),
                Tok::Ge,
                Tok::Minus,
                Tok::Float(1.5),
                Tok::RParen,
                Tok::AndAnd,
                Tok::Ident("c".into()),
                Tok::NotEq,
                Tok::Int(2),
                Tok::OrOr,
                Tok::Bang,
                Tok::Ident("d".into()),
                Tok::EqEq,
                Tok::Float(0.5),
            ]
        );
    }

    #[test]
    fn errors_are_diagnostics_with_positions() {
        let (lines, diags) = toks("set x to 1 # 2\nMessageBox \"open");
        assert_eq!(lines.len(), 2);
        assert_eq!(diags.len(), 2);
        assert_eq!(diags[0].category, Category::BadCharacter);
        assert_eq!(diags[0].at, Some(Span::new(1, 12)));
        assert_eq!(diags[1].category, Category::UnterminatedString);
        assert_eq!(diags[1].at, Some(Span::new(2, 12)));
        // The tokens after the bad character survive.
        assert_eq!(lines[0].last(), Some(&Tok::Int(2)));
    }

    #[test]
    fn digits_glued_to_letters_are_one_name() {
        let (lines, _) = toks("set x to 10mmAmmo");
        assert_eq!(lines[0][3], Tok::Ident("10mmAmmo".into()));
    }
}
