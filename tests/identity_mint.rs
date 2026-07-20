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
                    if let Some(m) = schema.ledger.as_ref().and_then(|l| {
                        l.entries.values().find(|e| {
                            e.kind == IdKind::Event
                                && e.baked_name == ev.key
                                && e.parent.as_deref() == td.id.as_deref()
                        })
                    }) {
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

/// A fixture exercising the V4 leverage-ids side-maps: `Feed` embeds sibling
/// `Card` (`@trait.Card`) and binds entity `Product`, so its `entity_ref_ids`
/// must carry `Product → <product id>` and its `trait_embed_ids` must carry
/// `Card → <card trait id>`.
const FIXTURE_EMBED: &str = r#"
    orbital Shop {
        entity Product {
            id : string!
            title : string = ""
        }
        trait Card -> Product [interaction, instance] {
            initial: idle
            state idle {
                INIT -> idle
                    (render-ui main { type: badge, label: "x" })
            }
        }
        trait Feed -> Product [interaction, collection] {
            initial: idle
            state idle {
                LOAD -> idle
                    (render-ui main { type: stack, direction: vertical, children: [@trait.Card] })
            }
        }
        page "/shop" -> Feed
    }
"#;

#[test]
fn leverage_id_side_maps_are_minted_and_stable() {
    let first = lower(FIXTURE_EMBED, None);
    let orb = &first.orbitals[0];
    let product_id = match &orb.entity {
        EntityRef::Inline(e) => e.id.clone().expect("product id"),
        _ => panic!("inline entity"),
    };
    let card_id = orb
        .traits
        .iter()
        .find_map(|t| match t {
            TraitReference::Inline(td) if td.name == "Card" => td.id.clone(),
            _ => None,
        })
        .expect("card trait id");
    let feed = orb
        .traits
        .iter()
        .find_map(|t| match t {
            TraitReference::Inline(td) if td.name == "Feed" => Some(td),
            _ => None,
        })
        .expect("feed trait");

    // Entity ref side-map: the bound Product resolves to the entity id.
    assert_eq!(
        feed.entity_ref_ids.as_ref().and_then(|m| m.get("Product")),
        Some(&product_id),
        "Feed.entityRefIds must map Product to its entity id: {:?}",
        feed.entity_ref_ids
    );
    // Trait embed side-map: the `@trait.Card` embed resolves to Card's trait id.
    assert_eq!(
        feed.trait_embed_ids.as_ref().and_then(|m| m.get("Card")),
        Some(&card_id),
        "Feed.traitEmbedIds must map Card to its trait id: {:?}",
        feed.trait_embed_ids
    );

    // Re-lower with prior → side-map ids are reused verbatim (byte-stable).
    let second = lower(FIXTURE_EMBED, Some(&first));
    let feed2 = second.orbitals[0]
        .traits
        .iter()
        .find_map(|t| match t {
            TraitReference::Inline(td) if td.name == "Feed" => Some(td),
            _ => None,
        })
        .expect("feed trait 2");
    assert_eq!(
        feed.entity_ref_ids, feed2.entity_ref_ids,
        "entityRefIds must be stable across re-lower with prior"
    );
    assert_eq!(
        feed.trait_embed_ids, feed2.trait_embed_ids,
        "traitEmbedIds must be stable across re-lower with prior"
    );
}

/// V4-W5: an `entity`-typed config knob whose default names an in-file entity
/// gets a stamped `refId` pointing at that entity's id, so a later rename of the
/// entity declaration flows into the config value through `identity_normalize`.
const FIXTURE_CONFIG_ENTITY: &str = r#"
    orbital Ledger {
        entity Account {
            id : string!
            balance : number = 0
        }
        trait Lifecycle -> Account [interaction, instance] {
            initial: idle
            state idle {
                RUN -> idle
                    (set @entity.balance 1)
            }
            config {
                targetEntity : entity = Account
            }
        }
        page "/ledger" -> Lifecycle
    }
"#;

#[test]
fn config_ref_ids_stamp_entity_typed_knob() {
    let first = lower(FIXTURE_CONFIG_ENTITY, None);
    let orb = &first.orbitals[0];
    let account_id = match &orb.entity {
        EntityRef::Inline(e) => e.id.clone().expect("account id"),
        _ => panic!("inline entity"),
    };
    let lifecycle = orb
        .traits
        .iter()
        .find_map(|t| match t {
            TraitReference::Inline(td) if td.name == "Lifecycle" => Some(td),
            _ => None,
        })
        .expect("lifecycle trait");
    let cf = lifecycle
        .config
        .as_ref()
        .and_then(|c| c.get("targetEntity"))
        .expect("targetEntity config field");
    assert_eq!(cf.field_type, "entity", "config field lowered to entity type");
    assert_eq!(
        cf.ref_id.as_ref(),
        Some(&account_id),
        "targetEntity refId must be stamped with Account's entity id: {:?}",
        cf.ref_id
    );

    // Re-lower with prior → the stamped refId is stable.
    let second = lower(FIXTURE_CONFIG_ENTITY, Some(&first));
    let cf2 = second.orbitals[0]
        .traits
        .iter()
        .find_map(|t| match t {
            TraitReference::Inline(td) if td.name == "Lifecycle" => td.config.as_ref(),
            _ => None,
        })
        .and_then(|c| c.get("targetEntity"))
        .expect("targetEntity config field 2");
    assert_eq!(cf.ref_id, cf2.ref_id, "config refId stable across re-lower");
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

/// Composed-surface backbone: a Reference-form declaration
/// (`trait X = Alias.traits.Y`) gets a LOCAL declaration id + ledger row, and
/// a page trait ref to it resolves its `refId` — so a `name:` override stays
/// declaration-only (FACTORY-PAGE-TRAIT-NAME-COUPLING, battery 2026-07-20).
#[test]
fn reference_form_declarations_mint_local_ids_and_page_refs_resolve() {
    let raw = serde_json::json!({
        "name": "DashboardOrbital",
        "version": "1.0.0",
        "orbitals": [{
            "name": "DashboardOrbital",
            "entity": { "name": "Metric", "fields": [{ "name": "id", "type": "string", "required": true }] },
            "uses": [{ "from": "std/behaviors/std-graphs", "as": "Graphs" }],
            "traits": [
                { "ref": "Graphs.traits.GraphItemGraph", "name": "DefaultRevenueChart" },
                { "ref": "Graphs.traits.GraphItemGraph" }
            ],
            "pages": [{
                "path": "/dash",
                "name": "DashPage",
                "traits": [{ "ref": "DefaultRevenueChart" }, { "ref": "GraphItemGraph" }]
            }]
        }]
    });
    let mut schema: OrbitalSchema = serde_json::from_value(raw).expect("schema parses");
    orbital_core::stamp::stamp_schema_identity(&mut schema, None, &std::collections::HashSet::new());

    let orbital = &schema.orbitals[0];
    // Both Reference-form declarations carry a local declaration id — the
    // named override AND the name-less ref (name derived from the last
    // `.`-segment).
    let decl_ids: Vec<Option<&str>> = orbital.traits.iter().map(|t| t.trait_decl_id()).collect();
    assert!(
        decl_ids.iter().all(|d| d.is_some()),
        "every Reference-form declaration must carry a local decl id: {decl_ids:?}"
    );

    // The ledger backs both rows under the LOCAL names.
    let ledger = schema.ledger.as_ref().expect("ledger present");
    let row_names: Vec<&str> = ledger.entries.values().map(|e| e.baked_name.as_str()).collect();
    assert!(row_names.contains(&"DefaultRevenueChart"), "ledger rows: {row_names:?}");
    assert!(row_names.contains(&"GraphItemGraph"), "ledger rows: {row_names:?}");

    // Page trait refs resolve their refId to the DECLARATION ids.
    let page_traits: &Vec<TraitReference> = match orbital.pages.as_ref().and_then(|p| p.first()) {
        Some(PageRef::Inline(p)) => p.traits.as_ref().expect("page traits"),
        Some(PageRef::Object(o)) => o.traits.as_ref().expect("page traits"),
        other => panic!("expected a page with traits, got {other:?}"),
    };
    let page_ref_ids: Vec<Option<&str>> = page_traits.iter().map(|t| t.trait_ref_id()).collect();
    assert_eq!(page_ref_ids[0], decl_ids[0], "page ref must resolve to the declaration id");
    assert_eq!(page_ref_ids[1], decl_ids[1]);

    // Re-stamp with prior = byte-stable ids (reuse by (kind, bakedName)).
    let prior = schema.clone();
    let mut restamped: OrbitalSchema = serde_json::from_value(serde_json::to_value(&schema).unwrap()).unwrap();
    orbital_core::stamp::stamp_schema_identity(&mut restamped, Some(&prior), &std::collections::HashSet::new());
    assert_eq!(
        restamped.orbitals[0].traits.iter().map(|t| t.trait_decl_id().map(str::to_string)).collect::<Vec<_>>(),
        schema.orbitals[0].traits.iter().map(|t| t.trait_decl_id().map(str::to_string)).collect::<Vec<_>>(),
        "re-stamp with prior must reuse Reference-decl ids verbatim"
    );
}
