//! L1 rung of the record-prop rule (2026-09-30): a render-ui prop the pattern
//! declares object-only takes ONE record. Binding a collection payload
//! (`data : [X]`) — or, for an `entity`-kind prop, a row of another entity —
//! is a type error at parse time. Shared predicate:
//! `orbital_core::type_compat::record_prop_binding_mismatch` (L2 uses it too).

use orbital_lolo::parse_and_lower_str;

fn program(loaded_type: &str, linked: &str, binding: &str) -> String {
    format!(
        r#"orbital ProbeOrbital {{
  type ProbeLoaded = Event {{ data : {loaded_type} }} "Probe rows loaded"

  entity Probe [persistent: probes] {{
    id : string!
    name : string
  }}

  entity Staff [persistent: staff] {{
    id : string!
  }}

  trait ProbeView -> {linked} {{
    initial: idle

    state idle {{
      INIT -> idle
        (fetch Probe {{ emit: {{ success: ProbeLoaded }} }})
        (render-ui main {{ type: stack, children: [] }})
      ProbeLoaded -> idle
        (render-ui main {{ type: detail-panel, entity: {binding} }})
    }}

    listens {{
      ProbeLoaded {{
        data : {loaded_type}
      }}
    }}
  }}

  page "/probe" -> ProbeView
}}
"#
    )
}

fn lower_err(src: &str) -> String {
    match parse_and_lower_str(src) {
        Ok(_) => panic!("expected a lowering error"),
        Err(e) => e.to_string(),
    }
}

fn lower_ok(src: &str) {
    if let Err(e) = parse_and_lower_str(src) {
        panic!("expected to lower cleanly, got: {e}");
    }
}

#[test]
fn a_collection_into_a_record_prop_is_a_type_error() {
    let err = lower_err(&program("[Probe]", "Probe", "?data"));
    assert!(err.contains("detail-panel") && err.contains("entity") && err.contains("collection"), "{err}");
}

#[test]
fn control_a_single_row_fits() {
    lower_ok(&program("Probe", "Probe", "?data"));
}

#[test]
fn control_the_traits_own_row_fits() {
    lower_ok(&program("[Probe]", "Probe", "@entity"));
}

#[test]
fn a_row_of_another_entity_is_a_type_error() {
    let err = lower_err(&program("Staff", "Probe", "?data"));
    assert!(err.contains("Staff") && err.contains("Probe"), "{err}");
}
