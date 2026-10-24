//! Arabic and Slovenian renderings of `tests/fixtures/02-multi-state-machine.lolo`
//! — every keyword/tag/category/effect/type word translated, user
//! identifiers (orbital/trait/entity/event/state names, string literals)
//! left native — lower to the same `.orb` as an equivalent English
//! rendering, after canonicalizing away the V4 identity fields (ids/ledger/
//! schemaVersion — freshly minted per lowering, not semantic content).
//!
//! Both fixtures add an `app`/`تطبيق`/`aplikacija` header, since language
//! selection (`Lang::for_lolo_first_token`) reads the program's first
//! token: the original fixture has no header (opens directly with
//! `orbital ...`), and a headerless file's first token is always the
//! `orbital` keyword itself, never `app` — so a headerless file can never
//! be detected as non-English. The English comparison source below carries
//! the same header for a fair diff.
//!
//! The Arabic fixture additionally renames the entity (`Order` ->
//! `طلب`) and one event (`CONFIRM` -> `تأكيد`) into Arabic script, proving
//! user identifiers translate untouched and caseless-script event names
//! parse (`is_event_name`'s non-ASCII-alphabetic first-char rule) — the
//! English comparison source uses the SAME renamed identifiers so the
//! diff isolates vocabulary translation from identifier choice.

use orbital_lolo::{parse_and_lower_str, parse_str};
use serde_json::Value;
use std::path::PathBuf;

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/i18n")
}

/// Drop V4 identity fields (opaque per-lowering ids + ledger +
/// schemaVersion — minted fresh each independent lowering) before
/// comparing two lowerings for semantic equality. Mirrors
/// `tests/formatter.rs`'s `canonicalize` (duplicated here: integration
/// test binaries don't share modules).
fn canonicalize(value: Value) -> Value {
    fn is_identity_key(k: &str) -> bool {
        k == "id"
            || k == "ledger"
            || k == "schemaVersion"
            || k.ends_with("Id")
            || k.ends_with("Ids")
    }
    match value {
        Value::Object(map) => {
            let mut sorted: std::collections::BTreeMap<String, Value> =
                std::collections::BTreeMap::new();
            for (k, v) in map {
                if is_identity_key(&k) {
                    continue;
                }
                sorted.insert(k, canonicalize(v));
            }
            let mut new_map = serde_json::Map::new();
            for (k, v) in sorted {
                new_map.insert(k, v);
            }
            Value::Object(new_map)
        }
        Value::Array(items) => Value::Array(items.into_iter().map(canonicalize).collect()),
        v => v,
    }
}

fn lower_canonical(source: &str) -> Value {
    let schema = parse_and_lower_str(source).unwrap_or_else(|e| panic!("lower failed: {e}"));
    let json = serde_json::to_value(&schema).expect("serialize schema");
    canonicalize(json)
}

/// English rendering of the Slovenian fixture: same header, same
/// identifiers, English vocabulary throughout.
const EXPECTED_SL_SOURCE: &str = r#"app OrderApp
orbital OrderOrbital {
  entity Order [persistent: orders] {
    id : string!
    status : string
  }
  trait OrderLifecycle -> Order {
    initial: pending
    state pending {
      CONFIRM -> confirmed
        (set @status "confirmed")
      CANCEL -> cancelled
        (set @status "cancelled")
    }
    state confirmed {
      SHIP -> shipped
        (set @status "shipped")
    }
    state shipped {
      DELIVER -> delivered
        (set @status "delivered")
    }
    state delivered {
    }
    state cancelled {
    }
  }
}
"#;

/// English rendering of the Arabic fixture: same header, English
/// vocabulary, but the SAME renamed identifiers the Arabic fixture uses
/// (entity `طلب`, event `تأكيد`) — isolates vocabulary translation from
/// identifier choice in the diff.
const EXPECTED_AR_SOURCE: &str = r#"app OrderApp
orbital OrderOrbital {
  entity طلب [persistent: orders] {
    id : string!
    status : string
  }
  trait OrderLifecycle -> طلب {
    initial: pending
    state pending {
      تأكيد -> confirmed
        (set @status "confirmed")
      CANCEL -> cancelled
        (set @status "cancelled")
    }
    state confirmed {
      SHIP -> shipped
        (set @status "shipped")
    }
    state shipped {
      DELIVER -> delivered
        (set @status "delivered")
    }
    state delivered {
    }
    state cancelled {
    }
  }
}
"#;

#[test]
fn slovenian_fixture_lowers_like_english() {
    let path = fixtures_dir().join("02-multi-state-machine.sl.lolo");
    let source = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path:?}: {e}"));

    assert_eq!(orbital_lolo::detect_language(&source), "sl");

    let got = lower_canonical(&source);
    let expected = lower_canonical(EXPECTED_SL_SOURCE);
    assert_eq!(
        got, expected,
        "Slovenian fixture's lowering must equal the English rendering's lowering"
    );
}

#[test]
fn arabic_fixture_lowers_like_english_with_same_renamed_identifiers() {
    let path = fixtures_dir().join("02-multi-state-machine.ar.lolo");
    let source = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path:?}: {e}"));

    assert_eq!(orbital_lolo::detect_language(&source), "ar");

    let got = lower_canonical(&source);
    let expected = lower_canonical(EXPECTED_AR_SOURCE);
    assert_eq!(
        got, expected,
        "Arabic fixture's lowering must equal the English-with-same-identifiers rendering's lowering"
    );
}

/// A headerless `.lolo` file (no `app` line) is still detected: the first
/// token is one of the top-level keywords (`orbital` here), and every one of
/// them selects the language, not only `app`.
#[test]
fn headerless_lolo_is_detected_from_its_first_top_level_keyword() {
    let source = "مدار OrderOrbital {\n  كيان Order [وقت_التشغيل] {\n    id : نص!\n  }\n}\n";
    assert_eq!(orbital_lolo::detect_language(source), "ar");
    let program = parse_str(source).expect("headerless Arabic program parses");
    assert_eq!(program.orbitals.len(), 1);
}

const EN_ENDPOINT_SOURCE: &str = r#"app Srv [version: "1.0.0"]
orbital Srv {
  entity Req [runtime] {
    x : number
  }
  trait Router -> Req [client] {
    initial: idle
    state idle {
      PING -> idle
        (set @entity.x ?x)
    }
    listens {
      PING -> input {
        x : number
      }
    }
  }
  trait Worker -> Req [optimistic] {
    initial: idle
    state idle {
      INIT -> idle
    }
  }
  trait Mirror -> Req [server] {
    initial: idle
    state idle {
      INIT -> idle
    }
  }
  page http { host: "0.0.0.0", port: 8080, path: "/" } as Api -> Router
  page stdio as Mcp -> Router
  page boot as Background -> Worker
}
"#;

const AR_ENDPOINT_SOURCE: &str = r#"تطبيق Srv [إصدار: "1.0.0"]
مدار Srv {
  كيان Req [وقت_التشغيل] {
    x : رقم
  }
  سمة Router -> Req [عميل] {
    أولي: idle
    حالة idle {
      PING -> idle
        (تعيين @entity.x ?x)
    }
    يستمع {
      PING -> مدخل {
        x : رقم
      }
    }
  }
  سمة Worker -> Req [متفائل] {
    أولي: idle
    حالة idle {
      INIT -> idle
    }
  }
  سمة Mirror -> Req [خادم] {
    أولي: idle
    حالة idle {
      INIT -> idle
    }
  }
  صفحة ويب { host: "0.0.0.0", port: 8080, path: "/" } كـ Api -> Router
  صفحة دخل_وخرج كـ Mcp -> Router
  صفحة إقلاع كـ Background -> Worker
}
"#;

const SL_ENDPOINT_SOURCE: &str = r#"aplikacija Srv [razlicica: "1.0.0"]
orbitala Srv {
  entiteta Req [izvajanje] {
    x : stevilo
  }
  znacilnost Router -> Req [odjemalec] {
    zacetno: idle
    stanje idle {
      PING -> idle
        (nastavi @entity.x ?x)
    }
    poslusa {
      PING -> vhod {
        x : stevilo
      }
    }
  }
  znacilnost Worker -> Req [optimisticno] {
    zacetno: idle
    stanje idle {
      INIT -> idle
    }
  }
  znacilnost Mirror -> Req [streznik] {
    zacetno: idle
    stanje idle {
      INIT -> idle
    }
  }
  stran splet { host: "0.0.0.0", port: 8080, path: "/" } kot Api -> Router
  stran vhod_izhod kot Mcp -> Router
  stran zagon kot Background -> Worker
}
"#;

#[test]
fn endpoint_words_lower_like_english_in_arabic_and_slovenian() {
    let expected = lower_canonical(EN_ENDPOINT_SOURCE);
    for (code, source) in [("ar", AR_ENDPOINT_SOURCE), ("sl", SL_ENDPOINT_SOURCE)] {
        assert_eq!(orbital_lolo::detect_language(source), code);
        assert_eq!(lower_canonical(source), expected, "{code}");
    }
}

#[test]
fn the_localized_server_word_lowers_to_the_explicit_server_mode() {
    use orbital_core_base::schema::{DispatchMode, TraitReference};
    for source in [EN_ENDPOINT_SOURCE, AR_ENDPOINT_SOURCE, SL_ENDPOINT_SOURCE] {
        let schema = parse_and_lower_str(source).expect("lower");
        let mirror = schema.orbitals[0]
            .traits
            .iter()
            .find_map(|t| match t {
                TraitReference::Inline(td) if td.name == "Mirror" => Some(td.dispatch),
                _ => None,
            })
            .expect("Mirror trait");
        assert_eq!(mirror, Some(DispatchMode::Server));
    }
}

#[test]
fn control_a_localized_dispatch_word_changes_the_lowering() {
    let plain = AR_ENDPOINT_SOURCE.replace(" [عميل]", "");
    assert_ne!(lower_canonical(&plain), lower_canonical(AR_ENDPOINT_SOURCE));
    let plain = SL_ENDPOINT_SOURCE.replace(" [odjemalec]", "");
    assert_ne!(lower_canonical(&plain), lower_canonical(SL_ENDPOINT_SOURCE));
}
