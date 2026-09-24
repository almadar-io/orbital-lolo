// Annotations belong to the local listens form; after a sourced `A.X -> Y` they are a precise parse error, never re-attributed.

use orbital_lolo::{parse_str, ParseError};

fn program(listens: &str, body_extra: &str) -> String {
    format!(
        r#"app Probe
orbital ProbeOrbital {{
  entity Item [runtime] {{
    id : string!
  }}
  trait Watcher -> Item {{
{body_extra}    listens {{
{listens}
    }}
    initial: idle
    state idle {{
      INIT -> idle
      GOT -> idle
    }}
  }}
  page "/" -> Watcher [initial]
}}"#
    )
}

#[test]
fn each_annotation_after_a_sourced_listen_is_rejected_by_name() {
    for annotation in ["description", "synonyms", "tier"] {
        let src = program(&format!("      Src.X -> GOT\n        @{annotation} \"words\""), "");
        let err = parse_str(&src).expect_err("a sourced listen takes no annotations");
        assert!(
            matches!(&err, ParseError::SourcedListenAnnotation { annotation: a, .. } if a == annotation),
            "{annotation}: {err:?}"
        );
    }
}

#[test]
fn the_rejection_holds_after_when_and_with_lines() {
    let src = program(
        "      Src.X -> GOT\n        when (> ?n 0)\n        with { n: ?n }\n        @description \"late\"",
        "",
    );
    assert!(matches!(parse_str(&src), Err(ParseError::SourcedListenAnnotation { .. })));
}

#[test]
fn the_local_form_still_takes_its_annotations() {
    let src = program("      GOT { n: number }\n        @description \"a local event\"", "");
    let program = parse_str(&src).expect("local listens annotations are legal");
    let listens = &program.orbitals[0].traits[0].listens;
    assert_eq!(listens[0].description.as_deref(), Some("a local event"));
}

#[test]
fn a_sourced_listen_without_annotations_still_parses() {
    let src = program("      Src.X -> GOT\n        when (> ?n 0)", "");
    assert!(parse_str(&src).is_ok());
}

#[test]
fn the_entity_binding_description_in_its_own_position_is_untouched() {
    let src = program("      Src.X -> GOT", "    @description \"what this binding is\"\n");
    let program = parse_str(&src).expect("a body-level binding description is legal");
    assert_eq!(
        program.orbitals[0].traits[0].entity_binding_description.as_deref(),
        Some("what this binding is")
    );
}
