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
use crate::error::Located;

/// `Vec<(String, String, Span)>` → `Option<HashMap<String, String>>`, `None`
/// when empty — the same "absent beats empty" convention every other
/// optional map on the schema follows. The span is dropped here; callers
/// that need it record it into the source map separately (spans are a
/// side-channel, never part of the emitted `.orb`).
fn pairs_to_map(pairs: &[(String, String, Span)]) -> Option<HashMap<String, String>> {
    if pairs.is_empty() {
        None
    } else {
        Some(
            pairs
                .iter()
                .map(|(k, v, _)| (k.clone(), v.clone()))
                .collect(),
        )
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

/// `Vec<(String, Span)>` → `Option<Vec<String>>`, `None` when empty — the
/// span-carrying sibling of `names_to_option` (`omit {}`).
fn spanned_names_to_option(names: &[(String, Span)]) -> Option<Vec<String>> {
    if names.is_empty() {
        None
    } else {
        Some(names.iter().map(|(n, _)| n.clone()).collect())
    }
}

/// `Vec<(String, Vec<String>, Span)>` → `Option<HashMap<String,
/// Vec<String>>>`, `None` when empty — the list-valued sibling of
/// `pairs_to_map` (roles, mounts).
fn list_pairs_to_map(
    pairs: &[(String, Vec<String>, Span)],
) -> Option<HashMap<String, Vec<String>>> {
    if pairs.is_empty() {
        None
    } else {
        Some(
            pairs
                .iter()
                .map(|(k, v, _)| (k.clone(), v.clone()))
                .collect(),
        )
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

    // `.lolo` spans (Stage C): the orbital's own overall span, the fallback
    // every per-key insert below strips down to (`LoloSourceMap`'s
    // longest-prefix rule) when a diagnostic's path has no exact key match
    // (e.g. `ORB_O_ENTITY_UNMAPPED`'s `site` suffix, which names a
    // trait/aux-entity, not a call-site key).
    source_map.insert(base.to_string(), orb.span);

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

    // `.lolo` spans (Stage C): record each override key's span so the L2
    // `ORB_O_*`/`ORB_U_*` validators (`orbital_ref_config.rs`,
    // the std `orbital-ref-extend` lint) resolve back to the offending `.lolo`
    // KEY, not just the orbital's own span. The suffix shape MUST match
    // the paren-style path `orbital_ref_config.rs` already builds from
    // `diag.path` (`{path}.config(key)`, `{path}.roles(key)`, …) — see
    // `Almadar_LOLO.md` §19 / the program report for why (parens, not
    // dots: `LoloSourceMap::normalize` only touches `[...]` brackets, so
    // the stored key must be byte-identical to the looked-up path).
    for (k, _, span) in &orb.fields_remap {
        source_map.insert(format!("{base}.fields({k})"), *span);
    }
    for (k, _, span) in &orb.entities_remap {
        source_map.insert(format!("{base}.entities({k})"), *span);
    }
    for (k, _, span) in &orb.roles_remap {
        source_map.insert(format!("{base}.roles({k})"), *span);
    }
    for (k, _, span) in &orb.pages_remap {
        source_map.insert(format!("{base}.pages({k})"), *span);
    }
    for (k, _, span) in &orb.mounts {
        source_map.insert(format!("{base}.pages({k})"), *span);
    }
    for (k, span) in &orb.omit {
        source_map.insert(format!("{base}.omit({k})"), *span);
    }
    for cf in &orb.config_overrides {
        source_map.insert(format!("{base}.config({})", cf.name), cf.span);
    }
    for f in &orb.extend_fields {
        source_map.insert(format!("{base}.extend({})", f.name), f.span);
    }
    for (field, _, span) in &orb.mock_overrides {
        source_map.insert(format!("{base}.mock({field})"), *span);
    }
    for f in &orb.retype_fields {
        source_map.insert(format!("{base}.retype({})", f.name), f.span);
    }
    for t in &orb.trait_overrides {
        source_map.insert(format!("{base}.traits({})", t.trait_name), t.span);
        for cf in &t.body.config {
            source_map.insert(
                format!("{base}.traits({}).config({})", t.trait_name, cf.name),
                cf.span,
            );
        }
    }
    // `uses A from "…" { config { … } }` — a per-orbital `uses` body's own
    // override keys (`ORB_U_CONFIG_UNKNOWN_KEY`'s `{path}.uses(alias).config(key)`
    // suffix). File-scope `uses` (declared outside any orbital, shared by
    // every orbital in the file) has no single owning orbital span and is
    // left untracked — a lookup for it falls back to this orbital's own
    // span via `LoloSourceMap`'s longest-prefix rule.
    for u in &orb.uses {
        for cf in &u.config {
            source_map.insert(
                format!("{base}.uses({}).config({})", u.alias, cf.name),
                cf.span,
            );
        }
    }

    // Stage C: `extend { … }` — full entity-field declarations appended to
    // the materialized primary entity (`resolve_orbital_reference`), AFTER
    // `fields {}` renames apply. Lowered through the SAME `lower_field` an
    // `entity { … }` body's own fields use, so a relation-typed added field
    // (e.g. `workspaceId: Workspace!`) resolves its target against `entity_names`
    // — the CONSUMER's own scope (this file's entities), never the upstream's.
    let extend: Option<Vec<FieldDefinition>> = if orb.extend_fields.is_empty() {
        None
    } else {
        let mut fields = Vec::with_capacity(orb.extend_fields.len());
        for f in &orb.extend_fields {
            fields.push(lower_field(f, aliases, entity_names, &[])?);
        }
        Some(fields)
    };

    let retype: Option<Vec<FieldDefinition>> = if orb.retype_fields.is_empty() {
        None
    } else {
        let mut fields = Vec::with_capacity(orb.retype_fields.len());
        for f in &orb.retype_fields {
            fields.push(lower_field(f, aliases, entity_names, &[])?);
        }
        Some(fields)
    };

    let traits: Option<HashMap<String, orbital_core_base::schema::OrbitalRefTraitOverride>> =
        if orb.trait_overrides.is_empty() {
            None
        } else {
            let mut map = std::collections::HashMap::new();
            for t in &orb.trait_overrides {
                if let Some(word) = t.retired_words.first() {
                    return Err(crate::error::trait_header_word_error(&word.word)
                        .located_exact(word.span));
                }
                let ref_data = TraitRefData {
                    reference: t.trait_name.clone(),
                    name_override: None,
                    events: t.body.events.clone(),
                    fields: t.body.fields.clone(),
                    config: t.body.config.clone(),
                    emits_scope: t.body.emits_scope.clone(),
                };
                let lowered = super::r#trait::lower_call_site_overrides(
                    &ref_data,
                    &t.body.listens,
                    aliases,
                    entity_names,
                )?;
                map.insert(
                    t.trait_name.clone(),
                    orbital_core_base::schema::OrbitalRefTraitOverride {
                        linked_entity: t.linked_entity.clone(),
                        config: lowered.config,
                        events: lowered.events,
                        fields: lowered.fields,
                        listens: lowered.listens,
                        emits_scope: lowered.emits_scope,
                        dispatch: t.dispatch.map(super::r#trait::lower_dispatch),
                    },
                );
            }
            Some(map)
        };

    let rebind_tags = orb.entity_rebind_tags.clone().unwrap_or_default();
    if let Some(word) = rebind_tags.retired_words.first() {
        return Err(crate::error::entity_header_word_error(&word.word).located_exact(word.span));
    }
    super::r#trait::check_listens_spelling(&orb.import_listens)?;
    let ref_object = OrbitalRefObject {
        page_modifiers: (!orb.page_modifiers.is_empty()).then(|| {
            orb.page_modifiers
                .iter()
                .map(|(path, mods)| (path.clone(), mods.clone()))
                .collect()
        }),
        reference: reference.clone(),
        ref_id: None,
        entity: orb.entity_rebind.clone(),
        fields: pairs_to_map(&orb.fields_remap),
        pages: pairs_to_map(&orb.pages_remap),
        omit: spanned_names_to_option(&orb.omit),
        only: names_to_option(&orb.only),
        config: config_overrides,
        events: if orb.events_remap.is_empty() {
            None
        } else {
            Some(orb.events_remap.iter().cloned().collect())
        },
        roles: list_pairs_to_map(&orb.roles_remap),
        entities: pairs_to_map(&orb.entities_remap),
        mounts: list_pairs_to_map(&orb.mounts),
        extend,
        mock: pairs_to_map(&orb.mock_overrides),
        retype,
        persistence: rebind_tags.persistence.as_ref().map(|p| match p {
            crate::ast::Persistence::Persistent => {
                orbital_core_base::schema::Persistence::Persistent
            }
            crate::ast::Persistence::Runtime => orbital_core_base::schema::Persistence::Runtime,
        }),
        collection: rebind_tags.collection.clone(),
        shared: rebind_tags.shared,
        identity: rebind_tags.identity,
        browser: rebind_tags.browser,
        traits,
        listens: if orb.import_listens.is_empty() {
            None
        } else {
            Some(
                orb.import_listens
                    .iter()
                    .map(|l| orbital_core_base::schema::OrbitalRefListen {
                        trait_name: l.target_trait.clone().unwrap_or_default(),
                        listen: crate::lower::emit_listen::lower_listen(l),
                    })
                    .collect(),
            )
        },
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
        traits.push(
            lower_trait_decl(
                t,
                aliases,
                entities,
                entity_names,
                event_aliases,
                &no_intrinsics,
                shared_entity_names,
                source_map,
                &trait_base,
            )
            .located(t.span)?,
        );
    }

    Ok(CoreOrbital {
        name: orb.name.clone(),
        id: None,
        uses,
        spec: false,
        expects: if orb.expects.is_empty() {
            None
        } else {
            Some(
                orb.expects
                    .iter()
                    .map(|e| super::lower_expects_decl(e, false, aliases, entities, entity_names))
                    .collect::<Result<Vec<_>, _>>()?,
            )
        },
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
        os_grants: (!orb.os_grants.is_empty()).then(|| orb.os_grants.clone()),
        design: None,
        types: None,
    })
}
