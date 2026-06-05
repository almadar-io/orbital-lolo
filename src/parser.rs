//! Recursive-descent parser for the .lolo surface language.
//!
//! Indentation is **not** significant: the parser is keyword/punctuation
//! driven. Effects under a transition are detected by looking for `(` at the
//! start of the next non-blank line. The parser walks a flat token stream
//! produced by `lexer::tokenize`.
//!
//! See `docs/Lolo_Language_Spec_v3.md` (the "Grammar (EBNF)" section) for the
//! reference grammar. The parser implements a slightly relaxed superset that
//! accepts both `state X { ... }` brace form and the indent form from the
//! spec.

use crate::ast::*;
use crate::error::ParseError;
use crate::lexer::{tokenize_with_comments, Token, TokenKind};
use orbital_core::schema::SExpression;
use std::collections::HashMap;

/// Parse a `.lolo` source string into a Program AST.
pub fn parse_str(source: &str) -> Result<Program, ParseError> {
    let (tokens, comments) = tokenize_with_comments(source)?;
    let mut parser = Parser::new(tokens);
    let mut program = parser.parse_program()?;
    program.comments = comments;
    Ok(program)
}

/// Alias matching the public lib name `parse`.
pub fn parse(source: &str) -> Result<Program, ParseError> {
    parse_str(source)
}

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
    fn new(tokens: Vec<Token>) -> Self {
        Self { tokens, pos: 0 }
    }

    // ----- Token cursor helpers -----

    fn peek(&self) -> &TokenKind {
        &self.tokens[self.pos].kind
    }

    fn peek_span(&self) -> Span {
        self.tokens[self.pos].span
    }

    fn peek_n(&self, n: usize) -> &TokenKind {
        let idx = (self.pos + n).min(self.tokens.len() - 1);
        &self.tokens[idx].kind
    }

    fn advance(&mut self) -> &Token {
        let tok = &self.tokens[self.pos];
        if !matches!(tok.kind, TokenKind::Eof) {
            self.pos += 1;
        }
        tok
    }

    fn at_eof(&self) -> bool {
        matches!(self.peek(), TokenKind::Eof)
    }

    fn skip_newlines(&mut self) {
        while matches!(self.peek(), TokenKind::Newline) {
            self.advance();
        }
    }

    fn expect_newline_or_eof(&mut self) -> Result<(), ParseError> {
        // A statement terminator is a newline, EOF, or a closing brace/paren
        // (which means we're inside a brace-form block and the next iteration
        // will pick up the brace).
        match self.peek() {
            TokenKind::Newline => {
                self.advance();
                Ok(())
            }
            TokenKind::Eof | TokenKind::RBrace | TokenKind::RParen | TokenKind::RBracket => Ok(()),
            _ => Err(ParseError::UnexpectedToken {
                expected: "newline or }".to_string(),
                got: format!("{:?}", self.peek()),
                span: self.peek_span(),
            }),
        }
    }

    fn expect_ident(&mut self) -> Result<String, ParseError> {
        match self.peek().clone() {
            TokenKind::Identifier(name) => {
                self.advance();
                Ok(name)
            }
            _ => Err(ParseError::ExpectedIdentifier {
                span: self.peek_span(),
            }),
        }
    }

    fn expect_keyword(&mut self, kw: &str) -> Result<(), ParseError> {
        match self.peek().clone() {
            TokenKind::Identifier(name) if name == kw => {
                self.advance();
                Ok(())
            }
            other => Err(ParseError::UnexpectedToken {
                expected: kw.to_string(),
                got: format!("{other:?}"),
                span: self.peek_span(),
            }),
        }
    }

    fn check_ident(&self, name: &str) -> bool {
        matches!(self.peek(), TokenKind::Identifier(n) if n == name)
    }

    fn expect_kind(&mut self, kind: &TokenKind, label: &str) -> Result<(), ParseError> {
        if std::mem::discriminant(self.peek()) == std::mem::discriminant(kind) {
            self.advance();
            Ok(())
        } else {
            Err(ParseError::UnexpectedToken {
                expected: label.to_string(),
                got: format!("{:?}", self.peek()),
                span: self.peek_span(),
            })
        }
    }

    // ----- Top level -----

    fn parse_program(&mut self) -> Result<Program, ParseError> {
        self.skip_newlines();

        // The `app` header is optional. Many `.orb` behaviors are single-orbital
        // files with no app wrapper, so we permit a `.lolo` file that opens
        // directly with `orbital ...`.
        let app = if self.check_ident("app") {
            self.parse_app_header()?
        } else {
            AppHeader {
                name: String::new(),
                version: None,
                description: None,
                span: self.peek_span(),
            }
        };

        let mut type_aliases = Vec::new();
        let mut orbitals = Vec::new();

        loop {
            self.skip_newlines();
            if self.at_eof() {
                break;
            }
            if self.check_ident("type") {
                type_aliases.push(self.parse_type_alias()?);
            } else if self.check_ident("orbital") {
                orbitals.push(self.parse_orbital()?);
            } else {
                return Err(ParseError::UnexpectedToken {
                    expected: "type or orbital".to_string(),
                    got: format!("{:?}", self.peek()),
                    span: self.peek_span(),
                });
            }
        }

        Ok(Program {
            app,
            type_aliases,
            orbitals,
            comments: Vec::new(),
        })
    }

    fn parse_app_header(&mut self) -> Result<AppHeader, ParseError> {
        let span = self.peek_span();
        self.expect_keyword("app")?;
        let name = self.expect_ident()?;
        // Optional version. Two forms:
        //   `v1` — a `vN` identifier (integer version, legacy).
        //   `"1.0.0"` — a string literal (canonical semver form; what the
        //                committed .orb files carry).
        let version = match self.peek().clone() {
            TokenKind::Identifier(v)
                if v.starts_with('v')
                    && v.len() > 1
                    && v[1..].chars().next().is_some_and(|c| c.is_ascii_digit()) =>
            {
                self.advance();
                Some(v[1..].to_string())
            }
            TokenKind::String(s) => {
                self.advance();
                Some(s)
            }
            _ => None,
        };
        // Optional newline-then-description string on the next line.
        if matches!(self.peek(), TokenKind::Newline) {
            self.advance();
        }
        let description = if let TokenKind::String(s) = self.peek().clone() {
            self.advance();
            self.expect_newline_or_eof()?;
            Some(s)
        } else {
            None
        };
        Ok(AppHeader {
            name,
            version,
            description,
            span,
        })
    }

    fn parse_type_alias(&mut self) -> Result<TypeAlias, ParseError> {
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

    fn parse_type_body(&mut self) -> Result<TypeBody, ParseError> {
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
    fn parse_type_body_atom(&mut self) -> Result<TypeBody, ParseError> {
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
    fn parse_type_entity_body(&mut self) -> Result<TypeBody, ParseError> {
        let (persistence, collection) = if matches!(self.peek(), TokenKind::LBracket) {
            self.parse_persistence_tag()?
        } else {
            (None, None)
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
    fn parse_type_trait_body(&mut self) -> Result<TypeBody, ParseError> {
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
    fn parse_type_page_body(&mut self) -> Result<TypeBody, ParseError> {
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
    fn parse_type_orbital_body(&mut self) -> Result<TypeBody, ParseError> {
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
    fn parse_type_event_body(&mut self) -> Result<TypeBody, ParseError> {
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
    fn try_parse_type_annotation(&mut self) -> Result<Option<TypeAnnotation>, ParseError> {
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

    // ----- Orbital -----

    fn parse_orbital(&mut self) -> Result<Orbital, ParseError> {
        let span = self.peek_span();
        self.expect_keyword("orbital")?;
        let name = self.expect_ident()?;

        // Optional `:: TypeName [args]` annotation applied to the orbital body.
        let type_annotation = self.try_parse_type_annotation()?;

        // Optional `{ ... }` brace form. The spec uses indentation, but we
        // accept braces too because round-trip pretty-printing emits them.
        let brace_form = matches!(self.peek(), TokenKind::LBrace);
        if brace_form {
            self.advance();
        }
        self.skip_newlines();

        let mut uses = Vec::new();
        let mut type_aliases = Vec::new();
        let mut entity = None;
        let mut traits = Vec::new();
        let mut pages = Vec::new();
        let mut body = Vec::new();

        loop {
            self.skip_newlines();
            if brace_form && matches!(self.peek(), TokenKind::RBrace) {
                self.advance();
                break;
            }
            if !brace_form
                && (self.at_eof() || self.check_ident("orbital") || self.check_ident("app"))
            {
                break;
            }
            if self.at_eof() {
                break;
            }
            if self.check_ident("uses") {
                uses.push(self.parse_use_decl()?);
            } else if self.check_ident("type") {
                type_aliases.push(self.parse_type_alias()?);
            } else if self.check_ident("entity") {
                entity = Some(self.parse_entity()?);
            } else if self.check_ident("trait") {
                traits.push(self.parse_trait()?);
            } else if self.check_ident("page") {
                pages.push(self.parse_page()?);
            } else if matches!(self.peek(), TokenKind::LParen) {
                // Top-level s-expression — typically `(behavior/compose ...)`.
                let expr = self.parse_sexpr()?;
                body.push(expr);
                self.expect_newline_or_eof()?;
            } else {
                return Err(ParseError::UnexpectedToken {
                    expected: "uses, type, entity, trait, page, or s-expression".to_string(),
                    got: format!("{:?}", self.peek()),
                    span: self.peek_span(),
                });
            }
        }

        Ok(Orbital {
            name,
            type_annotation,
            uses,
            type_aliases,
            entity,
            traits,
            pages,
            body,
            span,
        })
    }

    fn parse_use_decl(&mut self) -> Result<UseDecl, ParseError> {
        let span = self.peek_span();
        self.expect_keyword("uses")?;
        let alias = self.expect_ident()?;
        self.expect_keyword("from")?;
        let path = match self.peek().clone() {
            TokenKind::String(s) => {
                self.advance();
                s
            }
            _ => {
                return Err(ParseError::UnexpectedToken {
                    expected: "string path".to_string(),
                    got: format!("{:?}", self.peek()),
                    span: self.peek_span(),
                })
            }
        };
        self.expect_newline_or_eof()?;
        Ok(UseDecl { alias, path, span })
    }

    // ----- Entity -----

    fn parse_entity(&mut self) -> Result<Entity, ParseError> {
        let span = self.peek_span();
        self.expect_keyword("entity")?;
        let name = self.expect_ident()?;

        // Optional `:: TypeName [args]` annotation above the body.
        let type_annotation = self.try_parse_type_annotation()?;

        let (persistence, collection) = if matches!(self.peek(), TokenKind::LBracket) {
            self.parse_persistence_tag()?
        } else {
            (None, None)
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
            fields,
            span,
        })
    }

    fn is_entity_field_start(&self) -> bool {
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

    fn parse_persistence_tag(
        &mut self,
    ) -> Result<(Option<Persistence>, Option<String>), ParseError> {
        // `[persistent: name]` or `[runtime]` or `[singleton]` or `[local]` or `[instance]`.
        self.expect_kind(&TokenKind::LBracket, "[")?;
        let kind = self.expect_ident()?;
        let collection = if matches!(self.peek(), TokenKind::Colon) {
            self.advance();
            // Could be an identifier or a string literal.
            match self.peek().clone() {
                TokenKind::Identifier(s) => {
                    self.advance();
                    Some(s)
                }
                TokenKind::String(s) => {
                    self.advance();
                    Some(s)
                }
                _ => None,
            }
        } else {
            None
        };
        self.expect_kind(&TokenKind::RBracket, "]")?;
        let persistence = match kind.as_str() {
            "persistent" => Some(Persistence::Persistent),
            "runtime" => Some(Persistence::Runtime),
            "singleton" => Some(Persistence::Singleton),
            "local" => Some(Persistence::Local),
            "instance" => Some(Persistence::Instance),
            _ => {
                return Err(ParseError::UnknownKeyword {
                    keyword: kind,
                    span: self.peek_span(),
                })
            }
        };
        Ok((persistence, collection))
    }

    fn parse_field_decl(&mut self) -> Result<EntityField, ParseError> {
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

    fn parse_type_expr(&mut self) -> Result<TypeExpr, ParseError> {
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

    fn parse_type_atom(&mut self) -> Result<TypeExpr, ParseError> {
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
        // `trait` keyword as a type — value is a `@trait.X` reference. Allowed
        // only inside config { ... } blocks; entity field validation rejects it.
        if name == "trait" {
            return Ok(TypeExpr::Trait);
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

    // ----- Trait -----

    fn parse_trait(&mut self) -> Result<Trait, ParseError> {
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
                                expected: "at most one of `instance` | `collection` in trait modifiers".to_string(),
                                got: "duplicate scope marker".to_string(),
                                span: self.peek_span(),
                            });
                        }
                        scope = Some(TraitScope::Instance);
                    }
                    "collection" => {
                        if scope.is_some() {
                            return Err(ParseError::UnexpectedToken {
                                expected: "at most one of `instance` | `collection` in trait modifiers".to_string(),
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
    fn parse_trait_reference(
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

        // Body must be brace-form.
        self.expect_kind(&TokenKind::LBrace, "{")?;
        self.skip_newlines();

        let mut events_renames: Vec<(String, String)> = Vec::new();
        let mut fields_renames: Vec<(String, String)> = Vec::new();
        let mut config_values: Vec<(String, SExpression)> = Vec::new();
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
                // trait-ref call site — caller-supplied values for the imported
                // atom's `config` fields. Lowered to the trait reference's
                // `config` map; the inline phase substitutes `@config.<param>`
                // inside the imported trait's effects with these values.
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
                    let key = self.expect_ident()?;
                    self.expect_kind(&TokenKind::Colon, ":")?;
                    let value = self.parse_sexpr_or_value()?;
                    config_values.push((key, value));
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
            listens,
            ticks: Vec::new(),
            config: Vec::new(),
            span,
            reference: Some(TraitRefData {
                reference,
                events: events_renames,
                fields: fields_renames,
                config: config_values,
                emits_scope,
            }),
        })
    }

    /// Read a dotted identifier sequence: `ident (Dot ident)*` joined with `.`.
    /// Used for trait/page reference paths like `Modal.traits.ModalRecordModal`.
    fn parse_dotted_ident(&mut self) -> Result<String, ParseError> {
        let mut buf = self.expect_ident()?;
        while matches!(self.peek(), TokenKind::Dot) {
            self.advance();
            let next = self.expect_ident()?;
            buf.push('.');
            buf.push_str(&next);
        }
        Ok(buf)
    }

    fn parse_state(&mut self) -> Result<State, ParseError> {
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

    fn parse_transition(&mut self) -> Result<Transition, ParseError> {
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

    fn parse_emits_block(&mut self) -> Result<Vec<Emit>, ParseError> {
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
    fn parse_event_annotations(
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

    fn parse_emit_decl(&mut self) -> Result<Emit, ParseError> {
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

    fn parse_event_field_list(&mut self) -> Result<Vec<EventField>, ParseError> {
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

    fn parse_listens_block(&mut self) -> Result<Vec<Listen>, ParseError> {
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

    fn parse_listen_handler(&mut self) -> Result<Listen, ParseError> {
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

    fn parse_ticks_block(&mut self) -> Result<Vec<Tick>, ParseError> {
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

    fn parse_tick_interval(&mut self) -> Result<TickInterval, ParseError> {
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

    fn parse_config_block(&mut self) -> Result<Vec<ConfigField>, ParseError> {
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
                // Modern annotation form: `@label "..."`,
                // `@description "..."`, `@synonyms "..."`, and
                // `@tier "..."` on the lines following the declaration.
                // Repeated until a non-annotation token appears (the
                // next field or the closing brace). `@synonyms` is a
                // comma-separated user-vocabulary list used by Stage A
                // prompts + knob-embeddings; `@tier` is one of
                // `essential` | `customization` | `advanced` |
                // `internal` and drives the studio Questionnaire's
                // filter + disclosure.
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
                out.push(ConfigField {
                    name,
                    type_expr,
                    default,
                    label,
                    description,
                    synonyms,
                    tier,
                    span,
                });
            } else {
                break;
            }
        }
        Ok(out)
    }

    fn parse_page(&mut self) -> Result<Page, ParseError> {
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
        if matches!(self.peek(), TokenKind::Newline) {
            self.advance();
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
        })
    }

    // ----- S-expressions -----

    fn parse_sexpr(&mut self) -> Result<SExpression, ParseError> {
        let span = self.peek_span();
        self.expect_kind(&TokenKind::LParen, "(")?;
        if matches!(self.peek(), TokenKind::RParen) {
            self.advance();
            return Ok(SExpression::Array(Vec::new()));
        }
        // First element is the operator. Accept any identifier plus the
        // single-char comparison token `=` (tokenized as `Equals`; used in
        // legacy pre-V2 `.lolo` files as the Lisp-style equality operator).
        let op = match self.peek().clone() {
            TokenKind::Identifier(name) => {
                self.advance();
                SExpression::String(name)
            }
            TokenKind::Equals => {
                self.advance();
                SExpression::String("=".to_string())
            }
            other => {
                return Err(ParseError::InvalidSExpr {
                    message: format!("expected operator, got {other:?}"),
                    span,
                })
            }
        };
        let mut items = vec![op];
        // Special case: `(let ((x v) (y w)) body)` — the second arg is a list of bindings.
        let is_let = matches!(items.first(), Some(SExpression::String(s)) if s == "let");
        if is_let && matches!(self.peek(), TokenKind::LParen) {
            // Parse the binding list.
            let bindings = self.parse_let_bindings()?;
            items.push(bindings);
        }
        loop {
            self.skip_newlines();
            if matches!(self.peek(), TokenKind::RParen) {
                self.advance();
                break;
            }
            let arg = self.parse_sexpr_or_value()?;
            items.push(arg);
        }
        Ok(SExpression::Array(items))
    }

    fn parse_let_bindings(&mut self) -> Result<SExpression, ParseError> {
        // `((x v) (y w))` — outer list of inner pairs.
        self.expect_kind(&TokenKind::LParen, "(")?;
        let mut pairs = Vec::new();
        loop {
            self.skip_newlines();
            if matches!(self.peek(), TokenKind::RParen) {
                self.advance();
                break;
            }
            self.expect_kind(&TokenKind::LParen, "(")?;
            // name
            let name = match self.peek().clone() {
                TokenKind::Identifier(n) => {
                    self.advance();
                    SExpression::String(n)
                }
                other => {
                    return Err(ParseError::InvalidSExpr {
                        message: format!("let binding name expected, got {other:?}"),
                        span: self.peek_span(),
                    })
                }
            };
            let value = self.parse_sexpr_or_value()?;
            self.expect_kind(&TokenKind::RParen, ")")?;
            pairs.push(SExpression::Array(vec![name, value]));
        }
        Ok(SExpression::Array(pairs))
    }

    /// Parse one s-expression argument: an atom, a paren list, an object literal, or an array literal.
    fn parse_sexpr_or_value(&mut self) -> Result<SExpression, ParseError> {
        match self.peek().clone() {
            TokenKind::LParen => self.parse_sexpr(),
            TokenKind::LBrace => self.parse_object_literal(),
            TokenKind::LBracket => self.parse_array_literal(),
            TokenKind::String(s) => {
                self.advance();
                Ok(SExpression::String(s))
            }
            TokenKind::Integer(n) => {
                self.advance();
                Ok(SExpression::Number(n as f64))
            }
            TokenKind::Number(n) => {
                self.advance();
                Ok(SExpression::Number(n))
            }
            TokenKind::Boolean(b) => {
                self.advance();
                Ok(SExpression::Bool(b))
            }
            TokenKind::Null => {
                self.advance();
                Ok(SExpression::Null)
            }
            TokenKind::Sigil(name) => {
                self.advance();
                Ok(SExpression::String(format!("@{}", lower_self_sigil(&name))))
            }
            TokenKind::PayloadSigil(name) => {
                self.advance();
                if name.is_empty() {
                    Ok(SExpression::String("@payload".to_string()))
                } else {
                    Ok(SExpression::String(format!("@payload.{name}")))
                }
            }
            TokenKind::Identifier(name) => {
                self.advance();
                // Bare identifiers in argument position are atoms (event names,
                // entity names, slot names, mode names). Special atoms `now`
                // and `state` lower to bindings.
                let atom = match name.as_str() {
                    "now" => "@now".to_string(),
                    "state" => "@state".to_string(),
                    _ => name,
                };
                Ok(SExpression::String(atom))
            }
            other => Err(ParseError::InvalidSExpr {
                message: format!("unexpected token in s-expression: {other:?}"),
                span: self.peek_span(),
            }),
        }
    }

    fn parse_object_literal(&mut self) -> Result<SExpression, ParseError> {
        self.expect_kind(&TokenKind::LBrace, "{")?;
        let mut map: HashMap<String, SExpression> = HashMap::new();
        loop {
            self.skip_newlines();
            if matches!(self.peek(), TokenKind::RBrace) {
                self.advance();
                break;
            }
            // Key: identifier or string.
            let key = match self.peek().clone() {
                TokenKind::Identifier(s) => {
                    self.advance();
                    s
                }
                TokenKind::String(s) => {
                    self.advance();
                    s
                }
                other => {
                    return Err(ParseError::InvalidSExpr {
                        message: format!("object key expected, got {other:?}"),
                        span: self.peek_span(),
                    })
                }
            };
            self.expect_kind(&TokenKind::Colon, ":")?;
            let value = self.parse_sexpr_or_value()?;
            map.insert(key, value);
            if matches!(self.peek(), TokenKind::Comma) {
                self.advance();
            }
            self.skip_newlines();
        }
        Ok(SExpression::Object(map))
    }

    fn parse_array_literal(&mut self) -> Result<SExpression, ParseError> {
        self.expect_kind(&TokenKind::LBracket, "[")?;
        let mut items = Vec::new();
        loop {
            self.skip_newlines();
            if matches!(self.peek(), TokenKind::RBracket) {
                self.advance();
                break;
            }
            let value = self.parse_sexpr_or_value()?;
            items.push(value);
            if matches!(self.peek(), TokenKind::Comma) {
                self.advance();
            }
            self.skip_newlines();
        }
        Ok(SExpression::Array(items))
    }

    fn parse_duration_ms(&mut self) -> Result<u64, ParseError> {
        let span = self.peek_span();
        let n = match self.peek().clone() {
            TokenKind::Integer(n) => {
                self.advance();
                n as u64
            }
            other => {
                return Err(ParseError::UnexpectedToken {
                    expected: "duration number".to_string(),
                    got: format!("{other:?}"),
                    span,
                })
            }
        };
        let suf = match self.peek().clone() {
            TokenKind::Identifier(s) if matches!(s.as_str(), "ms" | "s" | "m" | "h" | "d") => {
                self.advance();
                s
            }
            _ => {
                return Err(ParseError::InvalidDuration {
                    value: n.to_string(),
                    span,
                })
            }
        };
        Ok(duration_to_ms(n, &suf))
    }
}

// ----- Helpers -----

/// Pass a self-sigil through to its `.orb` JSON form. Every `@<name>`
/// roots its own binding namespace — there is NO implicit `@<name>` →
/// `@entity.<name>` rewrite. Schema authors who want an entity field
/// write the qualifier explicitly (`@entity.title`), and lambda
/// parameters (`["fn", "metric", body]`) keep their bare names so
/// references like `@metric.label` resolve to the param at evaluation
/// time. The handful of well-known roots (`entity`, `now`, `state`,
/// `payload`, `config`, …) are preserved unchanged here for clarity;
/// every other name flows through verbatim so the runtime / codegen
/// can decide what to do with it (lambda binding, user-defined trait
/// scope, payload field, …).
fn lower_self_sigil(name: &str) -> String {
    if name.is_empty() {
        return "entity".to_string();
    }
    name.to_string()
}

fn duration_to_ms(n: u64, suf: &str) -> u64 {
    match suf {
        "ms" => n,
        "s" => n * 1000,
        "m" => n * 60_000,
        "h" => n * 3_600_000,
        "d" => n * 86_400_000,
        _ => n,
    }
}

fn is_primitive_type(name: &str) -> bool {
    matches!(
        name,
        "string"
            | "number"
            | "int"
            | "float"
            | "bool"
            | "boolean"
            | "timestamp"
            | "date"
            | "datetime"
            | "duration"
            | "money"
            | "object"
            | "void"
            | "null"
            | "any"
    )
}

/// Return `Some(type_name)` when the type expression contains a config-only
/// type (`TypeExpr::Trait`, `TypeExpr::Slot`, `TypeExpr::Pattern`) at any
/// nesting depth. Entity fields call this to reject these types — see
/// `ELOLO_CONFIG_TYPE_FORBIDDEN_IN_ENTITY`.
fn type_expr_config_only_type(t: &TypeExpr) -> Option<&'static str> {
    match t {
        TypeExpr::Trait => Some("trait"),
        TypeExpr::Slot => Some("slot"),
        TypeExpr::Pattern => Some("pattern"),
        TypeExpr::Array(inner) => type_expr_config_only_type(inner),
        TypeExpr::Optional(inner) => type_expr_config_only_type(inner),
        TypeExpr::Union(parts) => parts.iter().find_map(type_expr_config_only_type),
        TypeExpr::Object(fields) => fields.iter().find_map(|(_, ty, _)| type_expr_config_only_type(ty)),
        TypeExpr::Primitive(_)
        | TypeExpr::Named(_)
        | TypeExpr::StringLiteral(_)
        | TypeExpr::RelationMany(_)
        | TypeExpr::RelationManyNonEmpty(_) => None,
    }
}

fn is_block_keyword(name: &str) -> bool {
    matches!(
        name,
        "app"
            | "orbital"
            | "uses"
            | "entity"
            | "type"
            | "trait"
            | "state"
            | "page"
            | "emits"
            | "listens"
            | "ticks"
            | "config"
            | "initial"
            | "from"
            | "to"
            | "for"
            | "when"
            | "with"
            | "every"
            | "derived"
    )
}

fn is_event_name(name: &str) -> bool {
    // Event names may be UPPER_SNAKE (e.g. INIT, ADD_ITEM) or MixedCase/PascalCase
    // (e.g. BrowseItemLoaded, ConfirmActionLoadFailed). First char must be uppercase
    // ASCII letter; remaining chars are letters, digits, or underscores.
    let mut chars = name.chars();
    let Some(first) = chars.next() else { return false };
    if !first.is_ascii_uppercase() {
        return false;
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn is_event_name_at_peek(kind: &TokenKind) -> bool {
    if let TokenKind::Identifier(name) = kind {
        is_event_name(name)
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(src: &str) -> Program {
        parse_str(src).unwrap_or_else(|e| panic!("parse failed: {e}"))
    }

    #[test]
    fn parse_minimal_orbital() {
        let prog = p("orbital Foo {\n  entity Bar {\n    id : string!\n  }\n}");
        assert_eq!(prog.orbitals.len(), 1);
        assert_eq!(prog.orbitals[0].name, "Foo");
        let ent = prog.orbitals[0].entity.as_ref().unwrap();
        assert_eq!(ent.name, "Bar");
        assert_eq!(ent.fields.len(), 1);
        assert!(ent.fields[0].required);
    }

    #[test]
    fn parse_app_header() {
        let prog = p("app MyApp v1\norbital Foo {\n  entity Bar {\n    id : string!\n  }\n}");
        assert_eq!(prog.app.name, "MyApp");
        assert_eq!(prog.app.version, Some("1".to_string()));
    }

    #[test]
    fn parse_entity_with_persistence() {
        let prog = p("orbital Foo {\n  entity Bar [persistent: bars] {\n    id : string!\n  }\n}");
        let ent = prog.orbitals[0].entity.as_ref().unwrap();
        assert_eq!(ent.persistence, Some(Persistence::Persistent));
        assert_eq!(ent.collection, Some("bars".to_string()));
    }

    #[test]
    fn parse_runtime_persistence() {
        let prog = p("orbital Foo {\n  entity Bar [runtime] {\n    id : string!\n  }\n}");
        let ent = prog.orbitals[0].entity.as_ref().unwrap();
        assert_eq!(ent.persistence, Some(Persistence::Runtime));
    }

    #[test]
    fn parse_trait_with_state() {
        let src = "orbital O {\n  entity E { id : string! }\n  trait T -> E [interaction] {\n    state s {\n      EVT -> s\n        (set @x 1)\n    }\n  }\n}";
        let prog = p(src);
        let t = &prog.orbitals[0].traits[0];
        assert_eq!(t.name, "T");
        assert_eq!(t.linked_entity, Some("E".to_string()));
        assert_eq!(t.category, Some("interaction".to_string()));
        assert_eq!(t.states.len(), 1);
        assert_eq!(t.states[0].transitions.len(), 1);
    }

    #[test]
    fn parse_sexpr_simple() {
        let src = "orbital O {\n  entity E { id : string! }\n  trait T -> E {\n    state s {\n      E -> s\n        (set @entity.x 1)\n    }\n  }\n}";
        let prog = p(src);
        let t = &prog.orbitals[0].traits[0];
        let trans = &t.states[0].transitions[0];
        assert_eq!(trans.effects.len(), 1);
        let arr = trans.effects[0].as_array().unwrap();
        assert_eq!(arr[0].as_str(), Some("set"));
        assert_eq!(arr[1].as_str(), Some("@entity.x"));
    }

    #[test]
    fn parse_payload_sigil_lowering() {
        let src = "orbital O {\n  entity E { id : string! }\n  trait T -> E {\n    state s {\n      E -> s\n        (set @id ?id)\n    }\n  }\n}";
        let prog = p(src);
        let arr = prog.orbitals[0].traits[0].states[0].transitions[0].effects[0]
            .as_array()
            .unwrap();
        assert_eq!(arr[2].as_str(), Some("@payload.id"));
    }

    #[test]
    fn parse_let_bindings() {
        let src = "orbital O {\n  entity E { id : string! }\n  trait T -> E {\n    state s {\n      E -> s\n        (let ((x 1) (y 2)) (set @id 0))\n    }\n  }\n}";
        let prog = p(src);
        let eff = &prog.orbitals[0].traits[0].states[0].transitions[0].effects[0];
        let arr = eff.as_array().unwrap();
        assert_eq!(arr[0].as_str(), Some("let"));
        let bindings = arr[1].as_array().unwrap();
        assert_eq!(bindings.len(), 2);
    }

    #[test]
    fn parse_emit_with_payload() {
        let src = "orbital O {\n  entity E { id : string! }\n  trait T -> E {\n    state s {\n      E -> s\n        (emit FOO { x: 1 })\n    }\n  }\n}";
        let prog = p(src);
        let arr = prog.orbitals[0].traits[0].states[0].transitions[0].effects[0]
            .as_array()
            .unwrap();
        assert_eq!(arr[0].as_str(), Some("emit"));
        assert_eq!(arr[1].as_str(), Some("FOO"));
        assert!(matches!(arr[2], SExpression::Object(_)));
    }

    #[test]
    fn parse_modal_close() {
        let src = "orbital O {\n  entity E { id : string! }\n  trait T -> E {\n    state s {\n      CLOSE -> s\n        (render-ui modal null)\n    }\n  }\n}";
        let prog = p(src);
        let arr = prog.orbitals[0].traits[0].states[0].transitions[0].effects[0]
            .as_array()
            .unwrap();
        assert_eq!(arr[0].as_str(), Some("render-ui"));
        assert_eq!(arr[1].as_str(), Some("modal"));
        assert!(matches!(arr[2], SExpression::Null));
    }

    #[test]
    fn parse_rebindable_binding() {
        let src = "orbital O {\n  entity E { id : string! }\n  trait T -> @rebindable E {\n    @description \"the tracked record\"\n    @synonyms \"track X / audit X\"\n    state s {\n      E -> s\n        (set @entity.status \"x\")\n    }\n  }\n}";
        let prog = p(src);
        let t = &prog.orbitals[0].traits[0];
        assert!(t.entity_rebindable);
        assert_eq!(t.linked_entity.as_deref(), Some("E"));
        assert_eq!(t.entity_binding_description.as_deref(), Some("the tracked record"));
        assert_eq!(t.entity_binding_synonyms.as_deref(), Some("track X / audit X"));
    }

    #[test]
    fn parse_non_rebindable_binding_defaults_false() {
        let src = "orbital O {\n  entity E { id : string! }\n  trait T -> E {\n    state s {\n      E -> s\n        (set @entity.status \"x\")\n    }\n  }\n}";
        let prog = p(src);
        let t = &prog.orbitals[0].traits[0];
        assert!(!t.entity_rebindable);
        assert!(t.entity_binding_description.is_none());
    }

    #[test]
    fn parse_time_bound_state() {
        let src = "orbital O {\n  entity E { id : string! }\n  trait T -> E {\n    state s for 30s {\n      DONE -> s\n    }\n  }\n}";
        let prog = p(src);
        assert_eq!(
            prog.orbitals[0].traits[0].states[0].time_bound,
            Some(30_000)
        );
    }

    #[test]
    fn parse_type_alias() {
        let src = "type Status = active | inactive | pending\norbital O {\n  entity E { id : string! }\n}";
        let prog = p(src);
        assert_eq!(prog.type_aliases.len(), 1);
        match &prog.type_aliases[0].body {
            TypeBody::Enum(v) => assert_eq!(v.len(), 3),
            _ => panic!("expected enum"),
        }
    }

    #[test]
    fn parse_derived_field() {
        let src = "orbital O {\n  entity E [runtime] {\n    id : string!\n    count : int = derived (array/length @items)\n  }\n}";
        let prog = p(src);
        let f = &prog.orbitals[0].entity.as_ref().unwrap().fields[1];
        assert!(f.derived.is_some());
    }

    #[test]
    fn parse_emits_block() {
        let src = "orbital O {\n  entity E { id : string! }\n  trait T -> E {\n    state s { EVT -> s }\n    emits {\n      FOO -> external { id: string! }\n    }\n  }\n}";
        let prog = p(src);
        let t = &prog.orbitals[0].traits[0];
        assert_eq!(t.emits.len(), 1);
        assert_eq!(t.emits[0].event, "FOO");
        assert_eq!(t.emits[0].scope, Some("external".to_string()));
    }

    #[test]
    fn parse_page() {
        let src = "orbital O {\n  entity E { id : string! }\n  trait T -> E { state s { I -> s } }\n  page \"/foo\" -> T [view: list, initial]\n}";
        let prog = p(src);
        let p = &prog.orbitals[0].pages[0];
        assert_eq!(p.path, "/foo");
        assert_eq!(p.traits, vec!["T"]);
        assert_eq!(p.view, Some("list".to_string()));
        assert!(p.is_initial);
    }

    #[test]
    fn parse_config_field_tier_annotation() {
        let src = "orbital O {\n  entity E { id : string! }\n  trait T -> E {\n    state s { I -> s }\n    config {\n      currency : string = \"USD\"\n        @label \"Currency\"\n        @tier \"domain\"\n      auditPrefix : string = \"audit-\"\n        @tier \"internal\"\n    }\n  }\n}";
        let prog = p(src);
        let t = &prog.orbitals[0].traits[0];
        let currency = t.config.iter().find(|c| c.name == "currency").unwrap();
        let audit_prefix = t.config.iter().find(|c| c.name == "auditPrefix").unwrap();
        assert_eq!(currency.label.as_deref(), Some("Currency"));
        assert_eq!(currency.tier.as_deref(), Some("domain"));
        assert_eq!(audit_prefix.tier.as_deref(), Some("internal"));
    }

    #[test]
    fn parse_config_field_no_tier_is_none() {
        let src = "orbital O {\n  entity E { id : string! }\n  trait T -> E {\n    state s { I -> s }\n    config {\n      knob : string = \"x\"\n        @label \"Knob\"\n    }\n  }\n}";
        let prog = p(src);
        let t = &prog.orbitals[0].traits[0];
        assert_eq!(t.config[0].tier, None);
    }
}
