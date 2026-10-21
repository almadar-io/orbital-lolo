//! Type-alias / type-body / type-expression parsing for the .lolo parser.

use super::entity::EntityTags;
use super::*;

/// The six kind names a parenthesized type parameter may declare — shared by
/// the type-alias parameter form (`type Foo (E : Entity) = ...`, Increment
/// 3-I) and the trait-level parameter form (`trait Name (V : Type) -> ...`,
/// Increment 3-II). Only `Type` and `Entity` are meaningfully acted on by
/// the compiler today; the rest are stored but inert.
pub(crate) const VALID_TYPE_PARAM_KINDS: [&str; 6] =
    ["Type", "Entity", "Trait", "Page", "Orbital", "Event"];

/// Type-parameter names must start lowercase: in `.lolo`, PascalCase names
/// in type position ARE concrete types (entities, aliases), so `row : E`
/// would be locally indistinguishable from a reference to an entity named
/// `E`. Lowercase variables (`e`, `v`) make variable-vs-concrete obvious at
/// the use site (the Haskell convention, adopted 2026-08-18).
pub(crate) fn type_param_name_error(name: &str, span: crate::ast::Span) -> Option<ParseError> {
    if name
        .chars()
        .next()
        .map(|c| c.is_lowercase())
        .unwrap_or(false)
    {
        None
    } else {
        Some(ParseError::UnexpectedToken {
            expected: "a lowercase-initial type-parameter name (e.g. `e`, `v`) — PascalCase names are concrete types".to_string(),
            got: name.to_string(),
            span,
        })
    }
}

impl Parser {
    /// `{ name : T[!], "@quoted" : T, ... }` — the field list of a shape (struct body or inline object type).
    /// A bare name follows the identifier rule; a quoted name is any non-empty string, stored verbatim, so a
    /// shape can declare the keys `xml/parse` writes (`"@href"`, `"#text"`).
    pub(crate) fn parse_shape_fields(
        &mut self,
    ) -> Result<Vec<(String, TypeExpr, bool)>, ParseError> {
        self.expect_kind(&TokenKind::LBrace, "{")?;
        let mut fields: Vec<(String, TypeExpr, bool)> = Vec::new();
        let mut quoted: Vec<String> = Vec::new();
        loop {
            self.skip_newlines();
            if matches!(self.peek(), TokenKind::RBrace) {
                self.advance();
                break;
            }
            let name_span = self.peek_span();
            let name = if let TokenKind::String(s) = self.peek().clone() {
                self.advance();
                if s.is_empty() {
                    return Err(ParseError::InvalidType {
                        span: name_span,
                        message: "ELOLO_SHAPE_FIELD_NAME_EMPTY: a quoted shape field name must be a non-empty string".to_string(),
                    });
                }
                quoted.push(s.clone());
                s
            } else {
                self.expect_ident()?
            };
            if quoted.contains(&name) && fields.iter().any(|(n, _, _)| *n == name) {
                return Err(ParseError::InvalidType {
                    span: name_span,
                    message: format!(
                        "ELOLO_SHAPE_FIELD_NAME_DUPLICATE: the shape declares the field \"{name}\" more than once"
                    ),
                });
            }
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
        Ok(fields)
    }
}

impl Parser {
    pub(crate) fn parse_type_alias(&mut self) -> Result<TypeAlias, ParseError> {
        let span = self.peek_span();
        self.expect_keyword("type")?;
        let name = self.expect_ident()?;
        // Optional type parameter, either form:
        //   bare:   `type Browsable T = ...`         — no kind, defaults to Entity
        //   kinded: `type Foo (E : Entity) = ...`     — parenthesized name + kind
        let (type_param, type_param_kind) = if matches!(self.peek(), TokenKind::LParen) {
            self.advance();
            let pname_span = self.peek_span();
            let pname = self.expect_ident()?;
            if let Some(e) = type_param_name_error(&pname, pname_span) {
                return Err(e);
            }
            self.expect_kind(&TokenKind::Colon, ":")?;
            let kind_span = self.peek_span();
            let kind = self.expect_ident()?;
            if !VALID_TYPE_PARAM_KINDS.contains(&kind.as_str()) {
                return Err(ParseError::UnexpectedToken {
                    expected: "Type, Entity, Trait, Page, Orbital, or Event".to_string(),
                    got: kind,
                    span: kind_span,
                });
            }
            self.expect_kind(&TokenKind::RParen, ")")?;
            (Some(pname), Some(kind))
        } else if let TokenKind::Identifier(p) = self.peek().clone() {
            if !matches!(self.peek_n(1), TokenKind::Equals) {
                // Identifier not followed by `=` — must be malformed; let
                // the `expect_kind(Equals)` below produce the real error.
                (None, None)
            } else {
                let pname_span = self.peek_span();
                self.advance();
                if let Some(e) = type_param_name_error(&p, pname_span) {
                    return Err(e);
                }
                (Some(p), None)
            }
        } else {
            (None, None)
        };
        self.expect_kind(&TokenKind::Equals, "=")?;
        let body = self.parse_type_body()?;
        if type_body_contains_match(&body) {
            return Err(ParseError::InvalidType {
                span,
                message: format!(
                    "ELOLO_MATCH_POSITION_UNSUPPORTED: type alias '{}' cannot use `match` — \
                     match is only legal as an emits/listens payload field's whole type, never \
                     inside a type-alias body (an `Event` alias's payload especially: the \
                     alias-resolution lowering path never reads a match's conditional list, so \
                     it would silently drop)",
                    name
                ),
            });
        }
        self.expect_newline_or_eof()?;
        Ok(TypeAlias {
            name,
            type_param,
            type_param_kind,
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
            return Ok(TypeBody::Struct(self.parse_shape_fields()?));
        }

        // Everything else is an ordinary TYPE EXPRESSION. Delegating to
        // `parse_type_expr` is what makes `[string]`, `"lit"`, `Map k v` and a
        // mixed union like `string | number | FileValue` legal alias bodies:
        // one grammar for every type position instead of the identifier-only
        // subset this arm used to accept.
        let expr = self.parse_type_expr()?;

        // A union whose members are ALL bare names stays a string ENUM.
        // `up | down | left | right` is a vocabulary, not a type union, and the
        // parser cannot tell it from `DrawShape | DrawText` — only the assembled
        // alias map can. So the enum reading is kept here and the TYPE reading is
        // recovered later by `reclassify_struct_unions`, which has that map.
        // A member that could never have been a vocabulary word (an array, a
        // string literal, an inline object) is not ambiguous, so those unions go
        // through as type unions directly.
        if let TypeExpr::Union(parts) = &expr {
            let all_names: Option<Vec<String>> = parts
                .iter()
                .map(|p| match p {
                    TypeExpr::Named(n) | TypeExpr::Primitive(n) => Some(n.clone()),
                    _ => None,
                })
                .collect();
            if let Some(variants) = all_names {
                return Ok(TypeBody::Enum(variants));
            }
        }
        Ok(TypeBody::Alias(expr))
    }

    /// Parse the body of a `type Foo = Entity ...` shape.
    /// Accepts optional persistence tag + optional field block. When both
    /// are omitted, the alias names an empty entity shape (useful as a
    /// base for composition).
    pub(crate) fn parse_type_entity_body(&mut self) -> Result<TypeBody, ParseError> {
        // `TypeBody::Entity` (the `type X = Entity [...] { ... }` alias shape)
        // has no `shared`/`identity` field yet — only orbital-level `entity`
        // headers carry those through to `EntityDefinition` (lower/entity.rs).
        let tags_span = self.peek_span();
        let EntityTags {
            persistence,
            collection,
            store,
            ..
        } = if matches!(self.peek(), TokenKind::LBracket) {
            self.parse_persistence_tag()?
        } else {
            EntityTags::default()
        };
        if store.is_some() {
            return Err(ParseError::InvalidType {
                span: tags_span,
                message: "`store:` binds an orbital's entity to its driver: write it on the `entity` header, not a type alias".to_string(),
            });
        }
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
            let slot = self
                .lang
                .canon_any(&slot)
                .map(str::to_string)
                .unwrap_or(slot);
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
            let ident = self
                .lang
                .canon_keyword(&ident)
                .map(str::to_string)
                .unwrap_or(ident);
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

    /// Optionally consume a `@config.<knob>` argument to a sigil type
    /// constructor, returning the bare knob name.
    ///
    /// `@config.action` lexes as one `Sigil("config.action")`, so this is a
    /// prefix strip rather than a two-token parse. Returns `None` when the next
    /// token is not such a sigil, which is how the nullary forms (`@entity`,
    /// `@user`) fall through — the CALLER decides whether nullary is legal for
    /// its constructor, because arity is per-constructor.
    fn parse_config_ref_knob_arg(&mut self) -> Result<Option<String>, ParseError> {
        let TokenKind::Sigil(s) = self.peek().clone() else {
            return Ok(None);
        };
        let s = self.canon_sigil_root(&s);
        let Some(knob) = s.strip_prefix("config.") else {
            return Ok(None);
        };
        // A dotted path (`@config.a.b`) is not a knob reference; leave it
        // unconsumed so the caller reports it rather than silently truncating.
        if knob.is_empty() || knob.contains('.') {
            return Ok(None);
        }
        let knob = knob.to_string();
        self.advance();
        Ok(Some(knob))
    }

    pub(crate) fn parse_type_atom(&mut self) -> Result<TypeExpr, ParseError> {
        let base = self.parse_type_atom_base()?;
        // `T?` — nullable. The lexer reads `?` as the bare payload sigil; only a `?` written directly against the
        // type it follows (no space) is the nullable marker, so `value: ?` keeps its payload meaning.
        if matches!(self.peek(), TokenKind::PayloadSigil(name) if name.is_empty())
            && self.peek_is_adjacent_to_previous()
        {
            self.advance();
            return Ok(TypeExpr::Optional(Box::new(base)));
        }
        Ok(base)
    }

    fn peek_is_adjacent_to_previous(&self) -> bool {
        let Some(previous) = self.pos.checked_sub(1).map(|i| self.tokens[i].span) else {
            return false;
        };
        let next = self.tokens[self.pos].span;
        previous.line == next.line && previous.col + u32::from(previous.len) == next.col
    }

    fn parse_type_atom_base(&mut self) -> Result<TypeExpr, ParseError> {
        // `(T | U)` — a grouped type: how a union is written where only one atom fits
        // (`Map string (string | number)`, `[(string | number)]`).
        if matches!(self.peek(), TokenKind::LParen) {
            self.advance();
            let inner = self.parse_type_expr()?;
            self.expect_kind(&TokenKind::RParen, ")")?;
            return Ok(inner);
        }
        // `[T]` — an array; `[T1, T2, …]` — a tuple.
        if matches!(self.peek(), TokenKind::LBracket) {
            self.advance();
            let inner = self.parse_type_expr()?;
            if !matches!(self.peek(), TokenKind::Comma) {
                self.expect_kind(&TokenKind::RBracket, "]")?;
                return Ok(TypeExpr::Array(Box::new(inner)));
            }
            let mut slots = vec![inner];
            while matches!(self.peek(), TokenKind::Comma) {
                self.advance();
                slots.push(self.parse_type_expr()?);
            }
            self.expect_kind(&TokenKind::RBracket, "]")?;
            return Ok(TypeExpr::Tuple(slots));
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
            return Ok(TypeExpr::Object(self.parse_shape_fields()?));
        }
        // Sigil type constructors. In TYPE position a sigil denotes the TYPE of
        // the thing it names, taking arguments by juxtaposition exactly like
        // `Map K V` below. In EXPRESSION position the same sigils keep denoting
        // values (`@entity.field`, `@payload.x`, `@user.role`) — position is
        // what distinguishes type from value, and this arm is the only place
        // the type reading is produced, so the two grammars cannot overlap.
        //
        // Arity is part of each constructor's contract, not optional sugar:
        // `@payload` bare would have to mean "the current event's payload", but
        // a config knob has no current event and a payload position would be
        // self-referential — so it is rejected here rather than allowed to
        // lower into a dangling sentinel.
        if let TokenKind::Sigil(raw_s) = self.peek().clone() {
            let s = self.canon_sigil_root(&raw_s);
            match s.as_str() {
                // `@entity` — the bound entity's row (nullary), or
                // `@entity <knob>` — the row of the entity that knob names.
                "entity" => {
                    self.advance();
                    return match self.parse_config_ref_knob_arg()? {
                        Some(knob) => Ok(TypeExpr::NamedEntityRow(knob)),
                        None => Ok(TypeExpr::BoundEntityRow),
                    };
                }
                // `@payload @config.<knob>` — the payload of the event that
                // knob names, or `@payload <member>` — the payload of the
                // event that a SIBLING struct member names.
                "payload" => {
                    self.advance();
                    if let Some(knob) = self.parse_config_ref_knob_arg()? {
                        return Ok(TypeExpr::EventPayloadOf(PayloadOfTarget::ConfigKnob(knob)));
                    }
                    // A bare identifier — the sibling-member form. Not a
                    // sigil argument, so `parse_config_ref_knob_arg` (which
                    // only consumes `@config.X`) left it unconsumed above.
                    if let TokenKind::Identifier(name) = self.peek().clone() {
                        self.advance();
                        let name = self
                            .lang
                            .canon_any(&name)
                            .map(str::to_string)
                            .unwrap_or(name);
                        return Ok(TypeExpr::EventPayloadOf(PayloadOfTarget::SiblingMember(
                            name,
                        )));
                    }
                    return Err(ParseError::UnexpectedToken {
                        expected: "`@payload @config.<knob>` or `@payload <sibling>` — `@payload` \
                                   needs either the knob or the sibling struct member whose \
                                   value names the event; bare `@payload` has no meaning in a \
                                   type position"
                            .to_string(),
                        got: format!("@{s}"),
                        span: self.peek_span(),
                    });
                }
                // `@user` — the row of the identity this orbital `expects`.
                "user" => {
                    self.advance();
                    return Ok(TypeExpr::ExpectedIdentityRow);
                }
                _ => {}
            }
        }
        // `match @config.<knob> { ... }` — type-level match (§25). Reserved
        // keyword like `Map`/`trait`/`slot` below; checked before
        // `expect_ident` consumes the name so the arm-block parse can follow
        // immediately.
        if self.check_ident("match") {
            self.advance();
            return self.parse_match_type();
        }
        let name = self.expect_ident()?;
        let name = self
            .lang
            .canon_type(&name)
            .map(str::to_string)
            .unwrap_or(name);
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
        if name == "orbital" {
            return Ok(TypeExpr::Orbital);
        }
        if name == "behavior" {
            return Ok(TypeExpr::Behavior);
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
        if name == "EventAddress" {
            return Ok(TypeExpr::EventAddress);
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
        // `secret` keyword as a type — value is a credential reference (an
        // env-var key name), never the secret itself. Allowed only inside
        // config { ... } blocks (core's SECRET_CONFIG_TYPES contract, I-29).
        // Previously this silently parsed as Named("secret") — a meaningless
        // user type that defeated the whole contract.
        if name == "secret" {
            return Ok(TypeExpr::Secret);
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
        // `T.f` — a field of a declared type.
        if let TypeExpr::Named(ty) = &base {
            if matches!(self.peek(), TokenKind::Dot) {
                if let TokenKind::Identifier(field) = self.peek_n(1).clone() {
                    self.advance();
                    self.advance();
                    return Ok(TypeExpr::FieldOf {
                        ty: ty.clone(),
                        field,
                        resolved: None,
                    });
                }
            }
        }
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

    /// Parse a type-level `match` body: `@config.<knob> { <pattern> ->
    /// <type> ... }`. The leading `match` keyword is already consumed by
    /// the caller. Scrutinee is mandatory and restricted to `@config.<knob>`
    /// (v1) — reuses `parse_config_ref_knob_arg`, the same sigil-stripping
    /// helper `@entity <knob>`/`@payload <knob>` use. Per-arm `!` is a hard
    /// parse error; the whole-field `!` (after the match's closing `}`) is
    /// parsed by the field-list caller, unaffected by this function.
    fn parse_match_type(&mut self) -> Result<TypeExpr, ParseError> {
        let scrutinee_span = self.peek_span();
        let knob = self
            .parse_config_ref_knob_arg()?
            .ok_or_else(|| ParseError::InvalidType {
                span: scrutinee_span,
                message: "ELOLO_MATCH_SCRUTINEE_REQUIRED: `match` needs an `@config.<knob>` \
                          scrutinee, e.g. `match @config.inputType { ... }`"
                    .to_string(),
            })?;
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
            let ty = self.parse_type_expr()?;
            if matches!(self.peek(), TokenKind::Bang) {
                return Err(ParseError::InvalidType {
                    span: self.peek_span(),
                    message: "ELOLO_MATCH_ARM_BANG_FORBIDDEN: a match arm's type cannot carry \
                              its own `!` — the whole field's `!` goes after the match's \
                              closing `}`"
                        .to_string(),
                });
            }
            arms.push(MatchArm {
                pattern,
                ty,
                span: arm_span,
            });
            if matches!(self.peek(), TokenKind::Newline) {
                self.advance();
            }
        }
        Ok(TypeExpr::Match { knob, arms })
    }

    /// Parse one match-arm pattern: a string literal, a bare word, `true`/
    /// `false`, an integer, or `_` (wildcard). A float literal is a hard
    /// error — arm patterns compare against a config literal's canonical
    /// spelling, which has no fractional form. `pub(crate)` — reused by
    /// term-level `match` (Phase 2, `parser::trait_::parse_term_match`),
    /// which dispatches the SAME arm-pattern vocabulary on a value instead
    /// of a type.
    pub(crate) fn parse_match_pattern(&mut self) -> Result<MatchPattern, ParseError> {
        match self.peek().clone() {
            TokenKind::String(s) => {
                self.advance();
                Ok(MatchPattern::Str(s))
            }
            TokenKind::Boolean(b) => {
                self.advance();
                Ok(MatchPattern::Bool(b))
            }
            TokenKind::Integer(n) => {
                self.advance();
                Ok(MatchPattern::Int(n))
            }
            TokenKind::Number(_) => Err(ParseError::InvalidType {
                span: self.peek_span(),
                message: "ELOLO_MATCH_ARM_PATTERN_INVALID: a match arm pattern must be an \
                          integer, not a float"
                    .to_string(),
            }),
            TokenKind::Identifier(name) if name == "_" => {
                self.advance();
                Ok(MatchPattern::Wildcard)
            }
            // A native `true`/`false` lexes as an identifier; canonicalize it like the s-expression parser does.
            TokenKind::Identifier(name) if self.lang.canon_literal(&name) == Some("true") => {
                self.advance();
                Ok(MatchPattern::Bool(true))
            }
            TokenKind::Identifier(name) if self.lang.canon_literal(&name) == Some("false") => {
                self.advance();
                Ok(MatchPattern::Bool(false))
            }
            TokenKind::Identifier(name) if self.lang.canon_literal(&name).is_none() => {
                self.advance();
                Ok(MatchPattern::Word(name))
            }
            other => Err(ParseError::UnexpectedToken {
                expected: "a match arm pattern (string literal, bare word, true/false, \
                           integer, or `_`)"
                    .to_string(),
                got: format!("{:?}", other),
                span: self.peek_span(),
            }),
        }
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
