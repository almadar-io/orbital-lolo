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
use orbital_core_base::schema::{OrbitalSchema, TraitReference};
use orbital_lolo::parse_and_lower_str;
use std::path::Path;

fn compile_and_inline(src: &str) -> Result<OrbitalSchema, InlineError> {
    orbital_compiler::loader::register_source_lowerer(|path, _, _| orbital_test_support::lower_lolo_file(path));
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
    orbital_compiler::validate_comprehensive_without_services(schema)
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
        names.iter().any(|n| n.contains("InlineBadgeRender")),
        "non-lambda wrapper must keep trait composition: {names:?}"
    );

    resolve_to_oir(&schema).expect("OIR resolve (emit path) must succeed");
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

    // The un-spliced form's unknown-root failure is gone by construction.
    let result = validate(&schema);
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

const LET_RENDER_LOLO: &str = r#"orbital LetRender {
  uses UIBadge from "std/behaviors/ui-badge"
  entity Row [runtime] {
    id : string!
    n : number = 0
  }
  trait Show -> Row [client] {
    initial: idle
    state idle {
      INIT -> idle
      CALC -> idle
        (let ((total (+ 1 2)))
          (render-ui main <Stack direction={vertical}><UIBadge.traits.BadgeRender linkedEntity="Row" amount={@total} label="Total" /></Stack>))
    }
  }
  page "/" as LetRenderPage -> Show
}"#;

#[test]
fn render_prop_reading_a_let_binding_resolves_in_the_let_scope() {
    let schema = compile_and_inline(LET_RENDER_LOLO).expect("inline must succeed");
    let json = serde_json::to_string(&schema).unwrap();
    assert!(!json.contains("@trait."), "wrapper must be spliced away: {json}");
    assert!(json.contains(r#""amount":"@total""#), "the prop must sit inside the let body: {json}");
    assert_eq!(
        schema.orbitals[0].traits.len(),
        1,
        "the consumed wrapper must be removed"
    );
}

#[test]
fn control_a_prop_reading_no_binding_keeps_the_reference() {
    let src = LET_RENDER_LOLO.replace("amount={@total}", "amount={@nothing}");
    let schema = compile_and_inline(&src).expect("inline must succeed");
    let json = serde_json::to_string(&schema).unwrap();
    assert!(json.contains("@trait."), "an unbound name must not be captured by the let: {json}");
    assert_eq!(schema.orbitals[0].traits.len(), 2, "the wrapper is kept");
}

const LET_STAT_LOLO: &str = r#"orbital LetStat {
  uses Stat from "std/behaviors/ui-stat-display"
  entity Row [runtime] {
    id : string!
    n : number = 0
  }
  trait Show -> Row [client] {
    initial: idle
    state idle {
      INIT -> idle
      CALC -> idle
        (let ((total 3))
          (render-ui main <Stat.traits.StatDisplayRender linkedEntity={Row} format={number} label={"Total"} value={@total} ariaLabel={"Sum"} />))
    }
  }
  page "/" as LetStatPage -> Show
}"#;

fn show_trait_json(schema: &OrbitalSchema) -> String {
    let show = schema.orbitals[0]
        .traits
        .iter()
        .find_map(|tr| match tr {
            TraitReference::Inline(td) if td.name == "Show" => Some(td),
            _ => None,
        })
        .expect("Show trait");
    serde_json::to_string(&show.state_machine).unwrap()
}

#[test]
fn spliced_let_wrapper_resolves_its_own_config_forwards() {
    let schema = compile_and_inline(LET_STAT_LOLO).expect("inline must succeed");
    let json = show_trait_json(&schema);
    assert!(json.contains("@total"), "the prop must sit inside the let body: {json}");
    assert!(!json.contains("@config."), "no wrapper config forward may survive the splice: {json}");
    assert!(json.contains(r#""aria-label":"Sum""#), "the call-site prop wins: {json}");
}

#[test]
fn control_an_unspliced_wrapper_keeps_its_own_config_forwards() {
    let src = LET_STAT_LOLO.replace("value={@total}", "value={@nothing}");
    let schema = compile_and_inline(&src).expect("inline must succeed");
    let wrapper = schema.orbitals[0]
        .traits
        .iter()
        .find_map(|tr| match tr {
            TraitReference::Inline(td) if td.name != "Show" => Some(td),
            _ => None,
        })
        .expect("wrapper kept");
    assert!(wrapper.config.is_some(), "the kept wrapper keeps its config");
}

fn trait_ids_and_ledger_trait_rows(schema: &OrbitalSchema) -> (Vec<String>, Vec<String>) {
    let live = schema
        .orbitals
        .iter()
        .flat_map(|o| o.traits.iter())
        .filter_map(|tr| match tr {
            TraitReference::Inline(td) => td.id.clone(),
            _ => None,
        })
        .collect();
    let rows = schema
        .ledger
        .as_ref()
        .map(|l| {
            l.entries
                .values()
                .filter(|row| row.kind == orbital_core_base::identity::IdKind::Trait)
                .map(|row| row.id.clone())
                .collect()
        })
        .unwrap_or_default();
    (live, rows)
}

#[test]
fn let_splice_leaves_no_ledger_row_for_the_removed_wrapper() {
    let schema = compile_and_inline(LET_STAT_LOLO).expect("inline must succeed");
    assert_eq!(schema.orbitals[0].traits.len(), 1, "the wrapper must be removed");
    let (live, rows) = trait_ids_and_ledger_trait_rows(&schema);
    let stale: Vec<&String> = rows.iter().filter(|r| !live.contains(r)).collect();
    assert!(stale.is_empty(), "ledger rows carried by no trait: {stale:?}");
}

#[test]
fn control_an_unspliced_wrapper_keeps_its_ledger_row() {
    let src = LET_STAT_LOLO.replace("value={@total}", "value={@nothing}");
    let schema = compile_and_inline(&src).expect("inline must succeed");
    assert_eq!(schema.orbitals[0].traits.len(), 2, "the wrapper is kept");
    let (live, rows) = trait_ids_and_ledger_trait_rows(&schema);
    assert_eq!(live.len(), 2);
    assert!(live.iter().all(|id| rows.contains(id)), "every live trait keeps its row: {live:?} vs {rows:?}");
}
