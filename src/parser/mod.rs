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
use crate::lexer::{tokenize, tokenize_with_comments, Token, TokenKind};
use orbital_core_base::i18n::lang::{self, Lang};
use orbital_core_base::schema::SExpression;

/// Keys of the `app … [ … ]` modifier bracket.
pub(crate) const APP_MODIFIERS: &[&str] = &[
    "version",
    "description",
    "theme",
    "themes",
    "locales",
    "egress",
    "inbound",
    "store",
    "fs",
    "exec",
    "net",
    "env",
    "stdio",
    "sys",
    "desktop",
    "kv",
    "dev",
    "mem",
    "kernel",
    "ffi",
    "gpu",
    "credentials",
    "deployment",
    "public",
    "origin",
    "icon",
    "siteName",
    "signIn",
    "entries",
];

/// Keys of an `orbital Name [ … ]` modifier bracket.
pub(crate) const ORBITAL_MODIFIERS: &[&str] = grants::GRANT_KEYS;

/// Keys of a `page … [ … ]` modifier bracket.
pub(crate) const PAGE_MODIFIERS: &[&str] = &[
    "view",
    "entity",
    "initial",
    "notFound",
    "access",
    "indexing",
    "title",
    "description",
    "translationOf",
];

/// Keys a `pages {} remap entry's bracket takes — the page head modifiers plus
/// the page's nav entry (an inline page declares it with `@label`/`@icon`/`@roles`).
pub(crate) const REMAP_PAGE_MODIFIERS: &[&str] = &[
    "access",
    "indexing",
    "title",
    "description",
    "translationOf",
    "label",
    "icon",
    "roles",
];

mod entity;
pub(crate) mod expression;
mod grants;
pub(crate) mod jsx;
mod orbital;
mod page;
mod r#trait;
mod r#type;

/// Parse a `.lolo` source string into a Program AST.
pub fn parse_str(source: &str) -> Result<Program, ParseError> {
    let (tokens, comments) = tokenize_with_comments(source)?;
    let language = detect_lang(&tokens);
    let mut parser = Parser::new(tokens, language);
    let mut program = parser.parse_program()?;
    program.comments = comments;
    Ok(program)
}

/// Parse only the `app` header of a `.lolo` source, leaving the body unread.
pub fn parse_app_header(source: &str) -> Result<AppHeader, ParseError> {
    let (tokens, _) = tokenize_with_comments(source)?;
    let language = detect_lang(&tokens);
    let mut parser = Parser::new(tokens, language);
    parser.skip_newlines();
    parser.parse_app_header()
}

/// Alias matching the public lib name `parse`.
pub fn parse(source: &str) -> Result<Program, ParseError> {
    parse_str(source)
}

/// The `.lolo` source's language, detected the same way `parse_str` does:
/// the first identifier token is the `app` keyword in exactly one
/// non-English language, else English. Never a substring match. Returns
/// `"en"` on a lex error (the parse itself will report it).
pub fn detect_language(source: &str) -> &'static str {
    match tokenize(source) {
        Ok(tokens) => detect_lang(&tokens).code,
        Err(_) => lang::english().code,
    }
}

fn detect_lang(tokens: &[Token]) -> &'static Lang {
    tokens
        .iter()
        .find_map(|t| match &t.kind {
            TokenKind::Identifier(name) => Some(name.as_str()),
            _ => None,
        })
        .and_then(lang::for_lolo_first_token)
        .unwrap_or_else(lang::english)
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
    /// Parallel to `pending_inline_traits` (same push order/indices): the
    /// name of the trait whose body was being parsed when each fragment was
    /// hoisted, or `None` when the hoist happened outside any trait (e.g. an
    /// orbital-level `config {}`). Drives which inline page(s) each fragment
    /// is attached to at flush time.
    pending_inline_producers: Vec<Option<String>>,
    /// Name of the trait currently being parsed (`parse_trait`), so a hoist
    /// triggered from inside its body can record its producer. `None` when
    /// not inside any trait.
    current_hoist_producer: Option<String>,
    /// Monotonic counter for synthesized inline-ref names, scoped per orbital.
    inline_ref_counter: usize,
    /// Lexical binding scopes opened by `let` bindings and `fn`/`lambda`
    /// parameters. Only the parser can tell a bare identifier from a quoted
    /// string — both collapse to `SExpression::String` — so the bare-reference
    /// check has to live here.
    binding_scopes: Vec<Vec<String>>,
    /// How many `(quasiquote …)` templates enclose the expression being parsed;
    /// an `(unquote e)` is legal only when this is non-zero.
    quasiquote_depth: usize,
    /// How many `(quote …)`/`(quasiquote …)` bodies enclose the expression being parsed,
    /// reset to zero inside an `(unquote e)`; a bracket there is template JSON, never `["list", …]`.
    template_depth: usize,
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
    /// Set by the call parser for an argument the operator table declares `key-of` (the key of `object/get`, the
    /// field of `array/sort`): a bare identifier there names a field, so it is the literal key and never the
    /// `now`/`state` binding. Consumed on entry to `parse_sexpr_or_value`, like `pattern_type_value`.
    pub(crate) literal_key_position: bool,
    /// Active while a config value parses: a dotted atom (`Trait.EVENT`,
    /// `Orbital.Trait.EVENT`) is legal there and is recorded with its trail
    /// so the type check can require an `event` position.
    event_ref_capture: Option<Vec<(String, Span)>>,
    /// The `.lolo` source's detected language (English by default). Every
    /// keyword/shape/tag/category/capability/annotation/sigil/type/unit/
    /// literal/reserved-event lookup canonicalizes through this before
    /// falling back to the raw English comparison, so the parser accepts
    /// the same construct spelled in any known language.
    lang: &'static Lang,
}

impl Parser {
    /// The name of a trait the parser synthesizes for an inline `<X.traits.Y />` render: the producing trait's name,
    /// `Inline`, the referenced trait, and a file-wide counter, so atoms composed into one app never collide.
    pub(crate) fn synthetic_trait_name(&mut self, reference: &str) -> String {
        let short = reference.rsplit('.').next().filter(|s| !s.is_empty()).unwrap_or("Ref");
        let mut chars = short.chars();
        let short = match chars.next() {
            Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
            None => String::from("Ref"),
        };
        let producer = self.current_hoist_producer.clone().unwrap_or_default();
        loop {
            self.inline_ref_counter += 1;
            let candidate = format!("{producer}Inline{short}{}", self.inline_ref_counter);
            if !self.pending_inline_traits.iter().any(|t| t.name == candidate) {
                return candidate;
            }
        }
    }

    fn new(tokens: Vec<Token>, lang: &'static Lang) -> Self {
        Self {
            tokens,
            pos: 0,
            pending_inline_traits: Vec::new(),
            pending_inline_producers: Vec::new(),
            current_hoist_producer: None,
            inline_ref_counter: 0,
            binding_scopes: Vec::new(),
            quasiquote_depth: 0,
            template_depth: 0,
            expr_span_capture: None,
            expr_trail: String::new(),
            pattern_type_value: false,
            literal_key_position: false,
            event_ref_capture: None,
            lang,
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

    /// Record a placeholder span for the current trail, to be widened by
    /// [`Self::widen_reserved_span`] once the construct's end is known. It is
    /// recorded first, so it wins over spans nested parses record on the same trail.
    pub(crate) fn reserve_expr_span(&mut self) -> Option<usize> {
        let entry = (self.expr_trail.clone(), self.peek_span());
        let capture = self.expr_span_capture.as_mut()?;
        capture.push(entry);
        Some(capture.len() - 1)
    }

    /// Widen a reserved span to end where the last consumed token ends, when
    /// both sit on one line (a span is single-line).
    pub(crate) fn widen_reserved_span(&mut self, slot: Option<usize>) {
        let (Some(slot), Some(last)) = (slot, self.pos.checked_sub(1).map(|i| self.tokens[i].span))
        else {
            return;
        };
        if let Some(capture) = self.expr_span_capture.as_mut() {
            let start = capture[slot].1;
            if last.line == start.line && last.col + last.len as u32 >= start.col {
                capture[slot].1.len = (last.col + last.len as u32 - start.col) as u16;
            }
        }
    }

    /// Parse one config value, returning it with the dotted event atoms it
    /// holds, each keyed by its trail inside the value (`""`, `"[0].event"`).
    pub(crate) fn parse_config_value(
        &mut self,
    ) -> Result<(SExpression, Vec<(String, Span)>), ParseError> {
        let prev_capture = self.event_ref_capture.replace(Vec::new());
        let prev_trail = std::mem::take(&mut self.expr_trail);
        let result = self.parse_sexpr_or_value();
        self.expr_trail = prev_trail;
        let refs = self.event_ref_capture.take().unwrap_or_default();
        self.event_ref_capture = prev_capture;
        result.map(|v| (v, refs))
    }

    /// After a just-consumed identifier, take `.traits.<Pascal>` when the whole
    /// run is a trait-value literal and return its `{behavior, trait}` object;
    /// `None` (nothing consumed) otherwise.
    pub(crate) fn take_trait_value_literal(&mut self, head: &str) -> Option<SExpression> {
        let TokenKind::Identifier(traits) = self.peek_n(1).clone() else {
            return None;
        };
        let TokenKind::Identifier(name) = self.peek_n(3).clone() else {
            return None;
        };
        if !matches!(self.peek(), TokenKind::Dot)
            || (traits != "traits" && traits != "orbitals")
            || !matches!(self.peek_n(2), TokenKind::Dot)
        {
            return None;
        }
        let value = behavior_value_object(&format!("{head}.{traits}.{name}"))?;
        for _ in 0..4 {
            self.advance();
        }
        Some(value)
    }

    /// At a `.` opening a workspace-relative trait-value literal
    /// (`./<dir>/<name>.traits.<Pascal>`), consume the run and return its
    /// `{behavior, trait}` object; `None` (nothing consumed) otherwise. The
    /// lexer splits the run into `.`, `/`, `<dir>/<name>`, `.`, `traits`, `.`,
    /// `<Pascal>`, so the pieces must sit flush against one another.
    pub(crate) fn take_relative_trait_value_literal(&mut self) -> Option<SExpression> {
        let (TokenKind::Dot, TokenKind::Identifier(slash), TokenKind::Identifier(path)) =
            (self.peek(), self.peek_n(1), self.peek_n(2))
        else {
            return None;
        };
        if slash != "/" {
            return None;
        }
        let tail = (
            self.peek_n(3),
            self.peek_n(4),
            self.peek_n(5),
            self.peek_n(6),
        );
        let (
            TokenKind::Dot,
            TokenKind::Identifier(traits),
            TokenKind::Dot,
            TokenKind::Identifier(name),
        ) = tail
        else {
            return None;
        };
        if traits != "traits" && traits != "orbitals" {
            return None;
        }
        let end = (self.pos + 6).min(self.tokens.len() - 1);
        if end != self.pos + 6 {
            return None;
        }
        let flush = (self.pos..end).all(|i| {
            let (a, b) = (self.tokens[i].span, self.tokens[i + 1].span);
            a.line == b.line && a.col + u32::from(a.len) == b.col
        });
        if !flush {
            return None;
        }
        let value = behavior_value_object(&format!("./{path}.{traits}.{name}"))?;
        for _ in 0..7 {
            self.advance();
        }
        Some(value)
    }

    /// Inside a config value, extend a just-consumed identifier into a dotted
    /// event address and record it; `None` when not at one.
    pub(crate) fn take_dotted_event_ref(&mut self, head: &str, span: Span) -> Option<String> {
        self.event_ref_capture.as_ref()?;
        let mut out = head.to_string();
        while matches!(self.peek(), TokenKind::Dot)
            && matches!(self.peek_n(1), TokenKind::Identifier(_))
        {
            self.advance();
            if let TokenKind::Identifier(seg) = self.peek().clone() {
                self.advance();
                out.push('.');
                out.push_str(&seg);
            }
        }
        if out.len() == head.len() {
            return None;
        }
        let trail = self.expr_trail.clone();
        self.event_ref_capture.as_mut()?.push((trail, span));
        Some(out)
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

    pub(crate) fn scoped_span(&self, mut span: Span) -> Span {
        if let Some(last) = self
            .tokens
            .iter()
            .take(self.pos)
            .rev()
            .find(|token| !matches!(token.kind, TokenKind::Newline | TokenKind::Eof))
        {
            span.scope_end = Some((last.span.line, last.span.col + u32::from(last.span.len)));
        }
        span
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

    /// Canonicalize a bare `@annotation` sigil token (`@key`, `@label`,
    /// `@rebindable`, `@read`, …) via the `annotations` vocabulary section.
    /// These never carry a dotted path.
    pub(crate) fn canon_annotation_sigil(&self, name: &str) -> String {
        self.lang
            .canon_annotation(name)
            .map(str::to_string)
            .unwrap_or_else(|| name.to_string())
    }

    /// Canonicalize a `@root` / `@root.rest` sigil token's ROOT (the part
    /// before the first `.`) via the `sigils` vocabulary section, leaving
    /// everything after the `.` (a user field/knob name) untouched.
    pub(crate) fn canon_sigil_root(&self, raw: &str) -> String {
        canon_sigil_root_for(raw, self.lang)
    }

    /// Canonicalize an event name at the point it is captured: only the
    /// reserved lifecycle event (`INIT`) has a native translation
    /// (`reservedEvents` section) — every other event name is a user
    /// identifier and passes through untouched.
    pub(crate) fn canon_event_name(&self, name: String) -> String {
        self.lang
            .canon_reserved_event(&name)
            .map(str::to_string)
            .unwrap_or(name)
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
                Ok(self.canon_event_name(name))
            }
            TokenKind::Sigil(name) if config_ref_knob(&self.canon_sigil_root(&name)).is_some() => {
                self.advance();
                Ok(format!("@{}", self.canon_sigil_root(&name)))
            }
            _ => Err(ParseError::ExpectedIdentifier {
                got: format!("{:?}", self.peek()),
                span: self.peek_span(),
            }),
        }
    }

    pub(crate) fn expect_keyword(&mut self, kw: &str) -> Result<(), ParseError> {
        debug_assert!(
            crate::keywords::is_known_keyword(kw),
            "expect_keyword(\"{kw}\") is not in orbital-lolo's Vocabulary — add it to keywords.rs"
        );
        match self.peek().clone() {
            TokenKind::Identifier(name) if name == kw || self.lang.canon_any(&name) == Some(kw) => {
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
        debug_assert!(
            crate::keywords::is_known_keyword(name),
            "check_ident(\"{name}\") is not in orbital-lolo's Vocabulary — add it to keywords.rs"
        );
        match self.peek() {
            TokenKind::Identifier(n) => n == name || self.lang.canon_any(n) == Some(name),
            _ => false,
        }
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

        // The `app` header is required at the top level.
        let app = if self.check_ident("app") {
            self.parse_app_header()?
        } else {
            AppHeader {
                name: String::new(),
                version: None,
                description: None,
                theme: None,
                themes: Vec::new(),
                locales: Vec::new(),
                store: None,
                egress: Vec::new(),
                inbound: Vec::new(),
                os_grants: orbital_core_base::os_grants::OsGrants::default(),
                deployment: Vec::new(),
                public: None,
                site: orbital_core_base::schema::SiteMeta::default(),
                legacy_lines: false,
                config: Vec::new(),
                span: self.peek_span(),
            }
        };

        let mut type_aliases = Vec::new();
        let mut uses = Vec::new();
        let mut orbitals = Vec::new();

        loop {
            self.skip_newlines();
            if self.at_eof() {
                break;
            }
            if self.check_ident("type") {
                type_aliases.push(self.parse_type_alias()?);
            } else if self.check_ident("uses") {
                // File-scoped `uses` (orbital-import Stage A) — legal above
                // any `orbital` block, merged into every orbital's own `uses`
                // at lower time (local alias wins on collision).
                uses.push(self.parse_use_decl()?);
            } else if self.check_ident("orbital") {
                orbitals.push(self.parse_orbital()?);
            } else {
                return Err(ParseError::UnexpectedToken {
                    expected: "type, uses, or orbital".to_string(),
                    got: format!("{:?}", self.peek()),
                    span: self.peek_span(),
                });
            }
        }

        Ok(Program {
            app,
            type_aliases,
            uses,
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
        // A positional version is the pre-bracket form (reported as legacy);
        // the bracket's `version:` is canonical.
        let positional_version = match self.peek().clone() {
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
        let mut version = None;
        let mut description = None;
        let mut theme = None;
        let mut themes = Vec::new();
        let mut locales = Vec::new();
        let mut store = None;
        let mut egress = Vec::new();
        let mut inbound = Vec::new();
        let mut os_grants = orbital_core_base::os_grants::OsGrants::default();
        let mut deployment = Vec::new();
        let mut public = None;
        let mut site = orbital_core_base::schema::SiteMeta::default();
        let mut legacy_lines = positional_version.is_some();
        if matches!(self.peek(), TokenKind::LBracket) {
            self.parse_modifier_bracket("app", APP_MODIFIERS, |p, key| {
                p.expect_kind(&TokenKind::Colon, "':' after an app modifier")?;
                match key {
                    "version" => version = Some(p.parse_option_string(key)?),
                    "description" => description = Some(p.parse_option_string(key)?),
                    "theme" => theme = Some(p.parse_theme_value()?),
                    "themes" => themes = p.parse_theme_items()?,
                    "locales" => locales = p.parse_locale_items()?,
                    "egress" => egress = p.parse_string_items(key)?,
                    "inbound" => inbound = p.parse_string_items(key)?,
                    "store" => store = Some(p.expect_ident()?),
                    "deployment" => deployment = p.parse_deployment()?,
                    grant if grants::GRANT_KEYS.contains(&grant) => {
                        p.parse_grant_value(grant, &mut os_grants)?
                    }
                    "public" => public = Some(p.parse_option_string(key)?),
                    "origin" => site.origin = Some(p.parse_option_string(key)?),
                    "icon" => site.icon = Some(p.parse_option_string(key)?),
                    "siteName" => site.name = Some(p.parse_option_string(key)?),
                    "signIn" => site.sign_in = Some(p.parse_option_string(key)?),
                    "entries" => site.entries = p.parse_string_items(key)?,
                    _ => unreachable!("APP_MODIFIERS covers every key"),
                }
                Ok(())
            })?;
            self.expect_newline_or_eof()?;
            if version.is_some() && positional_version.is_some() {
                return Err(ParseError::DuplicateOption {
                    key: "version".to_string(),
                    context: "app",
                    span,
                });
            }
        } else {
            // Pre-bracket form: description / theme / locales / public on
            // their own lines. Still parsed; reported as legacy by validate.
            if matches!(self.peek(), TokenKind::Newline) {
                self.advance();
            }
            if let TokenKind::String(s) = self.peek().clone() {
                self.advance();
                self.expect_newline_or_eof()?;
                description = Some(s);
                legacy_lines = true;
            }
            self.skip_newlines();
            if self.check_ident("theme") {
                theme = Some(self.parse_orbital_theme()?);
                legacy_lines = true;
            }
            self.skip_newlines();
            if self.check_ident("locales") {
                self.advance();
                locales = self.parse_locale_items()?;
                self.expect_newline_or_eof()?;
                legacy_lines = true;
            }
            self.skip_newlines();
            if self.check_ident("public") {
                self.advance();
                public = Some(self.parse_option_string("public")?);
                self.expect_newline_or_eof()?;
                legacy_lines = true;
            }
        }
        // Optional app-level `config { ... }` block (orbital-import Stage A,
        // `OrbitalSchema.config`) — ORGANISM-wide knobs declared once, beside
        // `theme`, that every orbital's traits may forward via
        // `@config.<knob>`. Uses the DECLARATION grammar (`key : type =
        // default @annotations`), same as a trait's own `config {}`.
        self.skip_newlines();
        let config = if self.check_ident("config") {
            self.parse_config_block()?
        } else {
            Vec::new()
        };
        Ok(AppHeader {
            name,
            version: version.or(positional_version),
            description,
            theme,
            themes,
            locales,
            store,
            egress,
            inbound,
            os_grants,
            deployment,
            public,
            site,
            legacy_lines,
            config,
            span,
        })
    }

    /// `["a", "b", …]` — a list of strings (`egress`, `exec`, `env`).
    pub(crate) fn parse_string_items(&mut self, key: &str) -> Result<Vec<String>, ParseError> {
        self.expect_kind(&TokenKind::LBracket, "[")?;
        let mut origins = Vec::new();
        loop {
            self.skip_newlines();
            if matches!(self.peek(), TokenKind::RBracket) {
                self.advance();
                break;
            }
            origins.push(self.parse_option_string(key)?);
            self.skip_newlines();
            if matches!(self.peek(), TokenKind::Comma) {
                self.advance();
            }
        }
        Ok(origins)
    }

    /// `[en, ar, sl]` — the `locales` value.
    fn parse_locale_items(&mut self) -> Result<Vec<String>, ParseError> {
        self.expect_kind(&TokenKind::LBracket, "[")?;
        let mut locales = Vec::new();
        loop {
            self.skip_newlines();
            if matches!(self.peek(), TokenKind::RBracket) {
                self.advance();
                break;
            }
            locales.push(self.expect_ident()?);
            self.skip_newlines();
            if matches!(self.peek(), TokenKind::Comma) {
                self.advance();
            }
        }
        Ok(locales)
    }

    /// A modifier bracket `[key: value, flag, …]` shared by the `app` header
    /// and `page` declarations. Entries are comma- or newline-separated and
    /// may span lines. Keys are canonicalized through the language tables;
    /// an unknown or repeated key is an error. `on_entry` consumes the
    /// entry's `: value` (or nothing, for a flag).
    pub(crate) fn parse_modifier_bracket(
        &mut self,
        context: &'static str,
        keys: &[&str],
        mut on_entry: impl FnMut(&mut Self, &str) -> Result<(), ParseError>,
    ) -> Result<(), ParseError> {
        self.expect_kind(&TokenKind::LBracket, "[")?;
        let mut seen: Vec<String> = Vec::new();
        loop {
            self.skip_newlines();
            if matches!(self.peek(), TokenKind::RBracket) {
                self.advance();
                return Ok(());
            }
            let span = self.peek_span();
            let raw = self.expect_ident()?;
            let key = self
                .lang
                .canon_any(&raw)
                .filter(|k| keys.contains(k))
                .map(str::to_string)
                .unwrap_or(raw);
            if !keys.contains(&key.as_str()) {
                return Err(ParseError::UnknownOption {
                    key,
                    context,
                    valid: keys.join(", "),
                    span,
                });
            }
            if seen.contains(&key) {
                return Err(ParseError::DuplicateOption { key, context, span });
            }
            on_entry(self, &key)?;
            seen.push(key);
            self.skip_newlines();
            if matches!(self.peek(), TokenKind::Comma) {
                self.advance();
            }
        }
    }

    /// The value of one page head modifier (`access`, `indexing`, `title`,
    /// `description`, `translationOf`) into `mods`; the `:` is already consumed.
    pub(crate) fn parse_head_modifier(
        &mut self,
        key: &str,
        mods: &mut orbital_core_base::schema::PageModifiers,
    ) -> Result<(), ParseError> {
        use orbital_core_base::schema::{PageAccess, PageIndexing};
        match key {
            "access" => {
                mods.access = Some(
                    match self.parse_option_word(
                        key,
                        "ORB_P_ACCESS_INVALID",
                        &["public", "authenticated"],
                    )? {
                        "public" => PageAccess::Public,
                        _ => PageAccess::Authenticated,
                    },
                )
            }
            "indexing" => {
                mods.indexing = Some(
                    match self.parse_option_word(
                        key,
                        "ORB_P_INDEXING_INVALID",
                        &["index", "noindex"],
                    )? {
                        "index" => PageIndexing::Index,
                        _ => PageIndexing::Noindex,
                    },
                )
            }
            "title" => mods.title = Some(self.parse_page_meta(key)?),
            "description" => mods.description = Some(self.parse_page_meta(key)?),
            "translationOf" => mods.translation_of = Some(self.parse_option_string(key)?),
            "label" => mods.label = Some(self.parse_option_string(key)?),
            "icon" => mods.icon = Some(self.parse_option_string(key)?),
            "roles" => mods.roles = Some(self.parse_role_list(key)?),
            _ => unreachable!("REMAP_PAGE_MODIFIERS covers every head key"),
        }
        Ok(())
    }

    /// A nav entry's roles: one quoted, comma-separated list of role words.
    pub(crate) fn parse_role_list(&mut self, key: &str) -> Result<Vec<String>, ParseError> {
        let span = self.peek_span();
        let raw = self.parse_option_string(key)?;
        let roles: Vec<String> = raw
            .split(',')
            .map(str::trim)
            .filter(|r| !r.is_empty())
            .map(str::to_string)
            .collect();
        if roles.is_empty() {
            return Err(ParseError::InvalidOptionValue {
                code: "ORB_L_OPTION_VALUE",
                key: key.to_string(),
                message: "expected at least one role, e.g. \"manager, owner\"".to_string(),
                span,
            });
        }
        Ok(roles)
    }

    pub(crate) fn parse_option_string(&mut self, key: &str) -> Result<String, ParseError> {
        match self.peek().clone() {
            TokenKind::String(s) => {
                self.advance();
                Ok(s)
            }
            other => Err(ParseError::InvalidOptionValue {
                code: "ORB_L_OPTION_VALUE",
                key: key.to_string(),
                message: format!("expected a quoted string, got {other:?}"),
                span: self.peek_span(),
            }),
        }
    }

    /// A canonical identifier value from `allowed` (e.g. `access: public`).
    pub(crate) fn parse_option_word(
        &mut self,
        key: &str,
        code: &'static str,
        allowed: &[&'static str],
    ) -> Result<&'static str, ParseError> {
        let span = self.peek_span();
        let raw = self.expect_ident()?;
        let canon = self
            .lang
            .canon_any(&raw)
            .unwrap_or(raw.as_str())
            .to_string();
        allowed
            .iter()
            .find(|a| **a == canon)
            .copied()
            .ok_or(ParseError::InvalidOptionValue {
                code,
                key: key.to_string(),
                message: format!("`{raw}` is not one of {}", allowed.join(" | ")),
                span,
            })
    }

    /// `title:` / `description:` — a string literal or `(i18n/t "key")`.
    pub(crate) fn parse_page_meta(
        &mut self,
        key: &str,
    ) -> Result<orbital_core_base::schema::PageMeta, ParseError> {
        use orbital_core_base::schema::PageMeta;
        let span = self.peek_span();
        if let TokenKind::String(s) = self.peek().clone() {
            self.advance();
            return Ok(PageMeta::Literal(s));
        }
        let expr = self.parse_sexpr_or_value()?;
        if let SExpression::Array(items) = &expr {
            if let [SExpression::String(op), SExpression::String(k)] = items.as_slice() {
                if op == "i18n/t" && !k.starts_with('@') {
                    return Ok(PageMeta::message(k.clone()));
                }
            }
        }
        Err(ParseError::InvalidOptionValue {
            code: "ORB_P_META_FRAME",
            key: key.to_string(),
            message: "page metadata is a string literal or `(i18n/t \"key\")` — no entity, config or viewer bindings".to_string(),
            span,
        })
    }

    /// `themes:` value — `["<key>" | { … }, …]`, each entry a theme value.
    fn parse_theme_items(&mut self) -> Result<Vec<ThemeDecl>, ParseError> {
        self.expect_kind(&TokenKind::LBracket, "[")?;
        let mut themes = Vec::new();
        loop {
            self.skip_newlines();
            if matches!(self.peek(), TokenKind::RBracket) {
                self.advance();
                break;
            }
            themes.push(self.parse_theme_value()?);
            self.skip_newlines();
            if matches!(self.peek(), TokenKind::Comma) {
                self.advance();
            }
        }
        Ok(themes)
    }

    /// `theme:` value — a registry key string or an inline `{ … }` block.
    fn parse_theme_value(&mut self) -> Result<ThemeDecl, ParseError> {
        match self.peek().clone() {
            TokenKind::String(s) => {
                self.advance();
                Ok(ThemeDecl::Reference(s))
            }
            TokenKind::LBrace => self.parse_inline_theme(),
            other => Err(ParseError::UnexpectedToken {
                expected: "a quoted theme key, e.g. theme: \"linear-clean-light\", or an inline { … } block".to_string(),
                got: format!("{:?}", other),
                span: self.peek_span(),
            }),
        }
    }

    // ----- Orbital -----

    fn parse_orbital(&mut self) -> Result<Orbital, ParseError> {
        let span = self.peek_span();
        self.expect_keyword("orbital")?;
        let name = self.expect_ident()?;

        // Optional `[ fs: …, exec: … ]` grant bracket.
        let mut os_grants = orbital_core_base::os_grants::OsGrants::default();
        if matches!(self.peek(), TokenKind::LBracket) {
            self.parse_modifier_bracket("orbital", ORBITAL_MODIFIERS, |p, key| {
                p.expect_kind(&TokenKind::Colon, "':' after an orbital modifier")?;
                p.parse_grant_value(key, &mut os_grants)
            })?;
        }

        // Optional `:: TypeName [args]` annotation applied to the orbital body.
        let type_annotation = self.try_parse_type_annotation()?;

        // Reference form: `orbital Name [:: ...] = Alias.orbitals.Upstream { ... }`
        // (orbital-import Stage A). The `=` immediately after the
        // name/annotation disambiguates from the inline form, exactly as
        // `parse_trait` dispatches on `=` before falling into `->`.
        if matches!(self.peek(), TokenKind::Equals) {
            return self.parse_orbital_reference(name, span, type_annotation, os_grants);
        }

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
        let mut config = Vec::new();

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
                for decl in self.parse_expects_decls()? {
                    if let Some(dup) = expects_duplicate(&expects, &decl) {
                        return Err(ParseError::InvalidType {
                            span: decl.span,
                            message: format!(
                                "ELOLO_DUPLICATE_EXPECTATION: orbital declares {dup} more than once"
                            ),
                        });
                    }
                    expects.push(decl);
                }
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
            } else if self.check_ident("config") {
                // Orbital's own declared `config { ... }` block
                // (orbital-import Stage A, `Orbital.config`) — knobs THIS
                // orbital publishes, forwarded by its traits via
                // `@config.<knob>`. Declaration grammar, same as app-level
                // `config {}` and a trait's own `config {}`.
                config.extend(self.parse_config_block()?);
            } else if matches!(self.peek(), TokenKind::LParen) {
                // Top-level s-expression — typically `(behavior/compose ...)`.
                let expr = self.parse_sexpr()?;
                body.push(expr);
                self.expect_newline_or_eof()?;
            } else {
                return Err(ParseError::UnexpectedToken {
                    expected:
                        "uses, expects, type, entity, trait, page, theme, config, or s-expression"
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
        let pending_producers = std::mem::take(&mut self.pending_inline_producers);
        // NOTE: `inline_ref_counter` is intentionally NOT reset here. It must
        // stay file-global so synthesized names (`InlineX<n>`) are unique
        // across ALL orbitals in the file — the validator forbids the same
        // trait name in two orbitals (`ORB_T_DUPLICATE_NAME`). Resetting per
        // orbital made both orbitals emit `InlineTypographyRender1` etc.
        if !pending.is_empty() {
            let named: Vec<(String, Option<String>)> = pending
                .iter()
                .map(|t| t.name.clone())
                .zip(pending_producers)
                .collect();
            // EXEC-ENDPOINT-3a: a fragment with no declared mode takes its producer's.
            for (fragment, (_, producer)) in pending.into_iter().zip(&named) {
                let mut fragment = fragment;
                if fragment.dispatch.is_none() {
                    fragment.dispatch = producer
                        .as_deref()
                        .and_then(|p| traits.iter().find(|t| t.name == p))
                        .and_then(|t| t.dispatch.clone());
                }
                traits.push(fragment);
            }
            let inline_page_count = pages.iter().filter(|p| p.reference.is_none()).count();
            for page in pages.iter_mut() {
                // Only inline pages own their trait roster; a reference page's
                // `traits` are call-site overrides, not a mount list.
                if page.reference.is_some() {
                    continue;
                }
                // A fragment belongs to a page iff the page directly
                // declares the trait whose body produced it (a fragment
                // hoisted while parsing trait X mounts only where X is
                // mounted) — or the fragment has no producer (hoisted
                // outside any trait, e.g. orbital-level config: orbital-wide,
                // so every page gets it). With exactly one inline page there
                // is no attribution to resolve, so it keeps every fragment,
                // matching prior behavior byte-for-byte.
                let declared: std::collections::HashSet<String> =
                    page.traits.iter().cloned().collect();
                for (name, producer) in &named {
                    if declared.contains(name) {
                        continue;
                    }
                    let belongs = inline_page_count <= 1
                        || producer.is_none()
                        || producer.as_deref().is_some_and(|p| declared.contains(p));
                    if belongs {
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
            reference: None,
            entity_rebind: None,
            entity_rebind_tags: None,
            fields_remap: Vec::new(),
            pages_remap: Vec::new(),
            page_modifiers: Vec::new(),
            omit: Vec::new(),
            only: Vec::new(),
            config_overrides: Vec::new(),
            events_remap: Vec::new(),
            roles_remap: Vec::new(),
            entities_remap: Vec::new(),
            mounts: Vec::new(),
            config,
            os_grants,
            extend_fields: Vec::new(),
            mock_overrides: Vec::new(),
            retype_fields: Vec::new(),
            trait_overrides: Vec::new(),
            import_listens: Vec::new(),
            alias_entity_refs: Vec::new(),
            span: self.scoped_span(span),
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
            TokenKind::LBrace => {
                let theme = self.parse_inline_theme()?;
                self.expect_newline_or_eof()?;
                Ok(theme)
            }
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
        let mut display_name: Option<String> = None;
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
                TokenKind::Identifier(kw) if kw == "displayName" => {
                    self.advance();
                    self.expect_kind(&TokenKind::Colon, "':' after displayName")?;
                    match self.peek().clone() {
                        TokenKind::String(s) => {
                            self.advance();
                            display_name = Some(s);
                        }
                        other => {
                            return Err(ParseError::UnexpectedToken {
                                expected: "a quoted theme display name".to_string(),
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
                        expected: "name:, displayName:, tokens { … }, variants { … }, or }"
                            .to_string(),
                        got: format!("{:?}", other),
                        span: self.peek_span(),
                    })
                }
            }
        }
        let name = name.ok_or(ParseError::UnexpectedToken {
            expected: "an inline theme requires name: \"…\"".to_string(),
            got: "end of theme block".to_string(),
            span,
        })?;
        Ok(ThemeDecl::Inline(InlineTheme {
            name,
            display_name,
            tokens,
            variants,
        }))
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
                    if let Some((_, keys)) =
                        THEME_TYPED_AXIS_KEYS.iter().find(|(axis, _)| *axis == cat)
                    {
                        if let Some((bad, _)) = entries.iter().find(|(k, _)| !keys.contains(&k.as_str())) {
                            return Err(ParseError::UnexpectedToken {
                                expected: format!("a `{}` token: {}", cat, keys.join(", ")),
                                got: bad.clone(),
                                span: cat_span,
                            });
                        }
                        for (key, value) in &entries {
                            if let Some((_, _, allowed)) = THEME_TYPED_AXIS_VALUES
                                .iter()
                                .find(|(axis, k, _)| *axis == cat && k == key)
                            {
                                if !allowed.contains(&value.as_str()) {
                                    return Err(ParseError::UnexpectedToken {
                                        expected: format!("`{}.{}`: {}", cat, key, allowed.join(", ")),
                                        got: value.clone(),
                                        span: cat_span,
                                    });
                                }
                            }
                        }
                    }
                    match cat.as_str() {
                        "colors" => maps.colors = entries,
                        "radii" => maps.radii = entries,
                        "spacing" => maps.spacing = entries,
                        "typography" => maps.typography = entries,
                        "shadows" => maps.shadows = entries,
                        "typeScale" => maps.type_scale = entries,
                        "geometry" => maps.geometry = entries,
                        "elevation" => maps.elevation = entries,
                        "surface" => maps.surface = entries,
                        "motion" => maps.motion = entries,
                        "diagram" => maps.diagram = entries,
                        "scene" => maps.scene = entries,
                        _ => return Err(ParseError::UnexpectedToken {
                            expected:
                                "a token category: colors, radii, spacing, typography, shadows, typeScale, geometry, elevation, surface, motion, diagram, or scene"
                                    .to_string(),
                            got: cat,
                            span: cat_span,
                        }),
                    }
                }
                other => {
                    return Err(ParseError::UnexpectedToken {
                        expected:
                            "a token category (colors, radii, spacing, typography, shadows, typeScale, geometry, elevation, surface, motion, diagram, scene) or }"
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
        let lazy = self.check_ident("lazy") && matches!(self.peek_n(1), TokenKind::Identifier(_));
        if lazy {
            self.advance();
        }
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
        // Optional `{ config { ... } }` override body (orbital-import Stage
        // A) — sets the imported behavior's ORGANISM-wide `config {}` knobs
        // for every orbital taken from this alias. `config` may also be
        // written directly after `from "path"` without an enclosing brace
        // pair of its own — `parse_config_override_block` owns the `{ }`.
        let config = if matches!(self.peek(), TokenKind::LBrace) {
            self.advance();
            self.skip_newlines();
            let entries = if self.check_ident("config") {
                self.parse_config_override_block()?
            } else {
                Vec::new()
            };
            self.skip_newlines();
            self.expect_kind(&TokenKind::RBrace, "}")?;
            entries
        } else {
            Vec::new()
        };
        self.expect_newline_or_eof()?;
        Ok(UseDecl {
            alias,
            path,
            lazy,
            config,
            span,
        })
    }

    /// `expects` ("entity" Ident? tags? shape? | "trait" Ident "{" "emits" "{" EVENT+ "}" "}" | "page" STRING); `[identity]` makes it the roster
    /// — one expectation per line; the shape reuses the entity field parser.
    ///
    /// The identity name stays OPTIONAL on purpose. A program has exactly one
    /// `[identity]` entity and it is referenced as `@user`, never by its entity
    /// name — so the name here is informational, and requiring it would invite
    /// authors to write `Person.role` where the language means `@user.role`.
    /// What makes an identity expectation checkable is its SHAPE, not its name.
    fn parse_expects_decls(&mut self) -> Result<Vec<ExpectsDecl>, ParseError> {
        let span = self.peek_span();
        self.expect_keyword("expects")?;
        let kinds = if self.check_ident("entity") {
            self.advance();
            let name = if matches!(self.peek(), TokenKind::Identifier(_)) {
                Some(self.expect_ident()?)
            } else {
                None
            };
            let tags = if matches!(self.peek(), TokenKind::LBracket) {
                self.parse_expects_entity_tags()?
            } else {
                ExpectedTags::default()
            };
            let (shape, access, invariants) = self.parse_expects_shape()?;
            if tags.identity {
                if access.has_any() || !invariants.is_empty() {
                    return Err(ParseError::InvalidType {
                        span,
                        message: "an `expects entity [identity]` states the roster's shape only — access directives and invariants belong to a stored entity".to_string(),
                    });
                }
                vec![ExpectsKind::Identity { name, shape }]
            } else {
                let Some(name) = name else {
                    return Err(ParseError::UnexpectedToken {
                        expected: "an entity name, or `[identity]` for the viewer's roster"
                            .to_string(),
                        got: format!("{:?}", self.peek()),
                        span: self.peek_span(),
                    });
                };
                vec![ExpectsKind::Entity {
                    name,
                    shape,
                    persistence: tags.persistence,
                    shared: tags.shared,
                    browser: tags.browser,
                    own: tags.own,
                    access,
                    invariants,
                }]
            }
        } else if self.check_ident("trait") {
            self.advance();
            let trait_name = self.expect_ident()?;
            self.parse_expects_trait_body(&trait_name)?
        } else if self.check_ident("page") {
            self.advance();
            match self.peek().clone() {
                TokenKind::String(path) => {
                    self.advance();
                    let traits = self.parse_expects_page_traits()?;
                    vec![ExpectsKind::Page {
                        path,
                        protocol: None,
                        traits,
                    }]
                }
                TokenKind::Identifier(word) => {
                    self.advance();
                    let traits = self.parse_expects_page_traits()?;
                    vec![ExpectsKind::Page {
                        path: String::new(),
                        protocol: Some(self.lang.canon_keyword(&word).map(str::to_string).unwrap_or(word)),
                        traits,
                    }]
                }
                other => {
                    return Err(ParseError::UnexpectedToken {
                        expected: "a quoted route pattern (expects page \"/articles/:slug\") or a protocol word (expects page boot)"
                            .to_string(),
                        got: format!("{:?}", other),
                        span: self.peek_span(),
                    })
                }
            }
        } else if self.check_ident("config") {
            self.parse_expects_config()?
        } else if self.check_ident("identity") {
            let at = self.peek_span();
            self.advance();
            let spelled = if matches!(self.peek(), TokenKind::Identifier(_)) {
                format!("expects entity {} [identity]", self.expect_ident()?)
            } else {
                "expects entity [identity]".to_string()
            };
            return Err(ParseError::InvalidType {
                span: at,
                message: format!("`expects identity` is now `{spelled}` — `expects` prefixes the real entity declaration"),
            });
        } else if self.check_ident("event") {
            let at = self.peek_span();
            self.advance();
            let path = self.parse_dotted_ident()?;
            let (t, e) = path.rsplit_once('.').unwrap_or((path.as_str(), "EVENT"));
            return Err(ParseError::InvalidType {
                span: at,
                message: format!("`expects event {path}` is now `expects trait {t} {{ emits {{ {e} }} }}` — `expects` prefixes the real trait declaration"),
            });
        } else {
            return Err(ParseError::UnexpectedToken {
                expected: "entity, trait, page, or config after `expects`".to_string(),
                got: format!("{:?}", self.peek()),
                span: self.peek_span(),
            });
        };
        self.expect_newline_or_eof()?;
        Ok(kinds
            .into_iter()
            .map(|kind| ExpectsDecl { kind, span })
            .collect())
    }

    /// `expects config { name : type [= default] … }`: the real `config` block, restricted to each knob's name, type
    /// and default (annotations describe a provider, not a requirement).
    fn parse_expects_page_traits(&mut self) -> Result<Vec<String>, ParseError> {
        let mut traits = Vec::new();
        if matches!(self.peek(), TokenKind::Arrow | TokenKind::UnicodeArrow) {
            self.advance();
            loop {
                traits.push(self.expect_ident()?);
                if matches!(self.peek(), TokenKind::Comma) {
                    self.advance();
                    continue;
                }
                break;
            }
        }
        Ok(traits)
    }

    fn parse_expects_config(&mut self) -> Result<Vec<ExpectsKind>, ParseError> {
        let span = self.peek_span();
        let fields = self.parse_config_block()?;
        if fields.is_empty() {
            return Err(ParseError::InvalidType {
                span,
                message: "`expects config` must name at least one knob".to_string(),
            });
        }
        let mut out = Vec::with_capacity(fields.len());
        for field in fields {
            if field.label.is_some()
                || field.description.is_some()
                || field.synonyms.is_some()
                || field.tier.is_some()
            {
                return Err(ParseError::InvalidType {
                    span: field.span,
                    message: format!(
                        "an `expects config` knob names its type and default only — annotations on `{}` describe a provider, not a requirement",
                        field.name
                    ),
                });
            }
            out.push(ExpectsKind::Config { field });
        }
        Ok(out)
    }

    /// The header tags an `expects entity` takes: `identity` (the viewer's roster) or the storage facts of the real
    /// entity header (`persistent`/`runtime`, `shared`, `local`). A collection name (`persistent: orders`) and `mock`
    /// are the provider's own business.
    fn parse_expects_entity_tags(&mut self) -> Result<ExpectedTags, ParseError> {
        let span = self.peek_span();
        let tags = self.parse_tag_list(true)?;
        if let Some(retired) = tags.retired_words.first() {
            return Err(ParseError::InvalidType {
                span: retired.span,
                message: crate::error::retired_entity_word_message(&retired.word),
            });
        }
        if tags.collection.is_some() || tags.store.is_some() {
            return Err(ParseError::InvalidType {
                span,
                message: "an `expects entity` header names the storage mode, `shared` or `browser` only — a collection name and `store` describe a provider, not a requirement".to_string(),
            });
        }
        if tags.identity && (tags.persistence.is_some() || tags.shared || tags.browser || tags.own) {
            return Err(ParseError::InvalidType {
                span,
                message: "an `expects entity [identity]` takes only the `identity` tag".to_string(),
            });
        }
        Ok(ExpectedTags {
            identity: tags.identity,
            persistence: tags.persistence,
            shared: tags.shared,
            browser: tags.browser,
            own: tags.own,
        })
    }

    /// The body of `expects trait T { … }`: the real trait syntax with every body left off: `emits { E { fields } }`,
    /// `state NAME` and `state NAME { EVENT -> target }` (no guard, no effects), and the spec proposition
    /// `state NAME @holds <expr>`.
    fn parse_expects_trait_body(
        &mut self,
        trait_name: &str,
    ) -> Result<Vec<ExpectsKind>, ParseError> {
        let open = self.peek_span();
        self.expect_kind(&TokenKind::LBrace, "{")?;
        let mut out = Vec::new();
        loop {
            self.skip_newlines();
            if matches!(self.peek(), TokenKind::RBrace) {
                self.advance();
                break;
            }
            if self.check_ident("emits") {
                for (event, payload) in self.parse_expects_emits()? {
                    out.push(ExpectsKind::Event {
                        trait_name: trait_name.to_string(),
                        event,
                        payload,
                    });
                }
            } else if self.check_ident("state")
                && matches!(self.peek_n(2), TokenKind::Sigil(raw) if self.canon_annotation_sigil(raw) == "holds")
            {
                self.advance();
                let state = self.expect_ident()?;
                self.advance();
                let holds = self.parse_sexpr_or_value()?;
                out.push(ExpectsKind::State {
                    trait_name: trait_name.to_string(),
                    state,
                    holds: Some(holds),
                });
            } else if self.check_ident("state") {
                let state = self.parse_state()?;
                let at = state.span;
                if state.time_bound.is_some() {
                    return Err(ParseError::InvalidType {
                        span: at,
                        message: format!("an `expects trait` state `{}` names the state only — a time bound is the provider's", state.name),
                    });
                }
                if state.transitions.is_empty() || state.holds.is_some() {
                    out.push(ExpectsKind::State {
                        trait_name: trait_name.to_string(),
                        state: state.name.clone(),
                        holds: state.holds.clone(),
                    });
                }
                for t in state.transitions {
                    if t.guard.is_some() || !t.effects.is_empty() || t.term_match.is_some() {
                        return Err(ParseError::InvalidType {
                            span: t.span,
                            message: format!(
                                "an `expects trait` transition `{} -> {}` names the edge only — a guard, effects or a match belong to the provider",
                                t.event, t.target
                            ),
                        });
                    }
                    out.push(ExpectsKind::Transition {
                        trait_name: trait_name.to_string(),
                        from: state.name.clone(),
                        event: t.event,
                        to: t.target,
                        ensures: t.ensures,
                    });
                }
            } else {
                return Err(ParseError::UnexpectedToken {
                    expected: "`emits { … }` or `state NAME` inside `expects trait`".to_string(),
                    got: format!("{:?}", self.peek()),
                    span: self.peek_span(),
                });
            }
        }
        if out.is_empty() {
            return Err(ParseError::InvalidType {
                span: open,
                message:
                    "`expects trait` must state at least one emitted event, state or transition"
                        .to_string(),
            });
        }
        Ok(out)
    }

    /// `emits { EVENT [{ fields }] … }` inside `expects trait T` — the real `emits` grammar, restricted to the event
    /// name and its payload (scope and annotations describe a provider, not a requirement).
    fn parse_expects_emits(&mut self) -> Result<Vec<(String, Vec<EventField>)>, ParseError> {
        let span = self.peek_span();
        let emits = self.parse_emits_block()?;
        if emits.is_empty() {
            return Err(ParseError::InvalidType {
                span,
                message: "`expects trait` must list at least one emitted event".to_string(),
            });
        }
        let mut out = Vec::with_capacity(emits.len());
        for emit in emits {
            if emit.scope.is_some()
                || emit.description.is_some()
                || emit.synonyms.is_some()
                || emit.tier.is_some()
            {
                return Err(ParseError::InvalidType {
                    span: emit.span,
                    message: format!(
                        "an `expects trait` emit names the event and its payload only — scope and annotations on `{}` describe a provider, not a requirement",
                        emit.event
                    ),
                });
            }
            out.push((emit.event, emit.payload));
        }
        Ok(out)
    }

    /// Optional `{ field_decl+ }` after an `expects entity` header — the §6 entity field-declaration grammar,
    /// verbatim, plus the entity's own `@read|@create|@update|@delete` directives and `@invariant <expr>` lines.
    #[allow(clippy::type_complexity)]
    fn parse_expects_shape(
        &mut self,
    ) -> Result<(Option<Vec<EntityField>>, ExpectedAccess, Vec<SExpression>), ParseError> {
        let mut access = ExpectedAccess::default();
        let mut invariants = Vec::new();
        if !matches!(self.peek(), TokenKind::LBrace) {
            return Ok((None, access, invariants));
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
            if let Some(name) = self.at_access_directive() {
                self.parse_access_directive(&name, &mut access)?;
            } else if matches!(self.peek(), TokenKind::Sigil(raw) if self.canon_annotation_sigil(raw) == "invariant")
            {
                self.advance();
                invariants.push(self.parse_sexpr_or_value()?);
                self.expect_newline_or_eof()?;
            } else if self.is_entity_field_start() {
                fields.push(self.parse_field_decl()?);
            } else {
                return Err(ParseError::UnexpectedToken {
                    expected: "field declaration or }".to_string(),
                    got: format!("{:?}", self.peek()),
                    span: self.peek_span(),
                });
            }
        }
        if fields.is_empty() && !access.has_any() && invariants.is_empty() {
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
        Ok(((!fields.is_empty()).then_some(fields), access, invariants))
    }
}

/// The tags of an `expects entity` header.
#[derive(Default)]
struct ExpectedTags {
    identity: bool,
    persistence: Option<Persistence>,
    shared: bool,
    browser: bool,
    own: bool,
}

/// Duplicate key for `ELOLO_DUPLICATE_EXPECTATION`: same kind+name, or the
/// same event. Returns a human-readable description of the duplicate.
fn expects_duplicate(existing: &[ExpectsDecl], new: &ExpectsDecl) -> Option<String> {
    existing.iter().find_map(|e| match (&e.kind, &new.kind) {
        (ExpectsKind::Identity { name: a, .. }, ExpectsKind::Identity { name: b, .. })
            if a == b =>
        {
            Some(match b {
                Some(n) => format!("`expects entity {n} [identity]`"),
                None => "`expects entity [identity]`".to_string(),
            })
        }
        (ExpectsKind::Entity { name: a, .. }, ExpectsKind::Entity { name: b, .. }) if a == b => {
            Some(format!("`expects entity {b}`"))
        }
        (
            ExpectsKind::State {
                trait_name: ta,
                state: sa,
                ..
            },
            ExpectsKind::State {
                trait_name: tb,
                state: sb,
                ..
            },
        ) if ta == tb && sa == sb => Some(format!("`expects trait {tb}` state `{sb}`")),
        (
            ExpectsKind::Transition {
                trait_name: ta,
                from: fa,
                event: ea,
                ..
            },
            ExpectsKind::Transition {
                trait_name: tb,
                from: fb,
                event: eb,
                ..
            },
        ) if ta == tb && fa == fb && ea == eb => {
            Some(format!("`expects trait {tb}` transition `{fb}` on `{eb}`"))
        }
        (
            ExpectsKind::Event {
                trait_name: ta,
                event: ea,
                ..
            },
            ExpectsKind::Event {
                trait_name: tb,
                event: eb,
                ..
            },
        ) if ta == tb && ea == eb => Some(format!("`expects trait {tb} {{ emits {{ {eb} }} }}`")),
        (
            ExpectsKind::Page {
                path: a,
                protocol: pa,
                ..
            },
            ExpectsKind::Page {
                path: b,
                protocol: pb,
                ..
            },
        ) if a == b && pa == pb => Some(match pb {
            Some(word) => format!("`expects page {word}`"),
            None => format!("`expects page \"{b}\"`"),
        }),
        (ExpectsKind::Config { field: a }, ExpectsKind::Config { field: b })
            if a.name == b.name =>
        {
            Some(format!("`expects config {{ {} }}`", b.name))
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

/// Reject `match` as an EFFECT — a top-level entry in a transition's/tick's/
/// term-match-arm's effect list. Expression-match (Phase 3b) produces a
/// VALUE, never an effect list (arms are single expressions, not
/// `Vec<SExpression>`), so a bare `(match …)` there would need a second,
/// ad-hoc "arm = effects" grammar living outside the state machine —
/// control flow belongs in guards + transitions, not a value-producing
/// construct. Called at every effect-parsing call site (`parse_transition`,
/// `parse_term_match`, `parse_ticks`) right after each effect sexpr is
/// parsed; a nested `(match …)` used AS A VALUE inside an ordinary effect
/// (e.g. `(set @entity.x (match …))`) is unaffected — this only looks at
/// the effect's own head.
pub(crate) fn reject_match_effect_head(eff: &SExpression, span: Span) -> Result<(), ParseError> {
    if matches!(eff, SExpression::Array(arr) if matches!(arr.first(), Some(SExpression::String(s)) if s == "match"))
    {
        return Err(ParseError::InvalidSExpr {
            message: "ELOLO_MATCH_EXPR_EFFECT_HEAD: `match` can't be an effect — it produces a \
                      VALUE, not a list of effects; control flow lives in the state machine \
                      (guards + transitions, or transition-position `match`), not a \
                      value-producing expression. Use it as a value inside an effect instead, \
                      e.g. `(set @entity.x (match …))`"
                .to_string(),
            span,
        });
    }
    Ok(())
}

/// `name` is expected already-canonicalized (English) — every caller reaches
/// this after `Lang::canon_type`. The primitive/special-type word list has
/// one owner, `crate::keywords::vocabulary().types` (the parser's B1
/// `types` section, with per-word rationale); the only call site
/// (`parser::type::parse_type_atom`) intercepts the 8 config-only special
/// names (`Map trait entity event slot pattern secret SExpr`) before
/// falling through here, so their presence in `vocabulary().types`
/// alongside the true primitives is harmless.
pub(crate) fn is_primitive_type(name: &str) -> bool {
    crate::keywords::vocabulary().types.contains(&name)
}

/// Return `Some(type_name)` when the type expression contains a config-only
/// type (`TypeExpr::Slot`, `TypeExpr::Pattern`) at any
/// nesting depth. Entity fields call this to reject these types — see
/// `ELOLO_CONFIG_TYPE_FORBIDDEN_IN_ENTITY`.
pub(crate) fn type_expr_config_only_type(t: &TypeExpr) -> Option<&'static str> {
    type_expr_config_only_type_allowing(t, &[])
}

/// Config-only types that a runtime (never persisted) entity may hold: `SExpr` (owner ruling
/// 2026-10-13 — a played scene records `EffectTrace.args : SExpr[]`).
pub(crate) const RUNTIME_ENTITY_ALLOWED: &[&str] = &["SExpr"];

/// [`type_expr_config_only_type`] with some brands allowed.
pub(crate) fn type_expr_config_only_type_allowing(
    t: &TypeExpr,
    allow: &[&str],
) -> Option<&'static str> {
    let found = match t {
        TypeExpr::Trait | TypeExpr::Orbital | TypeExpr::Behavior => None,
        // `@entity` self-reference: payload-positions-only, so an entity field
        // must reject it the same way it rejects the config-only brands.
        TypeExpr::BoundEntityRow => Some("@entity"),
        TypeExpr::EventPayloadOf(_) => Some("@payload"),
        TypeExpr::NamedEntityRow(_) => Some("@entity <knob>"),
        TypeExpr::FieldRef { .. } => Some("field"),
        TypeExpr::ExpectedIdentityRow => Some("@user"),
        TypeExpr::EntityRef => Some("entity"),
        TypeExpr::EventRef => Some("event"),
        TypeExpr::EventAddress => Some("EventAddress"),
        TypeExpr::Slot => Some("slot"),
        TypeExpr::Pattern => Some("pattern"),
        TypeExpr::Secret => Some("secret"),
        TypeExpr::SExpr => Some("SExpr"),
        // `match` is payload-position-only (emits/local-listens fields);
        // entity fields and `expects` shapes reject it the same way they
        // reject the other payload-only sigils above.
        TypeExpr::Match { .. } => Some("match"),
        TypeExpr::Array(inner) => type_expr_config_only_type_allowing(inner, allow),
        TypeExpr::Tuple(slots) => slots
            .iter()
            .find_map(|p| type_expr_config_only_type_allowing(p, allow)),
        TypeExpr::Map(key, value) => type_expr_config_only_type_allowing(key, allow)
            .or_else(|| type_expr_config_only_type_allowing(value, allow)),
        TypeExpr::Optional(inner) => type_expr_config_only_type_allowing(inner, allow),
        TypeExpr::Union(parts) => parts
            .iter()
            .find_map(|p| type_expr_config_only_type_allowing(p, allow)),
        TypeExpr::Object(fields) => fields
            .iter()
            .find_map(|(_, ty, _)| type_expr_config_only_type_allowing(ty, allow)),
        TypeExpr::OpenObject { fields, additional } => fields
            .iter()
            .find_map(|(_, ty, _)| type_expr_config_only_type_allowing(ty, allow))
            .or_else(|| type_expr_config_only_type_allowing(additional, allow)),
        TypeExpr::Primitive(_)
        | TypeExpr::Named(_)
        | TypeExpr::StringLiteral(_)
        | TypeExpr::RelationMany(_)
        | TypeExpr::RelationManyNonEmpty(_) => None,
        TypeExpr::FieldOf { resolved, .. } => resolved
            .as_deref()
            .and_then(|r| type_expr_config_only_type_allowing(r, allow)),
    };
    found.filter(|ty| !allow.contains(ty))
}

/// True when `t` is (or nests, at any depth) a `TypeExpr::Match`. Does NOT
/// descend into a `Match`'s own arm types — an arm nesting another `match`
/// is a distinct, later check (`ELOLO_MATCH_ARM_TYPE_UNSUPPORTED`, v1 caps
/// arms to `Primitive`/`[Primitive]`). Used to gate match OUT of every
/// position except an event-payload field's own whole type: config fields,
/// type-alias bodies (including `Event` alias payloads — the position a
/// match there would otherwise silently drop its conditional through, since
/// the alias-resolution lowering path never reads `type_when`), and nested
/// inside another type.
pub(crate) fn type_expr_contains_match(t: &TypeExpr) -> bool {
    match t {
        TypeExpr::Match { .. } => true,
        TypeExpr::Array(inner) | TypeExpr::Optional(inner) => type_expr_contains_match(inner),
        TypeExpr::Map(key, value) => {
            type_expr_contains_match(key) || type_expr_contains_match(value)
        }
        TypeExpr::Union(parts) => parts.iter().any(type_expr_contains_match),
        TypeExpr::Object(fields) => fields.iter().any(|(_, ty, _)| type_expr_contains_match(ty)),
        TypeExpr::OpenObject { fields, additional } => {
            fields.iter().any(|(_, ty, _)| type_expr_contains_match(ty))
                || type_expr_contains_match(additional)
        }
        _ => false,
    }
}

/// True when any `TypeExpr` reachable from a type-alias body contains
/// `match` — the single gate point for `parse_type_alias` (covers `Entity`/
/// `Event`/`Trait`/`Page`/`Orbital`/struct/alias/composition bodies alike,
/// so a `type X = Event { value: match ... }` is rejected at the SOURCE of
/// the alias rather than trusted to every consumer).
pub(crate) fn type_body_contains_match(b: &TypeBody) -> bool {
    match b {
        TypeBody::Enum(_) => false,
        TypeBody::Struct(fields) => fields.iter().any(|(_, ty, _)| type_expr_contains_match(ty)),
        TypeBody::Alias(ty) => type_expr_contains_match(ty),
        TypeBody::Entity { fields, .. } => fields
            .iter()
            .any(|f| type_expr_contains_match(&f.type_expr)),
        TypeBody::Trait { linked_entity, .. } => {
            linked_entity.as_ref().is_some_and(type_expr_contains_match)
        }
        TypeBody::Page { trait_types, .. } => trait_types.iter().any(type_expr_contains_match),
        TypeBody::Orbital {
            entity,
            trait_types,
            page_type,
        } => {
            entity.as_ref().is_some_and(type_expr_contains_match)
                || trait_types.iter().any(type_expr_contains_match)
                || page_type.as_ref().is_some_and(type_expr_contains_match)
        }
        TypeBody::Event { payload, .. } => type_expr_contains_match(payload),
        TypeBody::Composition(parts) => parts.iter().any(type_body_contains_match),
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
    )
}

pub(crate) fn is_event_name(name: &str) -> bool {
    // Event names may be UPPER_SNAKE (e.g. INIT, ADD_ITEM) or MixedCase/PascalCase
    // (e.g. BrowseItemLoaded, ConfirmActionLoadFailed). First char must be an
    // ASCII uppercase letter, OR a non-ASCII alphabetic char (caseless
    // scripts, e.g. Arabic, have no case to distinguish); remaining chars
    // are letters, digits, or underscores.
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first.is_ascii_uppercase() || (!first.is_ascii() && first.is_alphabetic())) {
        return false;
    }
    chars.all(|c| c.is_alphanumeric() || c == '_')
}

pub(crate) fn is_event_name_at_peek(kind: &TokenKind, lang: &Lang) -> bool {
    match kind {
        TokenKind::Identifier(name) => is_event_name(name),
        TokenKind::Sigil(name) => config_ref_knob(&canon_sigil_root_for(name, lang)).is_some(),
        _ => false,
    }
}

/// `@config.<knob>` is the only legal reference form for an emit/listen event
/// name (Option B: the declared event name resolves to a config knob's
/// value). Returns the knob identifier when `name` (the lexer's dotted-sigil
/// body, e.g. `config.action`, already canonicalized via
/// `canon_sigil_root_for`) matches `config.<ident>`.
pub(crate) fn config_ref_knob(name: &str) -> Option<&str> {
    let knob = name.strip_prefix("config.")?;
    if knob.is_empty() || knob.contains('.') {
        return None;
    }
    Some(knob)
}

/// Canonicalize a `@root` / `@root.rest` sigil token's ROOT via the
/// `sigils` vocabulary section, leaving everything after the first `.`
/// (a user field/knob name) untouched. Free function (not a `Parser`
/// method) so call sites without a `Parser` in scope (`is_event_name_at_peek`)
/// can use it too; `Parser::canon_sigil_root` delegates here.
pub(crate) fn canon_sigil_root_for(raw: &str, lang: &Lang) -> String {
    match raw.split_once('.') {
        Some((root, rest)) => match lang.canon_sigil(root) {
            Some(en) => format!("{en}.{rest}"),
            None => raw.to_string(),
        },
        None => lang
            .canon_sigil(raw)
            .map(str::to_string)
            .unwrap_or_else(|| raw.to_string()),
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
        assert_eq!(prog.app.theme, None);
    }

    #[test]
    fn parse_app_header_theme_after_description() {
        let prog = p("app MyApp [version: \"1.0.0\", description: \"A themed app\", theme: \"linear-clean-light\"]\norbital Foo {\n  entity Bar {\n    id : string!\n  }\n}");
        assert_eq!(prog.app.description, Some("A themed app".to_string()));
        assert_eq!(
            prog.app.theme,
            Some(ThemeDecl::Reference("linear-clean-light".to_string()))
        );
        assert_eq!(prog.orbitals.len(), 1);
    }

    #[test]
    fn parse_app_header_theme_without_description() {
        let prog = p("app MyApp [version: \"1.0.0\", theme: \"terminal-dark\"]\norbital Foo {\n  entity Bar {\n    id : string!\n  }\n}");
        assert_eq!(prog.app.description, None);
        assert_eq!(
            prog.app.theme,
            Some(ThemeDecl::Reference("terminal-dark".to_string()))
        );
        assert_eq!(prog.orbitals.len(), 1);
    }

    #[test]
    fn app_theme_does_not_capture_orbital_theme() {
        let prog = p("app MyApp [version: \"1.0.0\"]\norbital Foo {\n  entity Bar {\n    id : string!\n  }\n  theme \"clay-light\"\n}");
        assert_eq!(prog.app.theme, None);
        assert_eq!(
            prog.orbitals[0].theme,
            Some(ThemeDecl::Reference("clay-light".to_string()))
        );
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

    const TYPED_AXES_THEME: &str = "orbital Foo {\n  entity Bar {\n    id : string!\n  }\n  theme {\n    name: \"deco\"\n    tokens {\n      typeScale {\n        displayFamily: \"Limelight, serif\"\n        headingTransform: \"uppercase\"\n      }\n      geometry {\n        cornerShape: \"bevel\"\n        borderStyle: \"double\"\n      }\n      elevation {\n        pressedElevation: \"inset 1px 1px 0 #000\"\n      }\n      surface {\n        pageImage: \"linear-gradient(#000, #111)\"\n      }\n    }\n    variants {\n      dark {\n        surface {\n          backdrop: \"blur(4px)\"\n        }\n      }\n    }\n  }\n}";

    #[test]
    fn parse_inline_theme_typed_axes() {
        let prog = p(TYPED_AXES_THEME);
        let Some(ThemeDecl::Inline(inline)) = prog.orbitals[0].theme.clone() else {
            panic!("expected inline theme");
        };
        assert_eq!(
            inline.tokens.type_scale,
            vec![
                ("displayFamily".to_string(), "Limelight, serif".to_string()),
                ("headingTransform".to_string(), "uppercase".to_string()),
            ]
        );
        assert_eq!(inline.tokens.geometry.len(), 2);
        assert_eq!(inline.tokens.elevation.len(), 1);
        assert_eq!(inline.tokens.surface.len(), 1);
        assert!(
            inline.tokens.colors.is_empty(),
            "control: legacy maps untouched"
        );
        assert_eq!(
            inline.variants[0].1.surface,
            vec![("backdrop".to_string(), "blur(4px)".to_string())]
        );
    }

    #[test]
    fn parse_inline_theme_typed_axis_rejects_unknown_key() {
        let src = TYPED_AXES_THEME.replace("cornerShape", "cornerShap");
        let err = parse_str(&src).expect_err("unknown geometry key must fail");
        let msg = format!("{err}");
        assert!(
            msg.contains("cornerShap") && msg.contains("cornerShape"),
            "got: {msg}"
        );
    }

    #[test]
    fn parse_inline_theme_typed_axis_rejects_nested_scale_key() {
        let src = TYPED_AXES_THEME.replace("headingTransform", "scale");
        assert!(
            parse_str(&src).is_err(),
            "nested typeScale fields stay .orb-only"
        );
    }

    #[test]
    fn parse_inline_theme_legacy_typography_accepts_any_key() {
        let src = TYPED_AXES_THEME.replace("typeScale {", "typography {");
        assert!(
            parse_str(&src).is_ok(),
            "control: typography stays free-form"
        );
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
        let src = "orbital O {\n  entity E { id : string! }\n  trait T -> E {\n    state s {\n      EVT -> s\n        (set @x 1)\n    }\n  }\n}";
        let prog = p(src);
        let t = &prog.orbitals[0].traits[0];
        assert_eq!(t.name, "T");
        assert_eq!(t.linked_entity, Some("E".to_string()));
        assert_eq!(t.dispatch, None);
        assert!(t.retired_words.is_empty());
        assert_eq!(t.states.len(), 1);
        assert_eq!(t.states[0].transitions.len(), 1);
    }

    #[test]
    fn parse_trait_dispatch_words() {
        for (word, mode) in [
            ("client", TraitDispatch::Client),
            ("optimistic", TraitDispatch::Optimistic),
            ("server", TraitDispatch::Server),
        ] {
            let src = format!("orbital O {{\n  entity E {{ id : string! }}\n  trait T -> E [{word}] {{\n    state s {{\n      EVT -> s\n        (set @x 1)\n    }}\n  }}\n}}");
            let prog = p(&src);
            let t = &prog.orbitals[0].traits[0];
            assert_eq!(t.dispatch, Some(mode));
            assert!(t.retired_words.is_empty());
        }
    }

    #[test]
    fn parse_trait_keeps_non_dispatch_header_words_for_lowering() {
        let src = "orbital O {\n  entity E { id : string! }\n  trait T -> E [interaction, instance, local] {\n    state s {\n      EVT -> s\n        (set @x 1)\n    }\n  }\n}";
        let prog = p(src);
        let t = &prog.orbitals[0].traits[0];
        assert_eq!(t.dispatch, None);
        let words: Vec<&str> = t.retired_words.iter().map(|w| w.word.as_str()).collect();
        assert_eq!(words, ["interaction", "instance", "local"]);
    }

    #[test]
    fn parse_trait_refuses_two_dispatch_modes() {
        let src = "orbital O {\n  entity E { id : string! }\n  trait T -> E [server, client] {\n    state s { EVT -> s }\n  }\n}";
        assert!(parse_str(src).is_err());
        let src = "orbital O {\n  entity E { id : string! }\n  trait T -> E [client, optimistic] {\n    state s { EVT -> s }\n  }\n}";
        assert!(parse_str(src).is_err());
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
    fn a_bracket_headed_by_an_operator_basename_with_a_nested_call_is_marked_data() {
        let src = "orbital O {\n  entity E [runtime] {\n    id : string!\n  }\n  trait T -> E {\n    initial: idle\n    state idle {\n      GO -> idle\n        (set @entity.id (json/stringify [\"delete\" \"X\" (object/get @payload \"id\")]))\n        (set @entity.id (json/stringify [\"date\" \"datetime\"]))\n        (set @entity.id (json/stringify [\"email\" \"sms\" \"push\"]))\n    }\n    emits { }\n    listens { GO { id : string! } }\n  }\n}\n";
        let json = serde_json::to_string(&crate::parse_and_lower_str(src).expect("lowers")).unwrap();
        assert!(json.contains(r#"["list","delete","X",["object/get""#), "a dropped-namespace look-alike holding a call is marked: {json}");
        assert!(json.contains(r#"["date","datetime"]"#), "control: constants headed by a basename stay as written");
        assert!(json.contains(r#"["email","sms","push"]"#), "control: a plain enum list stays as written");
    }

    #[test]
    fn a_question_mark_after_a_type_makes_it_nullable() {
        let src = "type Seen = { lastSeen : number?, tags : [string]? , name : string }\norbital O {\n  entity E { id : string!\n    due : number? }\n}";
        let prog = p(src);
        let TypeBody::Struct(fields) = &prog.type_aliases[0].body else {
            panic!("expected a struct");
        };
        assert!(
            matches!(&fields[0].1, TypeExpr::Optional(inner) if matches!(**inner, TypeExpr::Primitive(ref n) if n == "number"))
        );
        assert!(
            matches!(&fields[1].1, TypeExpr::Optional(inner) if matches!(**inner, TypeExpr::Array(_)))
        );
        assert!(
            matches!(&fields[2].1, TypeExpr::Primitive(n) if n == "string"),
            "control: no question mark, not nullable"
        );
        let entity = prog.orbitals[0].entity.as_ref().unwrap();
        assert!(entity.fields[1].nullable);
        assert!(
            !entity.fields[0].nullable,
            "control: a required field is not nullable"
        );
    }

    #[test]
    fn a_payload_sigil_after_whitespace_is_not_a_nullable_marker() {
        let src = "orbital O {\n  entity E { id : string! }\n  trait T -> E [client] {\n    initial: idle\n    state idle {\n      GO -> idle\n        (emit DONE { value: ? })\n    }\n  }\n}";
        let prog = p(src);
        assert_eq!(prog.orbitals.len(), 1);
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
    fn g_orb_444_alias_named_after_a_builtin_type_word_is_refused() {
        for word in ["SExpr", "Map", "EventAddress", "secret"] {
            let src = format!("type {word} = {{ id : string! }}\norbital O {{\n  entity E2 {{ id : string! }}\n}}");
            let err = parse_str(&src).expect_err("a reserved type word cannot name an alias");
            assert!(err.to_string().contains("ORB_T_RESERVED_TYPE_NAME"), "{word}: {err:?}");
        }
        parse_str("type Shape = { id : string! }\norbital O {\n  entity E2 {\n    id : string!\n  }\n}").expect("control: an ordinary alias name parses");
    }

    #[test]
    fn parse_derived_field() {
        let src = "orbital O {\n  entity E [runtime] {\n    id : string!\n    count : int @derived (array/length @items)\n  }\n}";
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
    fn parse_page_protocol_word_with_address() {
        let src = "orbital O {\n  entity E { id : string! }\n  trait T -> E { state s { I -> s } }\n  page http { host: @config.host, port: @config.port, path: \"/\" } as Api -> T [access: authenticated]\n}";
        let prog = p(src);
        let page = &prog.orbitals[0].pages[0];
        assert_eq!(page.protocol.as_deref(), Some("http"));
        assert_eq!(page.path, "");
        assert_eq!(page.name.as_deref(), Some("Api"));
        assert_eq!(page.traits, vec!["T"]);
        let Some(SExpression::Object(address)) = &page.address else {
            panic!("expected a record address, got {:?}", page.address);
        };
        let keys: Vec<&str> = address.keys().map(|k| k.as_str()).collect();
        assert_eq!(keys, ["host", "port", "path"]);
        assert_eq!(address["host"], SExpression::String(("@config.host".to_string()).into()));
    }

    #[test]
    fn parse_page_protocol_word_without_address() {
        for word in ["stdio", "boot"] {
            let src = format!("orbital O {{\n  entity E {{ id : string! }}\n  trait T -> E {{ state s {{ I -> s }} }}\n  page {word} as P -> T\n}}");
            let prog = p(&src);
            let page = &prog.orbitals[0].pages[0];
            assert_eq!(page.protocol.as_deref(), Some(word));
            assert!(page.address.is_none());
        }
    }

    #[test]
    fn parse_page_reference_form_carries_the_address_override() {
        let src = "orbital O {\n  entity E { id : string! }\n  page http { host: \"0.0.0.0\", port: 8080, path: \"/\" } = Server.pages.Api\n}";
        let prog = p(src);
        let page = &prog.orbitals[0].pages[0];
        assert_eq!(page.protocol.as_deref(), Some("http"));
        assert_eq!(page.reference.as_deref(), Some("Server.pages.Api"));
        assert!(page.address.is_some());
    }

    #[test]
    fn parse_page_route_has_no_protocol() {
        let prog = p("orbital O {\n  entity E { id : string! }\n  trait T -> E { state s { I -> s } }\n  page \"/foo\" -> T\n}");
        let page = &prog.orbitals[0].pages[0];
        assert!(page.protocol.is_none());
        assert!(page.address.is_none());
        assert_eq!(page.path, "/foo");
    }

    #[test]
    fn parse_entity_browser_tag_and_retired_tags() {
        let prog = p("orbital O {\n  entity E [persistent: rows, browser] { id : string! }\n}");
        let e = prog.orbitals[0].entity.as_ref().unwrap();
        assert!(e.browser && e.retired_words.is_empty());
        let prog = p("orbital O {\n  entity E [persistent: rows, local, mock] { id : string! }\n}");
        let e = prog.orbitals[0].entity.as_ref().unwrap();
        assert!(!e.browser);
        let words: Vec<&str> = e.retired_words.iter().map(|w| w.word.as_str()).collect();
        assert_eq!(words, ["local", "mock"]);
    }

    #[test]
    fn parse_listen_input_and_retired_external() {
        let src = |word: &str| format!("orbital O {{\n  entity E {{ id : string! }}\n  trait T -> E {{\n    state s {{ GO -> s }}\n    listens {{\n      GO -> {word} {{ id: string! }}\n    }}\n  }}\n}}");
        let prog = p(&src("input"));
        let l = &prog.orbitals[0].traits[0].listens[0];
        assert!(l.input && l.retired_external.is_none());
        let prog = p(&src("external"));
        let l = &prog.orbitals[0].traits[0].listens[0];
        assert_eq!(l.retired_external.as_ref().map(|w| w.word.as_str()), Some("external"));
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

    fn config_dup_src(a: &str, b: &str) -> String {
        format!("orbital O {{\n  uses M from \"std/behaviors/x\"\n  entity E {{ id : string! }}\n  trait A = M.traits.T -> E {{\n    config {{\n{a}\n    }}\n  }}\n  trait B = M.traits.T -> E {{\n    config {{\n{b}\n    }}\n  }}\n}}")
    }

    #[test]
    fn duplicate_call_site_config_key_errors() {
        let src = config_dup_src(
            "      viewerRole: @user.role\n      viewerRole: @user.role",
            "      x: 1",
        );
        let err = parse_str(&src).expect_err("duplicate key must fail");
        let msg = err.to_string();
        assert!(
            msg.contains("ELOLO_DUPLICATE_CONFIG_KEY"),
            "unexpected: {msg}"
        );
        assert!(msg.contains("viewerRole"), "must name the key: {msg}");
    }

    #[test]
    fn duplicate_declared_config_key_errors() {
        let src = "orbital O {\n  entity E { id : string! }\n  trait T -> E {\n    state s { I -> s }\n    config {\n      k : string = \"a\"\n      k : string = \"b\"\n    }\n  }\n}";
        let err = parse_str(src).expect_err("duplicate key must fail");
        assert!(
            err.to_string().contains("ELOLO_DUPLICATE_CONFIG_KEY"),
            "unexpected: {err}"
        );
    }

    #[test]
    fn distinct_config_keys_pass() {
        let src = config_dup_src("      a: 1\n      b: 2", "      c: 3");
        parse_str(&src).expect("distinct keys must parse");
    }

    #[test]
    fn same_config_key_in_different_blocks_passes() {
        let src = config_dup_src(
            "      viewerRole: @user.role",
            "      viewerRole: @user.role",
        );
        parse_str(&src).expect("the same key in two traits must parse");
    }

    fn config_override_default(entry: &str) -> SExpression {
        let src = config_dup_src(entry, "      x: 1");
        let prog = parse_str(&src).unwrap_or_else(|e| panic!("parse failed: {e}"));
        let cf = &prog.orbitals[0].traits[0]
            .reference
            .as_ref()
            .expect("trait ref")
            .config[0];
        assert!(
            matches!(&cf.type_expr, TypeExpr::Primitive(s) if s == "unknown"),
            "a nested `=` must not turn wiring into a declaration: {:?}",
            cf.type_expr
        );
        cf.default.clone().expect("wiring value")
    }

    #[test]
    fn call_site_config_value_takes_a_nested_paren_equals() {
        let paren =
            config_override_default("      allowEdit: (if (= @user.role \"reader\") false true)");
        assert_eq!(
            paren,
            SExpression::Array(vec![
                SExpression::string("if"),
                SExpression::Array(vec![
                    SExpression::string("="),
                    SExpression::string("@user.role"),
                    SExpression::string("reader"),
                ].into()),
                SExpression::Bool(false),
                SExpression::Bool(true),
            ].into())
        );
    }

    /// G-ORB-407: a bracket is data, so an operator-headed bracket is a `list`, never the call.
    #[test]
    fn call_site_config_value_bracket_is_a_data_list() {
        let paren = config_override_default(
            "      show: (and (= @user.role \"reader\") (not @entity.locked))",
        );
        let bracket = config_override_default(
            "      show: [\"and\", [\"=\", @user.role, \"reader\"], [\"not\", @entity.locked]]",
        );
        assert_ne!(paren, bracket);
        assert!(
            matches!(&bracket, SExpression::Array(items) if items.first() == Some(&SExpression::string("list"))),
            "{bracket:?}"
        );
    }

    #[test]
    fn call_site_config_declaration_form_still_parses() {
        let src = config_dup_src("      k : string = \"a\"", "      x: 1");
        let prog = parse_str(&src).unwrap_or_else(|e| panic!("parse failed: {e}"));
        let cf = &prog.orbitals[0].traits[0]
            .reference
            .as_ref()
            .expect("trait ref")
            .config[0];
        assert!(matches!(&cf.type_expr, TypeExpr::Primitive(s) if s == "string"));
    }

    #[test]
    fn parse_expects_all_forms() {
        let src = "orbital O {\n  expects entity Customer [identity] {\n    role : \"store-manager\" | \"operations-director\"\n  }\n  expects entity OrderRecord {\n    status : \"placed\" | \"shipped\" | \"delivered\"\n    totalAmount : number\n  }\n  expects entity CartItem\n  expects trait Checkout { emits { ORDER_PLACED } }\n  entity E { id : string! }\n}";
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
            ExpectsKind::Entity { name, shape, .. } => {
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
            ExpectsKind::Entity { name, shape, .. } => {
                assert_eq!(name, "CartItem");
                assert!(shape.is_none());
            }
            other => panic!("expected entity, got {other:?}"),
        }
        match &expects[3].kind {
            ExpectsKind::Event {
                trait_name, event, ..
            } => {
                assert_eq!(trait_name, "Checkout");
                assert_eq!(event, "ORDER_PLACED");
            }
            other => panic!("expected event, got {other:?}"),
        }
    }

    #[test]
    fn parse_expects_bare_identity() {
        let prog = p("orbital O {\n  expects entity [identity]\n  entity E { id : string! }\n}");
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
            p("orbital O {\n  expects entity Customer [identity]\n  entity E { id : string! }\n}");
        match &prog.orbitals[0].expects[0].kind {
            ExpectsKind::Identity { name, shape } => {
                assert_eq!(name.as_deref(), Some("Customer"));
                assert!(shape.is_none());
            }
            other => panic!("expected identity, got {other:?}"),
        }
    }

    #[test]
    fn expects_entity_with_the_identity_tag_is_an_identity_expectation() {
        let prog = p("orbital O {\n  expects entity Customer [identity] {\n    role : \"admin\" | \"buyer\"\n  }\n  expects entity Cart\n  entity E { id : string! }\n}");
        let expects = &prog.orbitals[0].expects;
        assert!(
            matches!(&expects[0].kind, ExpectsKind::Identity { name, shape }
            if name.as_deref() == Some("Customer") && shape.as_ref().map(Vec::len) == Some(1))
        );
        assert!(matches!(&expects[1].kind, ExpectsKind::Entity { name, .. } if name == "Cart"));
    }

    #[test]
    fn expects_entity_identity_tag_may_omit_the_name() {
        let prog = p("orbital O {\n  expects entity [identity]\n  entity E { id : string! }\n}");
        assert!(matches!(
            &prog.orbitals[0].expects[0].kind,
            ExpectsKind::Identity {
                name: None,
                shape: None
            }
        ));
    }

    #[test]
    fn expects_entity_without_a_name_or_identity_tag_errors() {
        assert!(
            parse_str("orbital O {\n  expects entity\n  entity E { id : string! }\n}").is_err()
        );
    }

    #[test]
    fn expects_trait_emits_lowers_to_one_event_expectation_per_emitted_event() {
        let prog = p("orbital O {\n  expects trait Checkout {\n    emits {\n      ORDER_PLACED\n      ORDER_CANCELLED\n    }\n  }\n  entity E { id : string! }\n}");
        let expects = &prog.orbitals[0].expects;
        assert_eq!(expects.len(), 2);
        assert!(
            matches!(&expects[0].kind, ExpectsKind::Event { trait_name, event, .. }
            if trait_name == "Checkout" && event == "ORDER_PLACED")
        );
        assert!(
            matches!(&expects[1].kind, ExpectsKind::Event { event, .. } if event == "ORDER_CANCELLED")
        );
    }

    #[test]
    fn expects_trait_duplicate_event_errors() {
        let src = "orbital O {\n  expects trait T { emits { A } }\n  expects trait T { emits { A } }\n  entity E { id : string! }\n}";
        assert!(parse_str(src)
            .expect_err("duplicate")
            .to_string()
            .contains("ELOLO_DUPLICATE_EXPECTATION"));
    }

    #[test]
    fn expects_trait_emit_carries_its_payload_fields() {
        let prog = p("orbital O {\n  expects trait T { emits { A { id : string!, note : string } } }\n  entity E { id : string! }\n}");
        match &prog.orbitals[0].expects[0].kind {
            ExpectsKind::Event { event, payload, .. } => {
                assert_eq!(event, "A");
                assert_eq!(payload.len(), 2);
                assert!(payload[0].required && !payload[1].required);
            }
            other => panic!("expected event, got {other:?}"),
        }
    }

    #[test]
    fn expects_trait_emit_refuses_a_scope_or_annotation() {
        let scope = "orbital O {\n  expects trait T { emits { A -> external } }\n  entity E { id : string! }\n}";
        assert!(parse_str(scope).is_err());
        let note = "orbital O {\n  expects trait T { emits {\n    A\n      @description \"x\"\n  } }\n  entity E { id : string! }\n}";
        assert!(parse_str(note).is_err());
    }

    #[test]
    fn expects_trait_without_emits_errors() {
        assert!(parse_str(
            "orbital O {\n  expects trait T { emits { } }\n  entity E { id : string! }\n}"
        )
        .is_err());
    }

    #[test]
    fn the_retired_expects_identity_and_event_spellings_name_their_replacement() {
        let id =
            parse_str("orbital O {\n  expects identity Customer\n  entity E { id : string! }\n}")
                .expect_err("retired");
        assert!(
            id.to_string()
                .contains("expects entity Customer [identity]"),
            "{id}"
        );
        let ev = parse_str(
            "orbital O {\n  expects event Checkout.ORDER_PLACED\n  entity E { id : string! }\n}",
        )
        .expect_err("retired");
        assert!(ev.to_string().contains("expects trait Checkout"), "{ev}");
    }

    #[test]
    fn expects_entity_states_its_storage_marks_and_access_in_the_real_syntax() {
        let src = "orbital O {\n  expects entity Order [persistent, shared] {\n    @read (= @entity.id \"x\")\n    @update none \"joins another entity\"\n    id : string! @key\n    email : string @unique\n  }\n  entity E { id : string! }\n}";
        let prog = p(src);
        match &prog.orbitals[0].expects[0].kind {
            ExpectsKind::Entity {
                name,
                shape,
                persistence,
                shared,
                browser,
                access,
                ..
            } => {
                assert_eq!(name, "Order");
                assert_eq!(*persistence, Some(Persistence::Persistent));
                assert!(*shared && !*browser);
                assert!(access.read_policy.is_some());
                assert_eq!(
                    access.access_waivers,
                    vec![("update".to_string(), "joins another entity".to_string())]
                );
                assert_eq!(shape.as_ref().map(Vec::len), Some(2));
            }
            other => panic!("expected an entity, got {other:?}"),
        }
    }

    #[test]
    fn expects_entity_may_state_access_alone() {
        let prog = p("orbital O {\n  expects entity Order {\n    @read none \"public\"\n  }\n  entity E { id : string! }\n}");
        assert!(matches!(&prog.orbitals[0].expects[0].kind,
            ExpectsKind::Entity { shape: None, access, .. } if access.has_any()));
    }

    #[test]
    fn expects_entity_refuses_a_collection_name_mock_and_a_duplicate_directive() {
        for bad in [
            "expects entity Order [persistent: orders]",
            "expects entity Order [mock]",
            "expects entity Order [local]",
            "expects entity Order {\n    @read none \"a\"\n    @read none \"b\"\n  }",
            "expects entity C [identity, persistent]",
        ] {
            let src = format!("orbital O {{\n  {bad}\n  entity E {{ id : string! }}\n}}");
            assert!(parse_str(&src).is_err(), "{bad}");
        }
    }

    #[test]
    fn expects_page_names_the_traits_it_mounts() {
        let prog = p("orbital O {\n  expects page \"/orders\" -> Checkout, Receipt\n  expects page \"/plain\"\n  entity E { id : string! }\n}");
        assert!(matches!(&prog.orbitals[0].expects[0].kind,
            ExpectsKind::Page { path, traits, protocol: None } if path == "/orders" && traits == &vec!["Checkout".to_string(), "Receipt".to_string()]));
        assert!(
            matches!(&prog.orbitals[0].expects[1].kind,
            ExpectsKind::Page { traits, .. } if traits.is_empty()),
            "control: a bare page names none"
        );
    }

    #[test]
    fn expects_page_protocol_word_names_an_endpoint() {
        let prog = p("orbital O {\n  expects page boot -> Seed, Tick\n  expects page stdio\n  entity E { id : string! }\n}");
        assert!(matches!(&prog.orbitals[0].expects[0].kind,
            ExpectsKind::Page { path, protocol: Some(w), traits } if path.is_empty() && w == "boot" && traits == &vec!["Seed".to_string(), "Tick".to_string()]));
        assert!(matches!(&prog.orbitals[0].expects[1].kind,
            ExpectsKind::Page { protocol: Some(w), traits, .. } if w == "stdio" && traits.is_empty()));
    }

    #[test]
    fn expects_trait_states_and_edges_use_the_real_state_syntax() {
        let src = "orbital O {\n  expects trait Checkout {\n    state idle { PAY -> paid }\n    state refunded\n    emits { PLACED }\n  }\n  entity E { id : string! }\n}";
        let prog = p(src);
        let k = &prog.orbitals[0].expects;
        assert_eq!(k.len(), 3);
        assert!(
            matches!(&k[0].kind, ExpectsKind::Transition { trait_name, from, event, to, .. }
            if trait_name == "Checkout" && from == "idle" && event == "PAY" && to == "paid")
        );
        assert!(matches!(&k[1].kind, ExpectsKind::State { state, .. } if state == "refunded"));
        assert!(matches!(&k[2].kind, ExpectsKind::Event { event, .. } if event == "PLACED"));
    }

    #[test]
    fn expects_trait_edges_refuse_guards_effects_and_time_bounds() {
        for bad in [
            "state idle { PAY -> paid when (> 1 0) }",
            "state idle { PAY -> paid (set @entity.x 1) }",
            "state idle for 5s",
        ] {
            let src = format!(
                "orbital O {{\n  expects trait T {{ {bad} }}\n  entity E {{ id : string! }}\n}}"
            );
            assert!(parse_str(&src).is_err(), "{bad}");
        }
    }

    #[test]
    fn expects_trait_duplicate_edge_errors() {
        let src = "orbital O {\n  expects trait T { state a { E -> b } }\n  expects trait T { state a { E -> b } }\n  entity E { id : string! }\n}";
        assert!(parse_str(src)
            .expect_err("duplicate")
            .to_string()
            .contains("ELOLO_DUPLICATE_EXPECTATION"));
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
        let src = "orbital O {\n  expects trait Checkout { emits { ORDER_PLACED } }\n  expects trait Checkout { emits { ORDER_PLACED } }\n  entity E { id : string! }\n}";
        let err = parse_str(src).expect_err("duplicate must fail");
        assert!(
            err.to_string().contains("ELOLO_DUPLICATE_EXPECTATION"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn parse_expects_duplicate_bare_identity_errors() {
        let src =
            "orbital O {\n  expects entity [identity]\n  expects entity [identity]\n  entity E { id : string! }\n}";
        let err = parse_str(src).expect_err("duplicate must fail");
        assert!(
            err.to_string().contains("ELOLO_DUPLICATE_EXPECTATION"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn parse_expects_same_name_different_kind_is_not_duplicate() {
        let src = "orbital O {\n  expects entity Customer [identity]\n  expects entity Customer\n  entity E { id : string! }\n}";
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

/// A behavior-value literal (`<spec>.traits.<T>` or `<spec>.orbitals.<O>`) as its
/// wire object; `None` when the run is not one.
fn behavior_value_object(raw: &str) -> Option<SExpression> {
    use orbital_core_base::schema::behavior_value::{OrbitalValueRef, TraitValueRef};
    if let Some(t) = TraitValueRef::parse(raw) {
        return Some(SExpression::object(
            orbital_core_base::schema::SObject::from([
                ("behavior".into(), SExpression::string(t.behavior)),
                ("trait".into(), SExpression::string(t.trait_name)),
            ]),
        ));
    }
    let o = OrbitalValueRef::parse(raw)?;
    Some(SExpression::object(
        orbital_core_base::schema::SObject::from([
            ("behavior".into(), SExpression::string(o.behavior)),
            ("orbital".into(), SExpression::string(o.orbital)),
        ]),
    ))
}
