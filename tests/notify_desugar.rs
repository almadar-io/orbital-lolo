//! `notify` is sugar over `render-ui toast` (owner ruling 2026-09-11): the
//! `.orb` IR never carries a `notify` effect kind. Covers the canonical
//! `(notify level message)` form for every level, `notify` nested inside
//! `do`/`if` bodies, a dynamic (`@config.*`) level binding, and both
//! lowering errors (unknown level, wrong arity).

use orbital_core_base::schema::TraitReference;
use orbital_lolo::{parse_and_lower_str, LoloError};

fn wrap(trait_body: &str) -> String {
    format!(
        r#"orbital NotifyDesugarOrbital {{
  entity Row [runtime] {{
    id : string!
  }}

  trait Exec -> Row [interaction] {{
{trait_body}
  }}
}}
"#
    )
}

fn lower_err(source: &str) -> String {
    match parse_and_lower_str(source) {
        Err(LoloError::Lower { inner, .. }) => inner.to_string(),
        Ok(_) => panic!("expected an error, lowering succeeded"),
        Err(other) => panic!("expected a Lower error, got {other}"),
    }
}

fn go_effects(source: &str) -> Vec<serde_json::Value> {
    let schema = parse_and_lower_str(source).expect("lowering should succeed");
    let exec = schema.orbitals[0]
        .traits
        .iter()
        .find_map(|t| match t {
            TraitReference::Inline(def) if def.name == "Exec" => Some(def),
            _ => None,
        })
        .expect("expected inline trait Exec");
    exec.state_machine
        .transitions
        .iter()
        .find(|t| t.event == "GO")
        .and_then(|t| t.effects.clone())
        .expect("GO transition should carry effects")
        .into_iter()
        .map(|e| e.to_canonical_json())
        .collect()
}

fn expected_toast(variant: &str, message: &str) -> serde_json::Value {
    serde_json::json!([
        "render-ui",
        "toast",
        {
            "type": "alert",
            "variant": variant,
            "message": message,
            "dismissible": true,
        }
    ])
}

#[test]
fn notify_desugars_to_render_ui_toast() {
    let src = wrap(
        r#"    state idle {
      GO -> idle
        (notify error "Could not save")
    }"#,
    );
    let effects = go_effects(&src);
    assert_eq!(effects, vec![expected_toast("error", "Could not save")]);
}

#[test]
fn notify_desugars_every_level() {
    for level in ["success", "error", "warning", "info"] {
        let src = wrap(&format!(
            r#"    state idle {{
      GO -> idle
        (notify {level} "msg")
    }}"#
        ));
        let effects = go_effects(&src);
        assert_eq!(effects, vec![expected_toast(level, "msg")]);
    }
}

#[test]
fn notify_desugars_nested_inside_do() {
    let src = wrap(
        r#"    state idle {
      GO -> idle
        (do (set @entity.id @entity.id) (notify info "done"))
    }"#,
    );
    let effects = go_effects(&src);
    assert_eq!(
        effects,
        vec![serde_json::json!([
            "do",
            ["set", "@entity.id", "@entity.id"],
            expected_toast("info", "done"),
        ])]
    );
}

#[test]
fn notify_desugars_nested_inside_if() {
    let src = wrap(
        r#"    state idle {
      GO -> idle
        (if (== @entity.id @entity.id) (notify success "ok") (notify error "no"))
    }"#,
    );
    let effects = go_effects(&src);
    assert_eq!(
        effects,
        vec![serde_json::json!([
            "if",
            ["==", "@entity.id", "@entity.id"],
            expected_toast("success", "ok"),
            expected_toast("error", "no"),
        ])]
    );
}

#[test]
fn notify_unknown_level_is_a_lowering_error() {
    let src = wrap(
        r#"    state idle {
      GO -> idle
        (notify "Cancelled" info)
    }"#,
    );
    let err = lower_err(&src);
    assert!(
        err.contains("ELOLO_NOTIFY_INVALID_LEVEL"),
        "expected ELOLO_NOTIFY_INVALID_LEVEL, got: {err}"
    );
    assert!(err.contains("Cancelled"), "expected the bad level quoted: {err}");
}

#[test]
fn notify_wrong_arity_is_a_lowering_error() {
    let src = wrap(
        r#"    state idle {
      GO -> idle
        (notify "only one arg")
    }"#,
    );
    let err = lower_err(&src);
    assert!(
        err.contains("ELOLO_NOTIFY_ARITY"),
        "expected ELOLO_NOTIFY_ARITY, got: {err}"
    );
}

#[test]
fn notify_dynamic_level_binding_is_left_unresolved() {
    let src = wrap(
        r#"    state idle {
      GO -> idle
        (notify @config.severity @config.template)
    }
    config {
      severity : string = "info"
      template : string = ""
    }"#,
    );
    let effects = go_effects(&src);
    assert_eq!(
        effects,
        vec![expected_toast("@config.severity", "@config.template")]
    );
}
