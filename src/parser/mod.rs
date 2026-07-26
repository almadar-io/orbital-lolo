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
}

impl Parser {
    fn new(tokens: Vec<Token>) -> Self {
        Self {
            tokens,
            pos: 0,
            pending_inline_traits: Vec::new(),
            inline_ref_counter: 0,
            binding_scopes: Vec::new(),
        }
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
        let mut extra_entities = Vec::new();
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
            type_aliases,
            entity,
            extra_entities,
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
        TypeExpr::EntityRef => Some("entity"),
        TypeExpr::EventRef => Some("event"),
        TypeExpr::Slot => Some("slot"),
        TypeExpr::Pattern => Some("pattern"),
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
