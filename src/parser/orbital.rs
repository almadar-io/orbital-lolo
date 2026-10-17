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
    entity_rebind_tags: Option<EntityRebindTags>,
    entities_remap: Vec<(String, String, Span)>,
    fields_remap: Vec<(String, String, Span)>,
    roles_remap: Vec<(String, Vec<String>, Span)>,
    pages_remap: Vec<(String, String, Span)>,
    page_modifiers: Vec<(String, orbital_core_base::schema::PageModifiers)>,
    mounts: Vec<(String, Vec<String>, Span)>,
    omit: Vec<(String, Span)>,
    only: Vec<String>,
    config_overrides: Vec<ConfigField>,
    events_remap: Vec<(String, String)>,
    extend_fields: Vec<EntityField>,
    mock_overrides: Vec<(String, String, Span)>,
    retype_fields: Vec<EntityField>,
    trait_overrides: Vec<TraitOverride>,
    import_listens: Vec<Listen>,
    expects: Vec<ExpectsDecl>,
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
        os_grants: orbital_core_base::os_grants::OsGrants,
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
        // No pages here to attribute against — just keep the producer
        // tracking vector aligned with `pending_inline_traits` for the next
        // orbital in the file.
        self.pending_inline_producers.clear();

        Ok(Orbital {
            name,
            type_annotation,
            uses: Vec::new(),
            expects: body.expects,
            type_aliases: Vec::new(),
            entity: None,
            extra_entities: Vec::new(),
            traits: sibling_traits,
            pages: Vec::new(),
            theme: None,
            body: Vec::new(),
            reference: Some(reference),
            entity_rebind: body.entity_rebind,
            entity_rebind_tags: body.entity_rebind_tags,
            fields_remap: body.fields_remap,
            pages_remap: body.pages_remap,
            page_modifiers: body.page_modifiers,
            omit: body.omit,
            only: body.only,
            config_overrides: body.config_overrides,
            events_remap: body.events_remap,
            roles_remap: body.roles_remap,
            entities_remap: body.entities_remap,
            mounts: body.mounts,
            extend_fields: body.extend_fields,
            mock_overrides: body.mock_overrides,
            retype_fields: body.retype_fields,
            trait_overrides: body.trait_overrides,
            import_listens: body.import_listens,
            // A reference-form orbital declares no knobs of its own — its
            // declared surface lives upstream; `config_overrides` above is
            // the override of THAT surface, not a second declaration.
            config: Vec::new(),
            os_grants,
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
                if !matches!(self.peek(), TokenKind::LBracket) {
                    out.entity_rebind = Some(self.expect_ident()?);
                }
                if matches!(self.peek(), TokenKind::LBracket) {
                    let tags = self.parse_persistence_tag()?;
                    out.entity_rebind_tags = Some(EntityRebindTags {
                        persistence: tags.persistence,
                        collection: tags.collection,
                        shared: tags.shared,
                        identity: tags.identity,
                        local: tags.local,
                        mock: tags.mock,
                    });
                }
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
                let (pages_remap, mounts, page_modifiers) = self.parse_orbital_ref_pages_map()?;
                out.pages_remap = pages_remap;
                out.mounts = mounts;
                out.page_modifiers = page_modifiers;
            } else if self.check_ident("expects") {
                let decl = self.parse_expects_decl()?;
                if let Some(dup) = super::expects_duplicate(&out.expects, &decl) {
                    return Err(ParseError::InvalidType {
                        span: decl.span,
                        message: format!(
                            "ELOLO_DUPLICATE_EXPECTATION: orbital declares {dup} more than once"
                        ),
                    });
                }
                out.expects.push(decl);
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
            } else if self.check_ident("mock") {
                out.mock_overrides = self.parse_orbital_ref_mock_map()?;
            } else if self.check_ident("retype") {
                out.retype_fields = self.parse_orbital_ref_extend_block()?;
            } else if self.check_ident("traits") {
                out.trait_overrides = self.parse_orbital_ref_traits_block()?;
            } else if self.check_ident("listens") {
                for l in self.parse_listens_block()? {
                    if l.target_trait.is_none() {
                        return Err(ParseError::UnexpectedToken {
                            expected: "`Source.EVENT -> ImportedTrait.TRIGGER` (an orbital import's listens route into one of its imported traits)".to_string(),
                            got: l.event.clone(),
                            span: l.span,
                        });
                    }
                    out.import_listens.push(l);
                }
            } else if self.check_ident("trait") {
                // Stage B sibling trait: legal inside the reference body,
                // never prefixed, `omit`/`only` never apply to it. Lowers
                // through the same path an inline orbital's own traits take.
                out.sibling_traits.push(self.parse_trait()?);
            } else {
                return Err(ParseError::UnexpectedToken {
                    expected:
                        "entity, entities, fields, roles, pages, omit, only, config, events, extend, retype, mock, traits, listens, expects, trait, or }"
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

    /// `mock { role: "customer, staff"  name: "Maya Chen, Tom Becker" }` — a
    /// field name → one string literal (the `@mock` sample list), newline- or
    /// comma-separated entries; a field named twice is a parse error. On entry
    /// the `mock` keyword has NOT been consumed.
    fn parse_orbital_ref_mock_map(&mut self) -> Result<Vec<(String, String, Span)>, ParseError> {
        self.advance(); // the `mock` keyword
        let open = self.peek_span();
        self.expect_kind(&TokenKind::LBrace, "{")?;
        let mut out: Vec<(String, String, Span)> = Vec::new();
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
            let field = self.expect_ident()?;
            if out.iter().any(|(f, _, _)| f == &field) {
                return Err(ParseError::InvalidType {
                    span: key_span,
                    message: format!("ELOLO_DUPLICATE_MOCK_FIELD: mock {{}} names `{field}` more than once"),
                });
            }
            self.expect_kind(&TokenKind::Colon, ":")?;
            let TokenKind::String(samples) = self.peek().clone() else {
                return Err(ParseError::UnexpectedToken {
                    expected: "a string of comma-separated samples".to_string(),
                    got: format!("{:?}", self.peek()),
                    span: self.peek_span(),
                });
            };
            self.advance();
            out.push((field, samples, key_span));
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

    /// `pages { "/up": "/local" [-> TraitA, TraitB] [[access: …, …]] ... }` — a
    /// string-literal path remap map with an optional mount tail (Stage B)
    /// naming sibling trait names to mount on that (upstream-keyed) page, then
    /// an optional page modifier bracket, comma- or newline-separated. On entry
    /// the `pages` keyword has NOT been consumed. Returns `(pages_remap,
    /// mounts, page_modifiers)`.
    #[allow(clippy::type_complexity)]
    fn parse_orbital_ref_pages_map(
        &mut self,
    ) -> Result<
        (
            Vec<(String, String, Span)>,
            Vec<(String, Vec<String>, Span)>,
            Vec<(String, orbital_core_base::schema::PageModifiers)>,
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
        let mut page_modifiers = Vec::new();
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
            if matches!(self.peek(), TokenKind::LBracket) {
                let mut mods = orbital_core_base::schema::PageModifiers::default();
                self.parse_modifier_bracket(
                    "pages remap entry",
                    super::REMAP_PAGE_MODIFIERS,
                    |p, key| {
                        p.expect_kind(&TokenKind::Colon, "':' after a page modifier")?;
                        p.parse_head_modifier(key, &mut mods)
                    },
                )?;
                page_modifiers.push((from.clone(), mods));
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
        Ok((out, mounts, page_modifiers))
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
            // A quoted entry names a page by its path (`omit { "/things/:id" }`).
            if let TokenKind::String(path) = self.peek().clone() {
                self.advance();
                out.push((path, span));
            } else {
                out.push((self.expect_ident()?, span));
            }
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

    /// `traits { UpstreamTrait [-> Entity] { <trait-reference body> } ... }` — per-imported-trait
    /// overrides keyed by the UPSTREAM trait name; each body is exactly the §8 trait-reference
    /// body (`parse_trait_ref_body`). `name:` is not an override here (imports are renamed by
    /// the unconditional prefix). On entry the `traits` keyword has NOT been consumed.
    fn parse_orbital_ref_traits_block(&mut self) -> Result<Vec<TraitOverride>, ParseError> {
        self.advance(); // the `traits` keyword
        let open = self.peek_span();
        self.expect_kind(&TokenKind::LBrace, "{")?;
        let mut out: Vec<TraitOverride> = Vec::new();
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
            let trait_name = self.expect_ident()?;
            if out.iter().any(|t| t.trait_name == trait_name) {
                return Err(ParseError::InvalidType {
                    span,
                    message: format!(
                        "ELOLO_DUPLICATE_TRAIT_OVERRIDE: traits {{}} names `{trait_name}` more than once"
                    ),
                });
            }
            let linked_entity = if matches!(self.peek(), TokenKind::Arrow | TokenKind::UnicodeArrow)
            {
                self.advance();
                Some(self.expect_ident()?)
            } else {
                None
            };
            let name_span = self.peek_span();
            let body = self.parse_trait_ref_body()?;
            if body.name_override.is_some() {
                return Err(ParseError::InvalidType {
                    span: name_span,
                    message: format!(
                        "ELOLO_TRAIT_OVERRIDE_NAME: `name:` is not an override of traits {{ {trait_name} }} — an imported trait is renamed by the import prefix"
                    ),
                });
            }
            out.push(TraitOverride {
                trait_name,
                linked_entity,
                body,
                span,
            });
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
