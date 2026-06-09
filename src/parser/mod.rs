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
mod page;
mod r#trait;
mod r#type;

pub(crate) use expression::lower_self_sigil;

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
            // Refinement primitives: a string + a validity rule. `icon` = a valid
            // Lucide name; `component` = a valid element tag (or a @trait ref).
            // Both lower to `string`; the value rule is enforced where the value
            // is used (render-ui icon / prop validation).
            | "icon"
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
        TypeExpr::Slot => Some("slot"),
        TypeExpr::Pattern => Some("pattern"),
        TypeExpr::SExpr => Some("SExpr"),
        TypeExpr::Array(inner) => type_expr_config_only_type(inner),
        TypeExpr::Map(key, value) => {
            type_expr_config_only_type(key).or_else(|| type_expr_config_only_type(value))
        }
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
    let Some(first) = chars.next() else { return false };
    if !first.is_ascii_uppercase() {
        return false;
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

pub(crate) fn is_event_name_at_peek(kind: &TokenKind) -> bool {
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
