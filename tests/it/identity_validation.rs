//! Rabit V4 Phase 3 end-to-end: a real `.lolo` lowered through Phase-2 id
//! stamping validates clean under the `ORB_ID_*` rule family, and corrupting a
//! single dual-carry edge makes the matching rule fire.

use orbital_compiler::validate_comprehensive_without_services;
use orbital_core_base::schema::{OrbitalSchema, TraitReference};
use orbital_lolo::parse_and_lower_str;

const FIXTURE: &str = r#"
    orbital Ledger {
        entity Account {
            id : string!
            balance : number = 0
        }
        trait Deposit -> Account {
            initial: idle
            state idle {
                DEPOSIT -> idle
                    (set @entity.balance 1)
                    (emit DEPOSITED { id: "x" })
            }
            emits {
                DEPOSITED { id : string! }
            }
        }
        page "/ledger" -> Deposit
    }
"#;

fn id_diagnostics(schema: &OrbitalSchema) -> Vec<String> {
    validate_comprehensive_without_services(schema)
        .all_issues()
        .filter(|e| e.code.category() == "identity")
        .map(|e| format!("{}: {}", e.code, e.message))
        .collect()
}

#[test]
fn stamped_lolo_validates_clean() {
    let schema = parse_and_lower_str(FIXTURE).expect("parse+lower");
    assert_eq!(schema.schema_version, Some(1), "ids stamped by Phase 2");
    let diags = id_diagnostics(&schema);
    assert!(
        diags.is_empty(),
        "healthy stamped .orb must be ORB_ID_* clean: {diags:?}"
    );
}

// P-009: the dual-carry rules (ORB_ID_UNKNOWN_REF and siblings) are the std lint std-orb-lint-id-refs, whose coverage is
// lint-id-refs.replay.goldens.json in almadar-std (this case: `unknown-ref-dangling-linked-entity`); the compiler no
// longer reports them.
#[test]
fn corrupting_a_dual_carry_edge_is_no_longer_a_compiler_diagnostic() {
    let mut schema = parse_and_lower_str(FIXTURE).expect("parse+lower");
    let td = schema.orbitals[0]
        .traits
        .iter_mut()
        .find_map(|t| match t {
            TraitReference::Inline(td) if td.name == "Deposit" => Some(td),
            _ => None,
        })
        .expect("Deposit trait");
    std::sync::Arc::make_mut(td).linked_entity_id = Some("ent_GHOST00000000000000000000".to_string());

    let diags = id_diagnostics(&schema);
    assert!(
        !diags.iter().any(|d| d.starts_with("ORB_ID_UNKNOWN_REF")),
        "ORB_ID_UNKNOWN_REF is owned by the std lint: {diags:?}"
    );
}
