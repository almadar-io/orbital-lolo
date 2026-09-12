use super::*;
use std::cell::RefCell;

thread_local! {
    /// Alias names currently being lowered on this thread. Guards the
    /// self-referential alias cycle that tagged unions make reachable.
    /// `pub(crate)` so `emit_listen.rs`'s payload-position struct lowering
    /// (`struct_payload_properties` and friends) shares this ONE guard
    /// instead of a second copy — same cycle, same set, checked from a
    /// different call site.
    pub(crate) static RESOLVING: RefCell<HashSet<String>> = RefCell::new(HashSet::new());
}

pub(crate) fn lower_entity(
    entity: &Entity,
    aliases: &BTreeMap<String, TypeAliasResolved>,
    entity_names: &HashSet<String>,
) -> Result<EntityDefinition, LowerError> {
    check_shared_entity_has_persistence_mode(entity)?;
    let mut fields = Vec::with_capacity(entity.fields.len());
    for f in &entity.fields {
        fields.push(lower_field(f, aliases, entity_names)?);
    }
    let persistence = match &entity.persistence {
        Some(Persistence::Persistent) => Some(CorePersistence::Persistent),
        Some(Persistence::Runtime) => Some(CorePersistence::Runtime),
        None => None,
    };
    Ok(EntityDefinition {
        name: entity.name.clone(),
        collection: entity.collection.clone(),
        persistence,
        fields,
        instances: None,
        shared: entity.shared,
        identity: entity.identity,
        read_policy: entity.read_policy.clone(),
        create_policy: entity.create_policy.clone(),
        update_policy: entity.update_policy.clone(),
        delete_policy: entity.delete_policy.clone(),
        access_waivers: if entity.access_waivers.is_empty() {
            None
        } else {
            let mut waivers = orbital_core::schema::AccessWaivers::default();
            for (op, reason) in &entity.access_waivers {
                match op.as_str() {
                    "read" => waivers.read = Some(reason.clone()),
                    "create" => waivers.create = Some(reason.clone()),
                    "update" => waivers.update = Some(reason.clone()),
                    "delete" => waivers.delete = Some(reason.clone()),
                    _ => {}
                }
            }
            Some(waivers)
        },
        id: None,
    })
}

pub(crate) fn lower_field(
    f: &EntityField,
    aliases: &BTreeMap<String, TypeAliasResolved>,
    entity_names: &HashSet<String>,
) -> Result<FieldDefinition, LowerError> {
    // Resolve the type expression. Named types may be enum aliases (lower to
    // string + values list), entity references (lower to a Relation field with
    // cardinality), or struct aliases (lower to inline object schema).
    let (field_type, values, items, properties, relation) =
        lower_type_expr(&f.type_expr, aliases, entity_names);
    Ok(FieldDefinition {
        merged_from: None,
        name: Some(f.name.clone()),
        field_type,
        required: if f.required { Some(true) } else { None },
        primary_key: if f.is_key { Some(true) } else { None },
        unique: if f.is_unique { Some(true) } else { None },
        default: f.default.clone(),
        values,
        relation,
        items,
        properties,
        intrinsic: if f.is_intrinsic { Some(true) } else { None },
        description: f.description.clone(),
        synonyms: f.synonyms.clone(),
    })
}

type LoweredType = (
    FieldType,
    Option<Vec<String>>,
    Option<Box<FieldDefinition>>,
    Option<HashMap<String, FieldDefinition>>,
    Option<RelationDefinition>,
);

pub(crate) fn lower_type_expr(
    ty: &TypeExpr,
    aliases: &BTreeMap<String, TypeAliasResolved>,
    entity_names: &HashSet<String>,
) -> LoweredType {
    match ty {
        TypeExpr::Primitive(name) => (primitive_to_field_type(name), None, None, None, None),
        // `@entity` is only legal in event payload positions (enforced by the
        // Stage-2 checker); an entity/config field reaching here is already an
        // error upstream — lower defensively to a shapeless object.
        TypeExpr::BoundEntityRow
        | TypeExpr::EventPayloadOf(PayloadOfTarget::ConfigKnob(_))
        | TypeExpr::NamedEntityRow(_)
        | TypeExpr::ExpectedIdentityRow => (FieldType::Object, None, None, None, None),
        // `@payload <sibling>` inside a struct member position (a
        // `type X = { action: event, actionPayload: @payload <action> }`
        // body): the concrete event isn't knowable here — `action` is a TYPE
        // (`event`), not a literal — so there is no single named shape to
        // carry through the closed `FieldType` wire tag. Lowers to a
        // shapeless `Object` (no `properties`, no `items`) — the same
        // "target declares nothing" carrier `resolve_field_shape` already
        // collapses to `OirType::Json`, which codegen renders as the generic
        // `EventPayload` TS type (`shell_typescript::backend::shared`).
        // `types::check_payload_sibling_refs` is the actual gate: it
        // validates the sibling exists and is `event`-typed, and (where both
        // are literal) that a literal instance's value matches the sibling's
        // declared payload — entirely at L1, against the AST, before this
        // erasure happens.
        TypeExpr::EventPayloadOf(PayloadOfTarget::SiblingMember(_)) => {
            (FieldType::Object, None, None, None, None)
        }
        TypeExpr::Trait => (FieldType::Trait, None, None, None, None),
        TypeExpr::Slot => (FieldType::Slot, None, None, None, None),
        TypeExpr::Pattern => (FieldType::Pattern, None, None, None, None),
        // Config-only (parser rejects it on entity fields); defensively a string ref.
        TypeExpr::Secret => (FieldType::String, None, None, None, None),
        // Config-only brands (gated by `config_only_type` in parser/mod.rs); in
        // an entity context they fall back — SExpr to an opaque object, the
        // entity/event name refs to a plain string value.
        TypeExpr::SExpr => (FieldType::Object, None, None, None, None),
        // Payload-position-only (gated by `type_expr_config_only_type` in
        // parser/mod.rs); an entity field reaching here is already an error
        // upstream — lower defensively to an opaque object, same treatment
        // as the other config/payload-only brands above.
        TypeExpr::Match { .. } => (FieldType::Object, None, None, None, None),
        TypeExpr::EntityRef => (FieldType::String, None, None, None, None),
        // Phase B: an `event`-typed struct-alias field (e.g. `ItemAction.event`)
        // keeps its type tag through lowering instead of erasing to `string` —
        // `check_field_def_shape`'s closed-circuit-parity acceptance check
        // (`config_type.rs`) needs the tag to fire. Entity-field positions stay
        // protected by the parser gate (`ELOLO_CONFIG_TYPE_FORBIDDEN_IN_ENTITY`);
        // this arm is reached only for struct aliases consumed by config knobs.
        TypeExpr::EventRef => (FieldType::Event, None, None, None, None),
        TypeExpr::Named(name) => {
            if let Some(alias) = aliases.get(name) {
                match alias {
                    TypeAliasResolved::Enum(values) => {
                        (FieldType::String, Some(values.clone()), None, None, None)
                    }
                    TypeAliasResolved::Type(inner) => {
                        // CYCLE GUARD. A recursive alias — `DrawGroupItem.items :
                        // [DrawItem]` where `DrawItem` includes `DrawGroupItem` —
                        // would recurse forever here (verified: `orb validate`
                        // aborted with a stack overflow before this existed).
                        // A back-reference lowers to the documented by-name form:
                        // `Union` carrying the NAME in `values` and no
                        // `properties`, which `resolve` maps to Json rather than
                        // expanding.
                        if !RESOLVING.with(|r| r.borrow_mut().insert(name.clone())) {
                            return (FieldType::Union, Some(vec![name.clone()]), None, None, None);
                        }
                        let out = lower_type_expr(inner, aliases, entity_names);
                        RESOLVING.with(|r| {
                            r.borrow_mut().remove(name);
                        });
                        out
                    }
                }
            } else if entity_names.contains(name) {
                // Phase 9.6.A: bare entity reference (`field : EntityName`) lowers
                // to a cardinality:One relation so @almadar/runtime's
                // `populateRelations` can hydrate it on `(fetch X { include: ["field"] })`.
                // Previously these lowered to `FieldType::String` with a TODO marker.
                (
                    FieldType::Relation,
                    None,
                    None,
                    None,
                    Some(RelationDefinition {
                        entity: name.clone(),
                        cardinality: Some(Cardinality::One),
                        entity_id: None,
                    }),
                )
            } else if let Some(ft) = primitive_field_type(name) {
                // Primitive spelled as a Named node (the parser's
                // `is_primitive_type` list is narrower than the full primitive
                // vocabulary — `json`/`node`/`render-ui` arrive here). Routes
                // `node` to its first-class tag; the erased spellings keep
                // their historical lowering.
                (ft, None, None, None, None)
            } else {
                // Unknown named type — preserve as a string field.
                (FieldType::String, None, None, None, None)
            }
        }
        TypeExpr::Array(inner) => {
            // Special-case `[EntityName]` (array of entity references): lower to
            // a single Relation field with cardinality:Many rather than an Array
            // wrapper around a foreign-key string. Companion to the bare-entity
            // case above. Type aliases that happen to look like entities still
            // take the regular array-of-struct path because they appear in
            // `aliases`, not in `entity_names`.
            if let TypeExpr::Named(name) = inner.as_ref() {
                if entity_names.contains(name) && !aliases.contains_key(name) {
                    return (
                        FieldType::Relation,
                        None,
                        None,
                        None,
                        Some(RelationDefinition {
                            entity: name.clone(),
                            cardinality: Some(Cardinality::Many),
                            entity_id: None,
                        }),
                    );
                }
            }
            // FC-5b: `[json]` is an array of free JSON values (typically object
            // records). Lower the element to an UNCONSTRAINED object — identical
            // to `[object]` — NOT `{type:"string"}` (the old unknown-name
            // fallback), which mis-typed every object element and tripped
            // `ORB_T_CONFIG_SHAPE_MISMATCH`. Authors wanting typed elements
            // declare a named alias (`type X = { … }`) and write `[X]`.
            // `json` is not in `is_primitive_type`, so `[json]` parses as
            // `Array(Named("json"))`; accept both the Named and Primitive forms.
            let inner_is_json = matches!(
                inner.as_ref(),
                TypeExpr::Named(p) | TypeExpr::Primitive(p) if p == "json"
            );
            let (inner_ft, inner_values, inner_items, inner_props, _inner_rel) = if inner_is_json {
                (FieldType::Object, None, None, None, None)
            } else {
                lower_type_expr(inner, aliases, entity_names)
            };
            let item = FieldDefinition {
                merged_from: None,
                name: None,
                field_type: inner_ft,
                required: None,
                primary_key: None,
                unique: None,
                default: None,
                values: inner_values,
                relation: None,
                items: inner_items,
                properties: inner_props,
                intrinsic: None,
                description: None,
                synonyms: None,
            };
            (FieldType::Array, None, Some(Box::new(item)), None, None)
        }
        // `Map K V` — carry the VALUE's resolved shape in the `items` carrier
        // (resolving named-alias values to their struct, exactly like array
        // elements). `resolve_config_field_shape` disambiguates map-vs-array by
        // the `Map<…>` field-type string and inlines this value shape into
        // `OirType::Map { key, value }`. The `FieldType::Object` tag keeps an
        // entity-level map (rare) as a plain object.
        TypeExpr::Map(_key, value) => {
            let (v_ft, v_values, v_items, v_props, _v_rel) =
                lower_type_expr(value, aliases, entity_names);
            let value_field = FieldDefinition {
                merged_from: None,
                name: None,
                field_type: v_ft,
                required: None,
                primary_key: None,
                unique: None,
                default: None,
                values: v_values,
                relation: None,
                items: v_items,
                properties: v_props,
                intrinsic: None,
                description: None,
                synonyms: None,
            };
            (
                FieldType::Object,
                None,
                Some(Box::new(value_field)),
                None,
                None,
            )
        }
        TypeExpr::Object(fields) => {
            let mut props = HashMap::new();
            for (k, v, req) in fields {
                let (ft, values, items, properties, relation) =
                    lower_type_expr(v, aliases, entity_names);
                props.insert(
                    k.clone(),
                    FieldDefinition {
                        merged_from: None,
                        name: Some(k.clone()),
                        field_type: ft,
                        // Mirror the lolo `!` marker — `name : T!` lowers to
                        // `required: Some(true)`, unmarked stays `Some(false)`
                        // so the codegen emits optional-by-default TS fields
                        // matching lolo authoring semantics.
                        required: Some(*req),
                        primary_key: None,
                        unique: None,
                        default: None,
                        values,
                        relation,
                        items,
                        properties,
                        intrinsic: None,
                        description: None,
                        synonyms: None,
                    },
                );
            }
            (FieldType::Object, None, None, Some(props), None)
        }
        TypeExpr::Union(parts) => {
            // A union whose members are all STRUCT aliases is a tagged union,
            // not a string enum. Checked FIRST: the `all_members` collapse
            // below would otherwise turn `DrawShape | DrawText` into a field
            // whose only legal values are those two literal WORDS — it would
            // accept the variant's name and reject the variant's value.
            if let Some(variants) = struct_union_variants(parts, aliases, entity_names) {
                let names: Vec<String> = variants.iter().map(|(n, _)| n.clone()).collect();
                let props: HashMap<String, FieldDefinition> = variants.into_iter().collect();
                return (FieldType::Union, Some(names), None, Some(props), None);
            }
            // If every variant is a bare Named / Primitive (`a | b | c`) or
            // a string literal (`"a" | "b" | "c"`), treat as a string enum and
            // flatten into `values: [...]` on the field.
            let all_members: Option<Vec<String>> = parts
                .iter()
                .map(|p| match p {
                    TypeExpr::Named(n) => Some(n.clone()),
                    TypeExpr::Primitive(n) => Some(n.clone()),
                    TypeExpr::StringLiteral(s) => Some(s.clone()),
                    _ => None,
                })
                .collect();
            if let Some(values) = all_members {
                (FieldType::String, Some(values), None, None, None)
            } else {
                (FieldType::String, None, None, None, None)
            }
        }
        TypeExpr::StringLiteral(lit) => {
            // A single-literal field type is a closed vocabulary of ONE — the
            // field's only legal value is that literal. Carrying it in `values`
            // (rather than erasing to plain `string`) is what makes
            // `type : "draw-shape"` a usable DISCRIMINATOR: union variant
            // selection reads single-valued vocabularies to decide which
            // variant a value belongs to. Erased, every variant looked
            // untagged and a value whose tag matched nothing was silently
            // accepted as the first variant.
            (FieldType::String, Some(vec![lit.clone()]), None, None, None)
        }
        TypeExpr::Optional(inner) => lower_type_expr(inner, aliases, entity_names),
        TypeExpr::RelationMany(_) | TypeExpr::RelationManyNonEmpty(_) => {
            (FieldType::Array, None, None, None, None)
        }
    }
}

/// The canonical primitive-type-name set and its `FieldType` mapping — the
/// single source consulted by BOTH the lowerer (`primitive_to_field_type`) and
/// the type checker (`types::is_primitive`), so the two layers can't disagree on
/// what counts as a primitive. `None` for a non-primitive name (an alias /
/// entity reference / typo), which the type checker then rejects.
pub(crate) fn primitive_field_type(name: &str) -> Option<FieldType> {
    Some(match name {
        // `icon`/`component` are refinement types over string (validity enforced
        // at the render-ui layer); they lower to a plain string field.
        "string" | "any" | "icon" | "component" => FieldType::String,
        // Promoted 2026-08-20 (was collapsed to String — the schema lied about
        // the value's shape): money is numeric at rest; file is the canonical
        // {name, url, mimeType, sizeBytes} struct. The declared type — never a
        // field-name heuristic — drives currency inputs/formatting and the
        // upload dropzone + download-chip rendering on both paths.
        "money" => FieldType::Money,
        "file" => FieldType::File,
        // Semantic string domains, now first-class field types rather than
        // subtypes that erased to `string`. Closes the "no format validation yet"
        // half of Gap G-LOLO-TYPENAME-SILENT-FALLBACK: the declared domain
        // survives lowering, so the validator can check a literal default and
        // codegen can emit the matching zod refinement.
        "email" => FieldType::Email,
        "url" => FieldType::Url,
        "phone" => FieldType::Phone,
        "uuid" => FieldType::Uuid,
        "image" => FieldType::Image,
        // Renderable content is first-class (type-integrity campaign): the
        // `.orb` carries the honest `node` tag instead of lying `string`.
        "node" => FieldType::Node,
        // De-facto accepted config/field types in the std corpus — `json` (free
        // value), `render-ui` (a render-ui pattern object config), `relation`
        // (a foreign-key reference). They lowered to a string field via the old
        // unknown-name fallback; recognized here to preserve that (`json` and
        // `render-ui` are being ELIMINATED, not re-typed — Gate C tracks them)
        // while still rejecting genuine typos.
        "json" | "render-ui" | "relation" => FieldType::String,
        "int" | "float" | "number" | "duration" => FieldType::Number,
        "bool" | "boolean" => FieldType::Boolean,
        "date" => FieldType::Date,
        "datetime" => FieldType::Datetime,
        "timestamp" => FieldType::Timestamp,
        "object" => FieldType::Object,
        // The honest replacement for `json` on a dynamic-key value bag. Unlike
        // the `json` line above (which ERASES to String and is being
        // eliminated), this keeps its own tag all the way to codegen.
        "scalar" => FieldType::Scalar,
        _ => return None,
    })
}

pub(crate) fn primitive_to_field_type(name: &str) -> FieldType {
    // The type checker rejects unknown type names up front, so by lowering time
    // every name here is a known primitive; the String fallback is defensive.
    primitive_field_type(name).unwrap_or(FieldType::String)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::TypeExpr;
    use std::collections::{BTreeMap, HashSet};

    fn lower(ty: &TypeExpr) -> LoweredType {
        lower_type_expr(ty, &BTreeMap::new(), &HashSet::new())
    }

    #[test]
    fn json_array_lowers_to_unconstrained_object_items() {
        // FC-5b: `[json]` element must be an unconstrained object (like
        // `[object]`), NOT `{type:"string"}` (which trips ORB_T_CONFIG_SHAPE_MISMATCH).
        // `json` is not in `is_primitive_type`, so it parses as `Named("json")`.
        let ty = TypeExpr::Array(Box::new(TypeExpr::Named("json".to_string())));
        let (ft, _values, items, _props, _rel) = lower(&ty);
        assert!(matches!(ft, FieldType::Array));
        let item = items.expect("array carries an item schema");
        assert!(
            matches!(item.field_type, FieldType::Object),
            "[json] element must lower to an object, got {:?}",
            item.field_type
        );
        assert!(
            item.properties.is_none(),
            "[json] element must be UNCONSTRAINED (no properties)"
        );
    }

    #[test]
    fn node_lowers_first_class_scalar_and_array() {
        // Type-integrity campaign: `node` is renderable content and must keep
        // its tag — never erase to string. `node` is not in
        // `is_primitive_type`, so it parses as `Named("node")` like `json`.
        let (ft, _, _, _, _) = lower(&TypeExpr::Named("node".to_string()));
        assert!(matches!(ft, FieldType::Node), "scalar node keeps its tag");

        let ty = TypeExpr::Array(Box::new(TypeExpr::Named("node".to_string())));
        let (ft, _values, items, _props, _rel) = lower(&ty);
        assert!(matches!(ft, FieldType::Array));
        let item = items.expect("array carries an item schema");
        assert!(
            matches!(item.field_type, FieldType::Node),
            "[node] element must lower to Node, got {:?}",
            item.field_type
        );
    }

    #[test]
    fn scalar_json_stays_string() {
        // The array-element refinement must not change the scalar `json` type.
        let (ft, _, _, _, _) = lower(&TypeExpr::Named("json".to_string()));
        assert!(matches!(ft, FieldType::String), "scalar json stays string");
    }

    /// Recursive named struct types (feature test 1): a PLAIN struct
    /// self-reference — `type MenuItem = { label : string!, subMenu :
    /// [MenuItem] }`, no union involved — must fire the SAME RESOLVING cycle
    /// guard the DrawItem tagged-union recursion introduced it for. Before
    /// this feature the guard already covered `TypeExpr::Named` generically
    /// (not union-specific), so this pins that behavior rather than changing
    /// it: full expansion at the root, a by-name `Union` back-reference
    /// (`values: ["MenuItem"]`, no `properties`) at the recursion point.
    #[test]
    fn plain_struct_self_reference_lowers_to_full_expansion_plus_backref() {
        let mut aliases = BTreeMap::new();
        aliases.insert(
            "MenuItem".to_string(),
            TypeAliasResolved::Type(TypeExpr::Object(vec![
                (
                    "label".to_string(),
                    TypeExpr::Primitive("string".to_string()),
                    true,
                ),
                (
                    "subMenu".to_string(),
                    TypeExpr::Array(Box::new(TypeExpr::Named("MenuItem".to_string()))),
                    false,
                ),
            ])),
        );
        let (ft, _values, _items, props, _rel) = lower_type_expr(
            &TypeExpr::Named("MenuItem".to_string()),
            &aliases,
            &HashSet::new(),
        );
        assert!(
            matches!(ft, FieldType::Object),
            "root MenuItem expands fully to an object, got {:?}",
            ft
        );
        let props = props.expect("root struct carries its fields");
        let sub_menu = props.get("subMenu").expect("subMenu field present");
        assert!(matches!(sub_menu.field_type, FieldType::Array));
        let item = sub_menu
            .items
            .as_ref()
            .expect("subMenu is an array with an item schema");
        assert!(
            matches!(item.field_type, FieldType::Union),
            "the recursion point degrades to the by-name Union back-reference, got {:?}",
            item.field_type
        );
        assert_eq!(
            item.values.as_deref(),
            Some(["MenuItem".to_string()].as_slice()),
            "the back-reference names the alias it re-enters"
        );
        assert!(
            item.properties.is_none(),
            "a back-reference carries NO properties — it's a reference, not an expansion"
        );
        // The guard must be fully released after lowering completes, so a
        // SECOND top-level use of the same alias (e.g. a sibling field on the
        // same entity) also gets the full expansion, not a spurious
        // back-reference from a leaked guard entry.
        let (ft2, _, _, props2, _) = lower_type_expr(
            &TypeExpr::Named("MenuItem".to_string()),
            &aliases,
            &HashSet::new(),
        );
        assert!(matches!(ft2, FieldType::Object));
        assert!(
            props2.is_some(),
            "guard release lets a second use re-expand"
        );
    }

    /// DrawItem regression (feature test 4): the pre-existing TAGGED-UNION
    /// recursion — `DrawItem = DrawShape | DrawGroup`, `DrawGroup.items :
    /// [DrawItem]` — must lower to the EXACT same `.orb` wire shape it did
    /// before this feature: `FieldType::Union` at the root with variant names
    /// in `values` and shapes in `properties`, and the SAME by-name
    /// `FieldType::Union { values: ["DrawItem"], properties: None }`
    /// back-reference at the recursion point that a plain struct
    /// self-reference now also produces. This feature changes what
    /// `orbital-compiler`'s resolve phase does with that back-reference
    /// (`Json` → `TypeRef`) — never the lowered bytes themselves.
    #[test]
    fn tagged_union_self_reference_still_lowers_to_by_name_backref() {
        let mut aliases = BTreeMap::new();
        aliases.insert(
            "DrawShape".to_string(),
            TypeAliasResolved::Type(TypeExpr::Object(vec![(
                "kind".to_string(),
                TypeExpr::StringLiteral("shape".to_string()),
                true,
            )])),
        );
        aliases.insert(
            "DrawGroup".to_string(),
            TypeAliasResolved::Type(TypeExpr::Object(vec![
                (
                    "kind".to_string(),
                    TypeExpr::StringLiteral("group".to_string()),
                    true,
                ),
                (
                    "items".to_string(),
                    TypeExpr::Array(Box::new(TypeExpr::Named("DrawItem".to_string()))),
                    false,
                ),
            ])),
        );
        aliases.insert(
            "DrawItem".to_string(),
            TypeAliasResolved::Type(TypeExpr::Union(vec![
                TypeExpr::Named("DrawShape".to_string()),
                TypeExpr::Named("DrawGroup".to_string()),
            ])),
        );
        let (ft, values, _items, props, _rel) = lower_type_expr(
            &TypeExpr::Named("DrawItem".to_string()),
            &aliases,
            &HashSet::new(),
        );
        assert!(
            matches!(ft, FieldType::Union),
            "root DrawItem is a tagged union, got {:?}",
            ft
        );
        let names = values.expect("union carries variant names");
        assert_eq!(
            names,
            vec!["DrawShape".to_string(), "DrawGroup".to_string()]
        );
        let props = props.expect("union carries variant shapes");
        let group = props.get("DrawGroup").expect("DrawGroup variant present");
        let group_props = group.properties.as_ref().expect("DrawGroup fields");
        let items_field = group_props.get("items").expect("items field");
        let item = items_field
            .items
            .as_ref()
            .expect("items is an array with an element schema");
        assert!(
            matches!(item.field_type, FieldType::Union),
            "the recursion point re-enters DrawItem as a by-name back-reference, got {:?}",
            item.field_type
        );
        assert_eq!(
            item.values.as_deref(),
            Some(["DrawItem".to_string()].as_slice())
        );
        assert!(
            item.properties.is_none(),
            "a back-reference carries NO properties — unchanged from before this feature"
        );
    }
}

/// Does `parts` read as a TYPE union rather than a string enum?
///
/// **This is the single reading of `A | B | C`, and every site that has to tell
/// the two apart must call it.** The two readings are mutually exclusive and
/// wildly different — a type union accepts each member's VALUES, a string enum
/// accepts each member's NAME as a literal word — so a second, separately-worded
/// copy of this rule is not a duplicate but a fork: the field lowers as a union
/// while a checker still measures it against the vocabulary, and the diagnostic
/// contradicts the type. That is exactly what
/// `checks::enum_members_of` used to do.
///
/// The rule is ALL-OR-NOTHING by design. One member that names no known type
/// and the whole union stays the enum it was authored as. That is what protects
/// the live corpus: `format : date | currency | number | boolean | percent` has
/// three primitive-NAMED members but `currency` names no type, so it keeps its
/// vocabulary reading, and `type Direction = up | down | left | right` has none
/// at all. Swept before this shipped — exactly one corpus union had
/// all-primitive members, and it was inside a `@description` string.
///
/// A member counts as a type when it is a struct alias, a primitive name, or a
/// structural expression (`[string]`), which needs no alias to be unambiguous.
pub(crate) fn is_type_union(
    parts: &[TypeExpr],
    aliases: &BTreeMap<String, TypeAliasResolved>,
) -> bool {
    is_type_union_with(parts, |name| match aliases.get(name) {
        Some(TypeAliasResolved::Type(TypeExpr::Object(_))) => Some(true),
        Some(_) => Some(false),
        None => None,
    })
}

/// Core of [`is_type_union`], parameterized over alias lookup so the L1 check
/// phase (`types.rs`, which sees `ResolveCtx` rather than the lowered alias
/// map) applies the ONE predicate instead of keeping a copy.
/// `named_is_struct`: `Some(true)` = resolves to a struct alias, `Some(false)`
/// = resolves to any other alias kind, `None` = unknown name (falls through to
/// the primitive-name check).
pub(crate) fn is_type_union_with(
    parts: &[TypeExpr],
    named_is_struct: impl Fn(&str) -> Option<bool>,
) -> bool {
    parts.len() >= 2
        && parts.iter().all(|p| match p {
            TypeExpr::Named(name) => match named_is_struct(name) {
                Some(is_struct) => is_struct,
                None => primitive_field_type(name).is_some(),
            },
            TypeExpr::Primitive(name) => primitive_field_type(name).is_some(),
            TypeExpr::Array(_) => true,
            _ => false,
        })
}

/// The variants of a TYPE union, as `(variant name, variant shape)` in
/// declaration order. `None` when [`is_type_union`] says this is a string enum,
/// in which case the caller falls back to the enum collapse.
///
/// ⚠️ Recursion is carried BY NAME, not inlined. `DrawGroup.items : [DrawItem]`
/// refers back to the union being lowered; inlining it would recurse forever
/// (`TypeAliasResolved::Type` lowering has no cycle guard). `lower_type_expr`
/// is therefore called on the variant body with the union's own members marked
/// in-progress, and a back-reference lowers to `FieldType::Union` carrying
/// `values` (the names) and NO `properties` — a reference, not an expansion.
fn struct_union_variants(
    parts: &[TypeExpr],
    aliases: &BTreeMap<String, TypeAliasResolved>,
    entity_names: &HashSet<String>,
) -> Option<Vec<(String, FieldDefinition)>> {
    if parts.len() < 2 {
        return None;
    }
    if !is_type_union(parts, aliases) {
        return None;
    }
    let mut out = Vec::with_capacity(parts.len());
    for p in parts {
        let (variant_name, inner): (String, TypeExpr) = match p {
            TypeExpr::Named(name) => match aliases.get(name) {
                Some(TypeAliasResolved::Type(inner @ TypeExpr::Object(_))) => {
                    (name.clone(), inner.clone())
                }
                // A named PRIMITIVE (`string`, `scalar`, `node`, …).
                None if primitive_field_type(name).is_some() => {
                    (name.clone(), TypeExpr::Primitive(name.clone()))
                }
                _ => return None,
            },
            TypeExpr::Primitive(name) if primitive_field_type(name).is_some() => {
                (name.clone(), TypeExpr::Primitive(name.clone()))
            }
            // `[string]` and friends — a structural variant needs no alias.
            TypeExpr::Array(_) => ("array".to_string(), p.clone()),
            _ => return None,
        };
        let name = &variant_name;
        let inner = &inner;
        let (ft, values, items, props, relation) = lower_type_expr(inner, aliases, entity_names);
        out.push((
            name.clone(),
            FieldDefinition {
                merged_from: None,
                name: Some(name.clone()),
                field_type: ft,
                required: None,
                primary_key: None,
                unique: None,
                default: None,
                values,
                relation,
                items,
                properties: props,
                intrinsic: None,
                description: None,
                synonyms: None,
            },
        ));
    }
    Some(out)
}
