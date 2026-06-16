//! orbital-lolo
//!
//! Lexer, parser, and lowering for the `.lolo` surface language.
//!
//! `.lolo` is a Lisp-flavoured surface notation for `.orb` JSON. This crate
//! lexes and parses `.lolo` source into an internal AST and lowers that AST
//! into an `OrbitalSchema` from `orbital-core`. The result feeds into the
//! existing `orbital-compiler` pipeline (Parse -> Validate -> Enrich -> Inline
//! -> Compose -> Resolve -> codegen) unchanged.
//!
//! See `docs/Lolo_Language_Spec_v3.md` for the surface language and
//! `docs/Lolo_Language_Spec_v3_PLAN.md` Section 6 for this crate's scope.

#![allow(clippy::too_many_arguments, clippy::large_enum_variant)]

pub mod ast;
pub mod error;
pub mod format;
pub mod lexer;
pub mod lower;
pub mod parser;
pub mod patterns_meta;
pub mod registry;
pub mod types;

pub use ast::{Ast, Comment, EntityField, EventField, Span, TypeExpr};
pub use error::{LexError, LoloError, LowerError, ParseError, TypeError};
pub use format::format_program;
pub use lexer::{tokenize, tokenize_with_comments, Token, TokenKind};
pub use lower::lower_to_orb;
pub use parser::{parse, parse_str};

use orbital_core::schema::OrbitalSchema;
use std::path::Path;

/// Convenience: read a `.lolo` file from disk, parse it, and lower it to an
/// `OrbitalSchema`. Errors are tagged with the source path so the CLI can
/// report `file:line:col` precision.
pub fn parse_and_lower(path: &Path) -> Result<OrbitalSchema, LoloError> {
    let source = std::fs::read_to_string(path).map_err(|e| LoloError::Io {
        path: path.display().to_string(),
        message: e.to_string(),
    })?;
    let ast = parse_str(&source).map_err(|e| LoloError::Parse {
        path: path.display().to_string(),
        inner: e,
    })?;
    let schema = lower_to_orb(&ast).map_err(|e| LoloError::Lower {
        path: path.display().to_string(),
        inner: e,
    })?;
    Ok(schema)
}

/// Parse and lower a `.lolo` source string. Used by tests and the
/// `--emit-orb` CLI flag when feeding from stdin.
pub fn parse_and_lower_str(source: &str) -> Result<OrbitalSchema, LoloError> {
    let ast = parse_str(source).map_err(|e| LoloError::Parse {
        path: "<input>".to_string(),
        inner: e,
    })?;
    let schema = lower_to_orb(&ast).map_err(|e| LoloError::Lower {
        path: "<input>".to_string(),
        inner: e,
    })?;
    Ok(schema)
}
