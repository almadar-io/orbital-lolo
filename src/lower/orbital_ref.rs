//! Lower an `orbital Name = Alias.orbitals.Upstream { ... }` reference
//! declaration into the `.orb` PLACEHOLDER shape (orbital-import Stage A,
//! `docs/Almadar_Orbital_Import.md` §4). This is NOT the resolved orbital —
//! it carries `OrbitalRefObject` plus a synthetic `entity` reference so the
//! schema is well-formed JSON, and `orbital-compiler`'s inline phase (a
//! later increment) is the pass that actually splices the upstream
//! organism's traits/pages/entity in and clears `reference` back to `None`.
//! Every field but `traits` on the produced `CoreOrbital` stays empty/
//! default — there is nothing here yet for validation, codegen, or the
//! runtime to see beyond "this orbital still needs resolving". `traits`
//! (Stage B) carries any sibling traits authored in the reference body —
//! those are locally owned, never prefixed, and lower now.

use super::*;

/// `Vec<(String, String)>` → `Option<HashMap<String, String>>`, `None` when
/// empty — the same "absent beats empty" convention every other optional
/// map on the schema follows.
fn pairs_to_map(pairs: &[(String, String)]) -> Option<HashMap<String, String>> {
    if pairs.is_empty() {
        None
    } else {
        Some(pairs.iter().cloned().collect())
    }
}

/// `Vec<String>` → `Option<Vec<String>>`, `None` when empty.
fn names_to_option(names: &[String]) -> Option<Vec<String>> {
    if names.is_empty() {
        None
    } else {
        Some(names.to_vec())
    }
}

/// `Vec<(String, Vec<String>)>` → `Option<HashMap<String, Vec<String>>>`,
/// `None` when empty — the list-valued sibling of `pairs_to_map` (roles,
/// mounts).
fn list_pairs_to_map(pairs: &[(String, Vec<String>)]) -> Option<HashMap<String, Vec<String>>> {
    if pairs.is_empty() {
        None
    } else {
        Some(pairs.iter().cloned().collect())
    }
}

pub(crate) fn lower_orbital_reference(
    orb: &Orbital,
    file_uses: &[UseDecl],
    aliases: &BTreeMap<String, TypeAliasResolved>,
    entity_names: &HashSet<String>,
    entities: &BTreeMap<String, Vec<(String, TypeExpr, bool)>>,
    event_aliases: &BTreeMap<String, EventTypeResolved>,
    shared_entity_names: &HashSet<String>,
    source_map: &mut crate::source_map::LoloSourceMap,
    base: &str,
) -> Result<CoreOrbital, LowerError> {
    // Dispatch in `lower_orbital` already checked `orb.reference.is_some()`.
    let reference = orb
        .reference
        .clone()
        .expect("lower_orbital_reference called without a reference");

    let merged_uses = merge_uses(file_uses, &orb.uses, aliases, entity_names)?;
    let uses = if merged_uses.is_empty() {
        None
    } else {
        Some(merged_uses)
    };

    // Override site, not a declaration — the config a caller sets in an
    // orbital-reference's `config {}` block overrides the UPSTREAM orbital's
    // own declared knobs; the self-reference guard is the declaration-site
    // rule and does not apply to an override.
    let config_overrides = lower_config_fields_to_map(
        &orb.config_overrides,
        &orb.name,
        false,
        &[],
        aliases,
        entity_names,
    )?;

    let ref_object = OrbitalRefObject {
        reference: reference.clone(),
        ref_id: None,
        entity: orb.entity_rebind.clone(),
        fields: pairs_to_map(&orb.fields_remap),
        pages: pairs_to_map(&orb.pages_remap),
        omit: names_to_option(&orb.omit),
        only: names_to_option(&orb.only),
        config: config_overrides,
        events: pairs_to_map(&orb.events_remap),
        roles: list_pairs_to_map(&orb.roles_remap),
        entities: pairs_to_map(&orb.entities_remap),
        mounts: list_pairs_to_map(&orb.mounts),
    };

    // Sibling traits (Stage B): authored locally in the reference body,
    // never prefixed, lowered through the same path an inline orbital's own
    // traits take. Their bound entity lives upstream — unknown at this
    // pre-inline stage — so no intrinsic fields are excluded here; the
    // compiler splice re-derives the real set once the upstream entity is
    // known.
    let no_intrinsics: HashSet<String> = HashSet::new();
    let mut traits = Vec::new();
    for (trait_index, t) in orb.traits.iter().enumerate() {
        let trait_base = format!("{base}.traits[{trait_index}]");
        traits.push(lower_trait_decl(
            t,
            aliases,
            entities,
            entity_names,
            event_aliases,
            &no_intrinsics,
            shared_entity_names,
            source_map,
            &trait_base,
        )?);
    }

    Ok(CoreOrbital {
        name: orb.name.clone(),
        id: None,
        uses,
        expects: None,
        theme: None,
        services: None,
        // Placeholder — a synthetic reference so the schema stays
        // well-formed JSON between lowering and inline. Never resolved by
        // anything downstream of inline; inline replaces the WHOLE orbital.
        entity: EntityRef::Reference(format!("{reference}.entity")),
        auxiliary_entities: None,
        traits,
        pages: None,
        emits: None,
        listens: None,
        exposes: None,
        reference: Some(ref_object),
        // The reference form declares no knobs of its own — its declared
        // surface lives upstream; `reference.config` above is the override
        // of THAT surface.
        config: None,
        domain_context: None,
        design: None,
        types: None,
    })
}
