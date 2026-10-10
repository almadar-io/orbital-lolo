use super::*;

/// The `_` arm's type of a `match`, or `None` when malformed (no wildcard —
/// `check_match_types` is the source of truth on well-formedness; these
/// rendering helpers just need to never panic on input it hasn't run
/// against yet). Shared by every `TypeExpr` rendering function below: a
/// `Match`'s string/compat/config form all delegate to its `_` arm.
fn match_wildcard_type(arms: &[MatchArm]) -> Option<&TypeExpr> {
    arms.iter()
        .find(|a| matches!(a.pattern, MatchPattern::Wildcard))
        .map(|a| &a.ty)
}

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
        TypeExpr::BoundEntityRow => "@entity".to_string(),
        TypeExpr::EventPayloadOf(PayloadOfTarget::ConfigKnob(knob)) => format!("@payload({knob})"),
        TypeExpr::EventPayloadOf(PayloadOfTarget::SiblingMember(member)) => {
            format!("@payload(.{member})")
        }
        TypeExpr::NamedEntityRow(knob) => format!("@entity({knob})"),
        TypeExpr::ExpectedIdentityRow => "@user".to_string(),
        TypeExpr::Named(name) => name.clone(),
        TypeExpr::Array(inner) => format!("[{}]", type_expr_to_config_string(inner)),
        TypeExpr::Tuple(slots) => format!(
            "[{}]",
            slots.iter().map(type_expr_to_config_string).collect::<Vec<_>>().join(", ")
        ),
        // `Map K V` → round-trippable `Map<K,V>` config-string (the key is a
        // comma-free atom, so resolve splits on the first comma). Parsed back
        // by `parse_payload_field_type` into `OirType::Map`.
        TypeExpr::Map(key, value) => format!(
            "Map<{},{}>",
            type_expr_to_config_string(key),
            type_expr_to_config_string(value)
        ),
        TypeExpr::FieldOf {
            ty,
            field,
            resolved,
        } => match resolved {
            Some(inner) => type_expr_to_config_string(inner),
            None => format!("{ty}.{field}"),
        },
        TypeExpr::Optional(inner) => type_expr_to_config_string(inner),
        TypeExpr::StringLiteral(_) => "string".to_string(),
        TypeExpr::Union(_) => "string".to_string(),
        TypeExpr::Object(_) => "object".to_string(),
        TypeExpr::RelationMany(_) | TypeExpr::RelationManyNonEmpty(_) => "array".to_string(),
        TypeExpr::Trait => "trait".to_string(),
        TypeExpr::Orbital => "orbital".to_string(),
        TypeExpr::Behavior => "behavior".to_string(),
        TypeExpr::EntityRef => "entity".to_string(),
        TypeExpr::EventRef => "event".to_string(),
        TypeExpr::EventAddress => "EventAddress".to_string(),
        TypeExpr::Slot => "slot".to_string(),
        TypeExpr::Pattern => "pattern".to_string(),
        TypeExpr::Secret => "secret".to_string(),
        TypeExpr::SExpr => "SExpr".to_string(),
        TypeExpr::Match { arms, .. } => match_wildcard_type(arms)
            .map(type_expr_to_config_string)
            .unwrap_or_else(|| "string".to_string()),
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
        // Bound-entity row: a record at the bus-payload level.
        TypeExpr::BoundEntityRow
        | TypeExpr::EventPayloadOf(_)
        | TypeExpr::NamedEntityRow(_)
        | TypeExpr::ExpectedIdentityRow => "object".to_string(),
        TypeExpr::StringLiteral(_) => "string".to_string(),
        TypeExpr::Array(_) | TypeExpr::Tuple(_) => "array".to_string(),
        // A map is a record/object at the bus-payload level.
        TypeExpr::Map(_, _) => "object".to_string(),
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
        TypeExpr::FieldOf {
            ty,
            field,
            resolved,
        } => match resolved {
            Some(inner) => type_expr_to_compat_str(inner),
            None => format!("{ty}.{field}"),
        },
        TypeExpr::RelationMany(_) | TypeExpr::RelationManyNonEmpty(_) => "array".to_string(),
        TypeExpr::Trait => "trait".to_string(),
        TypeExpr::Orbital => "orbital".to_string(),
        TypeExpr::Behavior => "behavior".to_string(),
        // Entity/event refs are name strings at the bus-payload level.
        TypeExpr::EntityRef | TypeExpr::EventRef | TypeExpr::EventAddress => "string".to_string(),
        TypeExpr::Slot => "slot".to_string(),
        TypeExpr::Pattern => "pattern".to_string(),
        TypeExpr::Secret => "secret".to_string(),
        // SExpr is a structured value at the bus-payload level.
        TypeExpr::SExpr => "object".to_string(),
        TypeExpr::Match { arms, .. } => match_wildcard_type(arms)
            .map(type_expr_to_compat_str)
            .unwrap_or_else(|| "string".to_string()),
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
    // A declared ARRAY type (`[string]`, `[object]`, `[Entity]`) accepts an
    // array literal: the shallow classifier types every array literal as
    // "array" and cannot see elements, and this table had NO arm for `[…]`
    // — so `data: [...]` against `data : [string]` could never pass and the
    // error read as a self-contradiction ("declared as [string] but the
    // literal value has type array"; run 28, ReviewReminders). Element-level
    // checking is the L2 emit-payload backstop's job.
    if declared_core.starts_with('[') && declared_core.ends_with(']') {
        return actual == "array";
    }
    match declared_core {
        "any" => true,
        "date" | "datetime" | "timestamp" => actual == "string" || actual == "number",
        // A semantic domain accepts a string literal/expression. Without this,
        // `SUBMIT_FORM { email: email }` payloads fail the emit check.
        "email" | "url" | "phone" | "uuid" | "image" => actual == "string",
        // Money is numeric at rest; file is the canonical struct value.
        "money" => actual == "number",
        "file" => actual == "object",
        "entity" | "relation" => actual == "object" || actual == "string",
        _ => false,
    }
}

pub(crate) fn type_expr_to_string(ty: &TypeExpr) -> String {
    match ty {
        TypeExpr::Primitive(s) | TypeExpr::Named(s) => s.clone(),
        TypeExpr::BoundEntityRow => "@entity".to_string(),
        TypeExpr::EventPayloadOf(PayloadOfTarget::ConfigKnob(knob)) => format!("@payload({knob})"),
        TypeExpr::EventPayloadOf(PayloadOfTarget::SiblingMember(member)) => {
            format!("@payload(.{member})")
        }
        TypeExpr::NamedEntityRow(knob) => format!("@entity({knob})"),
        TypeExpr::ExpectedIdentityRow => "@user".to_string(),
        TypeExpr::StringLiteral(_) => "string".to_string(),
        TypeExpr::Array(inner) => format!("[{}]", type_expr_to_string(inner)),
        TypeExpr::Tuple(slots) => format!(
            "[{}]",
            slots.iter().map(type_expr_to_string).collect::<Vec<_>>().join(", ")
        ),
        TypeExpr::Map(key, value) => format!(
            "Map<{},{}>",
            type_expr_to_string(key),
            type_expr_to_string(value)
        ),
        TypeExpr::Object(_) => "object".to_string(),
        TypeExpr::Union(_) => "string".to_string(),
        TypeExpr::Optional(inner) => type_expr_to_string(inner),
        TypeExpr::FieldOf {
            ty,
            field,
            resolved,
        } => match resolved {
            Some(inner) => type_expr_to_string(inner),
            None => format!("{ty}.{field}"),
        },
        TypeExpr::RelationMany(name) => format!("{name}*"),
        TypeExpr::RelationManyNonEmpty(name) => format!("{name}+"),
        TypeExpr::Trait => "trait".to_string(),
        TypeExpr::Orbital => "orbital".to_string(),
        TypeExpr::Behavior => "behavior".to_string(),
        TypeExpr::EntityRef => "entity".to_string(),
        TypeExpr::EventRef => "event".to_string(),
        TypeExpr::EventAddress => "EventAddress".to_string(),
        TypeExpr::Slot => "slot".to_string(),
        TypeExpr::Pattern => "pattern".to_string(),
        TypeExpr::Secret => "secret".to_string(),
        TypeExpr::SExpr => "SExpr".to_string(),
        TypeExpr::Match { arms, .. } => match_wildcard_type(arms)
            .map(type_expr_to_string)
            .unwrap_or_else(|| "string".to_string()),
    }
}

/// A `trait`-typed config field's default/override may be written as the
/// bare identifier `none` — the language's reserved "unbound" sentinel for
/// this type (owner ruling 2026-09-13: a `trait`-typed config knob may be
/// `none`). The generic value grammar (`parse_sexpr_or_value`) has no notion
/// of the field's declared type, so a bare `none` lowers to the literal
/// string `"none"` like any other identifier; canonicalize it to `Null`
/// here, at the one place `TypeExpr::Trait` is known, so every downstream
/// consumer (guard truthiness — `null` is falsy everywhere for free, no
/// evaluator change needed — the trait-typed-default validator, and the
/// render-ui unbound-embed check) sees one uniform "no value" shape instead
/// of a magic string. A no-op for every other type or value shape.
pub(crate) fn canonicalize_unbound_trait_default(
    type_expr: &TypeExpr,
    default: Option<SExpression>,
) -> Option<SExpression> {
    match (type_expr, &default) {
        (TypeExpr::Trait | TypeExpr::Orbital | TypeExpr::Behavior, Some(SExpression::String(s))) if s == "none" => Some(SExpression::Null),
        _ => default,
    }
}

/// Lower a `Vec<ConfigField>` (the AST's `config { ... }` grammar) into the
/// schema's `Option<HashMap<String, ConfigField>>` shape. Shared by every
/// site that carries this grammar: a trait's own declared `config {}`, the
/// app-level `config {}` (`OrbitalSchema.config`), an orbital's own
/// `config {}` (`Orbital.config`), and the two override sites (`uses { config
/// {} }`, an orbital reference's `config {}`) — extracted from the trait
/// path so none of the five duplicate the type-resolution + sentinel logic.
///
/// `type_params` is the owning trait's own declared type-parameter names
/// (empty for every non-trait site — `type_param_sentinel` is a no-op on an
/// empty slice). `check_self_reference` gates the same-key self-reference
/// guard (`config { k: T = @config.k }`, nonsensical on a DECLARATION): true
/// for the three declaration sites, false for the two override sites, where
/// `@config.<key>` legitimately forwards the ENCLOSING scope's own knob of
/// the same name into the referenced upstream (the same reasoning
/// `lower_trait_reference`'s call-site config already applies).
pub(crate) fn lower_config_fields_to_map(
    fields: &[ConfigField],
    owner_name: &str,
    check_self_reference: bool,
    type_params: &[&str],
    aliases: &BTreeMap<String, TypeAliasResolved>,
    entity_names: &HashSet<String>,
) -> Result<Option<HashMap<String, SchemaConfigField>>, LowerError> {
    if fields.is_empty() {
        return Ok(None);
    }
    let mut map: HashMap<String, SchemaConfigField> = HashMap::new();
    for cf in fields {
        if check_self_reference {
            if let Some(SExpression::String(s)) = &cf.default {
                if s.strip_prefix("@config.") == Some(cf.name.as_str()) {
                    return Err(LowerError::ConfigDefaultSelfReferential {
                        trait_name: owner_name.to_string(),
                        key: cf.name.clone(),
                    });
                }
            }
        }
        // A config field typed as one of the owner's own declared type
        // parameters (`config { result : V }`, trait-only) lowers its
        // `field_type` to the `$V` sentinel directly — checked first since a
        // param name never resolves as a struct alias/entity.
        let sentinel = emit_listen::type_param_sentinel(cf.type_expr.peel(), type_params);
        let (field_type, items, properties, values) = if let Some(s) = sentinel {
            (s, None, None, None)
        } else {
            let (_, _, items, properties, _) =
                lower_type_expr(cf.type_expr.peel(), aliases, entity_names);
            (
                type_expr_to_config_string(cf.type_expr.peel()),
                items,
                properties,
                type_expr_to_config_values(cf.type_expr.peel()),
            )
        };
        map.insert(
            cf.name.clone(),
            SchemaConfigField {
                field_type,
                default: canonicalize_unbound_trait_default(
                    cf.type_expr.peel(),
                    cf.default.as_ref().map(desugar_expr_match),
                ),
                label: cf.label.clone(),
                description: cf.description.clone(),
                synonyms: cf.synonyms.clone(),
                tier: cf.tier.clone(),
                values,
                items,
                properties,
                required: if cf.required { Some(true) } else { None },
                ref_id: None,
                forwarded_from: None,
                nested_forwarded_from: Vec::new(),
                payload_for: None,
                projected_from: cf.type_expr.projection(),
            },
        );
    }
    Ok(Some(map))
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
