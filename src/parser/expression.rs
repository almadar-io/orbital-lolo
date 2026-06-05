//! S-expression / object-literal / array-literal / duration parsing for the
//! .lolo parser.

use super::*;

impl Parser {
    pub(crate) fn parse_sexpr(&mut self) -> Result<SExpression, ParseError> {
        let span = self.peek_span();
        self.expect_kind(&TokenKind::LParen, "(")?;
        if matches!(self.peek(), TokenKind::RParen) {
            self.advance();
            return Ok(SExpression::Array(Vec::new()));
        }
        // First element is the operator. Accept any identifier plus the
        // single-char comparison token `=` (tokenized as `Equals`; used in
        // legacy pre-V2 `.lolo` files as the Lisp-style equality operator).
        let op = match self.peek().clone() {
            TokenKind::Identifier(name) => {
                self.advance();
                SExpression::String(name)
            }
            TokenKind::Equals => {
                self.advance();
                SExpression::String("=".to_string())
            }
            other => {
                return Err(ParseError::InvalidSExpr {
                    message: format!("expected operator, got {other:?}"),
                    span,
                })
            }
        };
        let mut items = vec![op];
        // Special case: `(let ((x v) (y w)) body)` — the second arg is a list of bindings.
        let is_let = matches!(items.first(), Some(SExpression::String(s)) if s == "let");
        if is_let && matches!(self.peek(), TokenKind::LParen) {
            // Parse the binding list.
            let bindings = self.parse_let_bindings()?;
            items.push(bindings);
        }
        loop {
            self.skip_newlines();
            if matches!(self.peek(), TokenKind::RParen) {
                self.advance();
                break;
            }
            let arg = self.parse_sexpr_or_value()?;
            items.push(arg);
        }
        Ok(SExpression::Array(items))
    }

    pub(crate) fn parse_let_bindings(&mut self) -> Result<SExpression, ParseError> {
        // `((x v) (y w))` — outer list of inner pairs.
        self.expect_kind(&TokenKind::LParen, "(")?;
        let mut pairs = Vec::new();
        loop {
            self.skip_newlines();
            if matches!(self.peek(), TokenKind::RParen) {
                self.advance();
                break;
            }
            self.expect_kind(&TokenKind::LParen, "(")?;
            // name
            let name = match self.peek().clone() {
                TokenKind::Identifier(n) => {
                    self.advance();
                    SExpression::String(n)
                }
                other => {
                    return Err(ParseError::InvalidSExpr {
                        message: format!("let binding name expected, got {other:?}"),
                        span: self.peek_span(),
                    })
                }
            };
            let value = self.parse_sexpr_or_value()?;
            self.expect_kind(&TokenKind::RParen, ")")?;
            pairs.push(SExpression::Array(vec![name, value]));
        }
        Ok(SExpression::Array(pairs))
    }

    /// Parse one s-expression argument: an atom, a paren list, an object literal, or an array literal.
    pub(crate) fn parse_sexpr_or_value(&mut self) -> Result<SExpression, ParseError> {
        match self.peek().clone() {
            TokenKind::LParen => self.parse_sexpr(),
            TokenKind::LBrace => self.parse_object_literal(),
            TokenKind::LBracket => self.parse_array_literal(),
            TokenKind::String(s) => {
                self.advance();
                Ok(SExpression::String(s))
            }
            TokenKind::Integer(n) => {
                self.advance();
                Ok(SExpression::Number(n as f64))
            }
            TokenKind::Number(n) => {
                self.advance();
                Ok(SExpression::Number(n))
            }
            TokenKind::Boolean(b) => {
                self.advance();
                Ok(SExpression::Bool(b))
            }
            TokenKind::Null => {
                self.advance();
                Ok(SExpression::Null)
            }
            TokenKind::Sigil(name) => {
                self.advance();
                Ok(SExpression::String(format!("@{}", lower_self_sigil(&name))))
            }
            TokenKind::PayloadSigil(name) => {
                self.advance();
                if name.is_empty() {
                    Ok(SExpression::String("@payload".to_string()))
                } else {
                    Ok(SExpression::String(format!("@payload.{name}")))
                }
            }
            TokenKind::Identifier(name) => {
                self.advance();
                // Bare identifiers in argument position are atoms (event names,
                // entity names, slot names, mode names). Special atoms `now`
                // and `state` lower to bindings.
                let atom = match name.as_str() {
                    "now" => "@now".to_string(),
                    "state" => "@state".to_string(),
                    _ => name,
                };
                Ok(SExpression::String(atom))
            }
            other => Err(ParseError::InvalidSExpr {
                message: format!("unexpected token in s-expression: {other:?}"),
                span: self.peek_span(),
            }),
        }
    }

    pub(crate) fn parse_object_literal(&mut self) -> Result<SExpression, ParseError> {
        self.expect_kind(&TokenKind::LBrace, "{")?;
        let mut map: HashMap<String, SExpression> = HashMap::new();
        loop {
            self.skip_newlines();
            if matches!(self.peek(), TokenKind::RBrace) {
                self.advance();
                break;
            }
            // Key: identifier or string.
            let key = match self.peek().clone() {
                TokenKind::Identifier(s) => {
                    self.advance();
                    s
                }
                TokenKind::String(s) => {
                    self.advance();
                    s
                }
                other => {
                    return Err(ParseError::InvalidSExpr {
                        message: format!("object key expected, got {other:?}"),
                        span: self.peek_span(),
                    })
                }
            };
            self.expect_kind(&TokenKind::Colon, ":")?;
            let value = self.parse_sexpr_or_value()?;
            map.insert(key, value);
            if matches!(self.peek(), TokenKind::Comma) {
                self.advance();
            }
            self.skip_newlines();
        }
        Ok(SExpression::Object(map))
    }

    pub(crate) fn parse_array_literal(&mut self) -> Result<SExpression, ParseError> {
        self.expect_kind(&TokenKind::LBracket, "[")?;
        let mut items = Vec::new();
        loop {
            self.skip_newlines();
            if matches!(self.peek(), TokenKind::RBracket) {
                self.advance();
                break;
            }
            let value = self.parse_sexpr_or_value()?;
            items.push(value);
            if matches!(self.peek(), TokenKind::Comma) {
                self.advance();
            }
            self.skip_newlines();
        }
        Ok(SExpression::Array(items))
    }

    pub(crate) fn parse_duration_ms(&mut self) -> Result<u64, ParseError> {
        let span = self.peek_span();
        let n = match self.peek().clone() {
            TokenKind::Integer(n) => {
                self.advance();
                n as u64
            }
            other => {
                return Err(ParseError::UnexpectedToken {
                    expected: "duration number".to_string(),
                    got: format!("{other:?}"),
                    span,
                })
            }
        };
        let suf = match self.peek().clone() {
            TokenKind::Identifier(s) if matches!(s.as_str(), "ms" | "s" | "m" | "h" | "d") => {
                self.advance();
                s
            }
            _ => {
                return Err(ParseError::InvalidDuration {
                    value: n.to_string(),
                    span,
                })
            }
        };
        Ok(duration_to_ms(n, &suf))
    }
}

/// Pass a self-sigil through to its `.orb` JSON form. Every `@<name>`
/// roots its own binding namespace — there is NO implicit `@<name>` →
/// `@entity.<name>` rewrite. Schema authors who want an entity field
/// write the qualifier explicitly (`@entity.title`), and lambda
/// parameters (`["fn", "metric", body]`) keep their bare names so
/// references like `@metric.label` resolve to the param at evaluation
/// time. The handful of well-known roots (`entity`, `now`, `state`,
/// `payload`, `config`, …) are preserved unchanged here for clarity;
/// every other name flows through verbatim so the runtime / codegen
/// can decide what to do with it (lambda binding, user-defined trait
/// scope, payload field, …).
pub(crate) fn lower_self_sigil(name: &str) -> String {
    if name.is_empty() {
        return "entity".to_string();
    }
    name.to_string()
}
