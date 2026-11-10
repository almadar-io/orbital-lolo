//! Entity / field / persistence parsing for the .lolo parser.

use super::*;

/// The entity header bracket's tags.
#[derive(Debug, Default)]
pub(crate) struct EntityTags {
    pub persistence: Option<Persistence>,
    pub collection: Option<String>,
    pub shared: bool,
    pub identity: bool,
    pub browser: bool,
    pub own: bool,
    pub retired_words: Vec<RetiredWord>,
    pub store: Option<String>,
}

impl Parser {
    pub(crate) fn parse_entity(&mut self) -> Result<Entity, ParseError> {
        let span = self.peek_span();
        self.expect_keyword("entity")?;
        let name = self.expect_ident()?;

        // Optional `:: TypeName [args]` annotation above the body.
        let type_annotation = self.try_parse_type_annotation()?;

        let EntityTags {
            persistence,
            collection,
            shared,
            identity,
            browser,
            retired_words,
            store,
            ..
        } = if matches!(self.peek(), TokenKind::LBracket) {
            self.parse_persistence_tag()?
        } else {
            EntityTags::default()
        };

        let brace_form = matches!(self.peek(), TokenKind::LBrace);
        if brace_form {
            self.advance();
        }
        self.skip_newlines();

        let mut fields = Vec::new();
        let mut access = ExpectedAccess::default();
        let mut invariants = Vec::new();
        let mut indexes = Vec::new();
        let mut instances: Vec<orbital_core_base::schema::SObject> = Vec::new();
        let mut locale_instances: std::collections::BTreeMap<
            String,
            Vec<orbital_core_base::schema::SObject>,
        > = std::collections::BTreeMap::new();
        loop {
            self.skip_newlines();
            if brace_form && matches!(self.peek(), TokenKind::RBrace) {
                self.advance();
                break;
            }
            if let Some(locale) = self.at_instances_block() {
                let block_span = self.peek_span();
                let seen = match &locale {
                    Some(l) => locale_instances.contains_key(l),
                    None => !instances.is_empty(),
                };
                if seen {
                    return Err(ParseError::InvalidType {
                        span: block_span,
                        message: match &locale {
                            Some(l) => format!("ELOLO_DUPLICATE_INSTANCES: entity declares `instances {l}` more than once"),
                            None => "ELOLO_DUPLICATE_INSTANCES: entity declares `instances` more than once".to_string(),
                        },
                    });
                }
                self.advance();
                if locale.is_some() {
                    self.advance();
                }
                let rows = self.parse_instance_rows(block_span)?;
                match locale {
                    Some(l) => {
                        locale_instances.insert(l, rows);
                    }
                    None => instances = rows,
                }
                self.expect_newline_or_eof()?;
                continue;
            }
            if let Some(name) = self.at_access_directive() {
                self.parse_access_directive(&name, &mut access)?;
                continue;
            }
            if matches!(self.peek(), TokenKind::Sigil(raw) if self.canon_annotation_sigil(raw) == "invariant")
            {
                self.advance();
                invariants.push(self.parse_sexpr_or_value()?);
                self.expect_newline_or_eof()?;
                continue;
            }
            if matches!(self.peek(), TokenKind::Sigil(raw) if self.canon_annotation_sigil(raw) == "index")
            {
                indexes.push(self.parse_index_directive()?);
                continue;
            }
            if self.is_entity_field_start() {
                fields.push(self.parse_field_decl()?);
            } else {
                break;
            }
        }

        // ELOLO_TRAIT_TYPE_FORBIDDEN_IN_ENTITY — `trait`, `slot`, and
        // `pattern` are for structural composition only (config blocks).
        // Entity rows hold per-row domain data; mixing these reference
        // semantics into entity fields would explode edge cases. Reject
        // here so authors get a clear error at parse time.
        let allow: &[&str] = if matches!(persistence, Some(Persistence::Runtime)) {
            RUNTIME_ENTITY_ALLOWED
        } else {
            &[]
        };
        for f in &fields {
            if let Some(ty_name) = type_expr_config_only_type_allowing(&f.type_expr, allow) {
                return Err(ParseError::InvalidType {
                    span: f.span,
                    message: format!(
                        "ELOLO_CONFIG_TYPE_FORBIDDEN_IN_ENTITY: entity field '{}' cannot use the `{}` type — `{}` is for config blocks only",
                        f.name, ty_name, ty_name
                    ),
                });
            }
        }

        Ok(Entity {
            name,
            type_annotation,
            persistence,
            collection,
            shared,
            identity,
            browser,
            retired_words,
            store,
            instances,
            locale_instances,
            read_policy: access.read_policy,
            access_waivers: access.access_waivers,
            create_policy: access.create_policy,
            update_policy: access.update_policy,
            delete_policy: access.delete_policy,
            invariants,
            indexes,
            fields,
            span,
        })
    }

    /// One `@index { fields: [..], orderBy?: ".." }` access path, in `fetch`'s own record keys.
    fn parse_index_directive(
        &mut self,
    ) -> Result<orbital_core_base::schema::EntityIndex, ParseError> {
        let directive_span = self.peek_span();
        self.advance();
        let invalid = |code: &str, detail: &str| ParseError::InvalidType {
            span: directive_span,
            message: format!("{code}: {detail}"),
        };
        let value = self.parse_sexpr_or_value()?;
        self.expect_newline_or_eof()?;
        let SExpression::Object(record) = value else {
            return Err(invalid(
                "ELOLO_INDEX_NOT_A_RECORD",
                "`@index` takes one record, e.g. `@index { fields: [\"graphId\"], orderBy: \"weight:desc\" }`",
            ));
        };
        if let Some(key) = record.keys().find(|k| !matches!(k.as_str(), "fields" | "orderBy")) {
            return Err(invalid(
                "ELOLO_INDEX_UNKNOWN_KEY",
                &format!("`@index` has no key `{key}`; its keys are `fields` and `orderBy`"),
            ));
        }
        let fields_invalid = || {
            invalid(
                "ELOLO_INDEX_FIELDS_INVALID",
                "`@index` needs `fields`: a non-empty list of field-name strings",
            )
        };
        let Some(SExpression::Array(items)) = record.get("fields") else {
            return Err(fields_invalid());
        };
        let fields = items
            .iter()
            .map(|item| match item {
                SExpression::String(name) if !name.is_empty() => Some(name.to_string()),
                _ => None,
            })
            .collect::<Option<Vec<String>>>()
            .filter(|names| !names.is_empty())
            .ok_or_else(fields_invalid)?;
        let order_by = match record.get("orderBy") {
            None => None,
            Some(SExpression::String(order)) if !order.is_empty() => Some(order.clone()),
            Some(_) => {
                return Err(invalid(
                    "ELOLO_INDEX_ORDER_BY_INVALID",
                    "`orderBy` is a string like \"weight\" or \"weight:desc\"",
                ))
            }
        };
        Ok(orbital_core_base::schema::EntityIndex { fields, order_by: order_by.map(|v| v.into_inner()) })
    }

    /// The `read|create|update|delete` op at a `@<op>` directive, if the cursor is on one.
    pub(crate) fn at_access_directive(&self) -> Option<String> {
        let TokenKind::Sigil(raw_name) = self.peek() else {
            return None;
        };
        let name = self.canon_annotation_sigil(raw_name);
        matches!(name.as_str(), "read" | "create" | "update" | "delete").then_some(name)
    }

    /// One `@<op> <expr>` policy or `@<op> none "<reason>"` waiver, recorded in `access`. The single parser of
    /// access directives: an entity body and an `expects entity` shape both read them here.
    pub(crate) fn parse_access_directive(
        &mut self,
        name: &str,
        access: &mut ExpectedAccess,
    ) -> Result<(), ParseError> {
        let directive_span = self.peek_span();
        self.advance();
        let duplicate = |access: &ExpectedAccess| {
            access.access_waivers.iter().any(|(op, _)| op == name || (name == "read" && op == "read server"))
                || match name {
                    "read" => access.read_policy.is_some(),
                    "create" => access.create_policy.is_some(),
                    "update" => access.update_policy.is_some(),
                    _ => access.delete_policy.is_some(),
                }
        };
        let dup_error = || ParseError::InvalidType {
            span: directive_span,
            message: format!(
                "ELOLO_DUPLICATE_ACCESS_DIRECTIVE: entity declares `@{name}` more than once"
            ),
        };
        // `@<op> none "<reason>"` — the omission, declared. Same runtime behaviour as leaving the directive out;
        // recorded so a lint can tell a considered omission from a forgotten one. The reason is mandatory.
        if name == "read" && matches!(self.peek(), TokenKind::Identifier(id) if id == "server") {
            self.advance();
            let reason = match self.peek().clone() {
                TokenKind::String(text) if !text.trim().is_empty() => {
                    self.advance();
                    text
                }
                _ => {
                    return Err(ParseError::InvalidType {
                        span: directive_span,
                        message: "ELOLO_ACCESS_WAIVER_NEEDS_REASON: `@read server` must state why only the server program reads these rows, e.g. `@read server \"looked up to resolve a request's viewer, before any viewer exists\"`".to_string(),
                    });
                }
            };
            self.expect_newline_or_eof()?;
            if duplicate(access) {
                return Err(dup_error());
            }
            access.access_waivers.push(("read server".to_string(), reason));
            return Ok(());
        }
        if matches!(self.peek(), TokenKind::Identifier(id) if id == "none") {
            self.advance();
            let reason = match self.peek().clone() {
                TokenKind::String(text) => {
                    self.advance();
                    text
                }
                _ => {
                    return Err(ParseError::InvalidType {
                        span: directive_span,
                        message: format!(
                            "ELOLO_ACCESS_WAIVER_NEEDS_REASON: `@{name} none` must state why no per-row predicate can express this rule, e.g. `@{name} none \"visibility follows Ticket, which a per-row predicate cannot join\"`"
                        ),
                    });
                }
            };
            if reason.trim().is_empty() {
                return Err(ParseError::InvalidType {
                    span: directive_span,
                    message: format!(
                        "ELOLO_ACCESS_WAIVER_NEEDS_REASON: `@{name} none` was given an empty reason"
                    ),
                });
            }
            self.expect_newline_or_eof()?;
            if duplicate(access) {
                return Err(dup_error());
            }
            access.access_waivers.push((name.to_string(), reason));
            return Ok(());
        }
        let expr = self.parse_boolean_slot("access policy")?;
        self.expect_newline_or_eof()?;
        if duplicate(access) {
            return Err(dup_error());
        }
        let slot = match name {
            "read" => &mut access.read_policy,
            "create" => &mut access.create_policy,
            "update" => &mut access.update_policy,
            _ => &mut access.delete_policy,
        };
        *slot = Some(expr);
        Ok(())
    }

    pub(crate) fn is_entity_field_start(&self) -> bool {
        // A field starts with an identifier (or `derived`) followed by `:`.
        // Entity bodies are closed by `}`, so no keywords need to be reserved
        // at this scope — any `identifier :` is unambiguously a field. This
        // lets domain-natural field names like `to`, `from`, `type`, `state`
        // declare entity fields without colliding with orbital-level keywords.
        match self.peek() {
            TokenKind::Identifier(_) => matches!(self.peek_n(1), TokenKind::Colon),
            _ => false,
        }
    }

    /// Parses the entity header bracket: a comma-separated, order-independent
    /// list of tags. At most one persistence mode (`persistent` — optionally
    /// `: collection` — or `runtime`) plus the orthogonal `shared` and
    /// `identity` flags, e.g. `[runtime, shared]`, `[persistent: bars]`,
    /// `[shared]`, `[persistent: people, identity]`.
    pub(crate) fn parse_persistence_tag(&mut self) -> Result<EntityTags, ParseError> {
        self.parse_tag_list(false)
    }

    /// The `[...]` tag list; `allow_self` admits the `self` mark, which only an `expects entity` carries.
    pub(crate) fn parse_tag_list(&mut self, allow_self: bool) -> Result<EntityTags, ParseError> {
        self.expect_kind(&TokenKind::LBracket, "[")?;
        let mut persistence: Option<Persistence> = None;
        let mut collection: Option<String> = None;
        let mut shared = false;
        let mut identity = false;
        let mut browser = false;
        let mut own = false;
        let mut retired_words: Vec<RetiredWord> = Vec::new();
        let mut store: Option<String> = None;
        loop {
            let tag_span = self.peek_span();
            let kind = self.expect_ident()?;
            let kind = self
                .lang
                .canon_tag(&kind)
                .map(str::to_string)
                .unwrap_or(kind);
            match kind.as_str() {
                "shared" => shared = true,
                "identity" => identity = true,
                "browser" => browser = true,
                "self" if allow_self => own = true,
                "local" | "mock" => retired_words.push(RetiredWord {
                    word: kind,
                    span: tag_span,
                }),
                "store" => {
                    self.expect_kind(&TokenKind::Colon, "':' after `store`")?;
                    store = Some(self.expect_ident()?);
                }
                "persistent" | "runtime" => {
                    let mode = if kind == "persistent" {
                        Persistence::Persistent
                    } else {
                        Persistence::Runtime
                    };
                    if let Some(existing) = &persistence {
                        if *existing != mode {
                            return Err(ParseError::InvalidPersistenceMode {
                                value: kind,
                                span: tag_span,
                            });
                        }
                    }
                    persistence = Some(mode);
                    if matches!(self.peek(), TokenKind::Colon) {
                        self.advance();
                        // Could be an identifier or a string literal.
                        collection = match self.peek().clone() {
                            TokenKind::Identifier(s) => {
                                self.advance();
                                Some(s)
                            }
                            TokenKind::String(s) => {
                                self.advance();
                                Some(s)
                            }
                            _ => None,
                        };
                    }
                }
                _ => {
                    return Err(ParseError::InvalidPersistenceMode {
                        value: kind,
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
        Ok(EntityTags {
            persistence,
            collection,
            shared,
            identity,
            browser,
            own,
            retired_words,
            store,
        })
    }

    /// `instances [` (the default rows, `Some(None)`) or `instances <lang> [` (one
    /// locale's rows, `Some(Some(lang))`) at the start of an entity-body line.
    fn at_instances_block(&self) -> Option<Option<String>> {
        let keyword = matches!(self.peek(), TokenKind::Identifier(name)
            if name == "instances" || self.lang.canon_any(name) == Some("instances"));
        if !keyword {
            return None;
        }
        match (self.peek_n(1), self.peek_n(2)) {
            (TokenKind::LBracket, _) => Some(None),
            (TokenKind::Identifier(lang), TokenKind::LBracket) => Some(Some(lang.clone())),
            _ => None,
        }
    }

    /// `[ { field: value, … }, … ]` — each row an object literal.
    fn parse_instance_rows(
        &mut self,
        span: Span,
    ) -> Result<Vec<orbital_core_base::schema::SObject>, ParseError> {
        let SExpression::Array(rows) = self.parse_sexpr_or_value()? else {
            return Err(ParseError::InvalidType {
                span,
                message: "ELOLO_INSTANCES_NOT_A_LIST: `instances` takes a list of rows, `instances [ { … }, … ]`".to_string(),
            });
        };
        rows.into_iter()
            .map(|row| match row {
                SExpression::Object(fields) => Ok(fields.into_inner().into_iter().collect()),
                other => Err(ParseError::InvalidType {
                    span,
                    message: format!(
                        "ELOLO_INSTANCE_NOT_A_ROW: every `instances` entry is a row object `{{ field: value }}`, got {other:?}"
                    ),
                }),
            })
            .collect()
    }

    pub(crate) fn parse_field_decl(&mut self) -> Result<EntityField, ParseError> {
        let span = self.peek_span();
        let name = self.expect_ident()?;
        self.expect_kind(&TokenKind::Colon, ":")?;
        let type_expr = self.parse_type_expr()?;
        // Optional `!` modifier (Bang token). Question is only used as a
        // payload sigil in this position; nullable types use `T?` syntax
        // resolved by `parse_type_atom` instead.
        let mut required = false;
        let mut nullable = false;
        if matches!(self.peek(), TokenKind::Bang) {
            required = true;
            self.advance();
        }
        // Trailing annotations: flag-valued `@key`/`@unique`/`@intrinsic` and
        // string-valued `@description "..."`/`@synonyms "..."`. Lexer produced
        // the sigils as Sigil tokens; the string arms consume the following
        // String token (authoring/build-time metadata for catalog + curation).
        let mut is_key = false;
        let mut is_unique = false;
        let mut is_intrinsic = false;
        let mut description = None;
        let mut synonyms = None;
        let mut mock = None;
        let mut derived = None;
        let mut display = None;
        while let TokenKind::Sigil(raw) = self.peek().clone() {
            let name = self.canon_annotation_sigil(&raw);
            match name.as_str() {
                "key" => {
                    is_key = true;
                    self.advance();
                }
                "unique" => {
                    is_unique = true;
                    self.advance();
                }
                "intrinsic" => {
                    is_intrinsic = true;
                    self.advance();
                }
                "derived" => {
                    self.advance();
                    derived = Some(self.parse_sexpr_or_value()?);
                }
                "display" => {
                    self.advance();
                    display = Some(self.expect_ident()?);
                }
                "description" | "synonyms" | "mock" => {
                    self.advance();
                    let value = match self.peek().clone() {
                        TokenKind::String(s) => {
                            self.advance();
                            s
                        }
                        other => {
                            return Err(ParseError::UnexpectedToken {
                                expected: format!("string value after @{}", name),
                                got: format!("{other:?}"),
                                span: self.peek_span(),
                            });
                        }
                    };
                    match name.as_str() {
                        "description" => description = Some(value),
                        "synonyms" => synonyms = Some(value),
                        _ => mock = Some(value),
                    }
                }
                _ => break,
            }
        }
        // Optional `= sexpr` default value.
        let mut default = None;
        if matches!(self.peek(), TokenKind::Equals) {
            let equals_span = self.peek_span();
            self.advance();
            default = Some(self.parse_sexpr_or_value()?);
            if derived.is_some() {
                return Err(ParseError::InvalidType {
                    span: equals_span,
                    message: format!(
                        "ELOLO_DERIVED_WITH_DEFAULT: field '{name}' is `@derived`, so the kernel computes it on every write and it has no default"
                    ),
                });
            }
        }
        // Detect leading nullable from a Question after a primitive identifier (rare).
        if matches!(type_expr, TypeExpr::Optional(_)) {
            nullable = true;
        }
        self.expect_newline_or_eof()?;
        Ok(EntityField {
            name,
            type_expr,
            required,
            nullable,
            is_key,
            is_unique,
            is_intrinsic,
            description,
            synonyms,
            mock,
            default,
            derived,
            display,
            span,
        })
    }
}
