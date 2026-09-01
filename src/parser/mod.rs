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
//!
//! The implementation is split across submodules by AST node kind:
//! `entity`, `r#trait`, `page`, `r#type`, and `expression`. Each adds an
//! `impl Parser` block; this module owns the `Parser` struct, the token-cursor
//! helpers, the top-level program/orbital parsing, and the shared free
//! helpers.

use crate::ast::*;
use crate::error::ParseError;
use crate::lexer::{tokenize_with_comments, Token, TokenKind};
use orbital_core::schema::SExpression;
use std::collections::HashMap;

mod entity;
mod expression;
mod jsx;
mod page;
mod r#trait;
mod r#type;

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
    /// Inline `@(Alias.traits.Upstream { … })` references collected during
    /// expression parsing. Flushed into the current orbital's `traits[]` (and
    /// onto each inline page's trait list) at the end of `parse_orbital`.
    /// Each carries a synthesized, collision-free `name` so the emitted
    /// `@trait.<name>` string resolves in the inline/validation phases.
    pending_inline_traits: Vec<crate::ast::Trait>,
    /// Monotonic counter for synthesized inline-ref names, scoped per orbital.
    inline_ref_counter: usize,
    /// Lexical binding scopes opened by `let` bindings and `fn`/`lambda`
    /// parameters. Only the parser can tell a bare identifier from a quoted
    /// string — both collapse to `SExpression::String` — so the bare-reference
    /// check has to live here.
    binding_scopes: Vec<Vec<String>>,
    /// L2 source-map span capture, active only inside a
    /// [`Self::capture_expr_spans`] window (one per transition guard / effect
    /// / tick guard / tick effect). `None` outside a window — recording is a
    /// no-op then, so ordinary parsing pays nothing.
    expr_span_capture: Option<Vec<(String, Span)>>,
    /// JSON-pointer-style trail accumulated by [`Self::with_trail_seg`] while
    /// a capture window is active — e.g. `"[1][0][1]"` for a `let` binding
    /// value. Meaningless outside a capture window.
    expr_trail: String,
    /// Set by the object-literal parser for the value of a `type:` key —
    /// that position holds a render-ui pattern name, so a bare identifier
    /// there is the pattern's own name string and never a lexical-binding
    /// reference. Consumed (reset) on entry to `parse_sexpr_or_value`, so it
    /// never leaks into nested expressions.
    pub(crate) pattern_type_value: bool,
}

impl Parser {
    fn new(tokens: Vec<Token>) -> Self {
        Self {
            tokens,
            pos: 0,
            pending_inline_traits: Vec::new(),
            inline_ref_counter: 0,
            binding_scopes: Vec::new(),
            expr_span_capture: None,
            expr_trail: String::new(),
            pattern_type_value: false,
        }
    }

    /// Run `f`, recording every [`Self::record_expr_span`] call made during
    /// it as a `(trail, span)` pair relative to the trail active on entry.
    /// Used once per transition guard / effect and per tick guard / effect so
    /// the lowerer can rebuild an L2 `.orb` JSON-pointer path back to the
    /// exact `.lolo` span. Windows never nest in practice (each call site is
    /// a single top-level expression parse); the save/restore below makes
    /// nesting safe anyway.
    pub(crate) fn capture_expr_spans<T>(
        &mut self,
        f: impl FnOnce(&mut Self) -> Result<T, ParseError>,
    ) -> Result<(T, Vec<(String, Span)>), ParseError> {
        let prev_capture = self.expr_span_capture.replace(Vec::new());
        let prev_trail = std::mem::take(&mut self.expr_trail);
        let result = f(self);
        self.expr_trail = prev_trail;
        let captured = self.expr_span_capture.take().unwrap_or_default();
        self.expr_span_capture = prev_capture;
        result.map(|v| (v, captured))
    }

    /// Record the current token's span under the current trail, when a
    /// capture window ([`Self::capture_expr_spans`]) is active.
    pub(crate) fn record_expr_span(&mut self) {
        if self.expr_span_capture.is_some() {
            let entry = (self.expr_trail.clone(), self.peek_span());
            self.expr_span_capture.as_mut().unwrap().push(entry);
        }
    }

    /// Push `seg` onto the trail for the duration of `f`, then restore it.
    /// A no-op (aside from the string manipulation) outside a capture
    /// window — cheap enough not to special-case.
    pub(crate) fn with_trail_seg<T>(
        &mut self,
        seg: String,
        f: impl FnOnce(&mut Self) -> Result<T, ParseError>,
    ) -> Result<T, ParseError> {
        let prev_len = self.expr_trail.len();
        self.expr_trail.push_str(&seg);
        let result = f(self);
        self.expr_trail.truncate(prev_len);
        result
    }

    /// True when `name` is bound by an enclosing `let` or lambda parameter.
    pub(crate) fn is_lexically_bound(&self, name: &str) -> bool {
        self.binding_scopes
            .iter()
            .any(|frame| frame.iter().any(|bound| bound == name))
    }

    // ----- Token cursor helpers -----

    pub(crate) fn peek(&self) -> &TokenKind {
        &self.tokens[self.pos].kind
    }

    pub(crate) fn peek_span(&self) -> Span {
        self.tokens[self.pos].span
    }

    pub(crate) fn peek_n(&self, n: usize) -> &TokenKind {
        let idx = (self.pos + n).min(self.tokens.len() - 1);
        &self.tokens[idx].kind
    }

    pub(crate) fn advance(&mut self) -> &Token {
        let tok = &self.tokens[self.pos];
        if !matches!(tok.kind, TokenKind::Eof) {
            self.pos += 1;
        }
        tok
    }

    pub(crate) fn at_eof(&self) -> bool {
        matches!(self.peek(), TokenKind::Eof)
    }

    pub(crate) fn skip_newlines(&mut self) {
        while matches!(self.peek(), TokenKind::Newline) {
            self.advance();
        }
    }

    pub(crate) fn expect_newline_or_eof(&mut self) -> Result<(), ParseError> {
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

    pub(crate) fn expect_ident(&mut self) -> Result<String, ParseError> {
        match self.peek().clone() {
            TokenKind::Identifier(name) => {
                self.advance();
                Ok(name)
            }
            _ => Err(ParseError::ExpectedIdentifier {
                got: format!("{:?}", self.peek()),
                span: self.peek_span(),
            }),
        }
    }

    /// An emit/listen event name: either a plain `EVENT` identifier or the
    /// `@config.<knob>` reference sigil (Option B — the declared event name
    /// resolves to a string-typed config knob's value at inline time).
    /// Stores the reference as the literal `"@config.<knob>"` string; later
    /// phases (lower, inline) treat that prefix as the resolution marker.
    pub(crate) fn expect_event_name_or_config_ref(&mut self) -> Result<String, ParseError> {
        match self.peek().clone() {
            TokenKind::Identifier(name) if is_event_name(&name) => {
                self.advance();
                Ok(name)
            }
            TokenKind::Sigil(name) if config_ref_knob(&name).is_some() => {
                self.advance();
                Ok(format!("@{name}"))
            }
            _ => Err(ParseError::ExpectedIdentifier {
                got: format!("{:?}", self.peek()),
                span: self.peek_span(),
            }),
        }
    }

    pub(crate) fn expect_keyword(&mut self, kw: &str) -> Result<(), ParseError> {
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

    pub(crate) fn check_ident(&self, name: &str) -> bool {
        matches!(self.peek(), TokenKind::Identifier(n) if n == name)
    }

    pub(crate) fn expect_kind(&mut self, kind: &TokenKind, label: &str) -> Result<(), ParseError> {
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
                theme: None,
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
        // Optional `theme "<key>"` line closing the header block — the
        // app-level skin every orbital's `@currentTheme` seeds from. Must be
        // consumed here: the top-level loop accepts only `type`/`orbital`.
        self.skip_newlines();
        let theme = if self.check_ident("theme") {
            Some(self.parse_orbital_theme()?)
        } else {
            None
        };
        Ok(AppHeader {
            name,
            version,
            description,
            theme,
            span,
        })
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
        let mut expects: Vec<ExpectsDecl> = Vec::new();
        let mut type_aliases = Vec::new();
        let mut entity = None;
        let mut extra_entities = Vec::new();
        let mut traits = Vec::new();
        let mut pages = Vec::new();
        let mut body = Vec::new();
        let mut theme: Option<ThemeDecl> = None;

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
            } else if self.check_ident("expects") {
                let decl = self.parse_expects_decl()?;
                if let Some(dup) = expects_duplicate(&expects, &decl) {
                    return Err(ParseError::InvalidType {
                        span: decl.span,
                        message: format!(
                            "ELOLO_DUPLICATE_EXPECTATION: orbital declares {dup} more than once"
                        ),
                    });
                }
                expects.push(decl);
            } else if self.check_ident("type") {
                type_aliases.push(self.parse_type_alias()?);
            } else if self.check_ident("entity") {
                // First declaration is the primary entity; later ones lower
                // into `auxiliary_entities` (never silently replace).
                let parsed = self.parse_entity()?;
                if entity.is_none() {
                    entity = Some(parsed);
                } else {
                    extra_entities.push(parsed);
                }
            } else if self.check_ident("trait") {
                traits.push(self.parse_trait()?);
            } else if self.check_ident("page") {
                pages.push(self.parse_page()?);
            } else if self.check_ident("theme") {
                theme = Some(self.parse_orbital_theme()?);
            } else if matches!(self.peek(), TokenKind::LParen) {
                // Top-level s-expression — typically `(behavior/compose ...)`.
                let expr = self.parse_sexpr()?;
                body.push(expr);
                self.expect_newline_or_eof()?;
            } else {
                return Err(ParseError::UnexpectedToken {
                    expected: "uses, expects, type, entity, trait, page, theme, or s-expression"
                        .to_string(),
                    got: format!("{:?}", self.peek()),
                    span: self.peek_span(),
                });
            }
        }

        // Flush inline `@(…)` references collected while parsing expressions
        // (render-ui children / config values). Each becomes a real
        // `trait Name = Alias.traits.X { … }` declaration appended to the
        // orbital, and its name is auto-registered on every inline page so
        // the runtime mounts it. After this, the orbital is identical to one
        // authored by hand with the two-step form.
        let pending = std::mem::take(&mut self.pending_inline_traits);
        // NOTE: `inline_ref_counter` is intentionally NOT reset here. It must
        // stay file-global so synthesized names (`InlineX<n>`) are unique
        // across ALL orbitals in the file — the validator forbids the same
        // trait name in two orbitals (`ORB_T_DUPLICATE_NAME`). Resetting per
        // orbital made both orbitals emit `InlineTypographyRender1` etc.
        if !pending.is_empty() {
            let names: Vec<String> = pending.iter().map(|t| t.name.clone()).collect();
            traits.extend(pending);
            for page in pages.iter_mut() {
                // Only inline pages own their trait roster; a reference page's
                // `traits` are call-site overrides, not a mount list.
                if page.reference.is_some() {
                    continue;
                }
                for name in &names {
                    if !page.traits.iter().any(|t| t == name) {
                        page.traits.push(name.clone());
                    }
                }
            }
        }

        Ok(Orbital {
            name,
            type_annotation,
            uses,
            expects,
            type_aliases,
            entity,
            extra_entities,
            traits,
            pages,
            theme,
            body,
            span,
        })
    }

    /// `theme "<name>"` — the orbital's active skin (a reference to a
    /// built-in registry theme), or `theme { name: "…" tokens { … }
    /// variants { … } }` — an inline token definition the orbital carries
    /// itself (lowers to `ThemeRef::Inline`).
    fn parse_orbital_theme(&mut self) -> Result<ThemeDecl, ParseError> {
        self.expect_keyword("theme")?;
        match self.peek().clone() {
            TokenKind::String(s) => {
                self.advance();
                self.expect_newline_or_eof()?;
                Ok(ThemeDecl::Reference(s))
            }
            TokenKind::LBrace => self.parse_inline_theme(),
            other => Err(ParseError::UnexpectedToken {
                expected: "a quoted theme key, e.g. theme \"linear-clean-light\", or an inline theme { … } block".to_string(),
                got: format!("{:?}", other),
                span: self.peek_span(),
            }),
        }
    }

    /// `{ name: "…" tokens { … } variants { dark { … } } }` — the opening
    /// `theme` keyword is consumed; the cursor sits on the `{`.
    fn parse_inline_theme(&mut self) -> Result<ThemeDecl, ParseError> {
        let span = self.peek_span();
        self.expect_kind(&TokenKind::LBrace, "'{' to open an inline theme")?;
        let mut name: Option<String> = None;
        let mut tokens = ThemeTokenMaps::default();
        let mut variants: Vec<(String, ThemeTokenMaps)> = Vec::new();
        loop {
            self.skip_newlines();
            match self.peek().clone() {
                TokenKind::RBrace => {
                    self.advance();
                    break;
                }
                TokenKind::Identifier(kw) if kw == "name" => {
                    self.advance();
                    self.expect_kind(&TokenKind::Colon, "':' after name")?;
                    match self.peek().clone() {
                        TokenKind::String(s) => {
                            self.advance();
                            name = Some(s);
                        }
                        other => {
                            return Err(ParseError::UnexpectedToken {
                                expected: "a quoted theme name".to_string(),
                                got: format!("{:?}", other),
                                span: self.peek_span(),
                            })
                        }
                    }
                    self.expect_newline_or_eof()?;
                }
                TokenKind::Identifier(kw) if kw == "tokens" => {
                    self.advance();
                    tokens = self.parse_theme_token_categories()?;
                }
                TokenKind::Identifier(kw) if kw == "variants" => {
                    self.advance();
                    self.expect_kind(&TokenKind::LBrace, "'{' to open variants")?;
                    loop {
                        self.skip_newlines();
                        if matches!(self.peek(), TokenKind::RBrace) {
                            self.advance();
                            break;
                        }
                        let vname = self.expect_ident()?;
                        let maps = self.parse_theme_token_categories()?;
                        variants.push((vname, maps));
                    }
                    self.expect_newline_or_eof()?;
                }
                other => {
                    return Err(ParseError::UnexpectedToken {
                        expected: "name:, tokens { … }, variants { … }, or }".to_string(),
                        got: format!("{:?}", other),
                        span: self.peek_span(),
                    })
                }
            }
        }
        self.expect_newline_or_eof()?;
        let name = name.ok_or(ParseError::UnexpectedToken {
            expected: "an inline theme requires name: \"…\"".to_string(),
            got: "end of theme block".to_string(),
            span,
        })?;
        Ok(ThemeDecl::Inline(InlineTheme { name, tokens, variants }))
    }

    /// `{ colors { … } radii { … } spacing { … } typography { … } shadows
    /// { … } }` — the flat token-category block shared by `tokens` and each
    /// named variant. The cursor sits on the category block's `{`.
    fn parse_theme_token_categories(&mut self) -> Result<ThemeTokenMaps, ParseError> {
        self.expect_kind(&TokenKind::LBrace, "'{' to open a token category block")?;
        let mut maps = ThemeTokenMaps::default();
        loop {
            self.skip_newlines();
            match self.peek().clone() {
                TokenKind::RBrace => {
                    self.advance();
                    break;
                }
                TokenKind::Identifier(cat) => {
                    let cat_span = self.peek_span();
                    self.advance();
                    let entries = self.parse_theme_string_map()?;
                    match cat.as_str() {
                        "colors" => maps.colors = entries,
                        "radii" => maps.radii = entries,
                        "spacing" => maps.spacing = entries,
                        "typography" => maps.typography = entries,
                        "shadows" => maps.shadows = entries,
                        _ => {
                            return Err(ParseError::UnexpectedToken {
                                expected: "a token category: colors, radii, spacing, typography, or shadows"
                                    .to_string(),
                                got: cat,
                                span: cat_span,
                            })
                        }
                    }
                }
                other => {
                    return Err(ParseError::UnexpectedToken {
                        expected: "a token category (colors, radii, spacing, typography, shadows) or }"
                            .to_string(),
                        got: format!("{:?}", other),
                        span: self.peek_span(),
                    })
                }
            }
        }
        self.expect_newline_or_eof()?;
        Ok(maps)
    }

    /// `{ key: "value" … }` — one `ident: "string"` entry per line (optional
    /// trailing comma). The cursor sits on the map's `{`.
    fn parse_theme_string_map(&mut self) -> Result<Vec<(String, String)>, ParseError> {
        self.expect_kind(&TokenKind::LBrace, "'{' to open a token map")?;
        let mut entries = Vec::new();
        loop {
            self.skip_newlines();
            match self.peek().clone() {
                TokenKind::RBrace => {
                    self.advance();
                    break;
                }
                TokenKind::Identifier(key) => {
                    self.advance();
                    self.expect_kind(&TokenKind::Colon, "':' after a token name")?;
                    match self.peek().clone() {
                        TokenKind::String(value) => {
                            self.advance();
                            entries.push((key, value));
                        }
                        other => {
                            return Err(ParseError::UnexpectedToken {
                                expected: "a quoted token value".to_string(),
                                got: format!("{:?}", other),
                                span: self.peek_span(),
                            })
                        }
                    }
                    if matches!(self.peek(), TokenKind::Comma) {
                        self.advance();
                    }
                    self.expect_newline_or_eof()?;
                }
                TokenKind::Integer(n) => {
                    // Numeric token keys are conventional for scale categories
                    // (spacing { 0: … 4: … } → --space-0 … --space-4).
                    self.advance();
                    let key = format!("{}", n);
                    self.expect_kind(&TokenKind::Colon, "':' after a token name")?;
                    match self.peek().clone() {
                        TokenKind::String(value) => {
                            self.advance();
                            entries.push((key, value));
                        }
                        other => {
                            return Err(ParseError::UnexpectedToken {
                                expected: "a quoted token value".to_string(),
                                got: format!("{:?}", other),
                                span: self.peek_span(),
                            })
                        }
                    }
                    if matches!(self.peek(), TokenKind::Comma) {
                        self.advance();
                    }
                    self.expect_newline_or_eof()?;
                }
                other => {
                    return Err(ParseError::UnexpectedToken {
                        expected: "a token name or }".to_string(),
                        got: format!("{:?}", other),
                        span: self.peek_span(),
                    })
                }
            }
        }
        Ok(entries)
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

    /// `expects` ("identity" Ident? shape? | "entity" Ident shape? | "event" Ident "." IDENT)
    /// — one expectation per line; the shape reuses the entity field parser.
    ///
    /// The identity name stays OPTIONAL on purpose. A program has exactly one
    /// `[identity]` entity and it is referenced as `@user`, never by its entity
    /// name — so the name here is informational, and requiring it would invite
    /// authors to write `Person.role` where the language means `@user.role`.
    /// What makes an identity expectation checkable is its SHAPE, not its name.
    fn parse_expects_decl(&mut self) -> Result<ExpectsDecl, ParseError> {
        let span = self.peek_span();
        self.expect_keyword("expects")?;
        let kind = if self.check_ident("identity") {
            self.advance();
            let name = if matches!(self.peek(), TokenKind::Identifier(_)) {
                Some(self.expect_ident()?)
            } else {
                None
            };
            let shape = self.parse_expects_shape()?;
            ExpectsKind::Identity { name, shape }
        } else if self.check_ident("entity") {
            self.advance();
            let name = self.expect_ident()?;
            let shape = self.parse_expects_shape()?;
            ExpectsKind::Entity { name, shape }
        } else if self.check_ident("event") {
            self.advance();
            let path_span = self.peek_span();
            let path = self.parse_dotted_ident()?;
            let Some((trait_name, event)) = path.rsplit_once('.') else {
                return Err(ParseError::UnexpectedToken {
                    expected: "dotted Trait.EVENT form, e.g. expects event Checkout.ORDER_PLACED"
                        .to_string(),
                    got: path,
                    span: path_span,
                });
            };
            ExpectsKind::Event {
                trait_name: trait_name.to_string(),
                event: event.to_string(),
            }
        } else if self.check_ident("page") {
            self.advance();
            match self.peek().clone() {
                TokenKind::String(path) => {
                    self.advance();
                    ExpectsKind::Page { path }
                }
                other => {
                    return Err(ParseError::UnexpectedToken {
                        expected: "a quoted route pattern, e.g. expects page \"/articles/:slug\""
                            .to_string(),
                        got: format!("{:?}", other),
                        span: self.peek_span(),
                    })
                }
            }
        } else {
            return Err(ParseError::UnexpectedToken {
                expected: "identity, entity, event, or page after `expects`".to_string(),
                got: format!("{:?}", self.peek()),
                span: self.peek_span(),
            });
        };
        self.expect_newline_or_eof()?;
        Ok(ExpectsDecl { kind, span })
    }

    /// Optional `{ field_decl+ }` after an `expects identity`/`expects entity`
    /// header — the §6 entity field-declaration grammar, verbatim.
    fn parse_expects_shape(&mut self) -> Result<Option<Vec<EntityField>>, ParseError> {
        if !matches!(self.peek(), TokenKind::LBrace) {
            return Ok(None);
        }
        let shape_span = self.peek_span();
        self.advance();
        let mut fields = Vec::new();
        loop {
            self.skip_newlines();
            if matches!(self.peek(), TokenKind::RBrace) {
                self.advance();
                break;
            }
            if self.at_eof() {
                return Err(ParseError::UnexpectedEof {
                    expected: "field declaration or }".to_string(),
                    span: self.peek_span(),
                });
            }
            if self.is_entity_field_start() {
                fields.push(self.parse_field_decl()?);
            } else {
                return Err(ParseError::UnexpectedToken {
                    expected: "field declaration or }".to_string(),
                    got: format!("{:?}", self.peek()),
                    span: self.peek_span(),
                });
            }
        }
        if fields.is_empty() {
            return Err(ParseError::InvalidType {
                span: shape_span,
                message: "expects shape must declare at least one field — omit the braces for the existence-only form".to_string(),
            });
        }
        // Same restriction as entity bodies: config-only reference types carry
        // no meaning in a data shape.
        for f in &fields {
            if let Some(ty_name) = type_expr_config_only_type(&f.type_expr) {
                return Err(ParseError::InvalidType {
                    span: f.span,
                    message: format!(
                        "ELOLO_CONFIG_TYPE_FORBIDDEN_IN_ENTITY: expects shape field '{}' cannot use the `{}` type — `{}` is for config blocks only",
                        f.name, ty_name, ty_name
                    ),
                });
            }
        }
        Ok(Some(fields))
    }
}

/// Duplicate key for `ELOLO_DUPLICATE_EXPECTATION`: same kind+name, or the
/// same event. Returns a human-readable description of the duplicate.
fn expects_duplicate(existing: &[ExpectsDecl], new: &ExpectsDecl) -> Option<String> {
    existing.iter().find_map(|e| match (&e.kind, &new.kind) {
        (ExpectsKind::Identity { name: a, .. }, ExpectsKind::Identity { name: b, .. })
            if a == b =>
        {
            Some(match b {
                Some(n) => format!("`expects identity {n}`"),
                None => "`expects identity`".to_string(),
            })
        }
        (ExpectsKind::Entity { name: a, .. }, ExpectsKind::Entity { name: b, .. }) if a == b => {
            Some(format!("`expects entity {b}`"))
        }
        (
            ExpectsKind::Event {
                trait_name: ta,
                event: ea,
            },
            ExpectsKind::Event {
                trait_name: tb,
                event: eb,
            },
        ) if ta == tb && ea == eb => Some(format!("`expects event {tb}.{eb}`")),
        (ExpectsKind::Page { path: a }, ExpectsKind::Page { path: b }) if a == b => {
            Some(format!("`expects page \"{b}\"`"))
        }
        _ => None,
    })
}

// ----- Helpers -----

pub(crate) fn duration_to_ms(n: u64, suf: &str) -> u64 {
    match suf {
        "ms" => n,
        "s" => n * 1000,
        "m" => n * 60_000,
        "h" => n * 3_600_000,
        "d" => n * 86_400_000,
        _ => n,
    }
}

pub(crate) fn is_primitive_type(name: &str) -> bool {
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
            // A single primitive value of bounded kind:
            // `string | number | boolean | null`. Listed here so `v : scalar`
            // parses as a PRIMITIVE rather than an unknown named type that
            // erases to string — the erasure that made `json` invisible.
            | "scalar"
            // Deliberately OPEN data — an audit snapshot of an arbitrary row, a
            // third-party response body, an upstream event's payload at a
            // generic wiring atom. Same wire shape as `json`, opposite intent:
            // `json` means "not typed yet", `opaque` means "not closeable, and
            // a struct here would be a lie". Listed so `v : opaque` parses as a
            // primitive rather than an unknown named type.
            | "opaque"
            // Uploaded file/attachment — a struct value at rest
            // ({name, url, mimeType, sizeBytes}); promoted 2026-08-20.
            | "file"
            // Semantic string domains — first-class types, not refinements.
            // Listing them here is what makes `email : email` parse as a
            // primitive instead of an unknown named type that erases to string.
            | "email"
            | "url"
            | "phone"
            | "uuid"
            | "image"
            // Refinement primitives: a string + a validity rule. `icon` = a valid
            // Lucide name; `component` = a valid element tag (or a @trait ref);
            // `asset` = a resolvable asset URL. All lower to `string`; the value
            // rule is enforced where the value is used (render-ui icon / prop
            // validation). The distinct config-type tag (`icon`/`asset`) survives
            // to the `.orb` `type` field so the property inspector can dispatch an
            // IconPicker / AssetPicker on it.
            | "icon"
            | "asset"
            | "component"
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
pub(crate) fn type_expr_config_only_type(t: &TypeExpr) -> Option<&'static str> {
    match t {
        TypeExpr::Trait => Some("trait"),
        // `@entity` self-reference: payload-positions-only, so an entity field
        // must reject it the same way it rejects the config-only brands.
        TypeExpr::BoundEntityRow => Some("@entity"),
        TypeExpr::EventPayloadOf(_) => Some("@payload"),
        TypeExpr::NamedEntityRow(_) => Some("@entity <knob>"),
        TypeExpr::ExpectedIdentityRow => Some("@user"),
        TypeExpr::EntityRef => Some("entity"),
        TypeExpr::EventRef => Some("event"),
        TypeExpr::Slot => Some("slot"),
        TypeExpr::Pattern => Some("pattern"),
        TypeExpr::Secret => Some("secret"),
        TypeExpr::SExpr => Some("SExpr"),
        TypeExpr::Array(inner) => type_expr_config_only_type(inner),
        TypeExpr::Map(key, value) => {
            type_expr_config_only_type(key).or_else(|| type_expr_config_only_type(value))
        }
        TypeExpr::Optional(inner) => type_expr_config_only_type(inner),
        TypeExpr::Union(parts) => parts.iter().find_map(type_expr_config_only_type),
        TypeExpr::Object(fields) => fields
            .iter()
            .find_map(|(_, ty, _)| type_expr_config_only_type(ty)),
        TypeExpr::Primitive(_)
        | TypeExpr::Named(_)
        | TypeExpr::StringLiteral(_)
        | TypeExpr::RelationMany(_)
        | TypeExpr::RelationManyNonEmpty(_) => None,
    }
}

pub(crate) fn is_block_keyword(name: &str) -> bool {
    matches!(
        name,
        "app"
            | "orbital"
            | "uses"
            | "expects"
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

pub(crate) fn is_event_name(name: &str) -> bool {
    // Event names may be UPPER_SNAKE (e.g. INIT, ADD_ITEM) or MixedCase/PascalCase
    // (e.g. BrowseItemLoaded, ConfirmActionLoadFailed). First char must be uppercase
    // ASCII letter; remaining chars are letters, digits, or underscores.
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_uppercase() {
        return false;
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

pub(crate) fn is_event_name_at_peek(kind: &TokenKind) -> bool {
    match kind {
        TokenKind::Identifier(name) => is_event_name(name),
        TokenKind::Sigil(name) => config_ref_knob(name).is_some(),
        _ => false,
    }
}

/// `@config.<knob>` is the only legal reference form for an emit/listen event
/// name (Option B: the declared event name resolves to a config knob's
/// value). Returns the knob identifier when `name` (the lexer's dotted-sigil
/// body, e.g. `config.action`) matches `config.<ident>`.
pub(crate) fn config_ref_knob(name: &str) -> Option<&str> {
    let knob = name.strip_prefix("config.")?;
    if knob.is_empty() || knob.contains('.') {
        return None;
    }
    Some(knob)
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
        assert_eq!(prog.app.theme, None);
    }

    #[test]
    fn parse_app_header_theme_after_description() {
        let prog = p("app MyApp \"1.0.0\"\n\"A themed app\"\ntheme \"linear-clean-light\"\norbital Foo {\n  entity Bar {\n    id : string!\n  }\n}");
        assert_eq!(prog.app.description, Some("A themed app".to_string()));
        assert_eq!(prog.app.theme, Some(ThemeDecl::Reference("linear-clean-light".to_string())));
        assert_eq!(prog.orbitals.len(), 1);
    }

    #[test]
    fn parse_app_header_theme_without_description() {
        let prog = p("app MyApp \"1.0.0\"\ntheme \"terminal-dark\"\norbital Foo {\n  entity Bar {\n    id : string!\n  }\n}");
        assert_eq!(prog.app.description, None);
        assert_eq!(prog.app.theme, Some(ThemeDecl::Reference("terminal-dark".to_string())));
        assert_eq!(prog.orbitals.len(), 1);
    }

    #[test]
    fn app_theme_does_not_capture_orbital_theme() {
        let prog = p("app MyApp \"1.0.0\"\norbital Foo {\n  entity Bar {\n    id : string!\n  }\n  theme \"clay-light\"\n}");
        assert_eq!(prog.app.theme, None);
        assert_eq!(prog.orbitals[0].theme, Some(ThemeDecl::Reference("clay-light".to_string())));
    }

    #[test]
    fn parse_inline_theme_block() {
        let src = "orbital Foo {\n  entity Bar {\n    id : string!\n  }\n  theme {\n    name: \"riya-graph-paper\"\n    tokens {\n      colors {\n        primary: \"#e11d48\"\n        background: \"#0b1020\"\n      }\n      typography {\n        fontFamily: \"future-narrow\"\n      }\n    }\n    variants {\n      dark {\n        colors {\n          background: \"#05070f\"\n        }\n      }\n    }\n  }\n}";
        let prog = p(src);
        let theme = prog.orbitals[0].theme.clone().expect("theme parsed");
        match theme {
            ThemeDecl::Inline(inline) => {
                assert_eq!(inline.name, "riya-graph-paper");
                assert_eq!(
                    inline.tokens.colors,
                    vec![
                        ("primary".to_string(), "#e11d48".to_string()),
                        ("background".to_string(), "#0b1020".to_string()),
                    ]
                );
                assert_eq!(
                    inline.tokens.typography,
                    vec![("fontFamily".to_string(), "future-narrow".to_string())]
                );
                assert_eq!(inline.variants.len(), 1);
                assert_eq!(inline.variants[0].0, "dark");
                assert_eq!(
                    inline.variants[0].1.colors,
                    vec![("background".to_string(), "#05070f".to_string())]
                );
            }
            other => panic!("expected inline theme, got {:?}", other),
        }
    }

    #[test]
    fn parse_inline_theme_requires_name() {
        let src = "orbital Foo {\n  entity Bar {\n    id : string!\n  }\n  theme {\n    tokens {\n      colors {\n        primary: \"#e11d48\"\n      }\n    }\n  }\n}";
        let err = parse_str(src).expect_err("name-less inline theme must fail");
        assert!(format!("{err}").contains("requires name"), "got: {err}");
    }

    #[test]
    fn parse_inline_theme_numeric_spacing_keys() {
        let src = "orbital Foo {\n  entity Bar {\n    id : string!\n  }\n  theme {\n    name: \"t\"\n    tokens {\n      spacing {\n        0: \"0px\"\n        4: \"16px\"\n        12: \"48px\"\n      }\n    }\n  }\n}";
        let prog = p(src);
        let theme = prog.orbitals[0].theme.clone().expect("theme parsed");
        match theme {
            ThemeDecl::Inline(inline) => {
                assert_eq!(
                    inline.tokens.spacing,
                    vec![
                        ("0".to_string(), "0px".to_string()),
                        ("4".to_string(), "16px".to_string()),
                        ("12".to_string(), "48px".to_string()),
                    ]
                );
            }
            other => panic!("expected inline theme, got {:?}", other),
        }
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
    fn parse_shared_composes_with_runtime() {
        let prog = p("orbital Foo {\n  entity Bar [runtime, shared] {\n    id : string!\n  }\n}");
        let ent = prog.orbitals[0].entity.as_ref().unwrap();
        assert_eq!(ent.persistence, Some(Persistence::Runtime));
        assert!(ent.shared);
    }

    #[test]
    fn parse_shared_composes_with_persistent_collection() {
        let prog = p(
            "orbital Foo {\n  entity Bar [persistent: users, shared] {\n    id : string!\n  }\n}",
        );
        let ent = prog.orbitals[0].entity.as_ref().unwrap();
        assert_eq!(ent.persistence, Some(Persistence::Persistent));
        assert_eq!(ent.collection, Some("users".to_string()));
        assert!(ent.shared);
    }

    #[test]
    fn parse_shared_alone() {
        let prog = p("orbital Foo {\n  entity Bar [shared] {\n    id : string!\n  }\n}");
        let ent = prog.orbitals[0].entity.as_ref().unwrap();
        assert_eq!(ent.persistence, None);
        assert!(ent.shared);
    }

    #[test]
    fn parse_shared_order_independent() {
        let prog = p("orbital Foo {\n  entity Bar [shared, runtime] {\n    id : string!\n  }\n}");
        let ent = prog.orbitals[0].entity.as_ref().unwrap();
        assert_eq!(ent.persistence, Some(Persistence::Runtime));
        assert!(ent.shared);
    }

    #[test]
    fn parse_entity_without_tags_is_not_shared() {
        let prog = p("orbital Foo {\n  entity Bar {\n    id : string!\n  }\n}");
        let ent = prog.orbitals[0].entity.as_ref().unwrap();
        assert!(!ent.shared);
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
        assert_eq!(
            t.entity_binding_description.as_deref(),
            Some("the tracked record")
        );
        assert_eq!(
            t.entity_binding_synonyms.as_deref(),
            Some("track X / audit X")
        );
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
    fn parse_type_alias_mixed_union_is_a_type_union() {
        // A union carrying a member that could never be a vocabulary word (an
        // array here) is unambiguously a TYPE union, so it must not collapse to
        // the string-enum reading that `parse_type_alias` above asserts.
        let src = "type ControlValue = string | number | [string] | FileValue\n\
                   orbital O {\n  entity E { id : string! }\n}";
        let prog = p(src);
        match &prog.type_aliases[0].body {
            TypeBody::Alias(TypeExpr::Union(parts)) => {
                assert_eq!(parts.len(), 4);
                assert!(matches!(parts[2], TypeExpr::Array(_)), "got {:?}", parts[2]);
            }
            other => panic!("expected an alias to a union, got {other:?}"),
        }
    }

    #[test]
    fn parse_type_alias_all_ident_union_stays_an_enum() {
        // The safety property: a union of bare names is indistinguishable from a
        // vocabulary at parse time, so it MUST stay an enum here. The type
        // reading, when there is one, is recovered later by
        // `reclassify_struct_unions`, which can see the whole alias map.
        // Guards `type Direction = up | down | left | right` in the live corpus.
        let src = "type Direction = up | down | left | right\n\
                   orbital O {\n  entity E { id : string! }\n}";
        let prog = p(src);
        match &prog.type_aliases[0].body {
            TypeBody::Enum(v) => assert_eq!(v, &["up", "down", "left", "right"]),
            other => panic!("expected enum, got {other:?}"),
        }
    }

    #[test]
    fn parse_type_alias_to_a_bare_array() {
        // `type X = [string]` was rejected outright before the alias body used
        // the general type grammar.
        let src = "type Tags = [string]\norbital O {\n  entity E { id : string! }\n}";
        let prog = p(src);
        match &prog.type_aliases[0].body {
            TypeBody::Alias(TypeExpr::Array(inner)) => {
                assert!(matches!(**inner, TypeExpr::Primitive(ref n) if n == "string"));
            }
            other => panic!("expected an alias to an array, got {other:?}"),
        }
    }

    #[test]
    fn parse_type_alias_bare_param_still_parses() {
        // Legacy single-ident param form, no parenthesized kind.
        let src = "type Browsable t = Trait -> T\norbital O {\n  entity E { id : string! }\n}";
        let prog = p(src);
        assert_eq!(prog.type_aliases[0].type_param.as_deref(), Some("t"));
        assert_eq!(prog.type_aliases[0].type_param_kind, None);
    }

    #[test]
    fn parse_type_alias_parenthesized_kinded_param() {
        // Increment 3-I: `type Name (e : Entity) = ...`.
        let src = "type RecurrenceSaved (e : Entity) = Event { row : e }\norbital O {\n  entity E2 { id : string! }\n}";
        let prog = p(src);
        assert_eq!(prog.type_aliases.len(), 1);
        let alias = &prog.type_aliases[0];
        assert_eq!(alias.name, "RecurrenceSaved");
        assert_eq!(alias.type_param.as_deref(), Some("e"));
        assert_eq!(alias.type_param_kind.as_deref(), Some("Entity"));
        match &alias.body {
            TypeBody::Event { .. } => {}
            other => panic!("expected Event body, got {other:?}"),
        }
    }

    #[test]
    fn parse_type_alias_bad_kind_name_errors() {
        let src = "type Foo (E : Bogus) = Event { row : e }\norbital O {\n  entity E2 { id : string! }\n}";
        let err = parse_str(src).expect_err("bad kind name must fail to parse");
        assert!(
            matches!(err, ParseError::UnexpectedToken { .. }),
            "got {err:?}"
        );
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
    fn parse_emits_config_ref_event_name() {
        // Option B: `@config.<knob>` as the declared emit event name.
        let src = "orbital O {\n  entity E { id : string! }\n  trait T -> E {\n    state s { EVT -> s }\n    emits {\n      @config.action -> external { id: string! }\n    }\n    config {\n      action : string = \"ACTION\"\n    }\n  }\n}";
        let prog = p(src);
        let t = &prog.orbitals[0].traits[0];
        assert_eq!(t.emits.len(), 1);
        assert_eq!(t.emits[0].event, "@config.action");
        assert_eq!(t.emits[0].scope, Some("external".to_string()));
    }

    #[test]
    fn config_ref_knob_shapes() {
        assert_eq!(config_ref_knob("config.action"), Some("action"));
        assert_eq!(config_ref_knob("config."), None);
        assert_eq!(config_ref_knob("config.a.b"), None);
        assert_eq!(config_ref_knob("entity.action"), None);
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
    fn parse_page_annotations() {
        let src = "orbital O {\n  entity E { id : string! }\n  trait T -> E { state s { I -> s } }\n  page \"/foo\" as FooPage -> T\n    @icon \"users\"\n    @label \"Foo\"\n}";
        let prog = p(src);
        let page = &prog.orbitals[0].pages[0];
        assert_eq!(page.path, "/foo");
        assert_eq!(page.name.as_deref(), Some("FooPage"));
        assert_eq!(page.icon.as_deref(), Some("users"));
        assert_eq!(page.label.as_deref(), Some("Foo"));
    }

    #[test]
    fn parse_page_no_annotations_is_none() {
        let src = "orbital O {\n  entity E { id : string! }\n  trait T -> E { state s { I -> s } }\n  page \"/foo\" -> T\n}";
        let prog = p(src);
        let page = &prog.orbitals[0].pages[0];
        assert!(page.icon.is_none());
        assert!(page.label.is_none());
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

    #[test]
    fn parse_expects_all_forms() {
        let src = "orbital O {\n  expects identity Customer {\n    role : \"store-manager\" | \"operations-director\"\n  }\n  expects entity OrderRecord {\n    status : \"placed\" | \"shipped\" | \"delivered\"\n    totalAmount : number\n  }\n  expects entity CartItem\n  expects event Checkout.ORDER_PLACED\n  entity E { id : string! }\n}";
        let prog = p(src);
        let expects = &prog.orbitals[0].expects;
        assert_eq!(expects.len(), 4);
        match &expects[0].kind {
            ExpectsKind::Identity { name, shape } => {
                assert_eq!(name.as_deref(), Some("Customer"));
                let shape = shape.as_ref().expect("identity shape");
                assert_eq!(shape.len(), 1);
                assert_eq!(shape[0].name, "role");
                assert!(matches!(&shape[0].type_expr, TypeExpr::Union(_)));
            }
            other => panic!("expected identity, got {other:?}"),
        }
        match &expects[1].kind {
            ExpectsKind::Entity { name, shape } => {
                assert_eq!(name, "OrderRecord");
                let shape = shape.as_ref().expect("entity shape");
                assert_eq!(shape.len(), 2);
                assert_eq!(shape[0].name, "status");
                assert_eq!(shape[1].name, "totalAmount");
                assert!(matches!(
                    &shape[1].type_expr,
                    TypeExpr::Primitive(p) if p == "number"
                ));
            }
            other => panic!("expected entity, got {other:?}"),
        }
        match &expects[2].kind {
            ExpectsKind::Entity { name, shape } => {
                assert_eq!(name, "CartItem");
                assert!(shape.is_none());
            }
            other => panic!("expected entity, got {other:?}"),
        }
        match &expects[3].kind {
            ExpectsKind::Event { trait_name, event } => {
                assert_eq!(trait_name, "Checkout");
                assert_eq!(event, "ORDER_PLACED");
            }
            other => panic!("expected event, got {other:?}"),
        }
    }

    #[test]
    fn parse_expects_bare_identity() {
        let prog = p("orbital O {\n  expects identity\n  entity E { id : string! }\n}");
        match &prog.orbitals[0].expects[0].kind {
            ExpectsKind::Identity { name, shape } => {
                assert!(name.is_none());
                assert!(shape.is_none());
            }
            other => panic!("expected identity, got {other:?}"),
        }
    }

    #[test]
    fn parse_expects_named_identity_without_shape() {
        let prog =
            p("orbital O {\n  expects identity Customer\n  entity E { id : string! }\n}");
        match &prog.orbitals[0].expects[0].kind {
            ExpectsKind::Identity { name, shape } => {
                assert_eq!(name.as_deref(), Some("Customer"));
                assert!(shape.is_none());
            }
            other => panic!("expected identity, got {other:?}"),
        }
    }

    #[test]
    fn parse_expects_absent_is_empty() {
        let prog = p("orbital O {\n  entity E { id : string! }\n}");
        assert!(prog.orbitals[0].expects.is_empty());
    }

    #[test]
    fn parse_expects_duplicate_entity_errors() {
        let src = "orbital O {\n  expects entity CartItem\n  expects entity CartItem\n  entity E { id : string! }\n}";
        let err = parse_str(src).expect_err("duplicate must fail");
        assert!(
            err.to_string().contains("ELOLO_DUPLICATE_EXPECTATION"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn parse_expects_duplicate_event_errors() {
        let src = "orbital O {\n  expects event Checkout.ORDER_PLACED\n  expects event Checkout.ORDER_PLACED\n  entity E { id : string! }\n}";
        let err = parse_str(src).expect_err("duplicate must fail");
        assert!(
            err.to_string().contains("ELOLO_DUPLICATE_EXPECTATION"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn parse_expects_duplicate_bare_identity_errors() {
        let src = "orbital O {\n  expects identity\n  expects identity\n  entity E { id : string! }\n}";
        let err = parse_str(src).expect_err("duplicate must fail");
        assert!(
            err.to_string().contains("ELOLO_DUPLICATE_EXPECTATION"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn parse_expects_same_name_different_kind_is_not_duplicate() {
        let src = "orbital O {\n  expects identity Customer\n  expects entity Customer\n  entity E { id : string! }\n}";
        let prog = p(src);
        assert_eq!(prog.orbitals[0].expects.len(), 2);
    }

    #[test]
    fn parse_expects_event_requires_dotted_form() {
        let src = "orbital O {\n  expects event ORDER_PLACED\n  entity E { id : string! }\n}";
        assert!(parse_str(src).is_err());
    }

    #[test]
    fn parse_expects_empty_shape_errors() {
        let src = "orbital O {\n  expects entity CartItem { }\n  entity E { id : string! }\n}";
        assert!(parse_str(src).is_err());
    }

    #[test]
    fn expects_is_block_keyword() {
        assert!(is_block_keyword("expects"));
    }
}
