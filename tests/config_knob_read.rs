//! G-ORB-027: a trait's declared config knob that nothing in its orbital reads
//! is dead surface — a call site can set it and nothing changes (std-graphs
//! declared `period` and grouped by raw timestamp). Every declared knob must be
//! read by `@config.<knob>` somewhere in the orbital: an effect, a guard, a
//! render tree (including hoisted render children), or another knob's default.

use orbital_lolo::parse_and_lower_str;

fn program(knobs: &str, init_body: &str, guard: &str) -> String {
    format!(
        r#"orbital ProbeOrbital {{
  entity Probe [persistent: probes] {{
    id : string!
    name : string
  }}

  trait ProbeView -> Probe [interaction, instance] {{
    initial: idle

    state idle {{
      INIT -> idle{guard}
        {init_body}
    }}

    config {{
{knobs}
    }}
  }}

  page "/probe" -> ProbeView
}}
"#
    )
}

fn lower(src: &str) -> Result<(), String> {
    parse_and_lower_str(src).map(|_| ()).map_err(|e| e.to_string())
}

#[test]
fn an_unread_knob_is_an_error_naming_it() {
    let src = program("      title : string = \"Probe\"\n      period : string = \"day\"", "(render-ui main { type: typography, content: @config.title })", "");
    let err = lower(&src).expect_err("period is read nowhere");
    assert!(err.contains("period") && err.contains("ProbeView"), "{err}");
    assert!(!err.contains("\"title\""), "title is read: {err}");
}

#[test]
fn control_a_knob_read_in_a_render_is_fine() {
    let src = program("      title : string = \"Probe\"", "(render-ui main { type: typography, content: @config.title })", "");
    lower(&src).expect("title is read");
}

#[test]
fn edge_a_knob_read_only_by_a_guard_is_fine() {
    let src = program("      enabled : boolean = true", "(render-ui main { type: typography, content: \"x\" })", " when @config.enabled");
    lower(&src).expect("enabled is read by the guard");
}

#[test]
fn edge_a_knob_read_only_by_another_knobs_default_is_fine() {
    let src = program("      title : string = \"Probe\"\n      heading : string = @config.title", "(render-ui main { type: typography, content: @config.heading })", "");
    lower(&src).expect("title is forwarded into heading");
}

#[test]
fn edge_a_knob_read_only_as_another_knobs_dependent_type_is_fine() {
    let src = program(
        "      action : event = \"PROBE\"\n      actionPayload : @payload action = {}",
        "(render-ui main { type: button, label: \"Go\", actionPayload: @config.actionPayload })",
        "",
    );
    lower(&src).expect("action types actionPayload");
}

#[test]
fn edge_a_knob_read_only_by_a_type_level_match_is_fine() {
    let src = r#"orbital ProbeOrbital {
  entity Probe [persistent: probes] {
    id : string!
  }
  trait ProbeView -> Probe [interaction, instance] {
    initial: idle
    state idle {
      INIT -> idle
        (render-ui main { type: typography, content: "x" })
      PICK -> idle
        (emit PICKED { value: ?value })
    }
    emits {
      PICKED -> external {
        value : match @config.inputType {
          "number" -> number
          _ -> string
        }
      }
    }
    config {
      inputType : string = "text"
    }
  }
  page "/probe" -> ProbeView
}
"#;
    lower(src).expect("inputType types the PICKED payload");
}
