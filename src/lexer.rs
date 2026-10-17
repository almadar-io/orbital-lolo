//! Streaming tokenizer for the .lolo surface language.
//!
//! Tokens are produced eagerly into a `Vec<Token>` for simplicity. The token
//! stream is small (a few thousand tokens for a real-world `.lolo` file), so
//! the cost is irrelevant compared to a streaming approach. Every token
//! carries a `Span` so the parser and lower passes can produce
//! `file:line:col` precise errors.

use crate::ast::{Comment, Span};
use crate::error::LexError;

/// One token in the source. Owned strings keep the lexer simple — `.lolo`
/// files are tiny by codegen standards.
#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}

/// Token kinds. Ordering is alphabetical inside each group for grep-ability.
#[derive(Debug, Clone, PartialEq)]
pub enum TokenKind {
    // ----- Punctuation -----
    LParen,
    RParen,
    LBracket,
    RBracket,
    LBrace,
    RBrace,
    Colon,
    /// `::` — used above declarations to apply a named type annotation
    /// (`entity Patient :: Auditable`). Emitted as a single token for
    /// unambiguous parsing; see the `:` lexer arm which looks ahead one char.
    DoubleColon,
    Comma,
    Pipe,
    /// `->` or `→`
    Arrow,
    /// `--EVENT-->` is split by the parser; the lexer just produces `--`
    /// before identifiers and `-->` after.
    DashDash,
    DashArrow,
    /// `→` (Unicode arrow)
    UnicodeArrow,
    Bang,
    Question,
    Dot,
    Equals,
    /// `=` followed by no other char (we always emit `Equals` here, the
    /// parser handles `==` from inside an s-expression).
    Star,
    Plus,
    Slash,
    /// `&` — type-composition operator. Used in `type T = A & B` to compose
    /// two shape aliases of the same kind.
    Ampersand,
    /// `@(` — start of an inline trait reference (sugar). Pairs with a trailing
    /// `)`. The body between is `Alias.traits.Upstream [-> Entity] { … }`,
    /// identical to a `trait Name = …` reference body. Desugared by the
    /// lowerer into a synthesized `trait` declaration + an `@trait.<Name>` ref.
    AtParen,

    // ----- Atoms -----
    Identifier(String),
    /// `@field`, `@entity.field`, `@now`, `@state`, `@payload.x`, `@EntityName.field`.
    Sigil(String),
    /// `?field`, `?` (bare), `?field.sub`. Lexed identically to a sigil but
    /// with a different prefix so the parser can distinguish.
    PayloadSigil(String),
    String(String),
    Integer(i64),
    Number(f64),
    Boolean(bool),
    Null,

    // ----- Layout -----
    Newline,
    Eof,
}

/// Tokenize a `.lolo` source string. Returns the full token vector or the
/// first lex error. Comments are dropped (see `tokenize_with_comments` to keep
/// them as trivia for the formatter).
pub fn tokenize(source: &str) -> Result<Vec<Token>, LexError> {
    Ok(tokenize_with_comments(source)?.0)
}

/// Like `tokenize`, but also returns the source comments (in source order) as
/// trivia so the formatter can re-emit them.
pub fn tokenize_with_comments(source: &str) -> Result<(Vec<Token>, Vec<Comment>), LexError> {
    let mut lexer = Lexer::new(source);
    lexer.run()?;
    Ok((lexer.tokens, lexer.comments))
}

struct Lexer {
    chars: Vec<(usize, char)>,
    pos: usize,
    line: u32,
    col: u32,
    tokens: Vec<Token>,
    comments: Vec<Comment>,
}

impl Lexer {
    fn new(src: &str) -> Self {
        let chars: Vec<(usize, char)> = src.char_indices().collect();
        Self {
            chars,
            pos: 0,
            line: 1,
            col: 1,
            tokens: Vec::new(),
            comments: Vec::new(),
        }
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).map(|&(_, c)| c)
    }

    fn peek_n(&self, n: usize) -> Option<char> {
        self.chars.get(self.pos + n).map(|&(_, c)| c)
    }

    fn advance(&mut self) -> Option<char> {
        let &(_, c) = self.chars.get(self.pos)?;
        self.pos += 1;
        if c == '\n' {
            self.line += 1;
            self.col = 1;
        } else {
            self.col += 1;
        }
        Some(c)
    }

    fn run(&mut self) -> Result<(), LexError> {
        while let Some(c) = self.peek() {
            let line = self.line;
            let col = self.col;
            match c {
                ' ' | '\t' | '\r' => {
                    self.advance();
                }
                '\n' => {
                    self.advance();
                    // Collapse runs of newlines into a single Newline token —
                    // the parser uses newlines as statement separators.
                    if !matches!(
                        self.tokens.last().map(|t| &t.kind),
                        Some(TokenKind::Newline)
                    ) {
                        self.tokens.push(Token {
                            kind: TokenKind::Newline,
                            span: Span::new(line, col, 1),
                        });
                    }
                }
                '#' => {
                    // `#` line comment, or `#= ... =#` block comment.
                    if self.peek_n(1) == Some('=') {
                        self.lex_block_comment()?;
                    } else {
                        self.skip_line_comment();
                    }
                }
                ';' if self.peek_n(1) == Some(';') => {
                    // `;;` line comment (legacy/Lisp style).
                    self.skip_line_comment();
                }
                '(' => {
                    self.advance();
                    self.tokens.push(Token {
                        kind: TokenKind::LParen,
                        span: Span::new(line, col, 1),
                    });
                }
                ')' => {
                    self.advance();
                    self.tokens.push(Token {
                        kind: TokenKind::RParen,
                        span: Span::new(line, col, 1),
                    });
                }
                '[' => {
                    self.advance();
                    self.tokens.push(Token {
                        kind: TokenKind::LBracket,
                        span: Span::new(line, col, 1),
                    });
                }
                ']' => {
                    self.advance();
                    self.tokens.push(Token {
                        kind: TokenKind::RBracket,
                        span: Span::new(line, col, 1),
                    });
                }
                '{' => {
                    self.advance();
                    self.tokens.push(Token {
                        kind: TokenKind::LBrace,
                        span: Span::new(line, col, 1),
                    });
                }
                '}' => {
                    self.advance();
                    self.tokens.push(Token {
                        kind: TokenKind::RBrace,
                        span: Span::new(line, col, 1),
                    });
                }
                ':' => {
                    self.advance();
                    if self.peek() == Some(':') {
                        self.advance();
                        self.tokens.push(Token {
                            kind: TokenKind::DoubleColon,
                            span: Span::new(line, col, 2),
                        });
                    } else {
                        self.tokens.push(Token {
                            kind: TokenKind::Colon,
                            span: Span::new(line, col, 1),
                        });
                    }
                }
                ',' => {
                    self.advance();
                    self.tokens.push(Token {
                        kind: TokenKind::Comma,
                        span: Span::new(line, col, 1),
                    });
                }
                '|' => {
                    self.advance();
                    self.tokens.push(Token {
                        kind: TokenKind::Pipe,
                        span: Span::new(line, col, 1),
                    });
                }
                '&' => {
                    self.advance();
                    self.tokens.push(Token {
                        kind: TokenKind::Ampersand,
                        span: Span::new(line, col, 1),
                    });
                }
                '!' => {
                    self.advance();
                    if self.peek() == Some('=') {
                        self.advance();
                        self.tokens.push(Token {
                            kind: TokenKind::Identifier("!=".to_string()),
                            span: Span::new(line, col, 2),
                        });
                    } else {
                        self.tokens.push(Token {
                            kind: TokenKind::Bang,
                            span: Span::new(line, col, 1),
                        });
                    }
                }
                '?' => {
                    // `?` could be the bare payload sigil or `?field`.
                    self.advance();
                    if let Some(ch) = self.peek() {
                        if ch.is_alphabetic() || ch == '_' {
                            let name = self.read_dotted_ident();
                            self.tokens.push(Token {
                                kind: TokenKind::PayloadSigil(name.clone()),
                                span: Span::new(line, col, name.chars().count() as u16 + 1),
                            });
                            continue;
                        }
                    }
                    self.tokens.push(Token {
                        kind: TokenKind::PayloadSigil(String::new()),
                        span: Span::new(line, col, 1),
                    });
                }
                '@' => {
                    self.advance();
                    // `@(` — inline trait-reference sugar. Emit a dedicated
                    // token so the expression parser can hoist the body.
                    if self.peek() == Some('(') {
                        self.advance();
                        self.tokens.push(Token {
                            kind: TokenKind::AtParen,
                            span: Span::new(line, col, 2),
                        });
                        continue;
                    }
                    let name = self.read_dotted_ident();
                    self.tokens.push(Token {
                        kind: TokenKind::Sigil(name.clone()),
                        span: Span::new(line, col, name.chars().count() as u16 + 1),
                    });
                }
                '"' => {
                    let s = self.lex_string()?;
                    let len = (self.col - col) as u16;
                    self.tokens.push(Token {
                        kind: TokenKind::String(s),
                        span: Span::new(line, col, len),
                    });
                }
                '-' => {
                    // `->`, `--->`, `--`, `--EVENT-->`, or negative number.
                    if self.peek_n(1) == Some('>') {
                        self.advance();
                        self.advance();
                        self.tokens.push(Token {
                            kind: TokenKind::Arrow,
                            span: Span::new(line, col, 2),
                        });
                    } else if self.peek_n(1) == Some('-') {
                        // `--` (event arrow start) — emit DashDash; parser
                        // will look ahead for an event name.
                        self.advance();
                        self.advance();
                        self.tokens.push(Token {
                            kind: TokenKind::DashDash,
                            span: Span::new(line, col, 2),
                        });
                    } else if let Some(d) = self.peek_n(1) {
                        if d.is_ascii_digit() {
                            self.advance();
                            self.lex_number(line, col, true)?;
                            continue;
                        }
                        self.advance();
                        self.tokens.push(Token {
                            kind: TokenKind::Identifier("-".to_string()),
                            span: Span::new(line, col, 1),
                        });
                    } else {
                        self.advance();
                        self.tokens.push(Token {
                            kind: TokenKind::Identifier("-".to_string()),
                            span: Span::new(line, col, 1),
                        });
                    }
                }
                '→' => {
                    // Unicode arrow.
                    self.advance();
                    self.tokens.push(Token {
                        kind: TokenKind::UnicodeArrow,
                        span: Span::new(line, col, 1),
                    });
                }
                '=' => {
                    self.advance();
                    if self.peek() == Some('=') {
                        self.advance();
                        self.tokens.push(Token {
                            kind: TokenKind::Identifier("==".to_string()),
                            span: Span::new(line, col, 2),
                        });
                    } else {
                        self.tokens.push(Token {
                            kind: TokenKind::Equals,
                            span: Span::new(line, col, 1),
                        });
                    }
                }
                '+' => {
                    self.advance();
                    self.tokens.push(Token {
                        kind: TokenKind::Identifier("+".to_string()),
                        span: Span::new(line, col, 1),
                    });
                }
                '*' => {
                    self.advance();
                    self.tokens.push(Token {
                        kind: TokenKind::Identifier("*".to_string()),
                        span: Span::new(line, col, 1),
                    });
                }
                '/' => {
                    self.advance();
                    self.tokens.push(Token {
                        kind: TokenKind::Identifier("/".to_string()),
                        span: Span::new(line, col, 1),
                    });
                }
                '%' => {
                    self.advance();
                    self.tokens.push(Token {
                        kind: TokenKind::Identifier("%".to_string()),
                        span: Span::new(line, col, 1),
                    });
                }
                '<' | '>' => {
                    let first = c;
                    self.advance();
                    if self.peek() == Some('=') {
                        self.advance();
                        self.tokens.push(Token {
                            kind: TokenKind::Identifier(format!("{first}=")),
                            span: Span::new(line, col, 2),
                        });
                    } else {
                        self.tokens.push(Token {
                            kind: TokenKind::Identifier(first.to_string()),
                            span: Span::new(line, col, 1),
                        });
                    }
                }
                '0'..='9' => {
                    self.lex_number(line, col, false)?;
                }
                '.' => {
                    // Bare `.` outside sigils. Used for dotted reference paths
                    // like `Modal.traits.ModalRecordModal` and `Source.EVENT`
                    // listen sources. The parser combines adjacent
                    // `Identifier Dot Identifier` runs into one dotted name.
                    self.advance();
                    self.tokens.push(Token {
                        kind: TokenKind::Dot,
                        span: Span::new(line, col, 1),
                    });
                }
                _ if c.is_alphabetic() || c == '_' => {
                    self.lex_identifier_or_keyword(line, col);
                }
                _ => {
                    return Err(LexError::UnexpectedChar {
                        ch: c,
                        line: self.line,
                        col: self.col,
                    });
                }
            }
        }
        self.tokens.push(Token {
            kind: TokenKind::Eof,
            span: Span::new(self.line, self.col, 0),
        });
        Ok(())
    }

    fn skip_line_comment(&mut self) {
        let line = self.line;
        let mut text = String::new();
        while let Some(c) = self.peek() {
            if c == '\n' {
                break;
            }
            text.push(c);
            self.advance();
        }
        let trimmed = text.trim_end();
        if !trimmed.is_empty() {
            self.comments.push(Comment {
                line,
                text: trimmed.to_string(),
            });
        }
    }

    fn lex_block_comment(&mut self) -> Result<(), LexError> {
        let line = self.line;
        let col = self.col;
        let mut text = String::from("#=");
        // Consume `#=`.
        self.advance();
        self.advance();
        loop {
            match self.peek() {
                Some('=') if self.peek_n(1) == Some('#') => {
                    self.advance();
                    self.advance();
                    text.push_str("=#");
                    self.comments.push(Comment { line, text });
                    return Ok(());
                }
                Some(c) => {
                    text.push(c);
                    self.advance();
                }
                None => {
                    return Err(LexError::UnterminatedBlockComment { line, col });
                }
            }
        }
    }

    fn read_dotted_ident(&mut self) -> String {
        let mut buf = String::new();
        while let Some(c) = self.peek() {
            if c.is_alphanumeric() || c == '_' || c == '.' {
                buf.push(c);
                self.advance();
            } else {
                break;
            }
        }
        buf
    }

    fn lex_string(&mut self) -> Result<String, LexError> {
        let line = self.line;
        let col = self.col;
        // Consume opening quote.
        self.advance();
        let mut buf = String::new();
        loop {
            match self.peek() {
                Some('"') => {
                    self.advance();
                    return Ok(buf);
                }
                Some('\\') => {
                    self.advance();
                    let escape_line = self.line;
                    let escape_col = self.col;
                    match self.advance() {
                        Some('n') => buf.push('\n'),
                        Some('t') => buf.push('\t'),
                        Some('r') => buf.push('\r'),
                        Some('"') => buf.push('"'),
                        Some('\\') => buf.push('\\'),
                        Some('/') => buf.push('/'),
                        // `\.` stays backslash-dot: the path splitter reads it as a literal dot (`"http\.method"`).
                        Some('.') => buf.push_str("\\."),
                        Some('u') => {
                            let ch = self.lex_unicode_escape(escape_line, escape_col)?;
                            buf.push(ch);
                        }
                        Some(c) => {
                            return Err(LexError::InvalidEscape {
                                ch: c,
                                line: escape_line,
                                col: escape_col,
                            })
                        }
                        None => return Err(LexError::UnterminatedString { line, col }),
                    }
                }
                Some(c) => {
                    buf.push(c);
                    self.advance();
                }
                None => return Err(LexError::UnterminatedString { line, col }),
            }
        }
    }

    /// The `{H..H}` after `\u` in a string: 1 to 6 hex digits naming a Unicode scalar value.
    fn lex_unicode_escape(&mut self, line: u32, col: u32) -> Result<char, LexError> {
        let bad = |reason: &str| LexError::InvalidUnicodeEscape {
            reason: reason.to_string(),
            line,
            col,
        };
        if self.peek() != Some('{') {
            return Err(bad("`\\u` must be followed by `{`, as in `\\u{1F}`"));
        }
        self.advance();
        let mut digits = String::new();
        loop {
            match self.peek() {
                Some('}') => {
                    self.advance();
                    break;
                }
                Some(c) if c.is_ascii_hexdigit() => {
                    digits.push(c);
                    self.advance();
                }
                Some(c) if c != '"' && c != '\n' => {
                    return Err(bad(&format!("`{c}` is not a hex digit")));
                }
                _ => return Err(bad("missing the closing `}`")),
            }
        }
        if digits.is_empty() {
            return Err(bad("no hex digits between `{` and `}`"));
        }
        if digits.len() > 6 {
            return Err(bad(&format!("`{digits}` has more than 6 hex digits")));
        }
        let code = u32::from_str_radix(&digits, 16).map_err(|_| bad("not a hex number"))?;
        char::from_u32(code).ok_or_else(|| {
            bad(&format!("U+{code:X} is not a Unicode scalar value (surrogates U+D800..U+DFFF and values above U+10FFFF are refused)"))
        })
    }

    fn lex_number(&mut self, line: u32, col: u32, negative: bool) -> Result<(), LexError> {
        let mut buf = String::new();
        if negative {
            buf.push('-');
        }
        let mut has_dot = false;
        while let Some(c) = self.peek() {
            if c.is_ascii_digit() {
                buf.push(c);
                self.advance();
            } else if c == '.' && !has_dot && self.peek_n(1).is_some_and(|d| d.is_ascii_digit()) {
                has_dot = true;
                buf.push('.');
                self.advance();
            } else {
                break;
            }
        }
        if has_dot {
            let value: f64 = buf.parse().map_err(|_| LexError::InvalidNumber {
                value: buf.clone(),
                line,
                col,
            })?;
            self.tokens.push(Token {
                kind: TokenKind::Number(value),
                span: Span::new(line, col, buf.len() as u16),
            });
        } else {
            let value: i64 = buf.parse().map_err(|_| LexError::InvalidNumber {
                value: buf.clone(),
                line,
                col,
            })?;
            self.tokens.push(Token {
                kind: TokenKind::Integer(value),
                span: Span::new(line, col, buf.len() as u16),
            });
        }
        Ok(())
    }

    fn lex_identifier_or_keyword(&mut self, line: u32, col: u32) {
        let mut buf = String::new();
        while let Some(c) = self.peek() {
            if c.is_alphanumeric() || c == '_' || c == '/' || c == '-' {
                buf.push(c);
                self.advance();
            } else {
                break;
            }
        }
        let len = buf.len() as u16;
        let kind = match buf.as_str() {
            "true" => TokenKind::Boolean(true),
            "false" => TokenKind::Boolean(false),
            "null" => TokenKind::Null,
            _ => TokenKind::Identifier(buf),
        };
        self.tokens.push(Token {
            kind,
            span: Span::new(line, col, len),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(src: &str) -> Vec<TokenKind> {
        tokenize(src)
            .unwrap()
            .into_iter()
            .map(|t| t.kind)
            .filter(|k| !matches!(k, TokenKind::Newline | TokenKind::Eof))
            .collect()
    }

    #[test]
    fn lex_punct() {
        assert_eq!(
            kinds("(){}[]:,|"),
            vec![
                TokenKind::LParen,
                TokenKind::RParen,
                TokenKind::LBrace,
                TokenKind::RBrace,
                TokenKind::LBracket,
                TokenKind::RBracket,
                TokenKind::Colon,
                TokenKind::Comma,
                TokenKind::Pipe,
            ]
        );
    }

    #[test]
    fn lex_keywords() {
        let toks = kinds("orbital entity trait state event from to emits");
        for t in &toks {
            assert!(matches!(t, TokenKind::Identifier(_)));
        }
    }

    #[test]
    fn lex_identifier() {
        assert_eq!(
            kinds("Order foo_bar baz-qux"),
            vec![
                TokenKind::Identifier("Order".to_string()),
                TokenKind::Identifier("foo_bar".to_string()),
                TokenKind::Identifier("baz-qux".to_string()),
            ]
        );
    }

    #[test]
    fn lex_namespaced_identifier() {
        assert_eq!(
            kinds("array/map math/clamp behavior/compose"),
            vec![
                TokenKind::Identifier("array/map".to_string()),
                TokenKind::Identifier("math/clamp".to_string()),
                TokenKind::Identifier("behavior/compose".to_string()),
            ]
        );
    }

    #[test]
    fn lex_sigil_self() {
        assert_eq!(
            kinds("@field @nested.path"),
            vec![
                TokenKind::Sigil("field".to_string()),
                TokenKind::Sigil("nested.path".to_string()),
            ]
        );
    }

    #[test]
    fn lex_payload_sigil() {
        assert_eq!(
            kinds("?id ?data.email"),
            vec![
                TokenKind::PayloadSigil("id".to_string()),
                TokenKind::PayloadSigil("data.email".to_string()),
            ]
        );
    }

    #[test]
    fn lex_string() {
        assert_eq!(
            kinds(r#""hello" "with\nesc" "quote\"""#),
            vec![
                TokenKind::String("hello".to_string()),
                TokenKind::String("with\nesc".to_string()),
                TokenKind::String("quote\"".to_string()),
            ]
        );
    }

    #[test]
    fn lex_number_int() {
        assert_eq!(
            kinds("42 -7 0"),
            vec![
                TokenKind::Integer(42),
                TokenKind::Integer(-7),
                TokenKind::Integer(0),
            ]
        );
    }

    #[test]
    fn lex_number_float() {
        assert_eq!(
            kinds("2.5 -0.5"),
            vec![TokenKind::Number(2.5), TokenKind::Number(-0.5)]
        );
    }

    #[test]
    fn lex_bool_null() {
        assert_eq!(
            kinds("true false null"),
            vec![
                TokenKind::Boolean(true),
                TokenKind::Boolean(false),
                TokenKind::Null
            ]
        );
    }

    #[test]
    fn lex_arrow() {
        assert_eq!(
            kinds("-> →"),
            vec![TokenKind::Arrow, TokenKind::UnicodeArrow]
        );
    }

    #[test]
    fn lex_comparison() {
        assert_eq!(
            kinds("== != < > <= >="),
            vec![
                TokenKind::Identifier("==".to_string()),
                TokenKind::Identifier("!=".to_string()),
                TokenKind::Identifier("<".to_string()),
                TokenKind::Identifier(">".to_string()),
                TokenKind::Identifier("<=".to_string()),
                TokenKind::Identifier(">=".to_string()),
            ]
        );
    }

    #[test]
    fn lex_arithmetic() {
        assert_eq!(
            kinds("+ - * / %"),
            vec![
                TokenKind::Identifier("+".to_string()),
                TokenKind::Identifier("-".to_string()),
                TokenKind::Identifier("*".to_string()),
                TokenKind::Identifier("/".to_string()),
                TokenKind::Identifier("%".to_string()),
            ]
        );
    }

    #[test]
    fn lex_line_comment_hash() {
        assert_eq!(
            kinds("# this is a comment\nfoo"),
            vec![TokenKind::Identifier("foo".to_string())]
        );
    }

    #[test]
    fn lex_line_comment_double_semi() {
        assert_eq!(
            kinds(";; lisp comment\nbar"),
            vec![TokenKind::Identifier("bar".to_string())]
        );
    }

    #[test]
    fn lex_block_comment() {
        assert_eq!(
            kinds("#= multi\nline =# foo"),
            vec![TokenKind::Identifier("foo".to_string())]
        );
    }

    #[test]
    fn lex_simple_sexpr() {
        let toks = kinds("(set @hp 100)");
        assert_eq!(
            toks,
            vec![
                TokenKind::LParen,
                TokenKind::Identifier("set".to_string()),
                TokenKind::Sigil("hp".to_string()),
                TokenKind::Integer(100),
                TokenKind::RParen,
            ]
        );
    }

    #[test]
    fn lex_nested_sexpr() {
        let toks = kinds("(>= @balance ?amount)");
        assert_eq!(
            toks,
            vec![
                TokenKind::LParen,
                TokenKind::Identifier(">=".to_string()),
                TokenKind::Sigil("balance".to_string()),
                TokenKind::PayloadSigil("amount".to_string()),
                TokenKind::RParen,
            ]
        );
    }

    #[test]
    fn lex_object_literal() {
        let toks = kinds("{ a: 1, b: \"x\" }");
        assert_eq!(
            toks,
            vec![
                TokenKind::LBrace,
                TokenKind::Identifier("a".to_string()),
                TokenKind::Colon,
                TokenKind::Integer(1),
                TokenKind::Comma,
                TokenKind::Identifier("b".to_string()),
                TokenKind::Colon,
                TokenKind::String("x".to_string()),
                TokenKind::RBrace,
            ]
        );
    }

    #[test]
    fn lex_array_literal() {
        let toks = kinds("[ 1, 2, 3 ]");
        assert_eq!(
            toks,
            vec![
                TokenKind::LBracket,
                TokenKind::Integer(1),
                TokenKind::Comma,
                TokenKind::Integer(2),
                TokenKind::Comma,
                TokenKind::Integer(3),
                TokenKind::RBracket,
            ]
        );
    }

    #[test]
    fn lex_state_block() {
        let toks = kinds("state pending\n  CONFIRM -> confirmed");
        assert_eq!(
            toks,
            vec![
                TokenKind::Identifier("state".to_string()),
                TokenKind::Identifier("pending".to_string()),
                TokenKind::Identifier("CONFIRM".to_string()),
                TokenKind::Arrow,
                TokenKind::Identifier("confirmed".to_string()),
            ]
        );
    }

    #[test]
    fn lex_required_field() {
        let toks = kinds("id : string!");
        assert_eq!(
            toks,
            vec![
                TokenKind::Identifier("id".to_string()),
                TokenKind::Colon,
                TokenKind::Identifier("string".to_string()),
                TokenKind::Bang,
            ]
        );
    }

    #[test]
    fn lex_unexpected_char() {
        assert!(matches!(
            tokenize("foo $ bar"),
            Err(LexError::UnexpectedChar { ch: '$', .. })
        ));
    }

    #[test]
    fn lex_double_colon() {
        assert_eq!(
            kinds("entity Patient :: Auditable"),
            vec![
                TokenKind::Identifier("entity".to_string()),
                TokenKind::Identifier("Patient".to_string()),
                TokenKind::DoubleColon,
                TokenKind::Identifier("Auditable".to_string()),
            ]
        );
    }

    #[test]
    fn lex_single_colon_unchanged() {
        // Single `:` must keep lexing as `Colon` so existing grammar
        // (`field : type`, object literals) is untouched.
        assert_eq!(
            kinds("id : string!"),
            vec![
                TokenKind::Identifier("id".to_string()),
                TokenKind::Colon,
                TokenKind::Identifier("string".to_string()),
                TokenKind::Bang,
            ]
        );
    }

    #[test]
    fn lex_ampersand() {
        assert_eq!(
            kinds("type Patient = Auditable & Entity"),
            vec![
                TokenKind::Identifier("type".to_string()),
                TokenKind::Identifier("Patient".to_string()),
                TokenKind::Equals,
                TokenKind::Identifier("Auditable".to_string()),
                TokenKind::Ampersand,
                TokenKind::Identifier("Entity".to_string()),
            ]
        );
    }

    #[test]
    fn lex_unterminated_string() {
        assert!(matches!(
            tokenize("\"unterminated"),
            Err(LexError::UnterminatedString { .. })
        ));
    }

    fn string_of(src: &str) -> Result<String, LexError> {
        tokenize(src).map(|ts| {
            ts.into_iter()
                .find_map(|t| match t.kind {
                    TokenKind::String(s) => Some(s),
                    _ => None,
                })
                .expect("a string token")
        })
    }

    #[test]
    fn unicode_escapes_lex_to_their_scalar_value() {
        assert_eq!(string_of(r#""a\u{1F}b""#).unwrap(), "a\u{1F}b");
        assert_eq!(string_of(r#""\u{0}""#).unwrap(), "\u{0}");
        assert_eq!(string_of(r#""\u{1F600}""#).unwrap(), "\u{1F600}");
        assert_eq!(string_of(r#""\u{10FFFF}\u{000041}\u{e9}""#).unwrap(), "\u{10FFFF}A\u{E9}");
        assert_eq!(string_of(r#""\\u{1F}""#).unwrap(), "\\u{1F}");
    }

    #[test]
    fn an_invalid_unicode_escape_is_a_clear_lex_error() {
        for (src, needle) in [
            (r#""\u1F""#, "must be followed by `{`"),
            (r#""\u""#, "must be followed by `{`"),
            (r#""\u{}""#, "no hex digits"),
            (r#""\u{1F""#, "closing `}`"),
            (r#""\u{1G}""#, "`G` is not a hex digit"),
            (r#""\u{1234567}""#, "more than 6 hex digits"),
            (r#""\u{D800}""#, "not a Unicode scalar value"),
            (r#""\u{110000}""#, "not a Unicode scalar value"),
        ] {
            let err = string_of(src).expect_err(src);
            assert!(matches!(err, LexError::InvalidUnicodeEscape { .. }), "{src}: {err:?}");
            assert!(err.to_string().contains(needle), "{src}: {err}");
        }
    }
}
