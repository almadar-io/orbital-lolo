//! A `let` binding or lambda parameter referenced without its `@` sigil is a
//! parse error.
//!
//! Both evaluators resolve lexical bindings only through the sigil
//! (`SExpressionEvaluator.ts` gates on `isBinding`; `orbital-core`'s
//! `eval_expr` does the same), so a bare occurrence evaluates to its own name
//! as a *string*. That is silent: `orb validate`, `orb verify`, `runtime-verify`
//! and the wiring lint were all green on `ui-debugger-board`, whose `CHECK`
//! guard filtered on `(object/get l isBug)` and therefore always saw an empty
//! list and always fired.
//!
//! The check lives in the parser because it is the only layer that can still
//! tell a bare identifier from a quoted string — both lower to
//! `SExpression::String`, so an `.orb`-level check could not distinguish
//! `(fn hero … hero)` from the legitimate literal `{ unitId: "hero" }`.

use orbital_lolo::{format_program, parse_str};

fn program(effect: &str) -> String {
    format!(
        r#"app probe "1.0.0"
"bare-binding probe"

orbital ProbeOrbital {{
  entity Probe [runtime] {{
    total : number = 0
    items : [any] = []
    status : "idle" = idle
  }}

  trait ProbeRun -> Probe [lifecycle, instance] {{
    initial: idle

    state idle {{
      INIT -> idle
        (set @entity.status idle)
      RUN -> idle
        {effect}
    }}
  }}

  page "/probe" as ProbePage -> ProbeRun
}}
"#
    )
}

fn parse_err(effect: &str) -> String {
    match parse_str(&program(effect)) {
        Ok(_) => panic!("expected a parse error for effect: {effect}"),
        Err(e) => format!("{e:?}"),
    }
}

fn parse_ok(effect: &str) {
    if let Err(e) = parse_str(&program(effect)) {
        panic!("expected effect to parse: {effect}\n  got: {e:?}");
    }
}

#[test]
fn lambda_param_referenced_bare_is_rejected() {
    let err = parse_err("(set @entity.total (array/len (array/filter @entity.items (fn i (object/get i flagged)))))");
    assert!(
        err.contains("BareBindingReference") && err.contains("\"i\""),
        "expected a BareBindingReference for `i`, got: {err}"
    );
}

#[test]
fn lambda_param_referenced_with_sigil_is_accepted() {
    parse_ok("(set @entity.total (array/len (array/filter @entity.items (fn i (object/get @i flagged)))))");
}

#[test]
fn let_binding_referenced_bare_is_rejected() {
    let err = parse_err("(let ((doubled (* @entity.total 2))) (set @entity.total doubled))");
    assert!(
        err.contains("BareBindingReference") && err.contains("\"doubled\""),
        "expected a BareBindingReference for `doubled`, got: {err}"
    );
}

#[test]
fn let_is_sequential_so_an_earlier_binding_is_in_scope_for_a_later_value() {
    let err = parse_err("(let ((a (* @entity.total 2)) (b (+ a 1))) (set @entity.total @b))");
    assert!(
        err.contains("BareBindingReference") && err.contains("\"a\""),
        "expected a BareBindingReference for `a` in the later binding's value, got: {err}"
    );
}

#[test]
fn a_name_bound_nowhere_stays_a_plain_atom() {
    // `flagged` is an object key, not a binding — bare atoms remain the
    // ordinary way to write a field name or an enum member.
    parse_ok("(set @entity.total (array/len (array/filter @entity.items (fn i (object/get @i flagged)))))");
}

#[test]
fn an_out_of_scope_binding_name_is_not_flagged() {
    // `i` is bound only inside the first lambda; the sibling lambda's bare
    // `i` names nothing and stays a literal.
    parse_ok(
        "(set @entity.total (+ (array/len (array/filter @entity.items (fn i (object/get @i flagged)))) (array/len (array/filter @entity.items (fn j (== (object/get @j kind) i))))))",
    );
}

#[test]
fn quoting_is_the_escape_hatch_for_a_colliding_literal() {
    // `(object/get @w "w")` in std-algo-mst: the wedge's weight field happens
    // to share the lambda parameter's name. Quoted, it is unambiguous.
    parse_ok("(set @entity.total (array/len (array/filter @entity.items (fn w (> (object/get @w \"w\") 0)))))");
}

#[test]
fn multi_param_lambda_binds_every_parameter() {
    let err = parse_err("(set @entity.total (array/reduce @entity.items 0 (fn acc it (+ acc (object/get @it qty)))))");
    assert!(
        err.contains("BareBindingReference") && err.contains("\"acc\""),
        "expected a BareBindingReference for `acc`, got: {err}"
    );
    parse_ok("(set @entity.total (array/reduce @entity.items 0 (fn acc it (+ @acc (object/get @it qty)))))");
}

#[test]
fn formatter_quotes_a_bound_name_so_its_output_reparses() {
    // The formatter prints bare atoms unquoted, which would re-read as the
    // rejected bare form. Quoting is value-preserving (`x` and `"x"` both
    // evaluate to the string `"x"`), so it can quote unconditionally when the
    // atom names an in-scope binding.
    let source =
        program("(set @entity.total (array/len (array/filter @entity.items (fn w (> (object/get @w \"w\") 0)))))");
    let ast = parse_str(&source).expect("source parses");
    let formatted = format_program(&ast);
    assert!(
        formatted.contains("\"w\""),
        "formatter must keep the colliding literal quoted:\n{formatted}"
    );
    parse_str(&formatted).expect("formatter output must re-parse");
}

#[test]
fn type_key_value_is_a_pattern_name_not_a_binding_reference() {
    // run 33 (StreakDisplay): `renderItem: (fn card { … type: card … })` —
    // the `type:` value is the render-ui pattern name, so the collision with
    // the lambda parameter is not a bare reference.
    parse_ok(
        "(render-ui main { renderItem: (fn card { children: [{ type: card title: @card.title }] }) })",
    );
}

#[test]
fn sibling_keys_still_reject_a_bare_bound_name() {
    let err = parse_err(
        "(render-ui main { renderItem: (fn card { children: [{ type: card title: card }] }) })",
    );
    assert!(
        err.contains("BareBindingReference") && err.contains("\"card\""),
        "expected a BareBindingReference for the non-type key, got: {err}"
    );
}

#[test]
fn type_key_exemption_does_not_leak_into_nested_expressions() {
    let err =
        parse_err("(render-ui main { renderItem: (fn card { children: [{ type: (do card) }] }) })");
    assert!(
        err.contains("BareBindingReference") && err.contains("\"card\""),
        "a bare bound name nested under a type: value must still error, got: {err}"
    );
}

fn lowered_fetch_filter(filter: &str) -> String {
    let source = format!(
        r#"app keyprobe
orbital KeyOrbital {{
  entity Thing [persistent: things] {{
    id : string!
    state : string = "a"
    now : number = 0
  }}
  trait KeyRun -> Thing [lifecycle, instance] {{
    initial: idle
    state idle {{
      INIT -> idle
        (fetch Thing {{ filter: {filter}, emit: {{ success: LOADED, failure: FAILED }} }})
    }}
  }}
}}"#
    );
    let orb = orbital_lolo::parse_and_lower_str(&source).unwrap();
    serde_json::to_value(&orb).unwrap()["orbitals"][0]["traits"][0]["stateMachine"]["transitions"][0]["effects"][0].to_string()
}

#[test]
fn a_bare_field_name_in_a_key_position_stays_a_literal_key() {
    for field in ["state", "now"] {
        let lowered = lowered_fetch_filter(&format!(r#"(= (object/get @entity {field}) "x")"#));
        assert!(lowered.contains(&format!(r#"["object/get","@entity","{field}"]"#)), "{field}: {lowered}");
    }
    let lowered = lowered_fetch_filter(r#"(= (object/has @entity state) true)"#);
    assert!(lowered.contains(r#"["object/has","@entity","state"]"#), "{lowered}");
}

#[test]
fn control_a_bare_state_in_a_value_position_is_still_the_binding() {
    let lowered = lowered_fetch_filter(r#"(= (object/get @entity id) state)"#);
    assert!(lowered.contains(r#""@state""#), "{lowered}");
    let explicit = lowered_fetch_filter(r#"(= (object/get @entity @state) "x")"#);
    assert!(explicit.contains(r#"["object/get","@entity","@state"]"#), "{explicit}");
}

#[test]
fn a_backslash_dot_in_a_key_string_reaches_the_path_splitter_as_backslash_dot() {
    let lowered = lowered_fetch_filter(r#"(= (object/get @entity "http\.method") "x")"#);
    assert!(lowered.contains(r#"["object/get","@entity","http\\.method"]"#), "{lowered}");
    let doubled = lowered_fetch_filter(r#"(= (object/get @entity "a\\.b") "x")"#);
    assert!(doubled.contains(r#"["object/get","@entity","a\\.b"]"#), "control: `\\\\.` lexes to the same backslash-dot: {doubled}");
}

fn lowered_set_value(value: &str) -> serde_json::Value {
    let source = program(&format!("(set @entity.items {value})"))
        .replace("items : [any] = []", "items : [string] = []\n    found : boolean = false")
        .replace("@entity.items (array/includes", "@entity.found (array/includes");
    let orb = orbital_lolo::parse_and_lower_str(&source).unwrap();
    let json = serde_json::to_value(&orb).unwrap();
    let transitions = json["orbitals"][0]["traits"][0]["stateMachine"]["transitions"].as_array().unwrap().clone();
    let run = transitions.iter().find(|t| t["event"] == "RUN").expect("RUN transition");
    run["effects"][0][2].clone()
}

#[test]
fn a_bracket_headed_by_an_operator_word_is_data() {
    let value = lowered_set_value(r#"(array/includes ["path", "size"] "size")"#);
    let sexpr: orbital_core_base::schema::SExpression = serde_json::from_value(value.clone()).unwrap();
    let got = orbital_core_eval::evaluator::operators::eval_expr(&sexpr, &orbital_core_eval::evaluator::BindingContext::new());
    assert!(matches!(got, Ok(orbital_core_base::value::Value::Bool(true))), "lowered: {value}, got: {got:?}");
}

#[test]
fn control_a_bracket_without_an_operator_head_stays_a_plain_array() {
    assert_eq!(lowered_set_value(r#"["idle", "walk"]"#), serde_json::json!(["idle", "walk"]));
}

#[test]
fn a_quoted_template_keeps_its_brackets_as_json() {
    let value = lowered_set_value(r#"(quote ["=", a, 0])"#).to_string();
    assert!(!value.contains("list"), "{value}");
}

#[test]
fn an_old_form_call_in_a_guard_is_a_type_error() {
    let source = program("(set @entity.total 1)").replace("RUN -> idle", r#"RUN -> idle when ["=", @entity.total, 0]"#);
    let err = format!("{:?}", parse_str(&source).expect_err("list in a guard"));
    assert!(err.contains("ELOLO_LIST_IN_BOOLEAN_SLOT"), "{err}");
    let nested = program("(set @entity.total 1)").replace("RUN -> idle", r#"RUN -> idle when (and true ["=", @entity.total, 0])"#);
    assert!(format!("{:?}", parse_str(&nested).expect_err("nested")).contains("ELOLO_LIST_IN_BOOLEAN_SLOT"));
    let ok = program("(set @entity.total 1)").replace("RUN -> idle", r#"RUN -> idle when (= @entity.total 0)"#);
    parse_str(&ok).expect("call-form guard parses");
}

#[test]
fn an_old_form_call_in_a_fetch_filter_is_a_type_error() {
    let source = program(r#"(fetch Probe { filter: ["=", @entity.total, 0], emit: { success: RUN, failure: RUN } })"#);
    let err = format!("{:?}", parse_str(&source).expect_err("list in a filter"));
    assert!(err.contains("ELOLO_LIST_IN_BOOLEAN_SLOT"), "{err}");
}
