use orbital_lolo::{format_program, parse_and_lower_str, parse_str};
use serde_json::Value;

fn canon(v: Value) -> Value {
    // Drop V4 identity fields (opaque per-lowering ids + ledger); this test
    // asserts semantic preservation, not id identity.
    fn is_identity_key(k: &str) -> bool {
        k == "id" || k == "ledger" || k == "schemaVersion" || k.ends_with("Id") || k.ends_with("Ids")
    }
    match v {
        Value::Object(map) => {
            let mut s: std::collections::BTreeMap<String, Value> =
                std::collections::BTreeMap::new();
            for (k, v) in map {
                if is_identity_key(&k) {
                    continue;
                }
                s.insert(k, canon(v));
            }
            let mut out = serde_json::Map::new();
            for (k, v) in s {
                out.insert(k, v);
            }
            Value::Object(out)
        }
        Value::Array(items) => Value::Array(items.into_iter().map(canon).collect()),
        o => o,
    }
}
fn strip_nulls(v: Value) -> Value {
    match v {
        Value::Object(map) => {
            let mut out = serde_json::Map::new();
            for (k, v) in map {
                let s = strip_nulls(v);
                if !s.is_null() {
                    out.insert(k, s);
                }
            }
            Value::Object(out)
        }
        Value::Array(items) => Value::Array(items.into_iter().map(strip_nulls).collect()),
        o => o,
    }
}

#[test]
fn jsx_authored_file_formats_and_roundtrips() {
    let jsx_src = r#"orbital BillOrbital {
  entity Bill [runtime] {
    id : string!
    total : number
  }
  trait BillView -> Bill [interaction] {
    state idle {
      INIT -> idle
        (render-ui main
          <Stack direction="vertical" gap="md">
            <UiButton.traits.ButtonRender linkedEntity="Bill" action={PAY} label="Pay">
              <trait.Icon />
              <trait.Label />
            </UiButton.traits.ButtonRender>
            <Divider />
            <DataGrid entity={@payload.data} cols={4} fields={[{ name: "total", label: "Total" }]} />
          </Stack>)
    }
  }
}
"#;
    // Parse + format (should emit clean JSX).
    let ast = parse_str(jsx_src).expect("parse jsx");
    let formatted = format_program(&ast);
    eprintln!("=== JSX FORMATTED ===\n{formatted}\n=== END ===");
    assert!(formatted.contains("<Stack"), "stack → <Stack>");
    assert!(
        formatted.contains("<UiButton.traits.ButtonRender"),
        "inline ref → JSX"
    );
    assert!(
        formatted.contains("<trait.Icon"),
        "named ref → <trait.Icon />"
    );
    assert!(formatted.contains("fields={"), "complex attr value");

    // Idempotent: re-format produces identical text.
    let ast2 = parse_str(&formatted).expect("re-parse");
    let formatted2 = format_program(&ast2);
    assert_eq!(formatted, formatted2, "formatter must be idempotent on JSX");

    // Semantics preserved across the format round-trip.
    let s1 = parse_and_lower_str(jsx_src).expect("lower jsx");
    let s2 = parse_and_lower_str(&formatted).expect("lower formatted");
    let v1 = canon(strip_nulls(serde_json::to_value(&s1).unwrap()));
    let v2 = canon(strip_nulls(serde_json::to_value(&s2).unwrap()));
    assert_eq!(v1, v2, "JSX round-trip must preserve lowered .orb");
}
