//! `.orb` → `.lolo`: rebuild the surface AST from a lowered `OrbitalSchema`
//! and print it with the formatter. Lowering is lossy (sugar, ids, derived
//! data); the contract is IR identity — `lower(print(unlower(S))) == S` modulo
//! identity fields — not source fidelity.

mod orbital;
mod theme;
mod trait_;
mod types;

use crate::ast::{AppHeader, Program, Span};
use orbital_core_base::schema::OrbitalSchema;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("cannot print as .lolo: {0}")]
pub struct UnlowerError(String);

impl UnlowerError {
    pub(crate) fn new(msg: impl Into<String>) -> Self {
        Self(msg.into())
    }
}

fn strip_namespace(value: &mut serde_json::Value, prefix: &str) {
    use orbital_core_base::messages::TRANSLATE_OP;
    match value {
        serde_json::Value::Array(items) => {
            if let [serde_json::Value::String(op), serde_json::Value::String(key), ..] =
                items.as_mut_slice()
            {
                if op == TRANSLATE_OP {
                    if let Some(bare) = key.strip_prefix(prefix) {
                        *key = bare.to_string();
                    }
                }
            }
            for item in items {
                strip_namespace(item, prefix);
            }
        }
        serde_json::Value::Object(map) => {
            for v in map.values_mut() {
                strip_namespace(v, prefix);
            }
        }
        _ => {}
    }
}

/// The lowerer bakes each `(xml/parse s Shape)` call's list paths and each `(tree/fold v step Shape)` call's
/// definition from the shape; the source spelling has only the shape.
fn strip_baked_list_paths(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Array(items) => {
            if items.len() == 4 && items[0].as_str() == Some("xml/parse") {
                items.truncate(3);
            }
            if items.len() == 5 && items[0].as_str() == Some("tree/fold") {
                items.truncate(4);
            }
            for item in items {
                strip_baked_list_paths(item);
            }
        }
        serde_json::Value::Object(map) => {
            for v in map.values_mut() {
                strip_baked_list_paths(v);
            }
        }
        _ => {}
    }
}

/// The lowerer replaces a `(let ((x : T v)) …)` binding's type with its declared shape text; the source
/// spelling is the type.
fn restore_local_annotations(value: &mut serde_json::Value) -> Result<(), UnlowerError> {
    match value {
        serde_json::Value::Array(items) => {
            if items.len() == 3 && items[0].as_str() == Some("let") {
                if let serde_json::Value::Array(bindings) = &mut items[1] {
                    for binding in bindings {
                        let Some(parts) = binding.as_array_mut() else {
                            continue;
                        };
                        if let [_, _, annotation @ serde_json::Value::String(_)] =
                            parts.as_mut_slice()
                        {
                            let text = annotation.as_str().unwrap_or_default().to_string();
                            *annotation = serde_json::to_value(types::declared_shape_type(&text)?)
                                .map_err(|e| UnlowerError::new(e.to_string()))?;
                        }
                    }
                }
            }
            for item in items {
                restore_local_annotations(item)?;
            }
        }
        serde_json::Value::Object(map) => {
            for v in map.values_mut() {
                restore_local_annotations(v)?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// Each baked `(tree/fold v step Shape definition)` definition: it keeps the by-name back-references the
/// orbital `types` section expands.
fn baked_fold_definitions(
    value: &serde_json::Value,
    out: &mut Vec<orbital_core_base::schema::FieldDefinition>,
) -> Result<(), UnlowerError> {
    match value {
        serde_json::Value::Array(items) => {
            if let [op, _, _, _, serde_json::Value::String(text)] = items.as_slice() {
                if op.as_str() == Some("tree/fold") {
                    out.push(
                        serde_json::from_str(text).map_err(|e| UnlowerError::new(e.to_string()))?,
                    );
                }
            }
            for item in items {
                baked_fold_definitions(item, out)?;
            }
        }
        serde_json::Value::Object(map) => {
            for v in map.values() {
                baked_fold_definitions(v, out)?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// The lowerer qualifies every `(i18n/t "key")` with the program name; the
/// source spelling is the bare key.
fn without_namespace(schema: &OrbitalSchema) -> Result<OrbitalSchema, UnlowerError> {
    let mut value = serde_json::to_value(schema).map_err(|e| UnlowerError::new(e.to_string()))?;
    let prefix = format!(
        "{}{}",
        schema.name,
        orbital_core_base::messages::NAMESPACE_SEPARATOR
    );
    strip_namespace(&mut value, &prefix);
    strip_baked_list_paths(&mut value);
    restore_local_annotations(&mut value)?;
    serde_json::from_value(value).map_err(|e| UnlowerError::new(e.to_string()))
}

/// Rebuild the surface AST for `schema`.
/// One IR transition's place in a printed trait: its index in the state machine and the 1-based
/// lines it spans.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct TransitionLines {
    pub index: usize,
    pub start: usize,
    pub end: usize,
}

/// A trait printed as `.lolo`, with where each of its transitions sits.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct PrintedTrait {
    pub text: String,
    pub transitions: Vec<TransitionLines>,
}

/// Print one trait as orb's formatter writes it, with the lines each IR transition occupies.
pub fn print_trait(
    td: &orbital_core_base::schema::TraitDefinition,
) -> Result<PrintedTrait, UnlowerError> {
    let (ast, _) = trait_::trait_definition(td)?;
    let order = trait_::transition_print_order(td)?;
    let (text, lines) =
        crate::format::format_node_with_transition_lines(crate::format::Node::Trait(&ast));
    if lines.len() != order.len() {
        return Err(UnlowerError::new(format!(
            "printed {} transitions of `{}`, which has {}",
            lines.len(),
            td.name,
            order.len()
        )));
    }
    let transitions = order
        .into_iter()
        .zip(lines)
        .map(|(index, (start, end))| TransitionLines { index, start, end })
        .collect();
    Ok(PrintedTrait { text, transitions })
}

pub fn unlower(schema: &OrbitalSchema) -> Result<Program, UnlowerError> {
    let _contract = types::PrintTypeContract::enter(schema.type_contract_version);
    let mut fold_shapes = Vec::new();
    baked_fold_definitions(
        &serde_json::to_value(schema).map_err(|e| UnlowerError::new(e.to_string()))?,
        &mut fold_shapes,
    )?;
    let schema = &without_namespace(schema)?;
    let app = AppHeader {
        name: schema.name.clone(),
        version: schema.version.clone(),
        description: schema.description.clone(),
        theme: schema.theme.as_ref().map(theme::theme_decl).transpose()?,
        themes: schema.themes.clone().unwrap_or_default(),
        locales: schema.locales.clone().unwrap_or_default(),
        store: schema.store.clone(),
        egress: schema.egress.clone().unwrap_or_default(),
        inbound: schema.inbound.clone().unwrap_or_default(),
        os_grants: schema.os_grants.clone().unwrap_or_default(),
        deployment: schema
            .deployment
            .iter()
            .flatten()
            .map(|(unit, orbitals)| (unit.clone(), orbitals.clone()))
            .collect(),
        public: schema.public_dir.clone(),
        site: schema.site.clone().unwrap_or_default(),
        legacy_lines: false,
        config: types::config_fields(&schema.config)?,
        span: Span::dummy(),
    };
    let mut global = types::AliasShapes::default();
    global
        .known_structs
        .extend(schema.types.iter().flat_map(|types| types.keys().cloned()));
    for def in &fold_shapes {
        types::collect_field_shapes(def, &mut global)?;
    }
    if let Some(cfg) = &schema.config {
        for cf in cfg.values() {
            types::collect_config_shapes(cf, &mut global)?;
        }
    }
    for o in &schema.orbitals {
        for t in &o.traits {
            if let orbital_core_base::schema::TraitReference::Inline(td) = t {
                types::collect_trait_hints(td, &mut global);
            }
        }
        global
            .known_structs
            .extend(o.types.iter().flat_map(|t| t.keys().cloned()));
        for e in std::iter::once(&o.entity).chain(o.auxiliary_entities.iter().flatten()) {
            if let orbital_core_base::schema::EntityRef::Inline(def) = e {
                global.entities.insert(def.name.clone());
            }
        }
    }
    for o in &schema.orbitals {
        let shapes = orbital::alias_shapes(o, &global)?;
        global.structs.extend(shapes.structs);
    }
    let orbitals = schema
        .orbitals
        .iter()
        .map(|o| orbital::orbital(o, &global))
        .collect::<Result<Vec<_>, _>>()?;
    let mut orbitals = orbitals;
    let uses = hoist_common_uses(&mut orbitals, &schema.orbitals)?;
    let mut type_aliases =
        hoist_common_aliases(&mut orbitals, &app, &global, schema.types.as_ref())?;
    let root_aliases = orbital::declared_type_aliases_from(
        schema.types.as_ref(),
        &global,
        &mut std::collections::BTreeMap::new(),
    )?;
    for alias in root_aliases {
        if !type_aliases
            .iter()
            .any(|existing| existing.name == alias.name)
        {
            type_aliases.push(alias);
        }
    }
    // Lowering copies every file-scoped type into each orbital's `types`; a copy identical to the file's prints once.
    if let Some(root) = &schema.types {
        for (ast, core) in orbitals.iter_mut().zip(&schema.orbitals) {
            let Some(local) = &core.types else { continue };
            ast.type_aliases
                .retain(|a| match (root.get(&a.name), local.get(&a.name)) {
                    (Some(r), Some(l)) => {
                        serde_json::to_value(r).ok() != serde_json::to_value(l).ok()
                    }
                    _ => true,
                });
        }
    }
    let program = Program {
        app,
        type_aliases,
        uses,
        orbitals,
        comments: Vec::new(),
    };
    Ok(program)
}

/// File-scoped `uses` lower into every orbital's own list, so the prefix every
/// orbital shares prints once at file scope — the only spelling a reference
/// orbital (`orbital X = A.orbitals.Y`) has for imports.
fn hoist_common_uses(
    orbitals: &mut [crate::ast::Orbital],
    core: &[orbital_core_base::schema::Orbital],
) -> Result<Vec<crate::ast::UseDecl>, UnlowerError> {
    let keys: Vec<Vec<String>> = core
        .iter()
        .map(|o| {
            o.uses
                .iter()
                .flatten()
                .map(|u| {
                    serde_json::to_value(u)
                        .map(|v| orbital_core_eval::util::canonical_json(&strip_ids(v)).to_string())
                        .unwrap_or_default()
                })
                .collect()
        })
        .collect();
    let common = keys
        .iter()
        .map(Vec::len)
        .min()
        .map(|min| {
            (0..min)
                .take_while(|&i| keys.iter().all(|k| k[i] == keys[0][i]))
                .count()
        })
        .unwrap_or(0);
    let hoist_all = orbitals.iter().any(|o| o.reference.is_some());
    let count = if hoist_all || orbitals.len() > 1 {
        common
    } else {
        0
    };
    for (o, ks) in orbitals.iter().zip(&keys) {
        if o.reference.is_some() && ks.len() > count {
            return Err(UnlowerError::new(
                "reference orbital with imports not shared by every orbital",
            ));
        }
    }
    let hoisted: Vec<crate::ast::UseDecl> = orbitals
        .first()
        .map(|o| o.uses.iter().take(count).cloned().collect())
        .unwrap_or_default();
    for o in orbitals.iter_mut() {
        o.uses.drain(..count.min(o.uses.len()));
    }
    Ok(hoisted)
}

/// `.lolo` source for `schema`.
pub fn schema_to_lolo(schema: &OrbitalSchema) -> Result<String, UnlowerError> {
    Ok(crate::format::format_program(&unlower(schema)?))
}

/// File-scoped aliases lower into every orbital's own `types`, so the ones all
/// orbitals share print once at file scope — the only scope the app-level
/// `config` can see.
fn hoist_common_aliases(
    orbitals: &mut [crate::ast::Orbital],
    app: &AppHeader,
    global: &types::AliasShapes,
    file_types: Option<
        &std::collections::BTreeMap<String, Vec<orbital_core_base::schema::PayloadField>>,
    >,
) -> Result<Vec<crate::ast::TypeAlias>, UnlowerError> {
    let mut hoisted: Vec<crate::ast::TypeAlias> = Vec::new();
    if orbitals.len() > 1 || !app.config.is_empty() {
        if let Some(first) = orbitals.iter().find(|o| o.reference.is_none()) {
            for a in &first.type_aliases {
                let file_scoped = file_types.is_some_and(|t| t.contains_key(&a.name));
                let everywhere = file_scoped
                    && orbitals.iter().all(|o| {
                        o.reference.is_some() || o.type_aliases.iter().any(|b| b.name == a.name)
                    });
                if everywhere {
                    hoisted.push(a.clone());
                }
            }
        }
        for o in orbitals.iter_mut() {
            o.type_aliases
                .retain(|b| !hoisted.iter().any(|h| h.name == b.name));
        }
    }
    // A name the app config uses that no inline orbital declares comes back from the config's own recorded shape.
    let mut pending = std::collections::BTreeSet::new();
    for cf in &app.config {
        orbital::collect_named(&cf.type_expr, &mut pending);
    }
    while let Some(n) = pending.pop_first() {
        if hoisted.iter().any(|h| h.name == n)
            || global.entities.contains(&n)
            || crate::lower::primitive_field_type(&n).is_some()
        {
            continue;
        }
        let body = if let Some(members) = global.structs.get(&n) {
            for (_, t, _) in members {
                orbital::collect_named(t, &mut pending);
            }
            crate::ast::TypeBody::Struct(members.clone())
        } else if let Some(variants) = global.unions.get(&n) {
            pending.extend(variants.iter().cloned());
            crate::ast::TypeBody::Enum(variants.clone())
        } else {
            return Err(UnlowerError::new(format!(
                "app config names type `{n}`, which the IR records nowhere"
            )));
        };
        hoisted.push(crate::ast::TypeAlias {
            name: n,
            type_param: None,
            type_param_kind: None,
            body,
            span: Span::dummy(),
        });
    }
    Ok(hoisted)
}

fn strip_ids(value: serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => serde_json::Value::Object(
            map.into_iter()
                .filter(|(k, _)| k != "id" && !k.ends_with("Id") && !k.ends_with("Ids"))
                .map(|(k, v)| (k, strip_ids(v)))
                .collect(),
        ),
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.into_iter().map(strip_ids).collect())
        }
        v => v,
    }
}
