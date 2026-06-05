use super::*;

/// Render a `TypeExpr` as the string form the resolve phase expects in
/// `ConfigField.field_type` ("string", "[string]", "number", etc.).
/// Mirrors the strings recognized by
/// `orbital-compiler::phases::resolve::type_string_to_oir`. Unknown
/// shapes fall back to "string" — same default the resolve phase uses
/// for unrecognized types, so the ts-config interface ends up typed
/// `field?: string` rather than failing the whole pipeline.
pub(crate) fn type_expr_to_config_string(ty: &TypeExpr) -> String {
    match ty {
        TypeExpr::Primitive(name) => name.clone(),
        TypeExpr::Named(name) => name.clone(),
        TypeExpr::Array(inner) => format!("[{}]", type_expr_to_config_string(inner)),
        TypeExpr::Optional(inner) => type_expr_to_config_string(inner),
        TypeExpr::StringLiteral(_) => "string".to_string(),
        TypeExpr::Union(_) => "string".to_string(),
        TypeExpr::Object(_) => "object".to_string(),
        TypeExpr::RelationMany(_) | TypeExpr::RelationManyNonEmpty(_) => "array".to_string(),
        TypeExpr::Trait => "trait".to_string(),
        TypeExpr::Slot => "slot".to_string(),
        TypeExpr::Pattern => "pattern".to_string(),
    }
}

/// Extract the literal-union variants when a config field is declared as
/// `"a" | "b" | "c"` (mirrors the entity-side `lower_type_expr`
/// `TypeExpr::Union` branch — same shape, same semantics). Returns
/// `None` when the type isn't a closed enum-style union, so downstream
/// consumers can tell "no constraint" from "constrained to {a, b, c}".
/// Unwraps a single layer of `Optional` so `"a" | "b"` and
/// `?"a" | "b"` both surface their members.
pub(crate) fn type_expr_to_config_values(ty: &TypeExpr) -> Option<Vec<String>> {
    match ty {
        TypeExpr::Optional(inner) => type_expr_to_config_values(inner),
        TypeExpr::Union(parts) => parts
            .iter()
            .map(|p| match p {
                TypeExpr::StringLiteral(s) => Some(s.clone()),
                TypeExpr::Named(n) => Some(n.clone()),
                TypeExpr::Primitive(n) => Some(n.clone()),
                _ => None,
            })
            .collect(),
        _ => None,
    }
}

/// Reduce a `TypeExpr` to a primitive string the compatibility check
/// understands. Mirrors the L2 emit-payload primitive vocabulary so the
/// two layers' decisions agree.
pub(crate) fn type_expr_to_compat_str(t: &TypeExpr) -> String {
    match t {
        TypeExpr::Primitive(p) => match p.as_str() {
            "int" | "float" | "double" | "long" => "number".to_string(),
            "bool" => "boolean".to_string(),
            other => other.to_string(),
        },
        TypeExpr::Named(n) => n.clone(), // entity / struct reference
        TypeExpr::StringLiteral(_) => "string".to_string(),
        TypeExpr::Array(_) => "array".to_string(),
        TypeExpr::Object(_) => "object".to_string(),
        TypeExpr::Union(members) => {
            // String-literal unions reduce to "string" (matches lowering's
            // enum-as-string emission). Mixed unions report as the first
            // member's type — best-effort; cross-type unions are rare in
            // practice.
            if members
                .iter()
                .all(|m| matches!(m, TypeExpr::StringLiteral(_)))
            {
                "string".to_string()
            } else {
                members
                    .first()
                    .map(type_expr_to_compat_str)
                    .unwrap_or_else(|| "object".to_string())
            }
        }
        TypeExpr::Optional(inner) => type_expr_to_compat_str(inner),
        TypeExpr::RelationMany(_) | TypeExpr::RelationManyNonEmpty(_) => "array".to_string(),
        TypeExpr::Trait => "trait".to_string(),
        TypeExpr::Slot => "slot".to_string(),
        TypeExpr::Pattern => "pattern".to_string(),
    }
}

/// Primitive-type compatibility check between declared and actual.
/// Mirrors the L2 emit-payload validator's `primitive_types_compatible`.
/// `any` matches anything; `date`/`datetime`/`timestamp` accept string
/// or number; `entity`/`relation` accept object or string; named entity
/// types (e.g. `Article`) accept object.
pub(crate) fn primitive_type_compatible(declared: &str, actual: &str) -> bool {
    let d = declared.trim_end_matches('!');
    let a = actual.trim_end_matches('!');
    if d == a {
        return true;
    }
    match d {
        "any" => true,
        "date" | "datetime" | "timestamp" => a == "string" || a == "number",
        "entity" | "relation" => a == "object" || a == "string",
        // Named entity / struct types accept object literals or string
        // (id reference).
        s if s.chars().next().map(|c| c.is_uppercase()).unwrap_or(false) => {
            a == "object" || a == "string"
        }
        _ => false,
    }
}

/// Lenient primitive compatibility check for emit payload literals.
/// Mirrors the L2 validator's `primitive_types_compatible` so the two
/// detection layers agree. `any` matches anything; `date`/`datetime`/
/// `timestamp` accept string or number literals; `entity`/`relation`
/// accept object or string literals.
pub(crate) fn emit_primitive_compatible(declared: &str, actual: &str) -> bool {
    if declared == actual {
        return true;
    }
    // Strip optional `!` suffix on declared — payload field types
    // sometimes serialize as `string!` to mark required.
    let declared_core = declared.trim_end_matches('!');
    if declared_core == actual {
        return true;
    }
    match declared_core {
        "any" => true,
        "date" | "datetime" | "timestamp" => actual == "string" || actual == "number",
        "entity" | "relation" => actual == "object" || actual == "string",
        _ => false,
    }
}

pub(crate) fn type_expr_to_string(ty: &TypeExpr) -> String {
    match ty {
        TypeExpr::Primitive(s) | TypeExpr::Named(s) => s.clone(),
        TypeExpr::StringLiteral(_) => "string".to_string(),
        TypeExpr::Array(inner) => format!("[{}]", type_expr_to_string(inner)),
        TypeExpr::Object(_) => "object".to_string(),
        TypeExpr::Union(_) => "string".to_string(),
        TypeExpr::Optional(inner) => type_expr_to_string(inner),
        TypeExpr::RelationMany(name) => format!("{name}*"),
        TypeExpr::RelationManyNonEmpty(name) => format!("{name}+"),
        TypeExpr::Trait => "trait".to_string(),
        TypeExpr::Slot => "slot".to_string(),
        TypeExpr::Pattern => "pattern".to_string(),
    }
}

/// Convert an event key to a human-readable display name for the
/// state-machine `events[].name` field. Rules (best-effort; authors override
/// via an events block when the trait-level metadata matters).
///
/// - `INIT` → `"Initialize"` (special case; convention from existing .orb).
/// - `UPPER_SNAKE_CASE` → Title Case: `REQUEST_REMOVE` → `"Request Remove"`.
/// - Single all-caps word → Title: `SAVE` → `"Save"`, `OPEN` → `"Open"`.
/// - `MixedCase` starting with the trait's linked entity name → the entity
///   name unchanged + space-lowercased suffix: given entity `ConfirmAction`,
///   `ConfirmActionLoaded` → `"ConfirmAction loaded"`,
///   `ConfirmActionLoadFailed` → `"ConfirmAction load failed"`.
/// - Unmatched cases return the key unchanged.
pub(crate) fn humanize_event_key(key: &str, entity_name: Option<&str>) -> String {
    if key == "INIT" {
        return "Initialize".to_string();
    }
    // UPPER_SNAKE_CASE.
    if !key.is_empty()
        && key
            .chars()
            .all(|c| c.is_ascii_uppercase() || c == '_' || c.is_ascii_digit())
    {
        return key
            .split('_')
            .filter(|w| !w.is_empty())
            .map(title_case_word)
            .collect::<Vec<_>>()
            .join(" ");
    }
    // MixedCase with a leading entity-name prefix.
    if let Some(ent) = entity_name {
        if key.starts_with(ent) && key.len() > ent.len() {
            let suffix = &key[ent.len()..];
            let humanized = split_camel(suffix)
                .into_iter()
                .map(|w| w.to_ascii_lowercase())
                .collect::<Vec<_>>()
                .join(" ");
            if humanized.is_empty() {
                return ent.to_string();
            }
            return format!("{ent} {humanized}");
        }
    }
    // Generic MixedCase split: keep leading word capitalised, lowercase tail.
    let parts = split_camel(key);
    if parts.len() >= 2 {
        let head = parts[0].clone();
        let tail = parts[1..]
            .iter()
            .map(|w| w.to_ascii_lowercase())
            .collect::<Vec<_>>()
            .join(" ");
        return format!("{head} {tail}");
    }
    key.to_string()
}

fn title_case_word(w: &str) -> String {
    let mut chars = w.chars();
    match chars.next() {
        Some(c) => c.to_ascii_uppercase().to_string() + &chars.as_str().to_ascii_lowercase(),
        None => String::new(),
    }
}

/// Split a CamelCase / PascalCase string into word segments at boundaries
/// between a lowercase char and an uppercase char. `ConfirmActionLoaded` →
/// `["Confirm", "Action", "Loaded"]`. Consecutive uppercase letters stay
/// together up to the next lowercase: `HTTPRequest` → `["HTTPRequest"]`
/// (degenerate, acceptable).
fn split_camel(s: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    let chars: Vec<char> = s.chars().collect();
    for i in 0..chars.len() {
        let c = chars[i];
        if i > 0 && c.is_ascii_uppercase() && chars[i - 1].is_ascii_lowercase() {
            out.push(std::mem::take(&mut cur));
        }
        cur.push(c);
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}
