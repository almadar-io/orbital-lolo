//! Trait / trait-reference / state / transition / emits / listens / ticks /
//! config parsing for the .lolo parser.

use super::*;

impl Parser {
    pub(crate) fn parse_trait(&mut self) -> Result<Trait, ParseError> {
        let span = self.peek_span();
        self.expect_keyword("trait")?;
        let name = self.expect_ident()?;
        // Any `@(…)`/JSX inline-ref hoisted while parsing this trait's body
        // (either form) is tagged with its name, so the fragment mounts only
        // on pages that themselves declare this trait — never every page in
        // the orbital.
        let previous_hoist_producer = self.current_hoist_producer.replace(name.clone());
        let result = self.parse_trait_body(name, span);
        self.current_hoist_producer = previous_hoist_producer;
        result
    }

    fn parse_trait_body(&mut self, name: String, span: Span) -> Result<Trait, ParseError> {
        // Increment 3-II: optional `(V : Type, R : Type)` trait-level type
        // parameter list, immediately after the name and before `::`/`->`/`=`
        // — unambiguous, since none of those three continuations start with
        // `(`. Only meaningful on the inline-declaration form (a reference
        // form has no legitimate use for it; parsed here regardless so a
        // stray `(...)` on a reference is consumed instead of producing a
        // confusing downstream parse error, and is simply unused there).
        let type_params = self.parse_trait_type_params()?;

        // Optional `:: TypeName [args]` annotation above the trait body.
        let type_annotation = self.try_parse_type_annotation()?;

        // Reference form: `trait Name [:: ...] = Alias.traits.Upstream [-> Entity] { ... }`.
        // The `=` immediately after the name/annotation disambiguates from the
        // inline form, which uses `->` for the linkedEntity binding.
        if matches!(self.peek(), TokenKind::Equals) {
            return self.parse_trait_reference(name, span, type_params, type_annotation);
        }

        // Optional `-> [@rebindable] Entity`. `@rebindable` between the
        // arrow and the entity opts the trait into consumer rebinding via
        // `linkedEntity`; without it the binding is fixed.
        let mut entity_rebindable = false;
        let mut linked_entity_span = None;
        let linked_entity = if matches!(self.peek(), TokenKind::Arrow | TokenKind::UnicodeArrow) {
            self.advance();
            let is_rebindable = matches!(self.peek(), TokenKind::Sigil(s) if self.canon_annotation_sigil(s) == "rebindable");
            if is_rebindable {
                self.advance();
                entity_rebindable = true;
            }
            linked_entity_span = Some(self.peek_span());
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
        let (category, scope, local, capabilities) = if matches!(self.peek(), TokenKind::LBracket) {
            self.advance();
            let mut category: Option<String> = None;
            let mut scope: Option<TraitScope> = None;
            let mut local = false;
            let mut capabilities: Vec<String> = Vec::new();
            loop {
                self.skip_newlines();
                if matches!(self.peek(), TokenKind::RBracket) {
                    self.advance();
                    break;
                }
                let ident = self.expect_ident()?;
                // The first non-scope word is a `categories`-section word,
                // later ones are `capabilities`-section words — the same
                // token position serves both, so try each section in turn
                // (safe: no English word appears in both, B1 rule 4).
                let ident = self
                    .lang
                    .canon_tag(&ident)
                    .or_else(|| self.lang.canon_category(&ident))
                    .or_else(|| self.lang.canon_capability(&ident))
                    .map(str::to_string)
                    .unwrap_or(ident);
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
                    "local" => {
                        local = true;
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
            (category, scope, local, capabilities)
        } else {
            (None, None, false, Vec::new())
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
            if let TokenKind::Sigil(raw_s) = self.peek().clone() {
                let s = self.canon_annotation_sigil(&raw_s);
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
                listens.extend(self.parse_trait_listens_block()?);
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
            type_params,
            type_annotation,
            linked_entity,
            linked_entity_span,
            entity_rebindable,
            entity_binding_description,
            entity_binding_synonyms,
            category,
            scope,
            local,
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

    /// Increment 3-II: parse an optional `(V : Type, R : Type)` trait-level
    /// type parameter list. Returns an empty `Vec` when the next token isn't
    /// `(`. Accepts BOTH comma- and space-separated params in one grammar:
    /// after each `name : kind` pair, an optional `,` is consumed if present;
    /// either way the loop continues straight into the next pair (or `)`).
    pub(crate) fn parse_trait_type_params(&mut self) -> Result<Vec<TraitTypeParam>, ParseError> {
        if !matches!(self.peek(), TokenKind::LParen) {
            return Ok(Vec::new());
        }
        self.advance();
        let mut params = Vec::new();
        loop {
            self.skip_newlines();
            if matches!(self.peek(), TokenKind::RParen) {
                self.advance();
                break;
            }
            let pname_span = self.peek_span();
            let pname = self.expect_ident()?;
            if let Some(e) = crate::parser::r#type::type_param_name_error(&pname, pname_span) {
                return Err(e);
            }
            self.expect_kind(&TokenKind::Colon, ":")?;
            let kind_span = self.peek_span();
            let kind = self.expect_ident()?;
            if !crate::parser::r#type::VALID_TYPE_PARAM_KINDS.contains(&kind.as_str()) {
                return Err(ParseError::UnexpectedToken {
                    expected: "Type, Entity, Trait, Page, Orbital, or Event".to_string(),
                    got: kind,
                    span: kind_span,
                });
            }
            // Gap C: optional `= <ident>` declaration default — a single
            // identifier token (primitive/entity/alias NAME), same value
            // vocabulary as a call-site `::` arg. No inline `{...}`/`Map K V`
            // in this increment.
            let default = if matches!(self.peek(), TokenKind::Equals) {
                self.advance();
                Some(self.expect_ident()?)
            } else {
                None
            };
            params.push(TraitTypeParam {
                name: pname,
                kind,
                default,
            });
            if matches!(self.peek(), TokenKind::Comma) {
                self.advance();
            }
            self.skip_newlines();
        }
        Ok(params)
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
        type_params: Vec<TraitTypeParam>,
        type_annotation: Option<TypeAnnotation>,
    ) -> Result<Trait, ParseError> {
        // Consume the `=`.
        self.expect_kind(&TokenKind::Equals, "=")?;

        // Read the dotted reference path: `Alias.traits.Upstream`.
        let reference = self.parse_dotted_ident()?;

        // Optional `-> Entity` linkedEntity override.
        let mut linked_entity_span = None;
        let linked_entity = if matches!(self.peek(), TokenKind::Arrow | TokenKind::UnicodeArrow) {
            self.advance();
            linked_entity_span = Some(self.peek_span());
            Some(self.expect_ident()?)
        } else {
            None
        };

        // Optional `[local]`: the rebind dispatches client-only. The atom owns
        // category/scope; `local` is the one modifier a call site sets.
        let mut local = false;
        if matches!(self.peek(), TokenKind::LBracket) {
            self.advance();
            loop {
                self.skip_newlines();
                if matches!(self.peek(), TokenKind::RBracket) {
                    self.advance();
                    break;
                }
                let span = self.peek_span();
                let ident = self.expect_ident()?;
                if ident != "local" {
                    return Err(ParseError::UnexpectedToken {
                        expected: "`local` (the only modifier a trait rebind sets; the atom owns category and scope)".to_string(),
                        got: ident,
                        span,
                    });
                }
                local = true;
                if matches!(self.peek(), TokenKind::Comma) {
                    self.advance();
                }
            }
        }

        let body = self.parse_trait_ref_body()?;

        Ok(Trait {
            name,
            type_params,
            type_annotation,
            linked_entity,
            linked_entity_span,
            entity_rebindable: false,
            entity_binding_description: None,
            entity_binding_synonyms: None,
            category: None,
            scope: None,
            local,
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
                name_override: body.name_override,
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
    pub(crate) fn parse_trait_ref_body(&mut self) -> Result<TraitRefBody, ParseError> {
        // Body must be brace-form.
        let body_open = self.peek_span();
        self.expect_kind(&TokenKind::LBrace, "{")?;
        self.skip_newlines();

        let mut events_renames: Vec<(String, String)> = Vec::new();
        let mut fields_renames: Vec<(String, String)> = Vec::new();
        let mut config_values: Vec<ConfigField> = Vec::new();
        let mut listens: Vec<Listen> = Vec::new();
        let mut emits_scope: Option<String> = None;
        let mut name_override: Option<String> = None;

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
            if self.check_ident("events") {
                self.advance();
                if matches!(self.peek(), TokenKind::Colon) {
                    self.advance();
                }
                let events_open = self.peek_span();
                self.expect_kind(&TokenKind::LBrace, "{")?;
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
                            open_span: events_open,
                            found: "end of file".to_string(),
                        });
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
                let fields_open = self.peek_span();
                self.expect_kind(&TokenKind::LBrace, "{")?;
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
                            open_span: fields_open,
                            found: "end of file".to_string(),
                        });
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
                config_values.extend(self.parse_config_override_block()?);
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
            } else if self.check_ident("name") {
                // `name: "InstanceName"` — instance-name override. Lets one
                // source trait be rebound multiple times in one orbital: the
                // declaration identifier stays free while this names the
                // instance (mount handle, duplicate-name disambiguation).
                self.advance();
                if matches!(self.peek(), TokenKind::Colon) {
                    self.advance();
                }
                let value = match self.peek().clone() {
                    TokenKind::String(v) => {
                        self.advance();
                        v
                    }
                    _ => self.expect_ident()?,
                };
                name_override = Some(value);
                if matches!(self.peek(), TokenKind::Newline) {
                    self.advance();
                }
            } else if self.check_ident("listens") {
                listens.extend(self.parse_trait_listens_block()?);
            } else if self.check_ident("emitsScope") {
                self.advance();
                if matches!(self.peek(), TokenKind::Colon) {
                    self.advance();
                }
                let scope = self.expect_ident()?;
                let scope = self
                    .lang
                    .canon_keyword(&scope)
                    .map(str::to_string)
                    .unwrap_or(scope);
                emits_scope = Some(scope);
                if matches!(self.peek(), TokenKind::Newline) {
                    self.advance();
                }
            } else {
                return Err(ParseError::UnexpectedToken {
                    expected: "events, fields, config, listens, emitsScope, name, or }".to_string(),
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
            name_override,
        })
    }

    /// Parse a call-site `config { ... }` OVERRIDE block — the shared body
    /// of a trait/orbital reference's `config {}` and a `uses { config {} }`
    /// override. On entry the next token is the `config` keyword itself
    /// (NOT yet consumed); on exit the closing `}` has been consumed.
    ///
    /// Each entry is EITHER:
    ///   - Plain wiring:   `key : value` — a `ConfigField` with only
    ///     `default` set and `field_type` = `"unknown"`.
    ///   - Full declaration: `key : type = default @label "..." ...` — a
    ///     fully-annotated `ConfigField`. Distinguished from plain wiring by
    ///     the presence of `=` before the entry separator (`,` / `}` /
    ///     newline).
    ///
    /// Extracted verbatim from `parse_trait_ref_body`'s inline `config` arm
    /// (behavior-preserving refactor) so the orbital-reference and
    /// `uses`-override call sites (orbital-import Stage A) share it instead
    /// of a second copy.
    pub(crate) fn parse_config_override_block(&mut self) -> Result<Vec<ConfigField>, ParseError> {
        self.expect_keyword("config")?;
        if matches!(self.peek(), TokenKind::Colon) {
            self.advance();
        }
        let config_open = self.peek_span();
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
                    open_span: config_open,
                    found: "end of file".to_string(),
                });
            }
            let span = self.peek_span();
            let key = self.expect_ident()?;
            self.expect_kind(&TokenKind::Colon, ":")?;
            // A JSX element value (`<Tag …>`) starts with `<` and its
            // attributes contain `=`, which would falsely trigger the
            // declaration-form heuristic below. A JSX element is never a
            // type expression, so when the value starts with `<` we always
            // take the plain-wiring branch and let `parse_sexpr_or_value`
            // desugar it.
            let is_jsx_value = matches!(self.peek(), TokenKind::Identifier(s) if s == "<");
            let field = if !is_jsx_value && self.has_equals_before_entry_end() {
                // Declaration form: `key : type = default [@annotations]`
                let type_expr = self.parse_type_expr()?;
                let (default, event_refs) = if matches!(self.peek(), TokenKind::Equals) {
                    self.advance();
                    let (value, refs) = self.parse_config_value()?;
                    (Some(value), refs)
                } else {
                    (None, Vec::new())
                };
                if matches!(self.peek(), TokenKind::Newline) {
                    self.advance();
                }
                let (label, description, synonyms, tier) = self.parse_config_annotations()?;
                ConfigField {
                    name: key,
                    type_expr,
                    default,
                    label,
                    description,
                    synonyms,
                    tier,
                    required: false,
                    event_refs,
                    span,
                }
            } else {
                // Plain wiring form: `key : value`
                let (value, event_refs) = self.parse_config_value()?;
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
                    event_refs,
                    span,
                }
            };
            reject_duplicate_config_key(&out, &field)?;
            out.push(field);
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
        let event = self.canon_event_name(event);
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
        let mut guard_spans = Vec::new();
        if self.check_ident("when") {
            self.advance();
            let (g, spans) = self.capture_expr_spans(|p| p.parse_sexpr_or_value())?;
            guard = Some(g);
            guard_spans = spans;
        }
        // `match @entity.<f> { … }` / `match @payload.<f> { … }` sugar
        // (Phase 2, pattern-matching doc §1(a)) — mutually exclusive with
        // `when`: one dispatch mechanism per arm set.
        if self.check_ident("match") {
            if guard.is_some() {
                return Err(ParseError::InvalidSExpr {
                    message: "ELOLO_MATCH_TERM_WITH_WHEN: a transition can't combine `when` \
                              and `match` — pick one dispatch mechanism per arm set"
                        .to_string(),
                    span: self.peek_span(),
                });
            }
            let term_match = self.parse_term_match()?;
            return Ok(Transition {
                event,
                target,
                guard: None,
                effects: Vec::new(),
                span,
                guard_spans: Vec::new(),
                effect_spans: Vec::new(),
                term_match: Some(term_match),
            });
        }
        // Optional newline before effects.
        if matches!(self.peek(), TokenKind::Newline) {
            self.advance();
        }
        let mut effects = Vec::new();
        let mut effect_spans = Vec::new();
        // Each effect is one s-expression on its own line. Stop when we see
        // anything that's not a `(` (i.e., next event, next state block, etc.).
        loop {
            self.skip_newlines();
            if matches!(self.peek(), TokenKind::LParen) {
                let eff_span = self.peek_span();
                let (eff, spans) = self.capture_expr_spans(|p| p.parse_sexpr())?;
                reject_match_effect_head(&eff, eff_span)?;
                effects.push(eff);
                effect_spans.push(spans);
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
            guard_spans,
            effect_spans,
            term_match: None,
        })
    }

    /// Parse a term-level `match` body after the leading `match` keyword
    /// (already confirmed present, not yet consumed): scrutinee, then
    /// `{ <pattern> -> <effects...> ... }` arms in authoring order. Each
    /// arm's effects loop mirrors `parse_transition`'s own effects loop
    /// exactly (zero or more leading-`(` s-expressions, one per line).
    fn parse_term_match(&mut self) -> Result<TermMatch, ParseError> {
        let match_span = self.peek_span();
        self.advance();
        let scrutinee = self.parse_term_match_scrutinee()?;
        self.expect_kind(&TokenKind::LBrace, "{")?;
        let mut arms = Vec::new();
        loop {
            self.skip_newlines();
            if matches!(self.peek(), TokenKind::RBrace) {
                self.advance();
                break;
            }
            let arm_span = self.peek_span();
            let pattern = self.parse_match_pattern()?;
            if !matches!(self.peek(), TokenKind::Arrow | TokenKind::UnicodeArrow) {
                return Err(ParseError::UnexpectedToken {
                    expected: "`->` after a match arm pattern".to_string(),
                    got: format!("{:?}", self.peek()),
                    span: self.peek_span(),
                });
            }
            self.advance();
            let mut effects = Vec::new();
            let mut effect_spans = Vec::new();
            loop {
                self.skip_newlines();
                if matches!(self.peek(), TokenKind::LParen) {
                    let eff_span = self.peek_span();
                    let (eff, spans) = self.capture_expr_spans(|p| p.parse_sexpr())?;
                    reject_match_effect_head(&eff, eff_span)?;
                    effects.push(eff);
                    effect_spans.push(spans);
                } else {
                    break;
                }
            }
            arms.push(TermMatchArm {
                pattern,
                effects,
                effect_spans,
                span: arm_span,
            });
        }
        Ok(TermMatch {
            scrutinee,
            arms,
            span: match_span,
        })
    }

    /// Parse a term-match scrutinee: `@entity.<field>` or `@payload.<field>`
    /// (v1). `@config.<knob>` is rejected — a term match on a compose-
    /// resolved knob is a degenerate constant dispatch; use type-level
    /// `match @config.<knob>` (§25) or a plain `when (== @config.<knob>
    /// ...)` guard instead. `@user.<field>` is rejected — the fail-open
    /// identity-resolution class needs its own design pass.
    fn parse_term_match_scrutinee(&mut self) -> Result<MatchScrutinee, ParseError> {
        let span = self.peek_span();
        let TokenKind::Sigil(raw_s) = self.peek().clone() else {
            return Err(ParseError::UnexpectedToken {
                expected: "`@entity.<field>` or `@payload.<field>` after `match`".to_string(),
                got: format!("{:?}", self.peek()),
                span,
            });
        };
        let s = self.canon_sigil_root(&raw_s);
        if let Some(path) = s.strip_prefix("entity.") {
            let (field, member) = Self::split_term_match_path(path, "@entity", span)?;
            self.advance();
            return Ok(MatchScrutinee::Entity { field, member });
        }
        if let Some(path) = s.strip_prefix("payload.") {
            let (field, member) = Self::split_term_match_path(path, "@payload", span)?;
            self.advance();
            return Ok(MatchScrutinee::Payload { field, member });
        }
        if s == "config" || s.starts_with("config.") {
            return Err(ParseError::InvalidSExpr {
                message: "ELOLO_MATCH_TERM_SCRUTINEE_CONFIG: a term match can't dispatch on \
                          `@config.<knob>` — a compose-resolved knob is a constant, so this \
                          would be a degenerate dispatch; use type-level `match \
                          @config.<knob>` (for a payload TYPE) or a plain `when (== \
                          @config.<knob> ...)` guard instead"
                    .to_string(),
                span,
            });
        }
        if s == "user" || s.starts_with("user.") {
            return Err(ParseError::InvalidSExpr {
                message: "ELOLO_MATCH_TERM_SCRUTINEE_USER: `@user.<field>` scrutinees aren't \
                          supported yet — the fail-open identity-resolution class needs its \
                          own design pass"
                    .to_string(),
                span,
            });
        }
        Err(ParseError::InvalidSExpr {
            message: format!(
                "ELOLO_MATCH_TERM_SCRUTINEE_UNSUPPORTED: `match` needs an `@entity.<field>` \
                 or `@payload.<field>` scrutinee, got `@{s}`"
            ),
            span,
        })
    }

    /// Split a term-match scrutinee path into its base field and optional
    /// discriminant member: one segment (`op`) or two (`item.type`, the
    /// Phase-3a tagged-union destructuring form). Anything else is malformed.
    fn split_term_match_path(
        path: &str,
        root: &str,
        span: Span,
    ) -> Result<(String, Option<String>), ParseError> {
        let mut segments = path.split('.');
        let field = segments.next().unwrap_or("");
        let member = segments.next();
        let ok = !field.is_empty()
            && member.map(|m| !m.is_empty()).unwrap_or(true)
            && segments.next().is_none();
        if !ok {
            return Err(ParseError::InvalidSExpr {
                message: format!(
                    "ELOLO_MATCH_TERM_SCRUTINEE_INVALID: `match {root}.<field>` needs one \
                     field segment, or `{root}.<field>.<member>` to dispatch on a tagged \
                     struct union's discriminating member"
                ),
                span,
            });
        }
        Ok((field.to_string(), member.map(str::to_string)))
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
            if self.at_eof() || !is_event_name_at_peek(self.peek(), self.lang) {
                // In brace form the only legal tokens here are `}` or the
                // next event name — breaking silently would leave the `}`
                // unconsumed and surface a misleading trait-body keyword
                // error far from the real problem.
                if brace_form && !self.at_eof() {
                    return Err(ParseError::InvalidSExpr {
                        message: format!(
                            "unexpected token inside `emits {{ … }}`: {:?}",
                            self.peek()
                        ),
                        span: self.peek_span(),
                    });
                }
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
                TokenKind::Sigil(s) => self.canon_annotation_sigil(&s),
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
        let event = self.expect_event_name_or_config_ref()?;
        let scope = if matches!(self.peek(), TokenKind::Arrow | TokenKind::UnicodeArrow) {
            self.advance();
            let s = self.expect_ident()?;
            let s = self.lang.canon_keyword(&s).map(str::to_string).unwrap_or(s);
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
            // `match` is legal here as the field's WHOLE type only — a
            // nested occurrence (`[match ...]`, a union member, an object
            // field) is the same forbidden-position class as config/alias
            // positions, just caught at this call site instead of theirs.
            if !matches!(type_expr, TypeExpr::Match { .. }) && type_expr_contains_match(&type_expr)
            {
                return Err(ParseError::InvalidType {
                    span,
                    message: format!(
                        "ELOLO_MATCH_POSITION_UNSUPPORTED: field '{}' nests `match` inside \
                         another type — match must be the field's whole type",
                        name
                    ),
                });
            }
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

    /// A trait's own `listens {}`: routes land on the trait itself, so a
    /// `-> Target.TRIGGER` (an orbital import's form) is rejected.
    fn parse_trait_listens_block(&mut self) -> Result<Vec<Listen>, ParseError> {
        let listens = self.parse_listens_block()?;
        if let Some(l) = listens.iter().find(|l| l.target_trait.is_some()) {
            return Err(ParseError::UnexpectedToken {
                expected: "`-> TRIGGER` (a trait's listens deliver to the trait itself; `-> Target.TRIGGER` is the orbital-import form)".to_string(),
                got: format!("-> {}.{}", l.target_trait.as_deref().unwrap_or(""), l.triggers.as_deref().unwrap_or("")),
                span: l.span,
            });
        }
        Ok(listens)
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
                    event: self.canon_event_name(first),
                    triggers: None,
                    guard: None,
                    payload_mapping: Vec::new(),
                    payload,
                    description,
                    synonyms,
                    tier,
                    target_trait: None,
                    external: false,
                    inbound: None,
                    span,
                })
            }
            // Declared external input: `EVENT -> external { fields }`. The
            // payload braces are mandatory (an empty `{}` declares no fields).
            TokenKind::Arrow | TokenKind::UnicodeArrow => {
                self.advance();
                let word = self.expect_ident()?;
                let word = self
                    .lang
                    .canon_keyword(&word)
                    .map(str::to_string)
                    .unwrap_or(word);
                if word != "external" && word != "inbound" {
                    return Err(ParseError::UnexpectedToken {
                        expected:
                            "`external` or `inbound` after `->` on a local listen (`EVENT -> external { … }`, `EVENT -> inbound \"name\" answers ANSWER { … }`)"
                                .to_string(),
                        got: word,
                        span: self.peek_span(),
                    });
                }
                let inbound = if word == "inbound" {
                    let name = match self.peek().clone() {
                        TokenKind::String(name) => {
                            self.advance();
                            name
                        }
                        other => {
                            return Err(ParseError::UnexpectedToken {
                                expected: "a hook name string after `-> inbound`".to_string(),
                                got: format!("{other:?}"),
                                span: self.peek_span(),
                            })
                        }
                    };
                    let answers_word = self.expect_ident()?;
                    let answers_word = self
                        .lang
                        .canon_keyword(&answers_word)
                        .map(str::to_string)
                        .unwrap_or(answers_word);
                    if answers_word != "answers" {
                        return Err(ParseError::UnexpectedToken {
                            expected: "`answers EVENT` after the hook name of `-> inbound`"
                                .to_string(),
                            got: answers_word,
                            span: self.peek_span(),
                        });
                    }
                    let answers = self.expect_ident()?;
                    let answers = self.canon_event_name(answers);
                    let verifies = if matches!(self.peek(), TokenKind::Identifier(w) if self.lang.canon_keyword(w).unwrap_or(w.as_str()) == "verifies") {
                        self.advance();
                        let mut take = |what: &str| match self.peek().clone() {
                            TokenKind::String(value) => {
                                self.advance();
                                Ok(value)
                            }
                            other => Err(ParseError::UnexpectedToken {
                                expected: format!("{what} string after `verifies`"),
                                got: format!("{other:?}"),
                                span: self.peek_span(),
                            }),
                        };
                        let header = take("a header name")?;
                        let credential = take("a credential name")?;
                        Some(InboundVerifies { header, credential })
                    } else {
                        None
                    };
                    Some(InboundListen { name, answers, verifies })
                } else {
                    None
                };
                if !matches!(self.peek(), TokenKind::LBrace) {
                    return Err(ParseError::UnexpectedToken {
                        expected: "a payload `{ … }` after `-> external` / `-> inbound` (write `{}` for none)"
                            .to_string(),
                        got: format!("{:?}", self.peek()),
                        span: self.peek_span(),
                    });
                }
                let payload = self.parse_event_field_list()?;
                if matches!(self.peek(), TokenKind::Newline) {
                    self.advance();
                }
                let (description, synonyms, tier) = self.parse_event_annotations()?;
                Ok(Listen {
                    event: self.canon_event_name(first),
                    triggers: None,
                    guard: None,
                    payload_mapping: Vec::new(),
                    payload,
                    description,
                    synonyms,
                    tier,
                    target_trait: None,
                    external: inbound.is_none(),
                    inbound,
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
                    event: self.canon_event_name(first),
                    triggers: None,
                    guard: None,
                    payload_mapping: Vec::new(),
                    payload: Vec::new(),
                    description,
                    synonyms,
                    tier,
                    target_trait: None,
                    external: false,
                    inbound: None,
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
                // Only the LAST segment is an event name — the leading
                // segments are an orbital / trait alias the author chose.
                let combined = if matches!(self.peek(), TokenKind::Dot) {
                    self.advance();
                    let third = self.expect_ident()?;
                    let third = self.canon_event_name(third);
                    format!("{first}.{second}.{third}")
                } else {
                    let second = self.canon_event_name(second);
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
                // `-> Target.TRIGGER` — an orbital import's route into one of
                // its imported traits.
                let (target_trait, triggers) = if matches!(self.peek(), TokenKind::Dot) {
                    self.advance();
                    (Some(triggers), self.expect_ident()?)
                } else {
                    (None, triggers)
                };
                let triggers = self.canon_event_name(triggers);
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
                if let TokenKind::Sigil(raw) = self.peek().clone() {
                    let annotation = self.canon_annotation_sigil(&raw);
                    if matches!(annotation.as_str(), "description" | "synonyms" | "tier") {
                        return Err(ParseError::SourcedListenAnnotation {
                            annotation,
                            span: self.peek_span(),
                        });
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
                    target_trait,
                    external: false,
                    inbound: None,
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
                let (background, visible) = if matches!(self.peek(), TokenKind::LBracket) {
                    self.parse_tick_tags()?
                } else {
                    (false, false)
                };
                if matches!(self.peek(), TokenKind::Newline) {
                    self.advance();
                }
                let mut guard = None;
                let mut guard_spans = Vec::new();
                if self.check_ident("when") {
                    self.advance();
                    let (g, spans) = self.capture_expr_spans(|p| p.parse_sexpr_or_value())?;
                    guard = Some(g);
                    guard_spans = spans;
                    if matches!(self.peek(), TokenKind::Newline) {
                        self.advance();
                    }
                }
                let mut effects = Vec::new();
                let mut effect_spans = Vec::new();
                loop {
                    self.skip_newlines();
                    if matches!(self.peek(), TokenKind::LParen) {
                        let eff_span = self.peek_span();
                        let (eff, spans) = self.capture_expr_spans(|p| p.parse_sexpr())?;
                        reject_match_effect_head(&eff, eff_span)?;
                        effects.push(eff);
                        effect_spans.push(spans);
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
                    background,
                    visible,
                    span,
                    guard_spans,
                    effect_spans,
                });
            } else {
                break;
            }
        }
        Ok(out)
    }

    /// `[background]` / `[visible]` after a tick's interval; any other tag is rejected.
    fn parse_tick_tags(&mut self) -> Result<(bool, bool), ParseError> {
        let open_span = self.peek_span();
        self.expect_kind(&TokenKind::LBracket, "[")?;
        let mut background = false;
        let mut visible = false;
        loop {
            let tag_span = self.peek_span();
            let tag = self.expect_ident()?;
            let tag = self.lang.canon_tag(&tag).map(str::to_string).unwrap_or(tag);
            match tag.as_str() {
                "background" => background = true,
                "visible" => visible = true,
                _ => {
                    return Err(ParseError::InvalidTickTag {
                        value: tag,
                        span: tag_span,
                    })
                }
            }
            if matches!(self.peek(), TokenKind::Comma) {
                self.advance();
                continue;
            }
            break;
        }
        self.expect_kind(&TokenKind::RBracket, "]")?;
        if background && visible {
            return Err(ParseError::ConflictingTickTags { span: open_span });
        }
        Ok((background, visible))
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
                if let TokenKind::Identifier(raw_suf) = self.peek().clone() {
                    let suf = self.lang.canon_unit(&raw_suf).unwrap_or(raw_suf.as_str());
                    if matches!(suf, "s" | "m" | "h" | "d" | "ms") {
                        self.advance();
                        let ms = duration_to_ms(n as u64, suf);
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
                if type_expr_contains_match(&type_expr) {
                    return Err(ParseError::InvalidType {
                        span,
                        message: format!(
                            "ELOLO_MATCH_POSITION_UNSUPPORTED: config field '{}' cannot use \
                             `match` — match is only legal as an emits/listens payload \
                             field's whole type",
                            name
                        ),
                    });
                }
                let required = if matches!(self.peek(), TokenKind::Bang) {
                    self.advance();
                    true
                } else {
                    false
                };
                let mut default = None;
                let mut event_refs = Vec::new();
                if matches!(self.peek(), TokenKind::Equals) {
                    self.advance();
                    let (value, refs) = self.parse_config_value()?;
                    default = Some(value);
                    event_refs = refs;
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
                let field = ConfigField {
                    name,
                    type_expr,
                    default,
                    label,
                    description,
                    synonyms,
                    tier,
                    required,
                    event_refs,
                    span,
                };
                reject_duplicate_config_key(&out, &field)?;
                out.push(field);
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
    ) -> Result<
        (
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
        ),
        ParseError,
    > {
        let mut label = None;
        let mut description = None;
        let mut synonyms = None;
        let mut tier = None;
        loop {
            self.skip_newlines();
            let sigil_name = match self.peek().clone() {
                TokenKind::Sigil(s) => self.canon_annotation_sigil(&s),
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
                TokenKind::Comma | TokenKind::RBrace | TokenKind::Newline | TokenKind::Eof => {
                    return false
                }
                _ => i += 1,
            }
        }
        false
    }
}

/// A `config { … }` block naming the same key twice: the later entry would
/// silently shadow (or be shadowed by) the earlier one.
fn reject_duplicate_config_key(
    existing: &[ConfigField],
    field: &ConfigField,
) -> Result<(), ParseError> {
    let Some(first) = existing.iter().find(|f| f.name == field.name) else {
        return Ok(());
    };
    Err(ParseError::InvalidType {
        span: field.span,
        message: format!(
            "ELOLO_DUPLICATE_CONFIG_KEY: config key '{}' is stated twice in the same block \
             (first at {}:{}, again at {}:{}) — keep one entry",
            field.name, first.span.line, first.span.col, field.span.line, field.span.col
        ),
    })
}
