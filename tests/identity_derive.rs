//! Live-behaviors W1: with no prior ledger, ids are DERIVED from the program
//! (its name) and each node's (kind, name, parent) — so lowering the same
//! `.lolo` twice gives the same ids with nothing stored, while a prior ledger
//! is still honored first.

use orbital_core_base::schema::{EntityRef, OrbitalSchema, TraitReference};
use orbital_lolo::{lower_to_orb_with_prior, parse_str};

fn program(app: &str, trait_name: &str) -> String {
    format!(
        r#"
    app {app} [ version: "1.0.0" ]
    orbital Ledger {{
        entity Account {{
            id : string!
            balance : number = 0
        }}
        trait {trait_name} -> Account {{
            initial: idle
            state idle {{
                DEPOSIT -> idle
                    (set @entity.balance 1)
                    (emit DEPOSITED {{ id: "x" }})
            }}
            emits {{
                DEPOSITED {{ id : string! }}
            }}
        }}
        page "/ledger" -> {trait_name}
    }}
"#
    )
}

fn lower(src: &str, prior: Option<&OrbitalSchema>) -> OrbitalSchema {
    lower_to_orb_with_prior(&parse_str(src).expect("parse"), prior).expect("lower")
}

fn ledger_json(schema: &OrbitalSchema) -> String {
    let mut entries: Vec<_> = schema
        .ledger
        .as_ref()
        .expect("ledger")
        .entries
        .values()
        .map(|e| serde_json::to_string(e).unwrap())
        .collect();
    entries.sort();
    entries.join("\n")
}

fn ids(schema: &OrbitalSchema) -> Vec<String> {
    let mut out: Vec<String> = schema
        .ledger
        .as_ref()
        .unwrap()
        .entries
        .keys()
        .cloned()
        .collect();
    out.sort();
    out
}

fn trait_id(schema: &OrbitalSchema) -> String {
    match &schema.orbitals[0].traits[0] {
        TraitReference::Inline(td) => td.id.clone().expect("trait id"),
        other => panic!("expected an inline trait, got {other:?}"),
    }
}

#[test]
fn lowering_twice_without_a_prior_gives_identical_ids_and_ledger() {
    let src = program("ledger-app", "Deposit");
    let a = lower(&src, None);
    let b = lower(&src, None);
    assert_eq!(ledger_json(&a), ledger_json(&b));
    assert_eq!(
        serde_json::to_string(&a).unwrap(),
        serde_json::to_string(&b).unwrap(),
        "the whole lowered program is byte-identical"
    );
}

#[test]
fn programs_with_different_names_never_share_ids() {
    let a = lower(&program("ledger-app", "Deposit"), None);
    let b = lower(&program("other-app", "Deposit"), None);
    let shared: Vec<_> = ids(&a)
        .into_iter()
        .filter(|id| ids(&b).contains(id))
        .collect();
    assert!(
        shared.is_empty(),
        "two behaviors composed into one program must not collide: {shared:?}"
    );
}

#[test]
fn a_renamed_node_gets_a_new_id_and_its_unchanged_siblings_keep_theirs() {
    let a = lower(&program("ledger-app", "Deposit"), None);
    let b = lower(&program("ledger-app", "Payment"), None);
    assert_ne!(trait_id(&a), trait_id(&b));
    let entity = |s: &OrbitalSchema| match &s.orbitals[0].entity {
        EntityRef::Inline(e) => e.id.clone().unwrap(),
        _ => panic!("inline entity"),
    };
    assert_eq!(entity(&a), entity(&b), "the untouched entity keeps its id");
}

#[test]
fn control_a_prior_ledger_is_still_honored_first() {
    let src = program("ledger-app", "Deposit");
    let mut prior = lower(&src, None);
    let old = trait_id(&prior);
    let ledger = prior.ledger.as_mut().unwrap();
    let entry = ledger.entries.remove(&old).unwrap();
    let pinned = "trt_01J0000000000000000000PINN".to_string();
    ledger.entries.insert(
        pinned.clone(),
        orbital_core_base::identity::LedgerEntry {
            id: pinned.clone(),
            ..entry
        },
    );
    let relowered = lower(&src, Some(&prior));
    assert_eq!(
        trait_id(&relowered),
        pinned,
        "the stored id wins over derivation"
    );
}
