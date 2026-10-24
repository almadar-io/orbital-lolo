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

// Directories under `src/` are organizational only: every module keeps its flat public path via `#[path]`.

// syntax — lex and the parser's own vocabulary
#[path = "syntax/ast.rs"]
pub mod ast;
#[path = "syntax/error.rs"]
pub mod error;
#[path = "syntax/keywords.rs"]
pub mod keywords;
#[path = "syntax/lexer.rs"]
pub mod lexer;

// the L1 stages: parse → lower; unlower goes back
pub mod lower;
pub mod parser;
pub mod unlower;

// check — alias and type resolution against the installed language
#[path = "check/patterns_meta.rs"]
pub mod patterns_meta;
#[path = "check/registry.rs"]
pub mod registry;
#[path = "check/types.rs"]
pub mod types;

// spans — side channels from IR back to source
#[path = "spans/embeds.rs"]
pub mod embeds;
#[path = "spans/source_map.rs"]
pub mod source_map;

// tooling — format, completion, the generated reference
#[path = "tooling/complete.rs"]
pub mod complete;
#[path = "tooling/format.rs"]
pub mod format;
#[path = "tooling/reference.rs"]
pub mod reference;

pub use ast::{Ast, Comment, EntityField, EventField, Span, TypeExpr};
pub use error::{LexError, LoloError, LowerError, ParseError, TypeError};
pub use format::{format_path, format_program};
pub use lexer::{tokenize, tokenize_with_comments, Token, TokenKind};
pub use lower::{
    lower_to_orb, lower_to_orb_with, lower_to_orb_with_map, lower_to_orb_with_map_with,
    lower_to_orb_with_prior, lower_to_orb_with_prior_and_map, lower_to_orb_with_prior_and_map_with,
    lower_to_orb_with_prior_with, stamp_schema_identity, LowerOptions,
};
pub use parser::{detect_language, parse, parse_str};
pub use source_map::LoloSourceMap;
pub use unlower::{schema_to_lolo, unlower, UnlowerError};

use orbital_core_base::schema::OrbitalSchema;
use std::path::Path;

/// Convenience: read a `.lolo` file from disk, parse it, and lower it to an
/// `OrbitalSchema`. Errors are tagged with the source path so the CLI can
/// report `file:line:col` precision.
pub fn parse_and_lower(path: &Path) -> Result<OrbitalSchema, LoloError> {
    let source = std::fs::read_to_string(path).map_err(|e| LoloError::Io {
        path: path.display().to_string(),
        message: e.to_string(),
    })?;
    parse_and_lower_str_at(&source, &path.display().to_string())
}

/// Parse and lower a `.lolo` source string. Used by tests and the
/// `--emit-orb` CLI flag when feeding from stdin.
pub fn parse_and_lower_str(source: &str) -> Result<OrbitalSchema, LoloError> {
    parse_and_lower_str_at(source, "<input>")
}

/// [`parse_and_lower_str`] against an explicit [`LowerOptions`] instead of
/// discovering installed behavior packages from the current directory — for
/// callers (tests, or a resolver that already has the package set) that don't
/// want filesystem discovery.
pub fn parse_and_lower_str_with(
    source: &str,
    options: &LowerOptions,
) -> Result<OrbitalSchema, LoloError> {
    let ast = traced_parse(source, "<input>")?;
    tracing::debug_span!(target: LOLO, "lower", file = "<input>")
        .in_scope(|| lower_to_orb_with(&ast, options))
        .map_err(|e| LoloError::Lower {
            path: "<input>".to_string(),
            inner: e,
        })
}

const LOLO: &str = orbital_core_base::logging::ns::LOLO;

fn traced_parse(source: &str, path: &str) -> Result<ast::Program, LoloError> {
    tracing::debug_span!(target: LOLO, "parse", file = path)
        .in_scope(|| parse_str(source))
        .map_err(|e| LoloError::Parse {
            path: path.to_string(),
            inner: e,
        })
}

/// [`parse_and_lower_str`] tagging errors with `path` instead of the generic
/// `"<input>"` placeholder — used when the real source file is known (CLI
/// commands reading from disk) so parse errors report `file:line:col`
/// instead of `<input>:line:col`.
pub fn parse_and_lower_str_at(source: &str, path: &str) -> Result<OrbitalSchema, LoloError> {
    parse_and_lower_str_with_prior_at(source, None, path)
}

/// Parse and lower a `.lolo` source string, reusing ids from `prior` (a
/// previously-emitted `.orb` schema for the same source) so re-emits do not
/// churn ids. See [`lower_to_orb_with_prior`].
pub fn parse_and_lower_str_with_prior(
    source: &str,
    prior: Option<&OrbitalSchema>,
) -> Result<OrbitalSchema, LoloError> {
    parse_and_lower_str_with_prior_at(source, prior, "<input>")
}

/// [`parse_and_lower_str_with_prior`] tagging errors with `path`.
pub fn parse_and_lower_str_with_prior_at(
    source: &str,
    prior: Option<&OrbitalSchema>,
    path: &str,
) -> Result<OrbitalSchema, LoloError> {
    let ast = traced_parse(source, path)?;
    let schema = tracing::debug_span!(target: LOLO, "lower", file = path)
        .in_scope(|| lower_to_orb_with_prior(&ast, prior))
        .map_err(|e| LoloError::Lower {
            path: path.to_string(),
            inner: e,
        })?;
    Ok(schema)
}

/// [`parse_and_lower_str_at`] additionally returning the L2 source map (see
/// [`source_map::LoloSourceMap`]) that maps a resolved-schema JSON-pointer
/// path back to the originating `.lolo` span.
pub fn parse_and_lower_str_with_map(
    source: &str,
    path: &str,
) -> Result<(OrbitalSchema, LoloSourceMap), LoloError> {
    let ast = traced_parse(source, path)?;
    let (schema, map) = tracing::debug_span!(target: LOLO, "lower", file = path)
        .in_scope(|| lower_to_orb_with_map(&ast))
        .map_err(|e| LoloError::Lower {
            path: path.to_string(),
            inner: e,
        })?;
    Ok((schema, map))
}
