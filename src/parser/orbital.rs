//! Orbital-reference parsing (orbital-import Stage A + B) — the `.lolo`
//! surface of `docs/Almadar_Orbital_Import.md` §4.1: `orbital Name =
//! Alias.orbitals.Y { ... }`. Mirrors `parser::trait_::parse_trait_reference` /
//! `parse_trait_ref_body` field-for-field, one level up (orbital scope
//! instead of trait scope). Stage B adds `entities {}` (out-of-orbital
//! entity remap), `roles {}` (role-vocabulary remap), a `pages {}` mount
//! tail (`-> TraitA, TraitB`), and sibling `trait` declarations.

use super::*;

/// Parsed body of an orbital-reference call site (`{ entity entities fields
/// roles pages omit only config events extend trait }`, Stage B + C).
/// Produced by `Parser::parse_orbital_ref_body` and folded into the
/// `Orbital` AST node by its one caller, `parse_orbital_reference`.
#[derive(Debug, Clone, Default)]
struct OrbitalRefBody {
    entity_rebind: Option<String>,
    entities_remap: Vec<(String, String, Span)>,
    fields_remap: Vec<(String, String, Span)>,
    roles_remap: Vec<(String, Vec<String>, Span)>,
    pages_remap: Vec<(String, String, Span)>,
    mounts: Vec<(String, Vec<String>, Span)>,
    omit: Vec<(String, Span)>,
    only: Vec<String>,
    config_overrides: Vec<ConfigField>,
    events_remap: Vec<(String, String)>,
    extend_fields: Vec<EntityField>,
    sibling_traits: Vec<Trait>,
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

        // Flush inline `@(…)` references collected while parsing sibling
        // trait bodies (render-ui children / config values) — the same
        // hoist `parse_orbital` runs for an inline orbital's own traits, so
        // a sibling's inline refs become real appended sibling traits too.
        // No page auto-registration here: a reference-form orbital owns no
        // local `Page` list — mounts are the explicit sibling-mount surface.
        let mut sibling_traits = body.sibling_traits;
        sibling_traits.extend(std::mem::take(&mut self.pending_inline_traits));

        Ok(Orbital {
            name,
            type_annotation,
            uses: Vec::new(),
            expects: Vec::new(),
            type_aliases: Vec::new(),
            entity: None,
            extra_entities: Vec::new(),
            traits: sibling_traits,
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
            roles_remap: body.roles_remap,
            entities_remap: body.entities_remap,
            mounts: body.mounts,
            extend_fields: body.extend_fields,
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
            } else if self.check_ident("entities") {
                out.entities_remap = self.parse_orbital_ref_name_map()?;
            } else if self.check_ident("fields") {
                out.fields_remap = self.parse_orbital_ref_name_map()?;
            } else if self.check_ident("roles") {
                out.roles_remap = self.parse_orbital_ref_roles_map()?;
            } else if self.check_ident("events") {
                out.events_remap = self
                    .parse_orbital_ref_name_map()?
                    .into_iter()
                    .map(|(from, to, _)| (from, to))
                    .collect();
            } else if self.check_ident("pages") {
                let (pages_remap, mounts) = self.parse_orbital_ref_pages_map()?;
                out.pages_remap = pages_remap;
                out.mounts = mounts;
            } else if self.check_ident("omit") {
                out.omit = self.parse_orbital_ref_name_list()?;
            } else if self.check_ident("only") {
                out.only = self
                    .parse_orbital_ref_name_list()?
                    .into_iter()
                    .map(|(name, _)| name)
                    .collect();
            } else if self.check_ident("config") {
                out.config_overrides = self.parse_config_override_block()?;
            } else if self.check_ident("extend") {
                out.extend_fields = self.parse_orbital_ref_extend_block()?;
            } else if self.check_ident("trait") {
                // Stage B sibling trait: legal inside the reference body,
                // never prefixed, `omit`/`only` never apply to it. Lowers
                // through the same path an inline orbital's own traits take.
                out.sibling_traits.push(self.parse_trait()?);
            } else {
                return Err(ParseError::UnexpectedToken {
                    expected:
                        "entity, entities, fields, roles, pages, omit, only, config, events, extend, trait, or }"
                            .to_string(),
                    got: format!("{:?}", self.peek()),
                    span: self.peek_span(),
                });
            }
        }

        Ok(out)
    }

    /// `KEYWORD { OLD: NEW ... }` — a bare-identifier rename map, comma- or
    /// newline-separated. Shared shape of `fields {}`, `entities {}` and
    /// `events {}` at orbital-reference scope; on entry the keyword itself
    /// has NOT been consumed. The third tuple element is the KEY token's
    /// span (`.lolo` spans, Stage C) — callers that don't need it (`events
    /// {}`) drop it.
    fn parse_orbital_ref_name_map(&mut self) -> Result<Vec<(String, String, Span)>, ParseError> {
        self.advance(); // the `fields`/`entities`/`events` keyword
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
            let key_span = self.peek_span();
            let from = self.expect_ident()?;
            self.expect_kind(&TokenKind::Colon, ":")?;
            let to = self.expect_ident()?;
            out.push((from, to, key_span));
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

    /// `roles { approver: [owner, project_manager]  employee: team_member }`
    /// (Stage B) — upstream role name → one bare NAME or a `[NAME, ...]`
    /// list, newline- or comma-separated entries. On entry the `roles`
    /// keyword has NOT been consumed.
    fn parse_orbital_ref_roles_map(
        &mut self,
    ) -> Result<Vec<(String, Vec<String>, Span)>, ParseError> {
        self.advance(); // the `roles` keyword
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
            let key_span = self.peek_span();
            let from = self.expect_ident()?;
            self.expect_kind(&TokenKind::Colon, ":")?;
            let targets = if matches!(self.peek(), TokenKind::LBracket) {
                self.advance();
                let mut names = Vec::new();
                loop {
                    self.skip_newlines();
                    if matches!(self.peek(), TokenKind::RBracket) {
                        self.advance();
                        break;
                    }
                    names.push(self.expect_ident()?);
                    if matches!(self.peek(), TokenKind::Comma) {
                        self.advance();
                        self.skip_newlines();
                    }
                }
                names
            } else {
                vec![self.expect_ident()?]
            };
            out.push((from, targets, key_span));
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

    /// `pages { "/up": "/local" [-> TraitA, TraitB] ... }` — a string-literal
    /// path remap map with an optional mount tail (Stage B) naming sibling
    /// trait names to mount on that (upstream-keyed) page, comma- or
    /// newline-separated. On entry the `pages` keyword has NOT been consumed.
    /// Returns `(pages_remap, mounts)`.
    fn parse_orbital_ref_pages_map(
        &mut self,
    ) -> Result<
        (
            Vec<(String, String, Span)>,
            Vec<(String, Vec<String>, Span)>,
        ),
        ParseError,
    > {
        self.advance(); // the `pages` keyword
        if matches!(self.peek(), TokenKind::Colon) {
            self.advance();
        }
        let open = self.peek_span();
        self.expect_kind(&TokenKind::LBrace, "{")?;
        let mut out = Vec::new();
        let mut mounts = Vec::new();
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
            let key_span = self.peek_span();
            let from = self.expect_string_literal("a page path string")?;
            self.expect_kind(&TokenKind::Colon, ":")?;
            let to = self.expect_string_literal("a page path string")?;
            if matches!(self.peek(), TokenKind::Arrow | TokenKind::UnicodeArrow) {
                self.advance();
                let mut mount_traits = Vec::new();
                loop {
                    mount_traits.push(self.expect_ident()?);
                    if matches!(self.peek(), TokenKind::Comma) {
                        self.advance();
                    } else {
                        break;
                    }
                }
                mounts.push((from.clone(), mount_traits, key_span));
            }
            out.push((from, to, key_span));
            if matches!(self.peek(), TokenKind::Comma) {
                self.advance();
            }
            self.skip_newlines();
        }
        if matches!(self.peek(), TokenKind::Newline) {
            self.advance();
        }
        Ok((out, mounts))
    }

    /// `omit { A B ... }` / `omit { A, B }` — a bare-NAME list, accepting
    /// both newline- and comma-separated entries (uniform-comma consistency,
    /// same convention `page`'s trait list and the trait modifier bracket
    /// list already follow). Shared shape of `omit {}` and `only {}`; on
    /// entry the keyword has NOT been consumed. Second tuple element: the
    /// name token's span (`only {}` drops it — no span-keyed diagnostic
    /// reads it today).
    fn parse_orbital_ref_name_list(&mut self) -> Result<Vec<(String, Span)>, ParseError> {
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
            let span = self.peek_span();
            out.push((self.expect_ident()?, span));
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

    /// `extend { <field-decl>+ }` (Stage C) — full entity-field syntax,
    /// reusing `parse_field_decl` (the same shape an `entity { … }` body's
    /// fields use). Appended to the primary entity at materialization,
    /// AFTER `fields {}` renames apply. On entry the `extend` keyword has
    /// NOT been consumed.
    fn parse_orbital_ref_extend_block(&mut self) -> Result<Vec<EntityField>, ParseError> {
        self.advance(); // the `extend` keyword
        let open = self.peek_span();
        self.expect_kind(&TokenKind::LBrace, "{")?;
        self.skip_newlines();
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
            out.push(self.parse_field_decl()?);
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
