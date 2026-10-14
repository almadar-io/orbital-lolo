//! Lowering + reinterpretation tests for the trait-level type-parameter
//! increment (3-II, `docs/Almadar_LOLO.md`).
//!
//! Mirrors `type_param_sentinel.rs` (Increment 3-I's `(e : Entity)` alias
//! parameter) for the two NEW surfaces:
//!   1. Declaration — `trait Name (v : Type, r : Type) -> Entity [...] { ... }`
//!      — a trait's own type-parameter header, usable directly in its
//!      `emits`/`listens`/`config` type positions.
//!   2. Reinterpretation — the pre-existing `::` annotation surface on a
//!      trait REFERENCE becomes call-site type-parameter arguments when the
//!      referenced source trait declares params; a REGRESSION suite proves
//!      the original alias-conformance meaning is untouched when the
//!      annotated name resolves as a real local type alias.
//!
//! L2 substitution (explicit primitive/entity args, Entity-kind default,
//! Type-kind sweep) is covered end-to-end in
//! `orbital-compiler/tests/trait_type_param_inline.rs`.

use orbital_core::TraitReference;
use orbital_lolo::{parse_and_lower_str, parse_str, LoloError, LowerError, TypeError};

// ---------------------------------------------------------------------------
// 1. Declaration: multi-param header, comma- and space-separated
// ---------------------------------------------------------------------------

const MULTI_PARAM_SRC: &str = r#"
orbital MLRungs {
  entity KeyLookup [runtime] {
    id : string!
  }

  trait LookupRun (v : Type, r : Type) -> KeyLookup [lifecycle, instance] {
    state idle {
      INIT -> idle (ref KeyLookup)
    }
    emits
      CandidateFound { candidate : v, request : r }
  }

  page "/lookup" -> LookupRun
}
"#;

#[test]
fn multi_param_comma_separated_header_parses() {
    let program = parse_str(MULTI_PARAM_SRC).expect("parse");
    let orbital = &program.orbitals[0];
    let trait_decl = orbital
        .traits
        .iter()
        .find(|t| t.name == "LookupRun")
        .expect("LookupRun trait");
    assert_eq!(trait_decl.type_params.len(), 2);
    assert_eq!(trait_decl.type_params[0].name, "v");
    assert_eq!(trait_decl.type_params[0].kind, "Type");
    assert_eq!(trait_decl.type_params[1].name, "r");
    assert_eq!(trait_decl.type_params[1].kind, "Type");
}

#[test]
fn multi_param_space_separated_header_parses_identically() {
    let space_separated = MULTI_PARAM_SRC.replace("(v : Type, r : Type)", "(v : Type r : Type)");
    let program = parse_str(&space_separated).expect("parse");
    let trait_decl = program.orbitals[0]
        .traits
        .iter()
        .find(|t| t.name == "LookupRun")
        .expect("LookupRun trait");
    assert_eq!(trait_decl.type_params.len(), 2);
    assert_eq!(trait_decl.type_params[0].name, "v");
    assert_eq!(trait_decl.type_params[1].name, "r");
}

#[test]
fn param_used_in_emit_payload_lowers_to_sentinel() {
    let schema = parse_and_lower_str(MULTI_PARAM_SRC).expect("parse_and_lower must succeed");
    let TraitReference::Inline(trait_def) = &schema.orbitals[0].traits[0] else {
        panic!("expected an inline trait definition");
    };

    // typeParams survive lowering onto the schema (kind info for the L2
    // substitution pass).
    let type_params = trait_def
        .type_params
        .as_ref()
        .expect("typeParams must be populated");
    assert_eq!(type_params.len(), 2);
    assert_eq!(type_params[0].name, "v");
    assert_eq!(type_params[1].name, "r");

    let emits = trait_def.emits.as_ref().expect("emits[] must be populated");
    let found = emits
        .iter()
        .find(|e| e.event == "CandidateFound")
        .expect("CandidateFound emit entry");
    let payload = found.payload_schema.as_ref().expect("payloadSchema");
    assert_eq!(payload.len(), 2);
    let candidate = payload.iter().find(|f| f.name == "candidate").unwrap();
    assert_eq!(candidate.field_type, "$v");
    assert!(candidate.properties.is_none());
    let request = payload.iter().find(|f| f.name == "request").unwrap();
    assert_eq!(request.field_type, "$r");
}

// ---------------------------------------------------------------------------
// 2. Reinterpretation: `::` on a trait reference becomes typeArgs pairs when
//    the referenced source's local annotation name is NOT a known alias.
// ---------------------------------------------------------------------------

fn ref_source() -> String {
    format!(
        r#"
{MULTI_PARAM_SRC}

orbital MLCandidateLookup {{
  uses MLRungsAlias from "./ml-rungs.orb"

  entity Booking [runtime] {{
    id : string!
  }}

  trait CandidateLookup :: v string r Booking = MLRungsAlias.traits.LookupRun -> Booking {{
    listens {{
    }}
  }}

  page "/candidates" -> CandidateLookup
}}
"#
    )
}

#[test]
fn call_site_type_args_pairs_lower_onto_the_reference() {
    // Two orbitals in one file: `check_program`'s per-orbital annotation
    // check only needs to see that "v" resolves to no LOCAL alias — it does
    // not need the second orbital's `uses` import to actually resolve
    // (L1 has no cross-file visibility into the source trait's params; see
    // the module doc and `types::check_trait_annotation`).
    let src = ref_source();
    let schema = parse_and_lower_str(&src).expect("parse_and_lower must succeed");
    let composer = &schema.orbitals[1];
    let TraitReference::Reference { type_args, .. } = &composer.traits[0] else {
        panic!("expected a trait reference");
    };
    let args = type_args
        .as_ref()
        .expect("`:: v string r Booking` must lower to typeArgs pairs");
    assert_eq!(args.get("v").map(String::as_str), Some("string"));
    assert_eq!(args.get("r").map(String::as_str), Some("Booking"));
}

#[test]
fn odd_type_arg_count_is_a_lower_error() {
    let src = ref_source().replace(
        ":: v string r Booking = MLRungsAlias",
        ":: v string r = MLRungsAlias",
    );
    let err = parse_and_lower_str(&src).expect_err("odd pairing must error");
    assert!(
        matches!(
            &err,
            LoloError::Lower { inner, .. } if matches!(inner.innermost(), LowerError::OddTypeArgCount { .. })
        ),
        "expected OddTypeArgCount, got {err:?}"
    );
}

// ---------------------------------------------------------------------------
// 2b. REGRESSION: a `::` annotation naming a real local type alias keeps its
//     pre-existing alias-conformance meaning untouched — no reinterpretation,
//     no typeArgs emitted, and the conformance check still runs (and still
//     catches a genuine mismatch).
// ---------------------------------------------------------------------------

const ALIAS_CONFORMANCE_SRC: &str = r#"
type Browsable t = Trait Browse.traits.BrowseItemBrowse -> t

orbital MyOrbital {
  uses Browse from "std/behaviors/std-browse"

  entity MyItem [runtime] {
    id : string!
    name : string
  }

  trait MyItemBrowse :: Browsable MyItem = Browse.traits.BrowseItemBrowse -> MyItem { }

  page "/items" -> MyItemBrowse
}
"#;

#[test]
fn alias_conformance_annotation_on_a_reference_is_unaffected() {
    let schema = parse_and_lower_str(ALIAS_CONFORMANCE_SRC)
        .expect("known alias-conformance trait must still lower");
    let TraitReference::Reference { type_args, .. } = &schema.orbitals[0].traits[0] else {
        panic!("expected a trait reference");
    };
    assert!(
        type_args.is_none(),
        "a genuine alias-conformance annotation must NOT be reinterpreted as typeArgs"
    );
}

#[test]
fn alias_conformance_mismatch_still_errors() {
    // `Browsable` requires the annotated trait's linkedEntity to equal the
    // supplied arg (`MyItem`); mismatching it must still raise the ORIGINAL
    // Stage-2 AnnotationMismatch — proving the reinterpretation change did
    // not also swallow real alias-conformance errors.
    let src = ALIAS_CONFORMANCE_SRC.replace(
        ":: Browsable MyItem = Browse.traits.BrowseItemBrowse -> MyItem",
        ":: Browsable OtherItem = Browse.traits.BrowseItemBrowse -> MyItem",
    );
    let err = parse_and_lower_str(&src).expect_err("mismatched alias arg must still error");
    assert!(
        matches!(
            err,
            LoloError::Lower {
                inner: LowerError::Type(TypeError::AnnotationMismatch { .. }),
                ..
            }
        ),
        "expected AnnotationMismatch, got {err:?}"
    );
}

// ---------------------------------------------------------------------------
// Gap A: orbital-scope struct aliases survive L1 onto `Orbital.types`.
// ---------------------------------------------------------------------------

const STRUCT_ALIAS_ORBITAL_SRC: &str = r#"
type FileScopeShape = { label : string! }

orbital ApprovalOrbital {
  type LocalShape = { id : string!, note : string }

  entity ApprovalRecord [runtime] {
    id : string!
  }

  trait ApprovalRun -> ApprovalRecord [lifecycle, instance] {
    state idle {
      INIT -> idle (ref ApprovalRecord)
    }
  }

  page "/approvals" -> ApprovalRun
}
"#;

#[test]
fn orbital_scope_and_file_scope_struct_aliases_populate_types_map() {
    let schema =
        parse_and_lower_str(STRUCT_ALIAS_ORBITAL_SRC).expect("parse_and_lower must succeed");
    let types = schema.orbitals[0]
        .types
        .as_ref()
        .expect("Orbital.types must be populated with struct aliases visible in this scope");
    let local = types
        .get("LocalShape")
        .expect("orbital-scope alias must be carried");
    let local_names: Vec<&str> = local.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(local_names, vec!["id", "note"]);

    let file_scope = types
        .get("FileScopeShape")
        .expect("file-scope alias visible from this orbital must be carried too");
    assert_eq!(file_scope[0].name, "label");
}

#[test]
fn orbital_with_no_struct_aliases_has_no_types_map() {
    let schema = parse_and_lower_str(MULTI_PARAM_SRC).expect("parse_and_lower must succeed");
    assert!(
        schema.orbitals[0].types.is_none(),
        "an orbital declaring no struct aliases must not grow a types map"
    );
}

// ---------------------------------------------------------------------------
// Gap C: `(name : Kind = <default>)` declaration defaults.
// ---------------------------------------------------------------------------

const PARAM_DEFAULT_SRC: &str = r#"
orbital MLRungsDefaulted {
  type ApprovalDemoPayload = { note : string }

  entity KeyLookup [runtime] {
    id : string!
  }

  trait LookupRun (v : Type = ApprovalDemoPayload, r : Type = string) -> KeyLookup [lifecycle, instance] {
    state idle {
      INIT -> idle (ref KeyLookup)
    }
    emits
      CandidateFound { candidate : v, request : r }
  }

  page "/lookup" -> LookupRun
}
"#;

#[test]
fn declaration_default_parses_and_lowers_onto_type_param_def() {
    let program = parse_str(PARAM_DEFAULT_SRC).expect("parse");
    let orbital = &program.orbitals[0];
    let trait_decl = orbital
        .traits
        .iter()
        .find(|t| t.name == "LookupRun")
        .expect("LookupRun trait");
    assert_eq!(
        trait_decl.type_params[0].default.as_deref(),
        Some("ApprovalDemoPayload")
    );
    assert_eq!(trait_decl.type_params[1].default.as_deref(), Some("string"));

    let schema = parse_and_lower_str(PARAM_DEFAULT_SRC).expect("parse_and_lower must succeed");
    let TraitReference::Inline(trait_def) = &schema.orbitals[0].traits[0] else {
        panic!("expected an inline trait definition");
    };
    let type_params = trait_def
        .type_params
        .as_ref()
        .expect("typeParams must be populated");
    assert_eq!(
        type_params[0].default.as_deref(),
        Some("ApprovalDemoPayload")
    );
    assert_eq!(type_params[1].default.as_deref(), Some("string"));
}

#[test]
fn param_with_no_default_lowers_default_none() {
    let schema = parse_and_lower_str(MULTI_PARAM_SRC).expect("parse_and_lower must succeed");
    let TraitReference::Inline(trait_def) = &schema.orbitals[0].traits[0] else {
        panic!("expected an inline trait definition");
    };
    let type_params = trait_def
        .type_params
        .as_ref()
        .expect("typeParams must be populated");
    assert!(type_params[0].default.is_none());
    assert!(type_params[1].default.is_none());
}

#[test]
fn unknown_declaration_default_name_is_a_lower_error() {
    let src = PARAM_DEFAULT_SRC.replace("v : Type = ApprovalDemoPayload", "v : Type = NoSuchAlias");
    let err = parse_and_lower_str(&src).expect_err("unknown default name must error");
    assert!(
        matches!(
            err,
            LoloError::Lower {
                inner: LowerError::Type(TypeError::UnknownType { ref name, .. }),
                ..
            } if name == "NoSuchAlias"
        ),
        "expected UnknownType(\"NoSuchAlias\"), got {err:?}"
    );
}
