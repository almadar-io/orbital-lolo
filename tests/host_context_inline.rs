//! End-to-end fixtures for host-context prop evaluation of inline trait
//! references (`<Alias.traits.Name prop={expr}/>` JSX sugar).
//!
//! Compiles real `.lolo` sources through parse → lower → inline (the
//! installed std package, Phase 2: no embedded registry fallback) →
//! comprehensive validation → OIR resolve, pinning both agreed semantics:
//!
//! 1. Shared-entity authority: inline wrappers forwarding `@entity.*` inside
//!    the single render-authority trait attribute to that authority
//!    (structural render-child attribution) — while a genuinely independent
//!    second reader still rejects.
//! 2. Lambda scope: render-only inline refs inside a `fn`-lambda render
//!    subtree are spliced at inline time, so the author's expressions resolve
//!    in the lambda's per-item scope; stateful refs in that position error.

use orbital_compiler::loader::LoaderConfig;
use orbital_compiler::phases::inline::{inline_schema, InlineConfig, InlineError};
use orbital_compiler::phases::resolve_to_oir;
use orbital_compiler::phases::validation::{ErrorCode, ValidationResult};
use orbital_core::schema::{OrbitalSchema, TraitReference};
use orbital_lolo::parse_and_lower_str;
use std::path::Path;

fn compile_and_inline(src: &str) -> Result<OrbitalSchema, InlineError> {
    let mut schema = parse_and_lower_str(src).expect("parse+lower must succeed");
    let std_pkg = orbital_test_support::installed_package(
        "almadar-std",
        Path::new(env!("CARGO_MANIFEST_DIR")),
    );
    let config = InlineConfig {
        loader_config: LoaderConfig {
            packages: vec![std_pkg],
            ..Default::default()
        },
        ..Default::default()
    };
    inline_schema(&mut schema, &config, None)?;
    Ok(schema)
}

fn validate(schema: &OrbitalSchema) -> ValidationResult {
    orbital_compiler::validate_comprehensive(schema)
}

fn assert_no_issue(result: &ValidationResult, code: ErrorCode, ctx: &str) {
    let hits: Vec<String> = result
        .all_issues()
        .filter(|e| e.code == code)
        .map(|e| e.message.clone())
        .collect();
    assert!(hits.is_empty(), "{ctx}: unexpected {code:?}: {hits:?}");
}

// ---------------------------------------------------------------------------
// Fixture 1 — flashcards shape: [runtime, shared] entity, one authority trait
// containing inline badge + flip-card refs forwarding @entity.*.
// ---------------------------------------------------------------------------

const FLASHCARDS_LOLO: &str = r#"orbital FlashcardOrbital {
  uses UIBadge from "std/behaviors/ui-badge"
  uses UIFlipCard from "std/behaviors/ui-flip-card"

  entity FlashcardDeck [runtime, shared] {
    id : string!
    displayIndex : number
    title : string
  }

  trait DeckView -> FlashcardDeck {
    state idle {
      INIT -> idle
        (set @entity.displayIndex 0)
        (set @entity.title "Deck")
        (render-ui main
          <Stack direction="vertical" gap="md">
            <UIBadge.traits.BadgeRender linkedEntity="FlashcardDeck" amount={@entity.displayIndex} label={@entity.title} />
            <UIFlipCard.traits.FlipCardRender linkedEntity="FlashcardDeck" />
          </Stack>)
    }
  }

  page "/deck" as DeckPage -> DeckView
}"#;

#[test]
fn flashcards_shared_entity_inline_wrappers_are_one_authority() {
    let schema = compile_and_inline(FLASHCARDS_LOLO).expect("inline must succeed");
    let result = validate(&schema);
    assert_no_issue(
        &result,
        ErrorCode::SharedEntityRenderAuthority,
        "authority + inline wrappers forwarding @entity.*",
    );
    // The wrappers are OUTSIDE any lambda: composition semantics are
    // unchanged — the hoisted synthetics must still exist as traits.
    let names: Vec<&str> = schema.orbitals[0]
        .traits
        .iter()
        .filter_map(|t| match t {
            TraitReference::Inline(td) => Some(td.name.as_str()),
            _ => None,
        })
        .collect();
    assert!(
        names.iter().any(|n| n.starts_with("InlineBadgeRender")),
        "non-lambda wrapper must keep trait composition: {names:?}"
    );

    resolve_to_oir(&schema).expect("OIR resolve (emit path) must succeed");
}

#[test]
fn flashcards_second_hand_written_reader_still_rejected() {
    // Same shape plus a genuinely independent second reader trait —
    // referenced from NO render tree — must still trip the authority rule.
    // Same slot ("main") as DeckView's authority render: the validator's
    // per-slot refinement (entity_mutation.rs) only contends readers that
    // share a (entity, slot) pair, so a differently-slotted second reader
    // is legitimately not a violation — this fixture must match slots to
    // actually exercise the single-authority-per-slot rule it asserts.
    let src = FLASHCARDS_LOLO.replace(
        "  page \"/deck\" as DeckPage -> DeckView",
        r#"  trait RogueReader -> FlashcardDeck {
    state idle {
      INIT -> idle
        (render-ui main { type: badge, label: @entity.title })
    }
  }

  page "/deck" as DeckPage -> DeckView"#,
    );
    let schema = compile_and_inline(&src).expect("inline must succeed");
    let result = validate(&schema);
    assert!(
        result
            .all_issues()
            .any(|e| e.code == ErrorCode::SharedEntityRenderAuthority),
        "an unreferenced second reader must still be rejected"
    );
}

// ---------------------------------------------------------------------------
// Fixture 2 — multi-party shape: data-list renderItem lambda with inline
// refs reading @party.*; emits merged into the host.
// ---------------------------------------------------------------------------

const MULTI_PARTY_LOLO: &str = r#"orbital PartyOrbital {
  uses UIBadge from "std/behaviors/ui-badge"
  uses UIButton from "std/behaviors/ui-button"

  entity PartyBoard [runtime] {
    id : string!
    parties : [{ title : string }]
  }

  trait PartyList -> PartyBoard {
    state idle {
      INIT -> idle
        (set @entity.parties [])
        (render-ui main {
          type: data-list
          entity: @entity.parties
          renderItem: (fn party
            <Stack direction="vertical" gap="sm">
              <UIBadge.traits.BadgeRender label={@party.title} />
              <UIButton.traits.ButtonRender label="Select" actionPayload={{ id: @party.id }} />
            </Stack>)
        })
    }
  }

  page "/parties" as PartyPage -> PartyList
}"#;

#[test]
fn multi_party_lambda_inline_refs_splice_into_host_scope() {
    let schema = compile_and_inline(MULTI_PARTY_LOLO).expect("inline must succeed");
    let orbital = &schema.orbitals[0];

    // The wrapper traits were consumed by the splice.
    let names: Vec<&str> = orbital
        .traits
        .iter()
        .filter_map(|t| match t {
            TraitReference::Inline(td) => Some(td.name.as_str()),
            _ => None,
        })
        .collect();
    assert!(
        !names.iter().any(|n| n.starts_with("Inline")),
        "spliced wrappers must be removed: {names:?}"
    );

    // The author's per-item expressions now live inside the host's lambda.
    let json = serde_json::to_string(&schema).expect("serialize");
    assert!(
        json.contains("@party.title") && json.contains("@party.id"),
        "lambda-scoped expressions must survive in the host tree"
    );
    assert!(
        !json.contains("@trait.Inline"),
        "no hoisted reference may remain in the render tree"
    );

    // Wrapper emits merged into the host (ButtonRender: ACTION). BadgeRender
    // declares no static emits since the presence-gated outlet change —
    // `onRemove` is an OFF-by-default knob whose emit is the caller-supplied
    // event name, so there is no REMOVE to merge.
    let host = orbital
        .traits
        .iter()
        .find_map(|t| match t {
            TraitReference::Inline(td) if td.name == "PartyList" => Some(td),
            _ => None,
        })
        .expect("host trait");
    let emits: Vec<&str> = host
        .emits
        .iter()
        .flatten()
        .map(|e| e.event.as_str())
        .collect();
    assert!(
        emits.contains(&"ACTION"),
        "wrapper emits must merge into the host: {emits:?}"
    );

    // Both failure classes from the un-spliced form are gone by construction.
    let result = validate(&schema);
    assert_no_issue(
        &result,
        ErrorCode::BindingTraitInvalidPosition,
        "renderItem",
    );
    assert_no_issue(
        &result,
        ErrorCode::BindingUnknownRoot,
        "lambda param @party",
    );

    // Emit path: the expanded nodes lower to OIR.
    let oir = resolve_to_oir(&schema).expect("OIR resolve must succeed");
    let oir_json = serde_json::to_string(&oir).expect("serialize OIR");
    assert!(
        oir_json.contains("@party.title"),
        "emitted IR must contain the expanded raw nodes inside the lambda"
    );
}

#[test]
fn multi_party_party_binding_outside_lambda_still_rejected() {
    // Pin: the splice must not blanket-legalize lambda-param roots. `@party`
    // OUTSIDE any lambda stays an unknown binding root.
    let src = MULTI_PARTY_LOLO.replace(
        "          entity: @entity.parties",
        "          entity: @entity.parties\n          header: @party.title",
    );
    let schema = compile_and_inline(&src).expect("inline must succeed");
    let result = validate(&schema);
    assert!(
        result
            .all_issues()
            .any(|e| e.code == ErrorCode::BindingUnknownRoot),
        "@party outside a lambda must still be rejected"
    );
}

// ---------------------------------------------------------------------------
// Fixture 3 — a stateful wrapper referenced inside a renderItem lambda is a
// clear error (structural test, not name-based).
// ---------------------------------------------------------------------------

const STATEFUL_IN_LAMBDA_LOLO: &str = r#"orbital TogglerOrbital {
  entity PartyBoard [runtime] {
    id : string!
    parties : [{ title : string }]
    on : boolean
  }

  trait Toggler -> PartyBoard {
    initial: off
    state off {
      TOGGLE -> on
        (set @entity.on true)
        (render-ui main { type: badge, label: "on" })
    }
    state on {
      TOGGLE -> off
        (set @entity.on false)
        (render-ui main { type: badge, label: "off" })
    }
  }

  trait Lister -> PartyBoard {
    state idle {
      INIT -> idle
        (set @entity.parties [])
        (render-ui list {
          type: data-list
          entity: @entity.parties
          renderItem: (fn party
            <Stack>
              <trait.Toggler />
            </Stack>)
        })
    }
  }

  page "/toggle" as TogglePage -> Lister
}"#;

#[test]
fn stateful_wrapper_in_lambda_is_rejected_with_clear_error() {
    let err = compile_and_inline(STATEFUL_IN_LAMBDA_LOLO)
        .expect_err("a stateful trait inside renderItem must be rejected");
    match err {
        InlineError::LambdaStatefulTrait { ref name, .. } => {
            assert_eq!(name, "Toggler");
            assert_eq!(err.structured_code(), "ORB_T_LAMBDA_STATEFUL_TRAIT");
        }
        other => panic!("expected LambdaStatefulTrait, got {other:?}"),
    }
}
