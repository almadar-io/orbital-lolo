//! Phase-2 keystone regression: id minting is stable across re-lowers.
//!
//! The contract: re-lowering the same `.lolo` with the PRIOR `.orb` schema as
//! `prior` reuses every id verbatim (zero churn); adding a node mints only that
//! node's ids fresh and leaves every pre-existing id untouched.

use orbital_core::identity::IdKind;
use orbital_core::schema::{EntityRef, OrbitalSchema, PageRef, TraitReference};
use orbital_lolo::{lower_to_orb_with_prior, parse_str};
use std::collections::BTreeMap;

const FIXTURE: &str = r#"
    orbital Ledger {
        entity Account {
            id : string!
            balance : number = 0
        }
        trait Deposit -> Account [interaction, instance] {
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

/// A fixture with one extra trait appended (`Withdraw`) — every prior node is
/// unchanged, only the new trait + its events are added.
const FIXTURE_PLUS_TRAIT: &str = r#"
    orbital Ledger {
        entity Account {
            id : string!
            balance : number = 0
        }
        trait Deposit -> Account [interaction, instance] {
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
        trait Withdraw -> Account [interaction, instance] {
            initial: idle
            state idle {
                WITHDRAW -> idle
                    (set @entity.balance 0)
                    (emit WITHDRAWN { id: "y" })
            }
            emits {
                WITHDRAWN { id : string! }
            }
        }
        page "/ledger" -> Deposit
    }
"#;

fn lower(src: &str, prior: Option<&OrbitalSchema>) -> OrbitalSchema {
    let ast = parse_str(src).expect("parse");
    lower_to_orb_with_prior(&ast, prior).expect("lower")
}

/// Collect every id in the schema keyed by a stable structural coordinate, so
/// two lowerings can be compared node-for-node.
fn id_map(schema: &OrbitalSchema) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for orb in &schema.orbitals {
        if let Some(id) = &orb.id {
            out.insert(format!("orbital:{}", orb.name), id.clone());
        }
        if let EntityRef::Inline(e) = &orb.entity {
            if let Some(id) = &e.id {
                out.insert(format!("entity:{}", e.name), id.clone());
            }
        }
        for tr in &orb.traits {
            if let TraitReference::Inline(td) = tr {
                if let Some(id) = &td.id {
                    out.insert(format!("trait:{}", td.name), id.clone());
                }
                for ev in td.state_machine.events.iter().flatten() {
                    if let Some(m) = schema
                        .ledger
                        .as_ref()
                        .and_then(|l| l.entries.values().find(|e| {
                            e.kind == IdKind::Event
                                && e.baked_name == ev.key
                                && e.parent.as_deref() == td.id.as_deref()
                        }))
                    {
                        out.insert(format!("event:{}:{}", td.name, ev.key), m.id.clone());
                    }
                }
            }
        }
        for page in orb.pages.iter().flatten() {
            if let PageRef::Inline(p) = page {
                if let Some(id) = &p.id {
                    out.insert(format!("page:{}", p.name), id.clone());
                }
            }
        }
    }
    out
}

#[test]
fn ids_are_stable_when_re_lowering_with_prior() {
    let first = lower(FIXTURE, None);
    let first_ids = id_map(&first);
    assert!(!first_ids.is_empty(), "expected minted ids on first lower");
    assert_eq!(first.schema_version, Some(1));
    assert!(first.ledger.is_some(), "ledger emitted");

    // Re-lower the SAME source, feeding the first output as prior.
    let second = lower(FIXTURE, Some(&first));
    let second_ids = id_map(&second);

    assert_eq!(
        first_ids, second_ids,
        "re-lowering the same .lolo with prior must reuse every id verbatim"
    );
}

#[test]
fn adding_a_trait_keeps_prior_ids_and_mints_only_the_new_node() {
    let first = lower(FIXTURE, None);
    let first_ids = id_map(&first);

    // Modify the fixture (add trait Withdraw), re-lower with the original as prior.
    let second = lower(FIXTURE_PLUS_TRAIT, Some(&first));
    let second_ids = id_map(&second);

    // Every prior id is unchanged.
    for (k, v) in &first_ids {
        assert_eq!(
            second_ids.get(k),
            Some(v),
            "prior node {k} must keep its id after adding a sibling trait"
        );
    }

    // Exactly the new trait + its event id are fresh (not present before).
    let fresh: Vec<&String> = second_ids
        .keys()
        .filter(|k| !first_ids.contains_key(*k))
        .collect();
    let mut fresh_sorted = fresh.clone();
    fresh_sorted.sort();
    assert_eq!(
        fresh_sorted,
        vec![
            &"event:Withdraw:WITHDRAW".to_string(),
            &"event:Withdraw:WITHDRAWN".to_string(),
            &"trait:Withdraw".to_string(),
        ],
        "only the added trait and its two events should mint fresh ids"
    );
}

#[test]
fn reference_edges_resolve_to_in_file_ids() {
    let schema = lower(FIXTURE, None);
    let orb = &schema.orbitals[0];
    let entity_id = match &orb.entity {
        EntityRef::Inline(e) => e.id.clone().unwrap(),
        _ => panic!("inline entity"),
    };
    let deposit = orb
        .traits
        .iter()
        .find_map(|t| match t {
            TraitReference::Inline(td) if td.name == "Deposit" => Some(td),
            _ => None,
        })
        .unwrap();

    // Trait -> entity edge carries the entity id.
    assert_eq!(deposit.linked_entity_id.as_ref(), Some(&entity_id));
    // Emit event edge resolves within the trait's own event namespace.
    let emit = &deposit.emits.as_ref().unwrap()[0];
    assert!(
        emit.event_id.as_deref().map(|id| id.starts_with("evt_")) == Some(true),
        "emit event_id resolved: {:?}",
        emit.event_id
    );
    // Page trait reference resolves to the in-file trait id.
    if let PageRef::Inline(p) = &orb.pages.as_ref().unwrap()[0] {
        let tr = &p.traits.as_ref().unwrap()[0];
        if let TraitReference::Reference { ref_id, .. } = tr {
            assert_eq!(ref_id.as_ref(), deposit.id.as_ref());
        } else {
            panic!("page trait should be a reference");
        }
    }
}
