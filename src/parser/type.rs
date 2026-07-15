//! Type-alias / type-body / type-expression parsing for the .lolo parser.

use super::*;

impl Parser {
    pub(crate) fn parse_type_alias(&mut self) -> Result<TypeAlias, ParseError> {
        let span = self.peek_span();
        self.expect_keyword("type")?;
        let name = self.expect_ident()?;
        // Optional single-ident type parameter: `type Browsable T = ...`.
        // If the next token is an Identifier, it's the parameter name; the
        // `=` must follow it. If the next token is `=`, there's no parameter.
        let type_param = if let TokenKind::Identifier(p) = self.peek().clone() {
            if !matches!(self.peek_n(1), TokenKind::Equals) {
                // Identifier not followed by `=` — must be malformed; let
                // the `expect_kind(Equals)` below produce the real error.
                None
            } else {
                self.advance();
                Some(p)
            }
        } else {
            None
        };
        self.expect_kind(&TokenKind::Equals, "=")?;
        let body = self.parse_type_body()?;
        self.expect_newline_or_eof()?;
        Ok(TypeAlias {
            name,
            type_param,
            body,
            span,
        })
    }

    pub(crate) fn parse_type_body(&mut self) -> Result<TypeBody, ParseError> {
        let first = self.parse_type_body_atom()?;
        // `A & B & C ...` composition. Flatten into one Composition node.
        if matches!(self.peek(), TokenKind::Ampersand) {
            let mut parts = vec![first];
            while matches!(self.peek(), TokenKind::Ampersand) {
                self.advance();
                parts.push(self.parse_type_body_atom()?);
            }
            return Ok(TypeBody::Composition(parts));
        }
        Ok(first)
    }

    /// Parse a single type-body atom — either a shape keyword body
    /// (`Entity { ... }`, `Trait ... -> T`, `Page ...`, `Orbital { ... }`),
    /// or the legacy enum / struct / nominal-alias forms.
    pub(crate) fn parse_type_body_atom(&mut self) -> Result<TypeBody, ParseError> {
        // Shape keyword dispatch. Each of these produces a new shape variant
        // that the type resolver (Stage 2) will substitute + check.
        if self.check_ident("Entity") {
            self.advance();
            return self.parse_type_entity_body();
        }
        if self.check_ident("Trait") {
            self.advance();
            return self.parse_type_trait_body();
        }
        if self.check_ident("Page") {
            self.advance();
            return self.parse_type_page_body();
        }
        if self.check_ident("Orbital") {
            self.advance();
            return self.parse_type_orbital_body();
        }
        if self.check_ident("Event") {
            self.advance();
            return self.parse_type_event_body();
        }

        // Struct: `{ field: T, ... }`. Field syntax mirrors entity fields:
        // `name : T[!]` where the optional `!` marks the field required.
        // The `!` marker is captured into the third tuple element so the
        // lowerer can stamp `FieldDefinition.required = Some(true)` —
        // matching lolo's authoring intent (unmarked = optional in TS).
        if matches!(self.peek(), TokenKind::LBrace) {
            self.advance();
            let mut fields = Vec::new();
            loop {
                self.skip_newlines();
                if matches!(self.peek(), TokenKind::RBrace) {
                    self.advance();
                    break;
                }
                let name = self.expect_ident()?;
                self.expect_kind(&TokenKind::Colon, ":")?;
                let ty = self.parse_type_expr()?;
                let required = if matches!(self.peek(), TokenKind::Bang) {
                    self.advance();
                    true
                } else {
                    false
                };
                fields.push((name, ty, required));
                if matches!(self.peek(), TokenKind::Comma) {
                    self.advance();
                }
                self.skip_newlines();
            }
            return Ok(TypeBody::Struct(fields));
        }

        // Possibly an enum: `a | b | c`. We parse the first identifier and look ahead.
        let first = self.expect_ident()?;
        if matches!(self.peek(), TokenKind::Pipe) {
            let mut variants = vec![first];
            while matches!(self.peek(), TokenKind::Pipe) {
                self.advance();
                variants.push(self.expect_ident()?);
            }
            return Ok(TypeBody::Enum(variants));
        }
        Ok(TypeBody::Alias(TypeExpr::Named(first)))
    }

    /// Parse the body of a `type Foo = Entity ...` shape.
    /// Accepts optional persistence tag + optional field block. When both
    /// are omitted, the alias names an empty entity shape (useful as a
    /// base for composition).
    pub(crate) fn parse_type_entity_body(&mut self) -> Result<TypeBody, ParseError> {
        // `TypeBody::Entity` (the `type X = Entity [...] { ... }` alias shape)
        // has no `shared` field yet — only orbital-level `entity` headers
        // carry it through to `EntityDefinition.shared` (lower/entity.rs).
        let (persistence, collection, _shared) = if matches!(self.peek(), TokenKind::LBracket) {
            self.parse_persistence_tag()?
        } else {
            (None, None, false)
        };
        let mut fields = Vec::new();
        if matches!(self.peek(), TokenKind::LBrace) {
            self.advance();
            loop {
                self.skip_newlines();
                if matches!(self.peek(), TokenKind::RBrace) {
                    self.advance();
                    break;
                }
                if self.is_entity_field_start() {
                    fields.push(self.parse_field_decl()?);
                } else {
                    break;
                }
            }
        }
        Ok(TypeBody::Entity {
            persistence,
            collection,
            fields,
        })
    }

    /// Parse the body of a `type Foo = Trait Upstream.traits.X -> T` shape.
    /// `upstream` is the dotted ref path (optional for fully-open trait
    /// shapes). `linked_entity` accepts either a named type or the alias's
    /// bound parameter (e.g. `T`).
    pub(crate) fn parse_type_trait_body(&mut self) -> Result<TypeBody, ParseError> {
        // Optional upstream dotted ref — only present if the next token is
        // an Identifier followed by `.`. Otherwise `T` on its own (after
        // the `->`) is still valid; we defer that decision.
        let upstream = if matches!(self.peek(), TokenKind::Identifier(_))
            && matches!(self.peek_n(1), TokenKind::Dot)
        {
            Some(self.parse_dotted_ident()?)
        } else {
            None
        };
        // Optional `-> T` or `-> Entity`.
        let linked_entity = if matches!(self.peek(), TokenKind::Arrow | TokenKind::UnicodeArrow) {
            self.advance();
            Some(self.parse_type_expr()?)
        } else {
            None
        };
        Ok(TypeBody::Trait {
            upstream,
            linked_entity,
        })
    }

    /// Parse the body of a `type Foo = Page "/path" -> Trait[...]` shape.
    /// Path is optional (shape may be parameterized); traits are a list of
    /// `TypeExpr` (each resolves to a trait shape at type-check time).
    pub(crate) fn parse_type_page_body(&mut self) -> Result<TypeBody, ParseError> {
        let path = if let TokenKind::String(s) = self.peek().clone() {
            self.advance();
            Some(s)
        } else {
            None
        };
        let mut trait_types = Vec::new();
        if matches!(self.peek(), TokenKind::Arrow | TokenKind::UnicodeArrow) {
            self.advance();
            trait_types.push(self.parse_type_expr()?);
            while matches!(self.peek(), TokenKind::Comma) {
                self.advance();
                trait_types.push(self.parse_type_expr()?);
            }
        }
        Ok(TypeBody::Page { path, trait_types })
    }

    /// Parse the body of a `type Foo = Orbital { entity: T; traits: ...; page: ... }` shape.
    /// The three slots are order-independent; each may appear at most once.
    pub(crate) fn parse_type_orbital_body(&mut self) -> Result<TypeBody, ParseError> {
        self.expect_kind(&TokenKind::LBrace, "{")?;
        let mut entity: Option<TypeExpr> = None;
        let mut trait_types: Vec<TypeExpr> = Vec::new();
        let mut page_type: Option<TypeExpr> = None;
        loop {
            self.skip_newlines();
            if matches!(self.peek(), TokenKind::RBrace) {
                self.advance();
                break;
            }
            let slot = self.expect_ident()?;
            self.expect_kind(&TokenKind::Colon, ":")?;
            match slot.as_str() {
                "entity" => {
                    entity = Some(self.parse_type_expr()?);
                }
                "traits" => {
                    trait_types.push(self.parse_type_expr()?);
                    while matches!(self.peek(), TokenKind::Comma) {
                        self.advance();
                        trait_types.push(self.parse_type_expr()?);
                    }
                }
                "page" => {
                    page_type = Some(self.parse_type_expr()?);
                }
                _ => {
                    return Err(ParseError::UnexpectedToken {
                        expected: "entity, traits, or page".to_string(),
                        got: slot,
                        span: self.peek_span(),
                    });
                }
            }
            // Optional trailing `;` or `,` between slots.
            if matches!(self.peek(), TokenKind::Comma) {
                self.advance();
            }
            self.skip_newlines();
        }
        Ok(TypeBody::Orbital {
            entity,
            trait_types,
            page_type,
        })
    }

    /// Parse the body of a `type Foo = Event Payload [external|internal]`
    /// declaration (Phase 2.5). The payload is any `TypeExpr` — a named type
    /// reference (`BrowseItem`), an inline struct (`{ message: string }`), a
    /// primitive, or a parameterised application. The optional bracket
    /// modifier at the tail sets the event's cross-orbital scope; unmarked
    /// defaults to internal at lower time.
    pub(crate) fn parse_type_event_body(&mut self) -> Result<TypeBody, ParseError> {
        let payload = self.parse_type_expr()?;
        let scope = if matches!(self.peek(), TokenKind::LBracket) {
            self.advance();
            let ident = self.expect_ident()?;
            let resolved = match ident.as_str() {
                "internal" => EventScopeAst::Internal,
                "external" => EventScopeAst::External,
                other => {
                    return Err(ParseError::UnexpectedToken {
                        expected: "internal or external".to_string(),
                        got: other.to_string(),
                        span: self.peek_span(),
                    });
                }
            };
            self.expect_kind(&TokenKind::RBracket, "]")?;
            Some(resolved)
        } else {
            None
        };
        // Optional trailing string literal = event description.
        let description = if let TokenKind::String(s) = self.peek().clone() {
            self.advance();
            Some(s)
        } else {
            None
        };
        Ok(TypeBody::Event {
            payload,
            scope,
            description,
        })
    }

    /// Try to consume a `::` type annotation on the current declaration.
    /// Returns `None` if the next token isn't `DoubleColon`. Otherwise
    /// consumes the `::` and reads `TypeName [arg1 arg2 ...]` — the args are
    /// whitespace-separated identifiers up to the next statement terminator
    /// (`{`, `=`, `->`, newline, `[` persistence tag, etc.).
    pub(crate) fn try_parse_type_annotation(
        &mut self,
    ) -> Result<Option<TypeAnnotation>, ParseError> {
        if !matches!(self.peek(), TokenKind::DoubleColon) {
            return Ok(None);
        }
        let span = self.peek_span();
        self.advance();
        let type_name = self.parse_dotted_ident()?;
        let mut type_args = Vec::new();
        // Positional args: one-identifier each, until we hit a boundary token.
        loop {
            match self.peek() {
                TokenKind::Identifier(n) => {
                    // Only consume if it genuinely looks like a type argument
                    // (alphabetic start). This guards against accidentally
                    // eating the next decl's keyword. Args don't end in dots,
                    // but peek_n helps disambiguate.
                    let name = n.clone();
                    self.advance();
                    type_args.push(name);
                }
                _ => break,
            }
        }
        Ok(Some(TypeAnnotation {
            type_name,
            type_args,
            span,
        }))
    }

    pub(crate) fn parse_type_expr(&mut self) -> Result<TypeExpr, ParseError> {
        let base = self.parse_type_atom()?;
        // `T | U | V` union form.
        if matches!(self.peek(), TokenKind::Pipe) {
            let mut parts = vec![base];
            while matches!(self.peek(), TokenKind::Pipe) {
                self.advance();
                parts.push(self.parse_type_atom()?);
            }
            return Ok(TypeExpr::Union(parts));
        }
        Ok(base)
    }

    pub(crate) fn parse_type_atom(&mut self) -> Result<TypeExpr, ParseError> {
        // `[T]`
        if matches!(self.peek(), TokenKind::LBracket) {
            self.advance();
            let inner = self.parse_type_expr()?;
            self.expect_kind(&TokenKind::RBracket, "]")?;
            return Ok(TypeExpr::Array(Box::new(inner)));
        }
        // `"literal"` — string-literal type, only useful as a union member.
        // `status : "active" | "inactive"` lowers to a string field with
        // `values: ["active", "inactive"]`.
        if let TokenKind::String(s) = self.peek().clone() {
            self.advance();
            return Ok(TypeExpr::StringLiteral(s));
        }
        // `{ k: T, ... }` / `{ k: T! }` inline object type. Honor the
        // optional `!` marker the same way struct-type-body does, so an
        // inline object passed into a config slot carries per-field
        // requiredness through to FieldDefinition + OirType::Struct.
        if matches!(self.peek(), TokenKind::LBrace) {
            self.advance();
            let mut fields = Vec::new();
            loop {
                self.skip_newlines();
                if matches!(self.peek(), TokenKind::RBrace) {
                    self.advance();
                    break;
                }
                let name = self.expect_ident()?;
                self.expect_kind(&TokenKind::Colon, ":")?;
                let ty = self.parse_type_expr()?;
                let required = if matches!(self.peek(), TokenKind::Bang) {
                    self.advance();
                    true
                } else {
                    false
                };
                fields.push((name, ty, required));
                if matches!(self.peek(), TokenKind::Comma) {
                    self.advance();
                }
                self.skip_newlines();
            }
            return Ok(TypeExpr::Object(fields));
        }
        let name = self.expect_ident()?;
        // `Map K V` — reserved built-in dynamic-key map constructor, applied by
        // juxtaposition (Haskell `Map k v` style): parse the key type-atom then
        // the value type-atom. Lowers to `Record<K, V>`. Reserved like the
        // config-only keywords below; no user type may be named `Map`.
        if name == "Map" {
            let key = self.parse_type_atom()?;
            let value = self.parse_type_atom()?;
            return Ok(TypeExpr::Map(Box::new(key), Box::new(value)));
        }
        // `trait` keyword as a type — value is a `@trait.X` reference. Allowed
        // only inside config { ... } blocks; entity field validation rejects it.
        if name == "trait" {
            return Ok(TypeExpr::Trait);
        }
        // `entity` / `event` keywords as types — value is an entity/event NAME
        // reference (V4-W5). Allowed only inside config { ... } blocks; the stamp
        // id-carries the value so a rename resolves it by id.
        if name == "entity" {
            return Ok(TypeExpr::EntityRef);
        }
        if name == "event" {
            return Ok(TypeExpr::EventRef);
        }
        // `slot` keyword as a type — value is a UI slot name. Allowed only
        // inside config { ... } blocks.
        if name == "slot" {
            return Ok(TypeExpr::Slot);
        }
        // `pattern` keyword as a type — value is a pattern type name. Allowed
        // only inside config { ... } blocks.
        if name == "pattern" {
            return Ok(TypeExpr::Pattern);
        }
        // `SExpr` keyword as a type — value is a compiled s-expression (the
        // `@almadar/core` `SExpr` brand). Allowed only inside config { ... }
        // blocks. Emits the recursive `SExpr` TS type.
        if name == "SExpr" {
            return Ok(TypeExpr::SExpr);
        }
        let base = if is_primitive_type(&name) {
            TypeExpr::Primitive(name)
        } else {
            TypeExpr::Named(name)
        };
        // Trailing relation modifier (we look at the next token without consuming it).
        match self.peek() {
            TokenKind::Identifier(s) if s == "*" => {
                self.advance();
                if let TypeExpr::Named(n) = base {
                    return Ok(TypeExpr::RelationMany(n));
                }
            }
            TokenKind::Identifier(s) if s == "+" => {
                self.advance();
                if let TypeExpr::Named(n) = base {
                    return Ok(TypeExpr::RelationManyNonEmpty(n));
                }
            }
            _ => {}
        }
        Ok(base)
    }

    /// Read a dotted identifier sequence: `ident (Dot ident)*` joined with `.`.
    /// Used for trait/page reference paths like `Modal.traits.ModalRecordModal`.
    pub(crate) fn parse_dotted_ident(&mut self) -> Result<String, ParseError> {
        let mut buf = self.expect_ident()?;
        while matches!(self.peek(), TokenKind::Dot) {
            self.advance();
            let next = self.expect_ident()?;
            buf.push('.');
            buf.push_str(&next);
        }
        Ok(buf)
    }
}
