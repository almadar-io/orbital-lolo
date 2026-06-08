use super::*;

pub(crate) fn lower_entity(
    entity: &Entity,
    aliases: &BTreeMap<String, TypeAliasResolved>,
    entity_names: &HashSet<String>,
) -> Result<EntityDefinition, LowerError> {
    let mut fields = Vec::with_capacity(entity.fields.len());
    for f in &entity.fields {
        fields.push(lower_field(f, aliases, entity_names)?);
    }
    let persistence = match &entity.persistence {
        Some(Persistence::Persistent) => Some(CorePersistence::Persistent),
        Some(Persistence::Runtime) => Some(CorePersistence::Runtime),
        Some(Persistence::Singleton) => Some(CorePersistence::Singleton),
        Some(Persistence::Instance) => Some(CorePersistence::Instance),
        Some(Persistence::Local) => Some(CorePersistence::Runtime),
        None => None,
    };
    Ok(EntityDefinition {
        name: entity.name.clone(),
        collection: entity.collection.clone(),
        persistence,
        fields,
        instances: None,
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
        TypeExpr::Trait => (FieldType::Trait, None, None, None, None),
        TypeExpr::Slot => (FieldType::Slot, None, None, None, None),
        TypeExpr::Pattern => (FieldType::Pattern, None, None, None, None),
        TypeExpr::Named(name) => {
            if let Some(alias) = aliases.get(name) {
                match alias {
                    TypeAliasResolved::Enum(values) => {
                        (FieldType::String, Some(values.clone()), None, None, None)
                    }
                    TypeAliasResolved::Type(inner) => lower_type_expr(inner, aliases, entity_names),
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
                    }),
                )
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
                        }),
                    );
                }
            }
            let (inner_ft, inner_values, inner_items, inner_props, _inner_rel) =
                lower_type_expr(inner, aliases, entity_names);
            let item = FieldDefinition {
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
            (FieldType::Object, None, Some(Box::new(value_field)), None, None)
        }
        TypeExpr::Object(fields) => {
            let mut props = HashMap::new();
            for (k, v, req) in fields {
                let (ft, values, items, properties, relation) =
                    lower_type_expr(v, aliases, entity_names);
                props.insert(
                    k.clone(),
                    FieldDefinition {
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
        TypeExpr::StringLiteral(_) => {
            // A bare literal type outside a union is degenerate — lower to
            // plain string. (Inside a union the `Union` branch above collects
            // all literals into `values`.)
            (FieldType::String, None, None, None, None)
        }
        TypeExpr::Optional(inner) => lower_type_expr(inner, aliases, entity_names),
        TypeExpr::RelationMany(_) | TypeExpr::RelationManyNonEmpty(_) => {
            (FieldType::Array, None, None, None, None)
        }
    }
}

pub(crate) fn primitive_to_field_type(name: &str) -> FieldType {
    match name {
        "string" | "money" | "any" => FieldType::String,
        "int" | "float" | "number" | "duration" => FieldType::Number,
        "bool" | "boolean" => FieldType::Boolean,
        "date" => FieldType::Date,
        "datetime" => FieldType::Datetime,
        "timestamp" => FieldType::Timestamp,
        "object" => FieldType::Object,
        _ => FieldType::String,
    }
}
