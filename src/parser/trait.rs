//! Trait / trait-reference / state / transition / emits / listens / ticks /
//! config parsing for the .lolo parser.

use super::*;

impl Parser {
    pub(crate) fn parse_trait(&mut self) -> Result<Trait, ParseError> {
        let span = self.peek_span();
        self.expect_keyword("trait")?;
        let name = self.expect_ident()?;

        // Optional `:: TypeName [args]` annotation above the trait body.
        let type_annotation = self.try_parse_type_annotation()?;

        // Reference form: `trait Name [:: ...] = Alias.traits.Upstream [-> Entity] { ... }`.
        // The `=` immediately after the name/annotation disambiguates from the
        // inline form, which uses `->` for the linkedEntity binding.
        if matches!(self.peek(), TokenKind::Equals) {
            return self.parse_trait_reference(name, span, type_annotation);
        }

        // Optional `-> [@rebindable] Entity`. `@rebindable` between the
        // arrow and the entity opts the trait into consumer rebinding via
        // `linkedEntity`; without it the binding is fixed.
        let mut entity_rebindable = false;
        let linked_entity = if matches!(self.peek(), TokenKind::Arrow | TokenKind::UnicodeArrow) {
            self.advance();
            if matches!(self.peek(), TokenKind::Sigil(ref s) if s == "rebindable") {
                self.advance();
                entity_rebindable = true;
            }
            Some(self.expect_ident()?)
        } else {
            None
        };
        // Optional `[category]` or `[category, scope, ...capabilities]`
        // modifier list. Before the V2 migration this was a single
        // identifier (the category). It then grew to accept the trait scope
        // modifier (`instance` | `collection`, Almadar_Entity_V2_Plan.md
        // §P0.3). Phase 3 of `Almadar_Domain_Language.md` adds capability
        // tags: any identifier beyond the scope marker and the first
        // (category) slot accumulates into `capabilities`. First non-scope
        // identifier still wins as the category.
        let (category, scope, capabilities) = if matches!(self.peek(), TokenKind::LBracket) {
            self.advance();
            let mut category: Option<String> = None;
            let mut scope: Option<TraitScope> = None;
            let mut capabilities: Vec<String> = Vec::new();
            loop {
                self.skip_newlines();
                if matches!(self.peek(), TokenKind::RBracket) {
                    self.advance();
                    break;
                }
                let ident = self.expect_ident()?;
                match ident.as_str() {
                    "instance" => {
                        if scope.is_some() {
                            return Err(ParseError::UnexpectedToken {
                                expected:
                                    "at most one of `instance` | `collection` in trait modifiers"
                                        .to_string(),
                                got: "duplicate scope marker".to_string(),
                                span: self.peek_span(),
                            });
                        }
                        scope = Some(TraitScope::Instance);
                    }
                    "collection" => {
                        if scope.is_some() {
                            return Err(ParseError::UnexpectedToken {
                                expected:
                                    "at most one of `instance` | `collection` in trait modifiers"
                                        .to_string(),
                                got: "duplicate scope marker".to_string(),
                                span: self.peek_span(),
                            });
                        }
                        scope = Some(TraitScope::Collection);
                    }
                    _ => {
                        if category.is_none() {
                            category = Some(ident);
                        } else {
                            capabilities.push(ident);
                        }
                    }
                }
                if matches!(self.peek(), TokenKind::Comma) {
                    self.advance();
                }
                self.skip_newlines();
            }
            (category, scope, capabilities)
        } else {
            (None, None, Vec::new())
        };

        let brace_form = matches!(self.peek(), TokenKind::LBrace);
        if brace_form {
            self.advance();
        }
        self.skip_newlines();

        let mut initial_state = None;
        let mut states = Vec::new();
        let mut emits = Vec::new();
        let mut listens = Vec::new();
        let mut ticks = Vec::new();
        let mut config = Vec::new();
        let mut entity_binding_description: Option<String> = None;
        let mut entity_binding_synonyms: Option<String> = None;

        loop {
            self.skip_newlines();
            if brace_form && matches!(self.peek(), TokenKind::RBrace) {
                self.advance();
                break;
            }
            if !brace_form
                && (self.at_eof()
                    || self.check_ident("trait")
                    || self.check_ident("page")
                    || self.check_ident("orbital")
                    || self.check_ident("entity"))
            {
                break;
            }
            // Body-level `@description "..."` / `@synonyms "..."` annotate the
            // `@rebindable` entity binding (catalog/embeddings discovery).
            if let TokenKind::Sigil(s) = self.peek().clone() {
                let annotation = match s.as_str() {
                    "description" | "synonyms" => s,
                    _ => {
                        return Err(ParseError::UnexpectedToken {
                            expected: "@description or @synonyms binding annotation".to_string(),
                            got: format!("@{}", s),
                            span: self.peek_span(),
                        });
                    }
                };
                self.advance();
                let value = match self.peek().clone() {
                    TokenKind::String(v) => {
                        self.advance();
                        v
                    }
                    other => {
                        return Err(ParseError::UnexpectedToken {
                            expected: format!("string literal after @{}", annotation),
                            got: format!("{:?}", other),
                            span: self.peek_span(),
                        });
                    }
                };
                if annotation == "description" {
                    entity_binding_description = Some(value);
                } else {
                    entity_binding_synonyms = Some(value);
                }
                if matches!(self.peek(), TokenKind::Newline) {
                    self.advance();
                }
                continue;
            }
            if self.check_ident("initial") {
                self.advance();
                // Optional colon: `initial: idle` and `initial idle` both parse
                // (uniform-colon consistency — see docs/Almadar_LOLO_Audit.md).
                if matches!(self.peek(), TokenKind::Colon) {
                    self.advance();
                }
                initial_state = Some(self.expect_ident()?);
                self.expect_newline_or_eof()?;
            } else if self.check_ident("state") {
                states.push(self.parse_state()?);
            } else if self.check_ident("emits") {
                emits.extend(self.parse_emits_block()?);
            } else if self.check_ident("listens") {
                listens.extend(self.parse_listens_block()?);
            } else if self.check_ident("ticks") {
                ticks.extend(self.parse_ticks_block()?);
            } else if self.check_ident("config") {
                config.extend(self.parse_config_block()?);
            } else if self.at_eof() {
                break;
            } else {
                return Err(ParseError::UnexpectedToken {
                    expected: "state, initial, emits, listens, ticks, or config".to_string(),
                    got: format!("{:?}", self.peek()),
                    span: self.peek_span(),
                });
            }
        }

        Ok(Trait {
            name,
            type_annotation,
            linked_entity,
            entity_rebindable,
            entity_binding_description,
            entity_binding_synonyms,
            category,
            scope,
            capabilities,
            initial_state,
            states,
            emits,
            listens,
            ticks,
            config,
            span,
            reference: None,
            is_inline_synthetic: false,
        })
    }

    /// Parse the body of a trait reference declaration.
    ///
    /// The leading `trait Name` has already been consumed; on entry the next
    /// token is `=`. The grammar is:
    ///
    /// ```text
    /// trait Name = DottedPath [-> Entity] {
    ///   events { OldEvent: NewEvent, ... }   # zero or more events maps
    ///   listens { ... }                      # standard listens block
    ///   emitsScope (internal | external)     # optional scope override
    /// }
    /// ```
    ///
    /// Inside the brace block the order of `events`, `listens`, and
    /// `emitsScope` does not matter and each may appear at most once.
    ///
    /// Phase 9.5.H: the `on EVENT { (sexpr) ... }` effects-override form is
    /// removed. Atoms own their transition effects; a molecule that wants to
    /// react to an atom's events must import the trait as-is and add a
    /// sibling coordinator trait that `listens { AtomTrait EVENT -> TRIG }`
    /// and runs its own transitions. See docs/Almadar_Entity_Anti_Pattern.md.
    pub(crate) fn parse_trait_reference(
        &mut self,
        name: String,
        span: Span,
        type_annotation: Option<TypeAnnotation>,
    ) -> Result<Trait, ParseError> {
        // Consume the `=`.
        self.expect_kind(&TokenKind::Equals, "=")?;

        // Read the dotted reference path: `Alias.traits.Upstream`.
        let reference = self.parse_dotted_ident()?;

        // Optional `-> Entity` linkedEntity override.
        let linked_entity = if matches!(self.peek(), TokenKind::Arrow | TokenKind::UnicodeArrow) {
            self.advance();
            Some(self.expect_ident()?)
        } else {
            None
        };

        let body = self.parse_trait_ref_body()?;

        Ok(Trait {
            name,
            type_annotation,
            linked_entity,
            entity_rebindable: false,
            entity_binding_description: None,
            entity_binding_synonyms: None,
            category: None,
            scope: None,
            capabilities: Vec::new(),
            initial_state: None,
            states: Vec::new(),
            emits: Vec::new(),
            listens: body.listens,
            ticks: Vec::new(),
            config: Vec::new(),
            span,
            reference: Some(TraitRefData {
                reference,
                events: body.events,
                fields: body.fields,
                config: body.config,
                emits_scope: body.emits_scope,
            }),
            is_inline_synthetic: false,
        })
    }

    /// Parsed body of a trait-reference call site (`{ events {} fields {}
    /// config {} listens {} emitsScope {} }`). Shared by the `trait Name =
    /// Alias.traits.X { … }` declaration form and the inline `@(Alias.traits.X
    /// { … })` sugar. On entry the leading `{` has NOT been consumed; on exit
    /// the closing `}` has been consumed.
    pub(crate) fn parse_trait_ref_body(
        &mut self,
    ) -> Result<TraitRefBody, ParseError> {
        // Body must be brace-form.
        self.expect_kind(&TokenKind::LBrace, "{")?;
        self.skip_newlines();

        let mut events_renames: Vec<(String, String)> = Vec::new();
        let mut fields_renames: Vec<(String, String)> = Vec::new();
        let mut config_values: Vec<ConfigField> = Vec::new();
        let mut listens: Vec<Listen> = Vec::new();
        let mut emits_scope: Option<String> = None;

        loop {
            self.skip_newlines();
            if matches!(self.peek(), TokenKind::RBrace) {
                self.advance();
                break;
            }
            if self.check_ident("events") {
                self.advance();
                if matches!(self.peek(), TokenKind::Colon) {
                    self.advance();
                }
                self.expect_kind(&TokenKind::LBrace, "{")?;
                loop {
                    self.skip_newlines();
                    if matches!(self.peek(), TokenKind::RBrace) {
                        self.advance();
                        break;
                    }
                    let from_event = self.expect_ident()?;
                    self.expect_kind(&TokenKind::Colon, ":")?;
                    let to_event = self.expect_ident()?;
                    events_renames.push((from_event, to_event));
                    if matches!(self.peek(), TokenKind::Comma) {
                        self.advance();
                    }
                    self.skip_newlines();
                }
                if matches!(self.peek(), TokenKind::Newline) {
                    self.advance();
                }
            } else if self.check_ident("fields") {
                // `fields: { title: "dishName", ... }` or `fields { ... }` —
                // entity-field rename map. Mirrors `events`: the inline phase
                // rewrites `@entity.title` reads to `@entity.dishName` so a
                // renamed DATA field on the rebind target satisfies the trait.
                self.advance();
                if matches!(self.peek(), TokenKind::Colon) {
                    self.advance();
                }
                self.expect_kind(&TokenKind::LBrace, "{")?;
                loop {
                    self.skip_newlines();
                    if matches!(self.peek(), TokenKind::RBrace) {
                        self.advance();
                        break;
                    }
                    let from_field = self.expect_ident()?;
                    self.expect_kind(&TokenKind::Colon, ":")?;
                    // Target field name: accept a bare ident or a string literal.
                    let to_field = match self.peek().clone() {
                        TokenKind::String(v) => {
                            self.advance();
                            v
                        }
                        _ => self.expect_ident()?,
                    };
                    fields_renames.push((from_field, to_field));
                    if matches!(self.peek(), TokenKind::Comma) {
                        self.advance();
                    }
                    self.skip_newlines();
                }
                if matches!(self.peek(), TokenKind::Newline) {
                    self.advance();
                }
            } else if self.check_ident("config") {
                // `config: { param: value, ... }` or `config { ... }` at the
                // trait-ref call site. Each entry is EITHER:
                //   - Plain wiring:   `key : value` — a `ConfigField` with
                //     only `default` set and `field_type` = `"unknown"`.
                //   - Full declaration: `key : type = default @label "..." ...`
                //     — a fully-annotated `ConfigField`. Distinguished from
                //     plain wiring by the presence of `=` before the entry
                //     separator (`,` / `}` / newline).
                self.advance();
                if matches!(self.peek(), TokenKind::Colon) {
                    self.advance();
                }
                self.expect_kind(&TokenKind::LBrace, "{")?;
                loop {
                    self.skip_newlines();
                    if matches!(self.peek(), TokenKind::RBrace) {
                        self.advance();
                        break;
                    }
                    let span = self.peek_span();
                    let key = self.expect_ident()?;
                    self.expect_kind(&TokenKind::Colon, ":")?;
                    // A JSX element value (`<Tag …>`) starts with `<` and its
                    // attributes contain `=`, which would falsely trigger the
                    // declaration-form heuristic below. A JSX element is never
                    // a type expression, so when the value starts with `<` we
                    // always take the plain-wiring branch and let
                    // `parse_sexpr_or_value` desugar it.
                    let is_jsx_value = matches!(self.peek(), TokenKind::Identifier(s) if s == "<");
                    let field = if !is_jsx_value && self.has_equals_before_entry_end() {
                        // Declaration form: `key : type = default [@annotations]`
                        let type_expr = self.parse_type_expr()?;
                        let default = if matches!(self.peek(), TokenKind::Equals) {
                            self.advance();
                            Some(self.parse_sexpr_or_value()?)
                        } else {
                            None
                        };
                        if matches!(self.peek(), TokenKind::Newline) {
                            self.advance();
                        }
                        let (label, description, synonyms, tier) =
                            self.parse_config_annotations()?;
                        ConfigField {
                            name: key,
                            type_expr,
                            default,
                            label,
                            description,
                            synonyms,
                            tier,
                            required: false,
                            span,
                        }
                    } else {
                        // Plain wiring form: `key : value`
                        let value = self.parse_sexpr_or_value()?;
                        if matches!(self.peek(), TokenKind::Newline) {
                            self.advance();
                        }
                        ConfigField {
                            name: key,
                            type_expr: TypeExpr::Primitive("unknown".to_string()),
                            default: Some(value),
                            label: None,
                            description: None,
                            synonyms: None,
                            tier: None,
                            required: false,
                            span,
                        }
                    };
                    config_values.push(field);
                    if matches!(self.peek(), TokenKind::Comma) {
                        self.advance();
                    }
                    self.skip_newlines();
                }
                if matches!(self.peek(), TokenKind::Newline) {
                    self.advance();
                }
            } else if self.check_ident("on") {
                // Phase 9.5.H: `on EVENT { (sexpr) ... }` effects-override has
                // been removed. Atoms own their transition effects. To react
                // to an atom's events, add a sibling trait with
                // `listens { AtomAlias EVENT -> TRIG }` and handle the event in
                // its own transitions. See docs/Almadar_Entity_Anti_Pattern.md.
                let event_span = self.peek_span();
                return Err(ParseError::UnexpectedToken {
                    expected:
                        "events, listens, or emitsScope — `on EVENT { ... }` effects-override is removed; daisy-chain via a coordinator trait's listens instead"
                            .to_string(),
                    got: "on".to_string(),
                    span: event_span,
                });
            } else if self.check_ident("listens") {
                listens.extend(self.parse_listens_block()?);
            } else if self.check_ident("emitsScope") {
                self.advance();
                if matches!(self.peek(), TokenKind::Colon) {
                    self.advance();
                }
                let scope = self.expect_ident()?;
                emits_scope = Some(scope);
                if matches!(self.peek(), TokenKind::Newline) {
                    self.advance();
                }
            } else {
                return Err(ParseError::UnexpectedToken {
                    expected: "events, fields, config, listens, emitsScope, or }".to_string(),
                    got: format!("{:?}", self.peek()),
                    span: self.peek_span(),
                });
            }
        }

        Ok(TraitRefBody {
            events: events_renames,
            fields: fields_renames,
            config: config_values,
            listens,
            emits_scope,
        })
    }

    pub(crate) fn parse_state(&mut self) -> Result<State, ParseError> {
        let span = self.peek_span();
        self.expect_keyword("state")?;
        let name = self.expect_ident()?;
        let mut time_bound = None;
        if self.check_ident("for") {
            self.advance();
            time_bound = Some(self.parse_duration_ms()?);
        }
        if matches!(self.peek(), TokenKind::Colon) {
            self.advance();
        }
        let brace_form = matches!(self.peek(), TokenKind::LBrace);
        if brace_form {
            self.advance();
        }
        self.skip_newlines();

        let mut transitions = Vec::new();
        loop {
            self.skip_newlines();
            if brace_form && matches!(self.peek(), TokenKind::RBrace) {
                self.advance();
                break;
            }
            // Stop on next block keyword or end.
            if self.at_eof()
                || self.check_ident("state")
                || self.check_ident("emits")
                || self.check_ident("listens")
                || self.check_ident("ticks")
                || self.check_ident("config")
                || self.check_ident("trait")
                || self.check_ident("page")
                || self.check_ident("entity")
                || self.check_ident("orbital")
                || self.check_ident("initial")
            {
                break;
            }
            if let TokenKind::Identifier(name) = self.peek() {
                if is_event_name(name) {
                    transitions.push(self.parse_transition()?);
                    continue;
                }
            }
            // Stop on anything else (rare during well-formed input).
            break;
        }

        Ok(State {
            name,
            time_bound,
            transitions,
            span,
        })
    }

    pub(crate) fn parse_transition(&mut self) -> Result<Transition, ParseError> {
        let span = self.peek_span();
        let event = self.expect_ident()?;
        // Arrow to target.
        if !matches!(self.peek(), TokenKind::Arrow | TokenKind::UnicodeArrow) {
            return Err(ParseError::UnexpectedToken {
                expected: "-> or →".to_string(),
                got: format!("{:?}", self.peek()),
                span: self.peek_span(),
            });
        }
        self.advance();
        let target = self.expect_ident()?;
        // Optional `when (sexpr)`.
        let mut guard = None;
        if self.check_ident("when") {
            self.advance();
            guard = Some(self.parse_sexpr_or_value()?);
        }
        // Optional newline before effects.
        if matches!(self.peek(), TokenKind::Newline) {
            self.advance();
        }
        let mut effects = Vec::new();
        // Each effect is one s-expression on its own line. Stop when we see
        // anything that's not a `(` (i.e., next event, next state block, etc.).
        loop {
            self.skip_newlines();
            if matches!(self.peek(), TokenKind::LParen) {
                let eff = self.parse_sexpr()?;
                effects.push(eff);
                if matches!(self.peek(), TokenKind::Newline) {
                    self.advance();
                }
            } else {
                break;
            }
        }
        Ok(Transition {
            event,
            target,
            guard,
            effects,
            span,
        })
    }

    pub(crate) fn parse_emits_block(&mut self) -> Result<Vec<Emit>, ParseError> {
        self.expect_keyword("emits")?;
        if matches!(self.peek(), TokenKind::Colon) {
            self.advance();
        }
        let brace_form = matches!(self.peek(), TokenKind::LBrace);
        if brace_form {
            self.advance();
        }
        self.skip_newlines();
        let mut out = Vec::new();
        loop {
            self.skip_newlines();
            if brace_form && matches!(self.peek(), TokenKind::RBrace) {
                self.advance();
                break;
            }
            if self.at_eof() || !is_event_name_at_peek(self.peek()) {
                break;
            }
            out.push(self.parse_emit_decl()?);
        }
        Ok(out)
    }

    /// Trailing event annotations (`@description`/`@synonyms`/`@tier` on the
    /// lines following an event declaration), mirror of the config-knob loop.
    /// Stops at the next event/`}`/non-annotation token.
    pub(crate) fn parse_event_annotations(
        &mut self,
    ) -> Result<(Option<String>, Option<String>, Option<String>), ParseError> {
        let mut description = None;
        let mut synonyms = None;
        let mut tier = None;
        loop {
            self.skip_newlines();
            let sigil_name = match self.peek().clone() {
                TokenKind::Sigil(s) => s,
                _ => break,
            };
            let annotation = match sigil_name.as_str() {
                "description" | "synonyms" | "tier" => sigil_name,
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
                "description" => description = Some(value),
                "synonyms" => synonyms = Some(value),
                "tier" => tier = Some(value),
                _ => unreachable!(),
            }
            if matches!(self.peek(), TokenKind::Newline) {
                self.advance();
            }
        }
        Ok((description, synonyms, tier))
    }

    pub(crate) fn parse_emit_decl(&mut self) -> Result<Emit, ParseError> {
        // Two shapes (post-Phase 10):
        //   `EVENT [-> internal|external] [{ fields }]`
        // The arrow form makes scope explicit; bare name + no arrow + no
        // braces is the most common case (alias lookup at lower-time).
        // The legacy `EVENT scope`/`EVENT :: TypeName` forms are gone.
        let span = self.peek_span();
        let event = self.expect_ident()?;
        let scope = if matches!(self.peek(), TokenKind::Arrow | TokenKind::UnicodeArrow) {
            self.advance();
            let s = self.expect_ident()?;
            if s != "internal" && s != "external" {
                return Err(ParseError::UnexpectedToken {
                    expected: "internal or external after `->`".to_string(),
                    got: s,
                    span: self.peek_span(),
                });
            }
            Some(s)
        } else {
            None
        };
        let payload = if matches!(self.peek(), TokenKind::LBrace) {
            self.parse_event_field_list()?
        } else {
            Vec::new()
        };
        if matches!(self.peek(), TokenKind::Newline) {
            self.advance();
        }
        let (description, synonyms, tier) = self.parse_event_annotations()?;
        Ok(Emit {
            event,
            scope,
            payload,
            description,
            synonyms,
            tier,
            span,
        })
    }

    pub(crate) fn parse_event_field_list(&mut self) -> Result<Vec<EventField>, ParseError> {
        self.expect_kind(&TokenKind::LBrace, "{")?;
        let mut fields = Vec::new();
        loop {
            self.skip_newlines();
            if matches!(self.peek(), TokenKind::RBrace) {
                self.advance();
                break;
            }
            let span = self.peek_span();
            let name = self.expect_ident()?;
            // Two field forms:
            //   `name : primitive`   — existing syntax, produces TypeExpr::Primitive or
            //                          whatever `parse_type_expr` returns.
            //   `name :: TypeName`   — Form B from §2.2 of Almadar_Entity_Anti_Pattern.md.
            //                          Produces TypeExpr::Named; the lowerer resolves
            //                          against the TypeRegistry and flattens into nested
            //                          PayloadField entries at emit time.
            let type_expr = if matches!(self.peek(), TokenKind::DoubleColon) {
                self.advance();
                let type_name = self.parse_dotted_ident()?;
                TypeExpr::Named(type_name)
            } else {
                self.expect_kind(&TokenKind::Colon, ":")?;
                self.parse_type_expr()?
            };
            let required = if matches!(self.peek(), TokenKind::Bang) {
                self.advance();
                true
            } else {
                false
            };
            fields.push(EventField {
                name,
                type_expr,
                required,
                span,
            });
            if matches!(self.peek(), TokenKind::Comma) {
                self.advance();
            }
            self.skip_newlines();
        }
        Ok(fields)
    }

    pub(crate) fn parse_listens_block(&mut self) -> Result<Vec<Listen>, ParseError> {
        self.expect_keyword("listens")?;
        if matches!(self.peek(), TokenKind::Colon) {
            self.advance();
        }
        let brace_form = matches!(self.peek(), TokenKind::LBrace);
        if brace_form {
            self.advance();
        }
        self.skip_newlines();
        let mut out = Vec::new();
        loop {
            self.skip_newlines();
            if brace_form && matches!(self.peek(), TokenKind::RBrace) {
                self.advance();
                break;
            }
            // A handler starts with an identifier (source), then we expect `.`, then EVENT.
            // Since the lexer emits Sigils with embedded dots, we look for an Identifier here.
            if let TokenKind::Identifier(_) = self.peek() {
                out.push(self.parse_listen_handler()?);
            } else {
                break;
            }
        }
        Ok(out)
    }

    pub(crate) fn parse_listen_handler(&mut self) -> Result<Listen, ParseError> {
        let span = self.peek_span();
        let first = self.expect_ident()?;

        // Two shapes (post-Phase 10):
        // 1. Local incoming: `EVENT [{ fields }]` + newline.
        //    No source, no trigger. Declares the payload shape for this key.
        //    Bare name without braces also legal (alias lookup at lower-time).
        // 2. Cross-orbital: `Source.EVENT -> TRIGGER [when ...] [with ...]`.
        //    Source and event separated by a dot (was a space pre-Phase 10).
        //
        // Disambiguation by token after the first identifier:
        //   `{`               → local with inline payload
        //   Newline/`}`/EOF   → local payloadless (alias lookup at lower-time)
        //   `.`               → cross-orbital (Source.EVENT)
        match self.peek().clone() {
            // Local incoming with inline payload fields.
            TokenKind::LBrace => {
                let payload = self.parse_event_field_list()?;
                if matches!(self.peek(), TokenKind::Newline) {
                    self.advance();
                }
                let (description, synonyms, tier) = self.parse_event_annotations()?;
                Ok(Listen {
                    event: first,
                    triggers: None,
                    guard: None,
                    payload_mapping: Vec::new(),
                    payload,
                    description,
                    synonyms,
                    tier,
                    span,
                })
            }
            // Local incoming, payloadless (alias lookup at lower time).
            TokenKind::Newline | TokenKind::RBrace | TokenKind::Eof => {
                if matches!(self.peek(), TokenKind::Newline) {
                    self.advance();
                }
                let (description, synonyms, tier) = self.parse_event_annotations()?;
                Ok(Listen {
                    event: first,
                    triggers: None,
                    guard: None,
                    payload_mapping: Vec::new(),
                    payload: Vec::new(),
                    description,
                    synonyms,
                    tier,
                    span,
                })
            }
            // Cross-trait listen: dotted form. Two shapes:
            //   `Source.EVENT -> TRIGGER`           (intra-orbital)
            //   `Orbital.Source.EVENT -> TRIGGER`   (cross-orbital)
            // Encoded as `Source.EVENT` or `Orbital.Source.EVENT` in
            // `Listen.event`; the lowerer splits the dots to populate
            // `ListenSource::Trait` vs `ListenSource::Orbital`.
            TokenKind::Dot => {
                self.advance();
                let second = self.expect_ident()?;
                // If a second `.` follows, the form is 3-part cross-orbital:
                // first=Orbital, second=Source, third=EVENT. Otherwise the
                // form is 2-part: first=Source, second=EVENT.
                let combined = if matches!(self.peek(), TokenKind::Dot) {
                    self.advance();
                    let third = self.expect_ident()?;
                    format!("{first}.{second}.{third}")
                } else {
                    format!("{first}.{second}")
                };
                if !matches!(self.peek(), TokenKind::Arrow | TokenKind::UnicodeArrow) {
                    return Err(ParseError::UnexpectedToken {
                        expected: "-> after listens dotted source".to_string(),
                        got: format!("{:?}", self.peek()),
                        span: self.peek_span(),
                    });
                }
                self.advance();
                let triggers = self.expect_ident()?;
                if matches!(self.peek(), TokenKind::Newline) {
                    self.advance();
                }
                let mut guard = None;
                let mut payload_mapping = Vec::new();
                // Optional indented `when (...)` and `with { ... }` lines.
                loop {
                    self.skip_newlines();
                    if self.check_ident("when") {
                        self.advance();
                        guard = Some(self.parse_sexpr_or_value()?);
                        if matches!(self.peek(), TokenKind::Newline) {
                            self.advance();
                        }
                    } else if self.check_ident("with") {
                        self.advance();
                        self.expect_kind(&TokenKind::LBrace, "{")?;
                        loop {
                            self.skip_newlines();
                            if matches!(self.peek(), TokenKind::RBrace) {
                                self.advance();
                                break;
                            }
                            let key = self.expect_ident()?;
                            self.expect_kind(&TokenKind::Colon, ":")?;
                            let value = self.parse_sexpr_or_value()?;
                            payload_mapping.push((key, value));
                            if matches!(self.peek(), TokenKind::Comma) {
                                self.advance();
                            }
                            self.skip_newlines();
                        }
                        if matches!(self.peek(), TokenKind::Newline) {
                            self.advance();
                        }
                    } else {
                        break;
                    }
                }
                Ok(Listen {
                    event: combined,
                    triggers: Some(triggers),
                    guard,
                    payload_mapping,
                    payload: Vec::new(),
                    description: None,
                    synonyms: None,
                    tier: None,
                    span,
                })
            }
            other => Err(ParseError::UnexpectedToken {
                expected: "{ or . or newline".to_string(),
                got: format!("{other:?}"),
                span: self.peek_span(),
            }),
        }
    }

    pub(crate) fn parse_ticks_block(&mut self) -> Result<Vec<Tick>, ParseError> {
        self.expect_keyword("ticks")?;
        if matches!(self.peek(), TokenKind::Colon) {
            self.advance();
        }
        let brace_form = matches!(self.peek(), TokenKind::LBrace);
        if brace_form {
            self.advance();
        }
        self.skip_newlines();
        let mut out = Vec::new();
        loop {
            self.skip_newlines();
            if brace_form && matches!(self.peek(), TokenKind::RBrace) {
                self.advance();
                break;
            }
            if let TokenKind::Identifier(name) = self.peek().clone() {
                if is_block_keyword(&name) {
                    break;
                }
                let span = self.peek_span();
                self.advance();
                self.expect_keyword("every")?;
                let interval = self.parse_tick_interval()?;
                if matches!(self.peek(), TokenKind::Newline) {
                    self.advance();
                }
                let mut guard = None;
                if self.check_ident("when") {
                    self.advance();
                    guard = Some(self.parse_sexpr_or_value()?);
                    if matches!(self.peek(), TokenKind::Newline) {
                        self.advance();
                    }
                }
                let mut effects = Vec::new();
                loop {
                    self.skip_newlines();
                    if matches!(self.peek(), TokenKind::LParen) {
                        effects.push(self.parse_sexpr()?);
                        if matches!(self.peek(), TokenKind::Newline) {
                            self.advance();
                        }
                    } else {
                        break;
                    }
                }
                out.push(Tick {
                    name,
                    interval,
                    guard,
                    effects,
                    span,
                });
            } else {
                break;
            }
        }
        Ok(out)
    }

    pub(crate) fn parse_tick_interval(&mut self) -> Result<TickInterval, ParseError> {
        match self.peek().clone() {
            TokenKind::String(s) => {
                self.advance();
                Ok(TickInterval::Cron(s))
            }
            TokenKind::Integer(n) => {
                self.advance();
                // May be followed by a duration suffix (`s`, `m`, `h`, `d`).
                if let TokenKind::Identifier(suf) = self.peek().clone() {
                    if matches!(suf.as_str(), "s" | "m" | "h" | "d" | "ms") {
                        self.advance();
                        let ms = duration_to_ms(n as u64, &suf);
                        return Ok(TickInterval::Millis(ms));
                    }
                }
                Ok(TickInterval::Millis(n as u64))
            }
            other => Err(ParseError::UnexpectedToken {
                expected: "interval (number or string)".to_string(),
                got: format!("{other:?}"),
                span: self.peek_span(),
            }),
        }
    }

    pub(crate) fn parse_config_block(&mut self) -> Result<Vec<ConfigField>, ParseError> {
        self.expect_keyword("config")?;
        if matches!(self.peek(), TokenKind::Colon) {
            self.advance();
        }
        let brace_form = matches!(self.peek(), TokenKind::LBrace);
        if brace_form {
            self.advance();
        }
        self.skip_newlines();
        let mut out = Vec::new();
        loop {
            self.skip_newlines();
            if brace_form && matches!(self.peek(), TokenKind::RBrace) {
                self.advance();
                break;
            }
            if let TokenKind::Identifier(_) = self.peek() {
                let span = self.peek_span();
                let name = self.expect_ident()?;
                if is_block_keyword(&name) {
                    break;
                }
                self.expect_kind(&TokenKind::Colon, ":")?;
                let type_expr = self.parse_type_expr()?;
                let required = if matches!(self.peek(), TokenKind::Bang) {
                    self.advance();
                    true
                } else {
                    false
                };
                let mut default = None;
                if matches!(self.peek(), TokenKind::Equals) {
                    self.advance();
                    default = Some(self.parse_sexpr_or_value()?);
                }
                // Legacy trailing-string description (kept as a fallback
                // for the rare authoring that uses it directly).
                let mut description = if let TokenKind::String(s) = self.peek().clone() {
                    self.advance();
                    Some(s)
                } else {
                    None
                };
                let mut label: Option<String> = None;
                let mut synonyms: Option<String> = None;
                let mut tier: Option<String> = None;
                if matches!(self.peek(), TokenKind::Newline) {
                    self.advance();
                }
                let (ann_label, ann_description, ann_synonyms, ann_tier) =
                    self.parse_config_annotations()?;
                if ann_label.is_some() {
                    label = ann_label;
                }
                if ann_description.is_some() {
                    description = ann_description;
                }
                if ann_synonyms.is_some() {
                    synonyms = ann_synonyms;
                }
                if ann_tier.is_some() {
                    tier = ann_tier;
                }
                out.push(ConfigField {
                    name,
                    type_expr,
                    default,
                    label,
                    description,
                    synonyms,
                    tier,
                    required,
                    span,
                });
            } else {
                break;
            }
        }
        Ok(out)
    }

    /// Consume trailing `@label`/`@description`/`@synonyms`/`@tier` annotation
    /// lines after a config field or call-site config entry. Stops at the first
    /// non-annotation token. Shared by `parse_config_block` (atom declarations)
    /// and the call-site config parser (organism overrides).
    fn parse_config_annotations(
        &mut self,
    ) -> Result<(Option<String>, Option<String>, Option<String>, Option<String>), ParseError> {
        let mut label = None;
        let mut description = None;
        let mut synonyms = None;
        let mut tier = None;
        loop {
            self.skip_newlines();
            let sigil_name = match self.peek().clone() {
                TokenKind::Sigil(s) => s,
                _ => break,
            };
            let annotation = match sigil_name.as_str() {
                "label" | "description" | "synonyms" | "tier" => sigil_name,
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
                "label" => label = Some(value),
                "description" => description = Some(value),
                "synonyms" => synonyms = Some(value),
                "tier" => tier = Some(value),
                _ => unreachable!(),
            }
            if matches!(self.peek(), TokenKind::Newline) {
                self.advance();
            }
        }
        Ok((label, description, synonyms, tier))
    }

    /// Scan forward from the current position looking for a `=` token before
    /// the next entry separator (`','`, `'}'`, newline, or EOF). Returns `true`
    /// when a `=` is found, indicating a declaration form (`key : type = value`)
    /// rather than a plain wiring value (`key : value`).
    ///
    /// This is a pure-lookahead operation — it does NOT advance the cursor.
    fn has_equals_before_entry_end(&self) -> bool {
        let mut i = self.pos;
        let len = self.tokens.len();
        while i < len {
            match &self.tokens[i].kind {
                TokenKind::Equals => return true,
                TokenKind::Comma
                | TokenKind::RBrace
                | TokenKind::Newline
                | TokenKind::Eof => return false,
                _ => i += 1,
            }
        }
        false
    }
}
