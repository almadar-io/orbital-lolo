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

/// The lowerer bakes each `(xml/parse s Shape)` call's list paths from the shape; the source
/// spelling has only the shape.
fn strip_baked_list_paths(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Array(items) => {
            if items.len() == 4 && items[0].as_str() == Some("xml/parse") {
                items.truncate(3);
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
    let schema = &without_namespace(schema)?;
    let app = AppHeader {
        name: schema.name.clone(),
        version: schema.version.clone(),
        description: schema.description.clone(),
        theme: schema.theme.as_ref().map(theme::theme_decl).transpose()?,
        themes: schema.themes.clone().unwrap_or_default(),
        locales: schema.locales.clone().unwrap_or_default(),
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
    let type_aliases = hoist_common_aliases(&mut orbitals, &app, &global);
    Ok(Program {
        app,
        type_aliases,
        uses,
        orbitals,
        comments: Vec::new(),
    })
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
) -> Vec<crate::ast::TypeAlias> {
    let mut hoisted: Vec<crate::ast::TypeAlias> = Vec::new();
    if orbitals.len() > 1 || !app.config.is_empty() {
        if let Some(first) = orbitals.iter().find(|o| o.reference.is_none()) {
            for a in &first.type_aliases {
                let everywhere = orbitals.iter().all(|o| {
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
    let mut used = std::collections::BTreeSet::new();
    for cf in &app.config {
        orbital::collect_named(&cf.type_expr, &mut used);
    }
    for n in used {
        let declared = hoisted.iter().any(|h| h.name == n);
        if !declared
            && !global.entities.contains(&n)
            && crate::lower::primitive_field_type(&n).is_none()
        {
            hoisted.push(crate::ast::TypeAlias {
                name: n,
                type_param: None,
                type_param_kind: None,
                body: crate::ast::TypeBody::Alias(crate::ast::TypeExpr::Primitive(
                    "string".to_string(),
                )),
                span: Span::dummy(),
            });
        }
    }
    hoisted
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
