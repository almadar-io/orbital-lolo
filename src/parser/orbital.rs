//! Orbital-reference parsing (orbital-import Stage A) — the `.lolo` surface
//! of `docs/Almadar_Orbital_Import.md` §4.1: `orbital Name = Alias.orbitals.Y
//! { ... }`. Mirrors `parser::trait_::parse_trait_reference` /
//! `parse_trait_ref_body` field-for-field, one level up (orbital scope
//! instead of trait scope).

use super::*;

/// Parsed body of an orbital-reference call site (`{ entity fields pages
/// omit only config events }`). Produced by `Parser::parse_orbital_ref_body`
/// and folded into the `Orbital` AST node by its one caller,
/// `parse_orbital_reference`.
#[derive(Debug, Clone, Default)]
struct OrbitalRefBody {
    entity_rebind: Option<String>,
    fields_remap: Vec<(String, String)>,
    pages_remap: Vec<(String, String)>,
    omit: Vec<String>,
    only: Vec<String>,
    config_overrides: Vec<ConfigField>,
    events_remap: Vec<(String, String)>,
}

impl Parser {
    /// `orbital Name [:: ...] = Alias.orbitals.Upstream { ... }`. On entry
    /// `name`/`span`/`type_annotation` have already been consumed by
    /// `parse_orbital`; the next token is `=`.
    pub(crate) fn parse_orbital_reference(
        &mut self,
        name: String,
        span: Span,
        type_annotation: Option<TypeAnnotation>,
    ) -> Result<Orbital, ParseError> {
        // Consume the `=`.
        self.expect_kind(&TokenKind::Equals, "=")?;

        // Read the dotted reference path: `Alias.orbitals.Upstream`.
        let reference = self.parse_dotted_ident()?;

        let body = self.parse_orbital_ref_body()?;

        Ok(Orbital {
            name,
            type_annotation,
            uses: Vec::new(),
            expects: Vec::new(),
            type_aliases: Vec::new(),
            entity: None,
            extra_entities: Vec::new(),
            traits: Vec::new(),
            pages: Vec::new(),
            theme: None,
            body: Vec::new(),
            reference: Some(reference),
            entity_rebind: body.entity_rebind,
            fields_remap: body.fields_remap,
            pages_remap: body.pages_remap,
            omit: body.omit,
            only: body.only,
            config_overrides: body.config_overrides,
            events_remap: body.events_remap,
            // A reference-form orbital declares no knobs of its own — its
            // declared surface lives upstream; `config_overrides` above is
            // the override of THAT surface, not a second declaration.
            config: Vec::new(),
            span,
        })
    }

    /// Parse the body of an orbital-reference declaration. On entry the
    /// leading `{` has NOT been consumed (mandatory, unlike a bare trait
    /// reference's optional body — an orbital reference always requires
    /// `{ ... }`, empty allowed); on exit the closing `}` has been consumed.
    fn parse_orbital_ref_body(&mut self) -> Result<OrbitalRefBody, ParseError> {
        let body_open = self.peek_span();
        self.expect_kind(&TokenKind::LBrace, "{")?;
        self.skip_newlines();

        let mut out = OrbitalRefBody::default();

        loop {
            self.skip_newlines();
            if matches!(self.peek(), TokenKind::RBrace) {
                self.advance();
                break;
            }
            if self.at_eof() {
                return Err(ParseError::UnclosedDelimiter {
                    delimiter: '{',
                    closer: '}',
                    open_span: body_open,
                    found: "end of file".to_string(),
                });
            }
            if self.check_ident("entity") {
                self.advance();
                out.entity_rebind = Some(self.expect_ident()?);
                self.expect_newline_or_eof()?;
            } else if self.check_ident("roles") {
                // Stage B: role-vocabulary remap. The grammar must not
                // silently accept the design doc's own `roles {}` example
                // before the rewrite machinery (`check_comparison`'s walker)
                // exists — see docs' plan, W2-R task, and
                // Almadar_Orbital_Import.md §5.2.
                let roles_span = self.peek_span();
                return Err(ParseError::UnexpectedToken {
                    expected:
                        "entity, fields, pages, omit, only, config, events, or } — `roles {}` is not supported until Stage B of orbital import"
                            .to_string(),
                    got: "roles".to_string(),
                    span: roles_span,
                });
            } else if self.check_ident("fields") {
                out.fields_remap = self.parse_orbital_ref_name_map()?;
            } else if self.check_ident("events") {
                out.events_remap = self.parse_orbital_ref_name_map()?;
            } else if self.check_ident("pages") {
                out.pages_remap = self.parse_orbital_ref_string_map()?;
            } else if self.check_ident("omit") {
                out.omit = self.parse_orbital_ref_name_list()?;
            } else if self.check_ident("only") {
                out.only = self.parse_orbital_ref_name_list()?;
            } else if self.check_ident("config") {
                out.config_overrides = self.parse_config_override_block()?;
            } else {
                return Err(ParseError::UnexpectedToken {
                    expected: "entity, fields, pages, omit, only, config, events, or }"
                        .to_string(),
                    got: format!("{:?}", self.peek()),
                    span: self.peek_span(),
                });
            }
        }

        Ok(out)
    }

    /// `KEYWORD { OLD: NEW ... }` — a bare-identifier rename map, comma- or
    /// newline-separated. Shared shape of `fields {}` and `events {}` at
    /// orbital-reference scope; on entry the keyword itself has NOT been
    /// consumed.
    fn parse_orbital_ref_name_map(&mut self) -> Result<Vec<(String, String)>, ParseError> {
        self.advance(); // the `fields`/`events` keyword
        if matches!(self.peek(), TokenKind::Colon) {
            self.advance();
        }
        let open = self.peek_span();
        self.expect_kind(&TokenKind::LBrace, "{")?;
        let mut out = Vec::new();
        loop {
            self.skip_newlines();
            if matches!(self.peek(), TokenKind::RBrace) {
                self.advance();
                break;
            }
            if self.at_eof() {
                return Err(ParseError::UnclosedDelimiter {
                    delimiter: '{',
                    closer: '}',
                    open_span: open,
                    found: "end of file".to_string(),
                });
            }
            let from = self.expect_ident()?;
            self.expect_kind(&TokenKind::Colon, ":")?;
            let to = self.expect_ident()?;
            out.push((from, to));
            if matches!(self.peek(), TokenKind::Comma) {
                self.advance();
            }
            self.skip_newlines();
        }
        if matches!(self.peek(), TokenKind::Newline) {
            self.advance();
        }
        Ok(out)
    }

    /// `pages { "/up": "/local" ... }` — a string-literal path remap map,
    /// comma- or newline-separated. On entry the `pages` keyword has NOT
    /// been consumed.
    fn parse_orbital_ref_string_map(&mut self) -> Result<Vec<(String, String)>, ParseError> {
        self.advance(); // the `pages` keyword
        if matches!(self.peek(), TokenKind::Colon) {
            self.advance();
        }
        let open = self.peek_span();
        self.expect_kind(&TokenKind::LBrace, "{")?;
        let mut out = Vec::new();
        loop {
            self.skip_newlines();
            if matches!(self.peek(), TokenKind::RBrace) {
                self.advance();
                break;
            }
            if self.at_eof() {
                return Err(ParseError::UnclosedDelimiter {
                    delimiter: '{',
                    closer: '}',
                    open_span: open,
                    found: "end of file".to_string(),
                });
            }
            let from = self.expect_string_literal("a page path string")?;
            self.expect_kind(&TokenKind::Colon, ":")?;
            let to = self.expect_string_literal("a page path string")?;
            out.push((from, to));
            if matches!(self.peek(), TokenKind::Comma) {
                self.advance();
            }
            self.skip_newlines();
        }
        if matches!(self.peek(), TokenKind::Newline) {
            self.advance();
        }
        Ok(out)
    }

    /// `omit { A B ... }` / `omit { A, B }` — a bare-NAME list, accepting
    /// both newline- and comma-separated entries (uniform-comma consistency,
    /// same convention `page`'s trait list and the trait modifier bracket
    /// list already follow). Shared shape of `omit {}` and `only {}`; on
    /// entry the keyword has NOT been consumed.
    fn parse_orbital_ref_name_list(&mut self) -> Result<Vec<String>, ParseError> {
        self.advance(); // the `omit`/`only` keyword
        let open = self.peek_span();
        self.expect_kind(&TokenKind::LBrace, "{")?;
        let mut out = Vec::new();
        loop {
            self.skip_newlines();
            if matches!(self.peek(), TokenKind::RBrace) {
                self.advance();
                break;
            }
            if self.at_eof() {
                return Err(ParseError::UnclosedDelimiter {
                    delimiter: '{',
                    closer: '}',
                    open_span: open,
                    found: "end of file".to_string(),
                });
            }
            out.push(self.expect_ident()?);
            if matches!(self.peek(), TokenKind::Comma) {
                self.advance();
            }
            self.skip_newlines();
        }
        if matches!(self.peek(), TokenKind::Newline) {
            self.advance();
        }
        Ok(out)
    }

    /// Expect a string-literal token, with a caller-supplied description
    /// for the error message.
    fn expect_string_literal(&mut self, what: &str) -> Result<String, ParseError> {
        match self.peek().clone() {
            TokenKind::String(s) => {
                self.advance();
                Ok(s)
            }
            other => Err(ParseError::UnexpectedToken {
                expected: what.to_string(),
                got: format!("{:?}", other),
                span: self.peek_span(),
            }),
        }
    }
}
