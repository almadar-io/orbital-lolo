//! Phase 3c — flow-typing refinement for term-level `match` (pattern-
//! matching doc §3a+3c). Inside arm N of a destructuring term match
//! (`match @payload.item.type { "draw-shape" -> … }`), the scrutinee's OWNER
//! value (`@payload.item`) refines to the variant arm N selects, so
//! variant-member reads are resolved against that variant's declared fields
//! instead of the conservative union. L1 validation-time ONLY — zero `.orb`
//! change, zero L2 change, zero runtime change — and zero behavior change
//! for non-match code (a dotted read outside a match keeps today's
//! defer-to-L2 posture).
//!
//! Also carries the L2-tolerance proof: the lowered `.orb` of a
//! destructuring match — ordinary guarded transitions whose effects read
//! variant members of a union-typed field — passes the compiler's full
//! validation with 0 errors and 0 warnings (verified against the `orb
//! validate` CLI as well during development: L2 accepts both real and even
//! nonexistent member reads on a union-typed field, so L1 refines, L2
//! tolerates, the runtime is untyped — the same trust chain every other
//! L1-typed construct rides).

use orbital_compiler::validate_comprehensive_without_services;
use orbital_lolo::{parse_and_lower_str, LoloError};

/// The tagged-union fixture: `DrawItem = DrawShape | DrawText`,
/// discriminated by each variant's single-valued closed `type` member.
fn wrap_draw_trait(trait_body: &str) -> String {
    format!(
        r#"orbital DrawOrbital {{
  type DrawShape = {{ type : "draw-shape"!, width : number, height : number }}
  type DrawText = {{ type : "draw-text"!, text : string }}
  type DrawItem = DrawShape | DrawText

  entity DrawState [runtime] {{
    id : string!
    lastWidth : number = 0
    lastText : string = ""
  }}

  trait DrawDispatch -> DrawState {{
{trait_body}
  }}

  page "/draw" -> DrawDispatch
}}
"#
    )
}

fn lower_err(source: &str) -> String {
    match parse_and_lower_str(source) {
        Err(LoloError::Lower { inner, .. }) => inner.to_string(),
        Err(other) => panic!("expected a Lower error, got {other}"),
        Ok(_) => panic!("expected an error, lowering succeeded"),
    }
}

const HAPPY_BODY: &str = r#"
    listens {
      DISPATCH { item : DrawItem! }
    }
    state idle {
      INIT -> idle
        (set @entity.lastWidth 0)
        (set @entity.lastText "")
      DISPATCH -> idle match @payload.item.type {
        "draw-shape" -> (set @entity.lastWidth @payload.item.width)
        "draw-text"  -> (set @entity.lastText @payload.item.text)
      }
    }
"#;

// ---------------------------------------------------------------------------
// Refinement positive
// ---------------------------------------------------------------------------

#[test]
fn variant_member_reads_validate_in_the_selected_arm() {
    // `@payload.item.width` exists only on DrawShape and `@payload.item.text`
    // only on DrawText — each read sits in the arm that selects its variant,
    // so both resolve and type-check clean.
    let source = wrap_draw_trait(HAPPY_BODY);
    parse_and_lower_str(&source)
        .expect("variant-member reads inside the matching arms must validate");
}

#[test]
fn refined_member_type_flows_into_the_set_value_check() {
    // The refinement resolves the member's TYPE, not just its presence:
    // `width : number` assigned into the string-typed `lastText`, in the
    // RIGHT arm, is a type mismatch — before refinement this dotted read was
    // silently deferred to L2.
    let source = wrap_draw_trait(
        r#"
    listens {
      DISPATCH { item : DrawItem! }
    }
    state idle {
      INIT -> idle
        (set @entity.lastWidth 0)
        (set @entity.lastText "")
      DISPATCH -> idle match @payload.item.type {
        "draw-shape" -> (set @entity.lastText @payload.item.width)
        "draw-text"  -> (set @entity.lastText @payload.item.text)
      }
    }
"#,
    );
    let msg = lower_err(&source);
    assert!(
        msg.contains("lastText") && msg.contains("number"),
        "the refined member's declared type must drive the set-value check — got: {msg}"
    );
}

// ---------------------------------------------------------------------------
// Refinement negative
// ---------------------------------------------------------------------------

#[test]
fn wrong_arm_member_read_is_rejected() {
    // `width` is a DrawShape member; reading it inside the "draw-text" arm —
    // where the owner is refined to DrawText — can never see a value.
    let source = wrap_draw_trait(
        r#"
    listens {
      DISPATCH { item : DrawItem! }
    }
    state idle {
      INIT -> idle
        (set @entity.lastWidth 0)
        (set @entity.lastText "")
      DISPATCH -> idle match @payload.item.type {
        "draw-shape" -> (set @entity.lastWidth @payload.item.width)
        "draw-text"  -> (set @entity.lastWidth @payload.item.width)
      }
    }
"#,
    );
    let msg = lower_err(&source);
    assert!(
        msg.contains("ELOLO_MATCH_REFINED_MEMBER_UNKNOWN"),
        "got: {msg}"
    );
    assert!(
        msg.contains("DrawText") && msg.contains("width"),
        "the selected variant and the unknown member must both be named — got: {msg}"
    );
    assert!(
        msg.contains("text"),
        "the variant's actual fields are listed — got: {msg}"
    );
}

#[test]
fn wildcard_arm_keeps_the_conservative_posture() {
    // `_` selects no single variant, so no refinement applies — a dotted
    // read there defers to L2 exactly like non-match code.
    let source = wrap_draw_trait(
        r#"
    listens {
      DISPATCH { item : DrawItem! }
    }
    state idle {
      INIT -> idle
        (set @entity.lastWidth 0)
        (set @entity.lastText "")
      DISPATCH -> idle match @payload.item.type {
        "draw-shape" -> (set @entity.lastWidth @payload.item.width)
        _            -> (set @entity.lastWidth @payload.item.width)
      }
    }
"#,
    );
    parse_and_lower_str(&source).expect("a `_` arm's dotted reads stay unrefined (defer to L2)");
}

#[test]
fn the_same_read_outside_a_match_keeps_todays_behavior() {
    // No behavior change for non-match code: outside any match arm a dotted
    // `@payload.<f>.<member>` read — even one refinement would reject, and
    // even one whose member exists on no variant at all — is deferred to L2
    // exactly as it is today (resolve_top_segment's nested-path deferral).
    let source = wrap_draw_trait(
        r#"
    listens {
      DISPATCH { item : DrawItem! }
    }
    state idle {
      INIT -> idle
        (set @entity.lastWidth 0)
        (set @entity.lastText "")
      DISPATCH -> idle
        (set @entity.lastText @payload.item.width)
        (set @entity.lastWidth @payload.item.nonexistent)
    }
"#,
    );
    parse_and_lower_str(&source)
        .expect("outside a match, dotted reads keep today's defer-to-L2 posture unchanged");
}

// ---------------------------------------------------------------------------
// L2 tolerance proof
// ---------------------------------------------------------------------------

#[test]
fn lowered_destructuring_match_passes_full_l2_validation() {
    let source = wrap_draw_trait(HAPPY_BODY);
    let schema = parse_and_lower_str(&source).expect("lowers clean");
    let result = validate_comprehensive_without_services(&schema);
    let issues: Vec<String> = result
        .all_issues()
        .map(|e| format!("{}: {}", e.code, e.message))
        .collect();
    assert!(
        result.errors.is_empty() && result.warnings.is_empty(),
        "the desugared destructuring match (guarded transitions + variant-member reads on a \
         union-typed field) must pass L2 validation 0/0 — got: {issues:?}"
    );
}
