use super::*;

/// Dispatch a `Page` AST node to either an inline definition or a reference,
/// returning the `PageRef` shape that the core schema expects.
pub(crate) fn lower_page_decl(p: &Page) -> PageRef {
    if let Some(reference) = &p.reference {
        let traits = if p.traits.is_empty() {
            None
        } else {
            Some(
                p.traits
                    .iter()
                    .map(|name| TraitReference::Reference {
                        local: false,
                        id: None,
                        reference: Some(name.clone()),
                        name: None,
                        linked_entity: None,
                        config: None,
                        events: None,
                        fields: None,
                        listens: None,
                        emits_scope: None,
                        event_ids: None,
                        linked_entity_id: None,
                        ref_id: None,
                        type_args: None,
                    })
                    .collect(),
            )
        };
        PageRef::Object(PageRefObject {
            reference: reference.clone(),
            path: Some(p.path.clone()),
            linked_entity: p.primary_entity.clone(),
            traits,
            linked_entity_id: None,
            ref_id: None,
            trait_ref_ids: None,
        })
    } else {
        lower_page(p)
    }
}

pub(crate) fn lower_page(p: &Page) -> PageRef {
    PageRef::Inline(PageDefinition {
        name: p
            .name
            .clone()
            .unwrap_or_else(|| page_name_from_path(&p.path)),
        path: p.path.clone(),
        view_type: p.view.clone(),
        primary_entity: p.primary_entity.clone(),
        is_initial: if p.is_initial { Some(true) } else { None },
        traits: Some(
            p.traits
                .iter()
                .map(|name| TraitReference::Reference {
                    local: false,
                    id: None,
                    reference: Some(name.clone()),
                    name: None,
                    linked_entity: None,
                    config: None,
                    events: None,
                    fields: None,
                    listens: None,
                    emits_scope: None,
                    event_ids: None,
                    linked_entity_id: None,
                    ref_id: None,
                    type_args: None,
                })
                .collect(),
        ),
        sections: None,
        icon: p.icon.clone(),
        label: p.label.clone(),
        id: None,
    })
}

pub(crate) fn page_name_from_path(path: &str) -> String {
    let trimmed = path.trim_start_matches('/').trim_end_matches('/');
    if trimmed.is_empty() {
        return "Home".to_string();
    }
    let mut name = String::new();
    let mut upper = true;
    for c in trimmed.chars() {
        if c == '/' || c == '-' || c == '_' || c == ':' {
            upper = true;
        } else if upper {
            name.push(c.to_ascii_uppercase());
            upper = false;
        } else {
            name.push(c);
        }
    }
    if name.is_empty() {
        "Home".to_string()
    } else {
        name
    }
}
