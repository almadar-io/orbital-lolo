//! Theme definitions back to the `.lolo` `theme` declaration.

use super::UnlowerError;
use crate::ast::{InlineTheme, ThemeDecl, ThemeTokenMaps, THEME_TYPED_AXIS_KEYS};
use orbital_core::schema::ThemeRef;
use serde_json::Value;

fn string_map(v: Option<&Value>) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = v
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
        .collect();
    out.sort();
    out
}

fn typed_axis(v: Option<&Value>, axis: &str) -> Vec<(String, String)> {
    let Some(obj) = v.and_then(Value::as_object) else {
        return Vec::new();
    };
    let keys: &[&str] = THEME_TYPED_AXIS_KEYS
        .iter()
        .find(|(a, _)| *a == axis)
        .map(|(_, k)| *k)
        .unwrap_or(&[]);
    keys.iter()
        .filter_map(|k| {
            obj.get(*k)
                .and_then(Value::as_str)
                .map(|s| ((*k).to_string(), s.to_string()))
        })
        .collect()
}

const AUTHORABLE: [&str; 13] = [
    "colors",
    "radii",
    "spacing",
    "typography",
    "shadows",
    "typeScale",
    "geometry",
    "elevation",
    "surface",
    "motion",
    "diagram",
    "scene",
    "displayName",
];

fn token_maps(v: &Value) -> Result<ThemeTokenMaps, UnlowerError> {
    if let Some(obj) = v.as_object() {
        if let Some(extra) = obj.keys().find(|k| !AUTHORABLE.contains(&k.as_str())) {
            return Err(UnlowerError::new(format!(
                "theme token axis `{extra}` has no .lolo spelling"
            )));
        }
    }
    let motion = v
        .get("motion")
        .and_then(|m| m.get("shapes"))
        .map(|s| typed_axis(Some(s), "motion"))
        .unwrap_or_default();
    Ok(ThemeTokenMaps {
        colors: string_map(v.get("colors")),
        radii: string_map(v.get("radii")),
        spacing: string_map(v.get("spacing")),
        typography: string_map(v.get("typography")),
        shadows: string_map(v.get("shadows")),
        type_scale: typed_axis(v.get("typeScale"), "typeScale"),
        geometry: typed_axis(v.get("geometry"), "geometry"),
        elevation: typed_axis(v.get("elevation"), "elevation"),
        surface: typed_axis(v.get("surface"), "surface"),
        motion,
        diagram: typed_axis(v.get("diagram"), "diagram"),
        scene: typed_axis(v.get("scene"), "scene"),
    })
}

pub(super) fn theme_decl(t: &ThemeRef) -> Result<ThemeDecl, UnlowerError> {
    match t {
        ThemeRef::Reference(k) => Ok(ThemeDecl::Reference(k.clone())),
        ThemeRef::Inline(def) => {
            let tokens =
                serde_json::to_value(&def.tokens).map_err(|e| UnlowerError::new(e.to_string()))?;
            let mut variants: Vec<(String, ThemeTokenMaps)> = Vec::new();
            for (name, variant) in def.variants.iter().flatten() {
                let v =
                    serde_json::to_value(variant).map_err(|e| UnlowerError::new(e.to_string()))?;
                variants.push((name.clone(), token_maps(&v)?));
            }
            variants.sort_by(|a, b| a.0.cmp(&b.0));
            Ok(ThemeDecl::Inline(InlineTheme {
                name: def.name.clone(),
                display_name: def.display_name.clone(),
                tokens: token_maps(&tokens)?,
                variants,
            }))
        }
    }
}
