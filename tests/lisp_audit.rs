//! Lisp-surface audit regression (Phase 0.A of Almadar_Compiler_Runtime_Gaps_PLAN.md).
//!
//! The 11 self-contained `.lolo` fixtures in `fixtures/lisp-audit/` were built to
//! stress the S-expression surface (deep `let` nesting, symbolic math, control
//! flow, namespaced ops, long state machines, guard truthiness, value
//! determinism). Each must: parse → format idempotently → re-parse → lower to an
//! `.orb` schema without error. This is the compiled-path parse/lower/format
//! twin of the `@almadar/runtime` + `@almadar/evaluator` JS-side twins.

use orbital_lolo::{format_program, parse_and_lower_str, parse_str};
use std::path::PathBuf;

fn fixtures_dir() -> PathBuf {
    let mut d = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    d.push("tests/fixtures/lisp-audit");
    d
}

/// Parse → format → re-parse → lower, asserting formatter idempotency and a
/// clean lower. Mirrors `formatter.rs::assert_round_trip` (which is not `pub`),
/// kept local to avoid coupling test binaries.
fn assert_audit_round_trip(name: &str) {
    let dir = fixtures_dir();
    let lolo_path = dir.join(format!("{name}.lolo"));
    let source = std::fs::read_to_string(&lolo_path)
        .unwrap_or_else(|e| panic!("read {lolo_path:?}: {e}"));

    let ast = parse_str(&source).unwrap_or_else(|e| panic!("parse {name}: {e:?}"));
    let formatted = format_program(&ast);

    // Formatter must be idempotent: parse(formatted) then format again == formatted.
    let ast2 =
        parse_str(&formatted).unwrap_or_else(|e| panic!("re-parse formatted {name}: {e:?}"));
    let formatted2 = format_program(&ast2);
    assert_eq!(
        formatted, formatted2,
        "{name}: format_program is not idempotent"
    );

    // Both the original and the formatted source must lower to a schema cleanly.
    let schema1 =
        parse_and_lower_str(&source).unwrap_or_else(|e| panic!("lower original {name}: {e:?}"));
    let schema2 = parse_and_lower_str(&formatted)
        .unwrap_or_else(|e| panic!("lower formatted {name}: {e:?}"));
    assert_eq!(
        schema1.orbitals.len(),
        schema2.orbitals.len(),
        "{name}: orbital count drifted between original and formatted source"
    );
}

const AUDIT_FIXTURES: &[&str] = &[
    "audit-bind-matrix",
    "audit-guard-raw-truthiness",
    "audit-guard-truthiness",
    "audit-long-state-machine",
    "audit-nest-ai",
    "audit-nest-deeplet",
    "audit-op-collection",
    "audit-op-control-flow",
    "audit-op-divergence-probes",
    "audit-op-symbolic-math",
    "audit-value-determinism",
];

#[test]
fn lisp_audit_fixtures_round_trip() {
    for name in AUDIT_FIXTURES {
        assert_audit_round_trip(name);
    }
}

/// One named test per fixture so a failure points straight at the culprit.
#[test]
fn lisp_audit_bind_matrix() {
    assert_audit_round_trip("audit-bind-matrix");
}
#[test]
fn lisp_audit_guard_raw_truthiness() {
    assert_audit_round_trip("audit-guard-raw-truthiness");
}
#[test]
fn lisp_audit_guard_truthiness() {
    assert_audit_round_trip("audit-guard-truthiness");
}
#[test]
fn lisp_audit_long_state_machine() {
    assert_audit_round_trip("audit-long-state-machine");
}
#[test]
fn lisp_audit_nest_ai() {
    assert_audit_round_trip("audit-nest-ai");
}
#[test]
fn lisp_audit_nest_deeplet() {
    assert_audit_round_trip("audit-nest-deeplet");
}
#[test]
fn lisp_audit_op_collection() {
    assert_audit_round_trip("audit-op-collection");
}
#[test]
fn lisp_audit_op_control_flow() {
    assert_audit_round_trip("audit-op-control-flow");
}
#[test]
fn lisp_audit_op_divergence_probes() {
    assert_audit_round_trip("audit-op-divergence-probes");
}
#[test]
fn lisp_audit_op_symbolic_math() {
    assert_audit_round_trip("audit-op-symbolic-math");
}
#[test]
fn lisp_audit_value_determinism() {
    assert_audit_round_trip("audit-value-determinism");
}
