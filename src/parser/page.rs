//! Page declaration parsing for the .lolo parser.

use super::*;

impl Parser {
    pub(crate) fn parse_page(&mut self) -> Result<Page, ParseError> {
        let span = self.peek_span();
        self.expect_keyword("page")?;
        // Path: stored as a string-like token. We accept either a literal
        // string or an identifier (for compatibility) — the test fixtures use
        // a quoted string, which lexes as TokenKind::String.
        let path = match self.peek().clone() {
            TokenKind::String(s) => {
                self.advance();
                s
            }
            TokenKind::Identifier(s) => {
                self.advance();
                s
            }
            _ => {
                return Err(ParseError::UnexpectedToken {
                    expected: "page path".to_string(),
                    got: format!("{:?}", self.peek()),
                    span: self.peek_span(),
                })
            }
        };
        // Optional `:: TypeName [args]` annotation between path and the
        // reference / traits / options sections.
        let type_annotation = self.try_parse_type_annotation()?;
        // Optional `as PageName` explicit name override.
        let name = if self.check_ident("as") {
            self.advance();
            Some(self.expect_ident()?)
        } else {
            None
        };
        // Optional `= Alias.pages.Upstream` reference form. The reference
        // turns this declaration into a `PageRef::Object` at lowering time;
        // any subsequent `-> Trait, Trait...` becomes the traits override.
        let reference = if matches!(self.peek(), TokenKind::Equals) {
            self.advance();
            Some(self.parse_dotted_ident()?)
        } else {
            None
        };
        // `-> Trait[, Trait...]`
        let mut traits = Vec::new();
        if matches!(self.peek(), TokenKind::Arrow | TokenKind::UnicodeArrow) {
            self.advance();
            traits.push(self.expect_ident()?);
            // Optional commas between page traits: `-> A, B` and `-> A B` both
            // parse (uniform-comma consistency — see docs/Almadar_LOLO_Audit.md).
            // No newline skip, so the list still ends at end-of-line / `[`.
            loop {
                if matches!(self.peek(), TokenKind::Comma) {
                    self.advance();
                }
                if matches!(self.peek(), TokenKind::Identifier(_)) {
                    traits.push(self.expect_ident()?);
                } else {
                    break;
                }
            }
        }
        // Optional `[options]`.
        let mut view = None;
        let mut primary_entity = None;
        let mut is_initial = false;
        if matches!(self.peek(), TokenKind::LBracket) {
            self.advance();
            loop {
                self.skip_newlines();
                if matches!(self.peek(), TokenKind::RBracket) {
                    self.advance();
                    break;
                }
                let key = self.expect_ident()?;
                if matches!(self.peek(), TokenKind::Colon) {
                    self.advance();
                    let value = self.expect_ident()?;
                    match key.as_str() {
                        "view" => view = Some(value),
                        "entity" => primary_entity = Some(value),
                        _ => {}
                    }
                } else if key == "initial" {
                    is_initial = true;
                }
                if matches!(self.peek(), TokenKind::Comma) {
                    self.advance();
                }
            }
        }
        // Trailing `@icon "..."` / `@label "..."` annotations (one or more
        // indented lines below the page declaration). Mirrors the config-knob
        // annotation reader (`parse_config_annotations`).
        let mut icon = None;
        let mut label = None;
        loop {
            self.skip_newlines();
            let annotation = match self.peek().clone() {
                TokenKind::Sigil(raw) => {
                    let s = self.canon_annotation_sigil(&raw);
                    if s == "icon" || s == "label" {
                        s
                    } else {
                        break;
                    }
                }
                _ => break,
            };
            self.advance();
            let value = match self.peek().clone() {
                TokenKind::String(s) => {
                    self.advance();
                    s
                }
                other => {
                    return Err(ParseError::UnexpectedToken {
                        expected: format!("string literal after @{}", annotation),
                        got: format!("{:?}", other),
                        span: self.peek_span(),
                    });
                }
            };
            match annotation.as_str() {
                "icon" => icon = Some(value),
                "label" => label = Some(value),
                _ => unreachable!(),
            }
        }
        Ok(Page {
            path,
            type_annotation,
            traits,
            view,
            primary_entity,
            is_initial,
            span,
            reference,
            name,
            icon,
            label,
        })
    }
}
