//! Browser-stored entities: `[persistent: x, local]` declares the rows live in
//! the browser, `mock` opts an empty store into the shared mock seeder, and
//! `instances [ … ]` writes out the rows the entity starts with.

use orbital_core::schema::{EntityDefinition, EntityRef, OrbitalSchema, SExpression};
use orbital_lolo::{format_program, parse_and_lower_str, parse_str};

fn primary(schema: &OrbitalSchema) -> &EntityDefinition {
    match &schema.orbitals[0].entity {
        EntityRef::Inline(def) => def,
        other => panic!("inline entity expected, got {other:?}"),
    }
}

fn program(tags: &str, body_extra: &str) -> String {
    format!(
        r#"app fixture "1.0.0"
"Fixture."
orbital InvoiceOrbital {{
  entity Invoice [{tags}] {{
    id : string!
    client : string
    amount : number
    status : "draft" | "sent" | "paid"
{body_extra}
  }}
}}
"#
    )
}

const ROWS: &str = r#"    instances [
      { id: "INV-1042", client: "Noor Logistics", amount: 4200, status: sent },
      { id: "INV-1043", client: "Bled Studio", amount: 980, status: paid }
    ]"#;

#[test]
fn local_lowers_to_a_browser_resident_persistent_entity() {
    let schema = parse_and_lower_str(&program("persistent: invoices, local", "")).unwrap();
    let e = primary(&schema);
    assert!(e.local);
    assert!(!e.seed_mock);
    assert_eq!(e.collection.as_deref(), Some("invoices"));
    assert!(e.is_client_resident());
}

#[test]
fn mock_lowers_to_seed_mock() {
    let schema = parse_and_lower_str(&program("persistent: invoices, local, mock", "")).unwrap();
    assert!(primary(&schema).seed_mock);
}

#[test]
fn instances_lower_to_the_written_rows() {
    let schema = parse_and_lower_str(&program("persistent: invoices, local", ROWS)).unwrap();
    let rows = primary(&schema).instances.as_ref().expect("instances");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].get("id"), Some(&SExpression::String("INV-1042".into())));
    assert_eq!(rows[0].get("amount"), Some(&SExpression::Number(4200.0)));
    assert_eq!(rows[1].get("status"), Some(&SExpression::String("paid".into())));
}

#[test]
fn control_an_entity_without_the_new_tags_or_rows_is_unchanged() {
    let schema = parse_and_lower_str(&program("persistent: invoices", "")).unwrap();
    let e = primary(&schema);
    assert!(!e.local && !e.seed_mock);
    assert!(e.instances.is_none());
    assert!(!e.is_client_resident());
}

#[test]
fn tags_are_order_independent() {
    let a = parse_and_lower_str(&program("local, persistent: invoices", "")).unwrap();
    assert!(primary(&a).local);
}

#[test]
fn duplicate_instances_is_a_parse_error() {
    let err = parse_and_lower_str(&program("persistent: invoices, local", &format!("{ROWS}\n{ROWS}")))
        .unwrap_err()
        .to_string();
    assert!(err.contains("ELOLO_DUPLICATE_INSTANCES"), "{err}");
}

#[test]
fn a_row_that_is_not_an_object_is_a_parse_error() {
    let err = parse_and_lower_str(&program("persistent: invoices, local", "    instances [\"INV-1\"]"))
        .unwrap_err()
        .to_string();
    assert!(err.contains("ELOLO_INSTANCE_NOT_A_ROW"), "{err}");
}

#[test]
fn arabic_and_slovenian_spellings_lower_identically() {
    let ar = "مدار InvoiceOrbital {\n  كيان Invoice [دائم: invoices, محلي, وهمي] {\n    id : نص!\n    amount : رقم\n    نسخ [\n      { id: \"INV-1\", amount: 5 }\n    ]\n  }\n}\n";
    let sl = "orbitala InvoiceOrbital {\n  entiteta Invoice [trajno: invoices, lokalno, lazno] {\n    id : niz!\n    amount : stevilo\n    primerki [\n      { id: \"INV-1\", amount: 5 }\n    ]\n  }\n}\n";
    for source in [ar, sl] {
        let schema = parse_and_lower_str(source).unwrap_or_else(|e| panic!("{e}\n{source}"));
        let e = primary(&schema);
        assert!(e.local && e.seed_mock, "{source}");
        assert_eq!(e.instances.as_ref().map(Vec::len), Some(1), "{source}");
    }
}

#[test]
fn the_formatter_round_trips_tags_and_rows() {
    let source = program("persistent: invoices, local, mock", ROWS);
    let formatted = format_program(&parse_str(&source).unwrap());
    let before = serde_json::to_value(parse_and_lower_str(&source).unwrap()).unwrap();
    let after = serde_json::to_value(parse_and_lower_str(&formatted).unwrap_or_else(|e| panic!("{e}\n{formatted}"))).unwrap();
    let entity = |mut v: serde_json::Value| {
        let mut e = v["orbitals"][0]["entity"].take();
        e.as_object_mut().unwrap().remove("id");
        e
    };
    assert_eq!(entity(before), entity(after), "{formatted}");
}

const LOCALE_ROWS: &str = r#"    instances [
      { id: "INV-1042", client: "Noor Logistics", amount: 4200, status: sent }
    ]
    instances ar [
      { id: "INV-1042", client: "نور للخدمات اللوجستية", amount: 4200, status: sent }
    ]
    instances sl [
      { id: "INV-1042", client: "Noor Logistika", amount: 4200, status: sent }
    ]"#;

#[test]
fn locale_instances_lower_per_locale_beside_the_default_rows() {
    let schema = parse_and_lower_str(&program("persistent: invoices, local", LOCALE_ROWS)).unwrap();
    let e = primary(&schema);
    assert_eq!(e.instances.as_ref().map(Vec::len), Some(1));
    let by_locale = e.locale_instances.as_ref().expect("locale instances");
    assert_eq!(by_locale.keys().cloned().collect::<Vec<_>>(), vec!["ar".to_string(), "sl".to_string()]);
    assert_eq!(by_locale["ar"][0].get("client"), Some(&SExpression::String("نور للخدمات اللوجستية".into())));
}

#[test]
fn control_without_locale_blocks_there_are_no_locale_instances() {
    let schema = parse_and_lower_str(&program("persistent: invoices, local", ROWS)).unwrap();
    assert!(primary(&schema).locale_instances.is_none());
}

#[test]
fn a_locale_block_written_twice_is_a_parse_error() {
    let twice = format!("{LOCALE_ROWS}\n    instances ar [\n      {{ id: \"INV-1042\" }}\n    ]");
    let err = parse_and_lower_str(&program("persistent: invoices, local", &twice)).unwrap_err().to_string();
    assert!(err.contains("ELOLO_DUPLICATE_INSTANCES"), "{err}");
}

#[test]
fn the_formatter_round_trips_locale_blocks() {
    let source = program("persistent: invoices, local", LOCALE_ROWS);
    let formatted = format_program(&parse_str(&source).unwrap());
    assert!(formatted.contains("instances ar ["), "{formatted}");
    let before = serde_json::to_value(parse_and_lower_str(&source).unwrap()).unwrap();
    let after = serde_json::to_value(parse_and_lower_str(&formatted).unwrap_or_else(|e| panic!("{e}\n{formatted}"))).unwrap();
    let entity = |mut v: serde_json::Value| {
        let mut e = v["orbitals"][0]["entity"].take();
        e.as_object_mut().unwrap().remove("id");
        e
    };
    assert_eq!(entity(before), entity(after), "{formatted}");
}
