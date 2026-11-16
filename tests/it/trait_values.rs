//! Behaviors as values: a `trait`-typed field and the literal
//! `<spec>.traits.<Pascal>`, lowered to the object `{behavior, trait}`.

use orbital_lolo::{parse_and_lower_str, parse_str};

fn lowered_json(src: &str) -> serde_json::Value {
    let schema = parse_and_lower_str(src).unwrap_or_else(|e| panic!("lowering failed: {e}"));
    serde_json::to_value(&schema).unwrap()
}

fn parse_err(src: &str) -> String {
    match parse_str(src) {
        Ok(_) => panic!("expected a parse error"),
        Err(e) => e.to_string(),
    }
}

fn program(field: &str, effect: &str) -> String {
    format!(
        r#"orbital Holder {{
  entity Panel [runtime] {{
    id : string!
    {field}
  }}

  trait Hold -> Panel {{
    initial: idle

    state idle {{
      INIT -> idle
        (render-ui main {{ type: button, label: "Go", action: GO }})
      GO -> idle
        {effect}
    }}
  }}

  page "/hold" -> Hold
}}
"#
    )
}

fn entity_field<'a>(json: &'a serde_json::Value, name: &str) -> &'a serde_json::Value {
    json["orbitals"][0]["entity"]["fields"]
        .as_array()
        .expect("fields")
        .iter()
        .find(|f| f["name"] == name)
        .unwrap_or_else(|| panic!("field {name}"))
}

const KANBAN: &str = "std/std-kanban.traits.KanbanBoard";

#[test]
fn trait_is_a_legal_entity_field_type() {
    let json = lowered_json(&program("panel : trait", "(set @entity.id \"x\")"));
    assert_eq!(entity_field(&json, "panel")["type"], "trait");
}

#[test]
fn trait_field_default_none_is_null() {
    let json = lowered_json(&program("panel : trait = none", "(set @entity.id \"x\")"));
    let field = entity_field(&json, "panel");
    assert_eq!(field["type"], "trait");
    assert!(field.get("default").is_none_or(|d| d.is_null()), "{field}");
}

#[test]
fn trait_field_default_literal_lowers_to_the_object_form() {
    let json = lowered_json(&program(
        &format!("panel : trait = {KANBAN}"),
        "(set @entity.id \"x\")",
    ));
    assert_eq!(
        entity_field(&json, "panel")["default"],
        serde_json::json!({"behavior": "std/std-kanban", "trait": "KanbanBoard"})
    );
}

#[test]
fn literal_in_an_s_expression_argument_lowers_to_the_object_form() {
    let json = lowered_json(&program(
        "panel : trait = none",
        &format!("(set @entity.panel {KANBAN})"),
    ));
    let effect = &json["orbitals"][0]["traits"][0]["stateMachine"]["transitions"][1]["effects"][0];
    assert_eq!(
        effect[2],
        serde_json::json!({"behavior": "std/std-kanban", "trait": "KanbanBoard"}),
        "{effect}"
    );
}

#[test]
fn literal_with_an_import_prefix_and_std_spelling() {
    let json = lowered_json(&program(
        "a : trait = almadar-behaviors/vim-mode.traits.VimMode\n    b : trait = std/behaviors/std-kanban.traits.KanbanBoard",
        "(set @entity.id \"x\")",
    ));
    assert_eq!(
        entity_field(&json, "a")["default"],
        serde_json::json!({"behavior": "almadar-behaviors/vim-mode", "trait": "VimMode"})
    );
    assert_eq!(
        entity_field(&json, "b")["default"],
        serde_json::json!({"behavior": "std/behaviors/std-kanban", "trait": "KanbanBoard"})
    );
}

#[test]
fn control_a_string_literal_stays_a_string() {
    let json = lowered_json(&program(
        "note : string = \"std/std-kanban.traits.KanbanBoard\"",
        "(set @entity.id \"x\")",
    ));
    assert_eq!(
        entity_field(&json, "note")["default"],
        "std/std-kanban.traits.KanbanBoard"
    );
}

#[test]
fn control_other_config_only_types_stay_forbidden_in_entities() {
    // `SExpr` is allowed in a [runtime] entity (owner ruling 2026-10-13); see entity_config_type_through_alias.rs.
    for ty in ["slot", "pattern", "secret", "entity", "event"] {
        let msg = parse_err(&program(&format!("x : {ty}"), "(set @entity.id \"x\")"));
        assert!(
            msg.contains("ELOLO_CONFIG_TYPE_FORBIDDEN_IN_ENTITY"),
            "{ty}: {msg}"
        );
    }
}

#[test]
fn control_a_non_pascal_trait_segment_is_not_a_trait_value() {
    assert!(parse_str(&program(
        "panel : trait = std/std-kanban.traits.kanbanBoard",
        "(set @entity.id \"x\")"
    ))
    .is_err());
}

#[test]
fn relative_workspace_literal_lowers_to_the_object_form() {
    let json = lowered_json(&program(
        "panel : trait = ./orbitals/TaskBoard.traits.TaskBoardView",
        "(set @entity.id \"x\")",
    ));
    assert_eq!(
        entity_field(&json, "panel")["default"],
        serde_json::json!({"behavior": "./orbitals/TaskBoard", "trait": "TaskBoardView"})
    );
}

#[test]
fn control_relative_escape_and_spaced_run_are_not_trait_values() {
    for bad in [
        "panel : trait = ../outside/X.traits.Y",
        "panel : trait = ./orbitals/X .traits.Y",
        "panel : trait = ./X.traits.Y",
    ] {
        assert!(
            parse_str(&program(bad, "(set @entity.id \"x\")")).is_err(),
            "{bad}"
        );
    }
}
