//! Phase-2 identity stamping: mint ids for authored nodes and resolve in-file
//! name references to id edges, on the already-lowered `OrbitalSchema`.
//!
//! Two invariants drive the design:
//!
//! - **Zero churn.** Given a `prior` schema, every node's id is looked up by
//!   `(kind, bakedName, parentId)` in the prior ledger and REUSED verbatim;
//!   only genuinely new nodes mint fresh. Re-lowering the same `.lolo` with its
//!   own prior `.orb` therefore produces byte-identical ids.
//! - **Dual-carry.** Names still carry every edge; the id sibling is populated
//!   only when the target resolves in-file. Cross-file / alias references
//!   (`Alias.traits.X`, `Alias.entity`) leave the id `None`.
//!
//! Hoisted render-embed synthetics (parser `is_inline_synthetic` traits) get a
//! DERIVED id rooted in the embedding parent (`derive_id`) instead of an
//! entropy mint, so the embed edge is recoverable from the id and re-derivation
//! never churns.

use orbital_core::identity::{
    derive_id, mint_id, IdKind, IdentityLedger, LedgerEntry, LedgerOwner,
};
use orbital_core::schema::{
    EntityRef, FieldDefinition, ListenSource, Orbital, OrbitalSchema, PageRef, SExpression,
    TraitDefinition, TraitReference,
};
use std::collections::{HashMap, HashSet};

/// The `.orb` identity schema version stamped once ids are minted.
const SCHEMA_VERSION: u32 = 1;

/// Mint ids across `schema` and populate the dual-carry id fields + ledger.
/// `prior` is the previously-emitted schema for this source (if any); its
/// ledger anchors id reuse. `synthetic_traits` is the set of trait names the
/// parser hoisted from inline `@(...)` / JSX refs (their id derives from the
/// embedding parent, never entropy).
pub(crate) fn stamp_identity(
    schema: &mut OrbitalSchema,
    prior: Option<&OrbitalSchema>,
    synthetic_traits: &HashSet<String>,
) {
    let mut minter = Minter::new(prior.and_then(|p| p.ledger.as_ref()));

    for orbital in &mut schema.orbitals {
        stamp_orbital(orbital, synthetic_traits, &mut minter);
    }

    schema.ledger = Some(IdentityLedger {
        schema_version: SCHEMA_VERSION,
        entries: minter.entries,
    });
    schema.schema_version = Some(SCHEMA_VERSION);
}

/// Id allocator: reuses prior ids by `(kind, bakedName, parentId)`, mints fresh
/// otherwise, and accumulates the emitted ledger.
struct Minter<'a> {
    /// `(kind, bakedName, parentId) -> id`, built from the prior ledger.
    prior_by_key: HashMap<(IdKind, String, Option<String>), String>,
    prior_ledger: Option<&'a IdentityLedger>,
    entropy: Box<dyn FnMut(usize) -> Vec<u8>>,
    entries: HashMap<String, LedgerEntry>,
}

impl<'a> Minter<'a> {
    fn new(prior_ledger: Option<&'a IdentityLedger>) -> Self {
        let mut prior_by_key = HashMap::new();
        if let Some(ledger) = prior_ledger {
            for entry in ledger.entries.values() {
                prior_by_key.insert(
                    (entry.kind, entry.baked_name.clone(), entry.parent.clone()),
                    entry.id.clone(),
                );
            }
        }
        Minter {
            prior_by_key,
            prior_ledger,
            entropy: Box::new(orbital_core::identity::std_entropy()),
            entries: HashMap::new(),
        }
    }

    /// Reuse a prior id for `(kind, baked, parent)` if one exists, copying its
    /// ledger row verbatim (preserving `curName`/`renames`). Returns `None` on
    /// a genuinely new node.
    fn reuse(&mut self, kind: IdKind, baked: &str, parent: Option<&str>) -> Option<String> {
        let key = (kind, baked.to_string(), parent.map(str::to_string));
        let id = self.prior_by_key.get(&key)?.clone();
        if let Some(entry) = self.prior_ledger.and_then(|l| l.entries.get(&id)) {
            self.entries.insert(id.clone(), entry.clone());
        }
        Some(id)
    }

    fn record(&mut self, id: String, kind: IdKind, baked: &str, parent: Option<&str>) {
        self.entries.insert(
            id.clone(),
            LedgerEntry {
                id,
                kind,
                baked_name: baked.to_string(),
                cur_name: baked.to_string(),
                renames: Vec::new(),
                owner: LedgerOwner::Workspace,
                parent: parent.map(str::to_string),
            },
        );
    }

    /// Mint (or reuse) a fresh entropy id for a declaration node.
    fn mint(&mut self, kind: IdKind, baked: &str, parent: Option<&str>) -> String {
        if let Some(id) = self.reuse(kind, baked, parent) {
            return id;
        }
        let id = mint_id(kind, &mut self.entropy);
        self.record(id.clone(), kind, baked, parent);
        id
    }

    /// Derive (or reuse) a parent-rooted id for a hoisted synthetic node.
    fn derive(&mut self, kind: IdKind, baked: &str, parent_id: &str) -> String {
        if let Some(id) = self.reuse(kind, baked, None) {
            return id;
        }
        let id = derive_id(parent_id, baked);
        self.record(id.clone(), kind, baked, Some(parent_id));
        id
    }
}

/// Per-orbital resolution index built during declaration minting.
#[derive(Default)]
struct OrbitalIds {
    /// Entity name -> id (primary + auxiliary, in this orbital's scope).
    entities: HashMap<String, String>,
    /// Trait name -> id (inline traits declared in this orbital).
    traits: HashMap<String, String>,
    /// trait id -> (event name -> event id).
    trait_events: HashMap<String, HashMap<String, String>>,
    /// Event name -> some event id (any owner; best-effort cross-trait resolve).
    events_any: HashMap<String, String>,
}

fn stamp_orbital(orbital: &mut Orbital, synthetic_traits: &HashSet<String>, minter: &mut Minter) {
    orbital.id = Some(minter.mint(IdKind::Orbital, &orbital.name, None));

    let mut ids = OrbitalIds::default();

    // Entities (primary + auxiliary).
    if let EntityRef::Inline(entity) = &mut orbital.entity {
        let id = minter.mint(IdKind::Entity, &entity.name, None);
        ids.entities.insert(entity.name.clone(), id.clone());
        entity.id = Some(id);
    }
    if let Some(aux) = orbital.auxiliary_entities.as_mut() {
        for a in aux.iter_mut() {
            if let EntityRef::Inline(entity) = a {
                let id = minter.mint(IdKind::Entity, &entity.name, None);
                ids.entities.insert(entity.name.clone(), id.clone());
                entity.id = Some(id);
            }
        }
    }

    // Traits — mint declaration ids in dependency order so a hoisted synthetic
    // derives from its (already-minted) embedding parent. Discover the embed
    // edges by scanning each inline trait's render effects for `@trait.<child>`
    // strings that name a synthetic sibling.
    let parent_of = embed_parents(orbital, synthetic_traits);
    mint_trait_ids(orbital, synthetic_traits, &parent_of, &mut ids, minter);

    // Per-trait event ids (parent = the owning trait id).
    for tr in &mut orbital.traits {
        if let TraitReference::Inline(td) = tr {
            let Some(trait_id) = td.id.clone() else {
                continue;
            };
            let mut event_ids: HashMap<String, String> = HashMap::new();
            for ev in td.state_machine.events.iter().flatten() {
                let id = minter.mint(IdKind::Event, &ev.key, Some(&trait_id));
                ids.events_any.entry(ev.key.clone()).or_insert(id.clone());
                event_ids.insert(ev.key.clone(), id);
            }
            ids.trait_events.insert(trait_id, event_ids);
        }
    }

    // Pages.
    for page in orbital.pages.iter_mut().flatten() {
        if let PageRef::Inline(p) = page {
            p.id = Some(minter.mint(IdKind::Page, &p.name, None));
        }
    }

    // Reference edges (dual-carry id siblings).
    resolve_orbital_edges(orbital, &ids);
}

/// Map each hoisted-synthetic trait name to its unique embedding parent trait
/// name, discovered from `@trait.<synthetic>` strings inside parent render
/// effects. A synthetic referenced by 0 or >1 parents is left unmapped (it then
/// mints normally rather than deriving from an ambiguous root).
fn embed_parents(orbital: &Orbital, synthetic_traits: &HashSet<String>) -> HashMap<String, String> {
    let mut candidates: HashMap<String, Vec<String>> = HashMap::new();
    for tr in &orbital.traits {
        let TraitReference::Inline(td) = tr else {
            continue;
        };
        let mut refs: HashSet<String> = HashSet::new();
        for transition in &td.state_machine.transitions {
            for effect in transition.effects.iter().flatten() {
                collect_trait_refs(effect, &mut refs);
            }
        }
        for tick in td.ticks.iter().flatten() {
            for effect in &tick.effects {
                collect_trait_refs(effect, &mut refs);
            }
        }
        for child in refs {
            if synthetic_traits.contains(&child) && child != td.name {
                candidates.entry(child).or_default().push(td.name.clone());
            }
        }
    }
    candidates
        .into_iter()
        .filter_map(|(child, parents)| (parents.len() == 1).then(|| (child, parents[0].clone())))
        .collect()
}

/// Collect bare `@trait.<Name>` sibling references (no dotted alias path) from
/// an S-expression tree.
fn collect_trait_refs(expr: &SExpression, out: &mut HashSet<String>) {
    match expr {
        SExpression::String(s) => {
            if let Some(name) = s.strip_prefix("@trait.") {
                if !name.is_empty() && !name.contains('.') {
                    out.insert(name.to_string());
                }
            }
        }
        SExpression::Array(items) => items.iter().for_each(|i| collect_trait_refs(i, out)),
        SExpression::Object(map) => map.values().for_each(|v| collect_trait_refs(v, out)),
        _ => {}
    }
}

/// Mint trait declaration ids, deriving hoisted synthetics from their parent
/// once the parent id is known (iterative fixpoint over the embed edges).
fn mint_trait_ids(
    orbital: &mut Orbital,
    synthetic_traits: &HashSet<String>,
    parent_of: &HashMap<String, String>,
    ids: &mut OrbitalIds,
    minter: &mut Minter,
) {
    let mut pending: Vec<usize> = orbital
        .traits
        .iter()
        .enumerate()
        .filter_map(|(i, tr)| matches!(tr, TraitReference::Inline(_)).then_some(i))
        .collect();

    loop {
        let mut progressed = false;
        let mut still_pending = Vec::new();
        for idx in pending {
            let name = match &orbital.traits[idx] {
                TraitReference::Inline(td) => td.name.clone(),
                _ => continue,
            };
            let derive_parent = synthetic_traits
                .contains(&name)
                .then(|| parent_of.get(&name))
                .flatten();
            let id = match derive_parent {
                Some(parent_name) => match ids.traits.get(parent_name) {
                    Some(parent_id) => minter.derive(IdKind::Trait, &name, &parent_id.clone()),
                    None => {
                        // Parent not minted yet; defer this round.
                        still_pending.push(idx);
                        continue;
                    }
                },
                None => minter.mint(IdKind::Trait, &name, None),
            };
            if let TraitReference::Inline(td) = &mut orbital.traits[idx] {
                td.id = Some(id.clone());
            }
            ids.traits.insert(name, id);
            progressed = true;
        }
        if still_pending.is_empty() {
            break;
        }
        if !progressed {
            // Cycle / missing parent — mint the remainder normally so no node
            // is left id-less.
            for idx in &still_pending {
                if let TraitReference::Inline(td) = &orbital.traits[*idx] {
                    let name = td.name.clone();
                    let id = minter.mint(IdKind::Trait, &name, None);
                    if let TraitReference::Inline(td) = &mut orbital.traits[*idx] {
                        td.id = Some(id.clone());
                    }
                    ids.traits.insert(name, id);
                }
            }
            break;
        }
        pending = still_pending;
    }
}

/// Populate the dual-carry id siblings on every in-file reference surface.
fn resolve_orbital_edges(orbital: &mut Orbital, ids: &OrbitalIds) {
    // Extends form on the primary entity (`{ extends: "Alias.entity" }`) — only
    // resolves for a bare in-file entity name (dotted alias paths stay None).
    if let EntityRef::Extends(e) = &mut orbital.entity {
        e.extends_id = ids.entities.get(&e.reference).cloned();
    }

    // Entity relation targets (recurse into nested array/object fields).
    if let EntityRef::Inline(entity) = &mut orbital.entity {
        for f in &mut entity.fields {
            resolve_field_relations(f, ids);
        }
    }

    for tr in orbital.traits.iter_mut() {
        resolve_trait_ref_edges(tr, ids);
    }

    for page in orbital.pages.iter_mut().flatten() {
        match page {
            PageRef::Inline(p) => {
                // `PageDefinition` carries no `primaryEntityId` sibling yet
                // (Phase-1 note: page-level primary entity is un-id-carried on
                // both paths); dual-carry keeps the name edge until core adds it.
                if let Some(traits) = p.traits.as_mut() {
                    for tr in traits {
                        resolve_trait_ref_edges(tr, ids);
                    }
                }
            }
            PageRef::Object(o) => {
                o.linked_entity_id = o
                    .linked_entity
                    .as_ref()
                    .and_then(|n| ids.entities.get(n).cloned());
                if let Some(traits) = o.traits.as_mut() {
                    let mut ref_ids = Vec::with_capacity(traits.len());
                    for tr in traits.iter_mut() {
                        resolve_trait_ref_edges(tr, ids);
                        ref_ids.push(trait_ref_id_of(tr, ids).unwrap_or_default());
                    }
                    if ref_ids.iter().any(|s| !s.is_empty()) {
                        o.trait_ref_ids = Some(ref_ids);
                    }
                }
            }
            PageRef::Reference(_) => {}
        }
    }
}

/// Resolve a single relation field's `entity_id`, recursing into array items
/// and object properties.
fn resolve_field_relations(field: &mut FieldDefinition, ids: &OrbitalIds) {
    if let Some(rel) = field.relation.as_mut() {
        rel.entity_id = ids.entities.get(&rel.entity).cloned();
    }
    if let Some(items) = field.items.as_mut() {
        resolve_field_relations(items, ids);
    }
    if let Some(props) = field.properties.as_mut() {
        for p in props.values_mut() {
            resolve_field_relations(p, ids);
        }
    }
}

/// The trait-id an in-file reference resolves to (for a page's `trait_ref_ids`
/// mirror). `None` for cross-file/alias refs.
fn trait_ref_id_of(tr: &TraitReference, ids: &OrbitalIds) -> Option<String> {
    match tr {
        TraitReference::Inline(td) => td.id.clone(),
        TraitReference::Reference { reference, .. } => reference
            .as_ref()
            .filter(|r| !r.contains('.'))
            .and_then(|r| ids.traits.get(r).cloned()),
    }
}

fn resolve_trait_ref_edges(tr: &mut TraitReference, ids: &OrbitalIds) {
    match tr {
        TraitReference::Inline(td) => resolve_inline_trait_edges(td, ids),
        TraitReference::Reference {
            reference,
            ref_id,
            linked_entity,
            linked_entity_id,
            ..
        } => {
            // Bare in-file trait name resolves (page `-> TraitName`); dotted
            // `Alias.traits.X` import paths stay None.
            *ref_id = reference
                .as_ref()
                .filter(|r| !r.contains('.'))
                .and_then(|r| ids.traits.get(r).cloned());
            *linked_entity_id = linked_entity
                .as_ref()
                .and_then(|n| ids.entities.get(n).cloned());
        }
    }
}

fn resolve_inline_trait_edges(td: &mut TraitDefinition, ids: &OrbitalIds) {
    td.linked_entity_id = td
        .linked_entity
        .as_ref()
        .and_then(|n| ids.entities.get(n).cloned());

    let own_events = td.id.as_ref().and_then(|id| ids.trait_events.get(id));

    if let Some(emits) = td.emits.as_mut() {
        for e in emits {
            e.event_id = own_events.and_then(|m| m.get(&e.event).cloned());
        }
    }
    if let Some(listens) = td.listens.as_mut() {
        for l in listens {
            // The listened-for event belongs to a source trait (cross-orbital);
            // resolve best-effort against any same-named event in this file.
            l.event_id = ids.events_any.get(&l.event).cloned();
            // The internal event a listen triggers is this trait's own event.
            if let Some(trigger) = &l.triggers {
                l.triggers_id = own_events
                    .and_then(|m| m.get(trigger).cloned())
                    .or_else(|| ids.events_any.get(trigger).cloned());
            }
            if let Some(source) = l.source.as_mut() {
                resolve_listen_source(source, ids);
            }
        }
    }

    // V4 event-dispatch layer: stamp each declared event's own id and each
    // transition's triggering-event id from this trait's minted event ids. The
    // transition→event edge is the id-keyed dispatch target the runtimes match
    // on (id-primary, name fallback) once names are stripped.
    if let Some(oe) = own_events {
        for ev in td.state_machine.events.iter_mut().flatten() {
            ev.id = oe.get(&ev.key).cloned();
        }
        for t in td.state_machine.transitions.iter_mut() {
            t.event_id = oe.get(&t.event).cloned();
        }
    }
}

fn resolve_listen_source(source: &mut ListenSource, ids: &OrbitalIds) {
    match source {
        ListenSource::Trait {
            trait_name,
            trait_id,
        } => {
            *trait_id = ids.traits.get(trait_name).cloned();
        }
        ListenSource::Orbital {
            orbital: _,
            orbital_id: _,
            trait_name,
            trait_id,
        } => {
            // Cross-orbital source trait: resolve in-file if the named trait is
            // local; orbital_id resolution is a cross-file concern (left None).
            *trait_id = ids.traits.get(trait_name).cloned();
        }
        ListenSource::Any => {}
    }
}
