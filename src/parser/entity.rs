//! Entity / field / persistence parsing for the .lolo parser.

use super::*;

impl Parser {
    pub(crate) fn parse_entity(&mut self) -> Result<Entity, ParseError> {
        let span = self.peek_span();
        self.expect_keyword("entity")?;
        let name = self.expect_ident()?;

        // Optional `:: TypeName [args]` annotation above the body.
        let type_annotation = self.try_parse_type_annotation()?;

        let (persistence, collection, shared) = if matches!(self.peek(), TokenKind::LBracket) {
            self.parse_persistence_tag()?
        } else {
            (None, None, false)
        };

        let brace_form = matches!(self.peek(), TokenKind::LBrace);
        if brace_form {
            self.advance();
        }
        self.skip_newlines();

        let mut fields = Vec::new();
        loop {
            self.skip_newlines();
            if brace_form && matches!(self.peek(), TokenKind::RBrace) {
                self.advance();
                break;
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
        for f in &fields {
            if let Some(ty_name) = type_expr_config_only_type(&f.type_expr) {
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
            fields,
            span,
        })
    }

    pub(crate) fn is_entity_field_start(&self) -> bool {
        // A field starts with an identifier (or `derived`) followed by `:`.
        // Entity bodies are closed by `}`, so no keywords need to be reserved
        // at this scope — any `identifier :` is unambiguously a field. This
        // lets domain-natural field names like `to`, `from`, `type`, `state`
        // declare entity fields without colliding with orbital-level keywords.
        match self.peek() {
            TokenKind::Identifier(name) => {
                matches!(self.peek_n(1), TokenKind::Colon)
                    || (name == "derived" && matches!(self.peek_n(1), TokenKind::Identifier(_)))
            }
            _ => false,
        }
    }

    /// Parses the entity header bracket: a comma-separated, order-independent
    /// list of tags. At most one persistence mode (`persistent` — optionally
    /// `: collection` — or `runtime`) plus the orthogonal `shared` flag, e.g.
    /// `[runtime, shared]`, `[persistent: bars]`, `[shared]`.
    pub(crate) fn parse_persistence_tag(
        &mut self,
    ) -> Result<(Option<Persistence>, Option<String>, bool), ParseError> {
        self.expect_kind(&TokenKind::LBracket, "[")?;
        let mut persistence: Option<Persistence> = None;
        let mut collection: Option<String> = None;
        let mut shared = false;
        loop {
            let tag_span = self.peek_span();
            let kind = self.expect_ident()?;
            match kind.as_str() {
                "shared" => shared = true,
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
        Ok((persistence, collection, shared))
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
        loop {
            match self.peek().clone() {
                TokenKind::Sigil(name) if name == "key" => {
                    is_key = true;
                    self.advance();
                }
                TokenKind::Sigil(name) if name == "unique" => {
                    is_unique = true;
                    self.advance();
                }
                TokenKind::Sigil(name) if name == "intrinsic" => {
                    is_intrinsic = true;
                    self.advance();
                }
                TokenKind::Sigil(name) if name == "description" || name == "synonyms" => {
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
                    if name == "description" {
                        description = Some(value);
                    } else {
                        synonyms = Some(value);
                    }
                }
                _ => break,
            }
        }
        // Optional `= sexpr` default value, or `= derived (...)` derivation.
        let mut default = None;
        let mut derived = None;
        if matches!(self.peek(), TokenKind::Equals) {
            self.advance();
            if self.check_ident("derived") {
                self.advance();
                let expr = self.parse_sexpr_or_value()?;
                derived = Some(expr);
            } else {
                let expr = self.parse_sexpr_or_value()?;
                default = Some(expr);
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
            default,
            derived,
            span,
        })
    }
}
