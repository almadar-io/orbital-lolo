use super::*;
use orbital_core_base::lang::protocols::{self, ProtocolSide};

/// Dispatch a `Page` AST node to either an inline definition or a reference,
/// returning the `PageRef` shape that the core schema expects.
pub(crate) fn lower_page_decl(p: &Page) -> Result<PageRef, LowerError> {
    check_protocol(p)?;
    if let Some(reference) = &p.reference {
        let traits = if p.traits.is_empty() {
            None
        } else {
            Some(
                p.traits
                    .iter()
                    .map(|name| TraitReference::Reference {
                        dispatch: None,
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
        Ok(PageRef::Object(PageRefObject {
            protocol: p.protocol.clone(),
            address: p.address.clone(),
            translation_of: p.translation_of.clone(),
            reference: reference.clone(),
            path: (!p.path.is_empty()).then(|| p.path.clone()),
            linked_entity: p.primary_entity.clone(),
            traits,
            linked_entity_id: None,
            ref_id: None,
            trait_ref_ids: None,
            access: p.access,
            indexing: p.indexing,
            title: p.title.clone(),
            description: p.description.clone(),
            label: p.label.clone(),
            icon: p.icon.clone(),
            roles: p.roles.clone(),
        }))
    } else {
        lower_page(p)
    }
}

fn protocol_error(code: &'static str, message: String) -> LowerError {
    LowerError::Coded { code, message }
}

fn check_protocol(p: &Page) -> Result<(), LowerError> {
    let Some(word) = &p.protocol else {
        return Ok(());
    };
    let spec = protocols::protocol(word).ok_or_else(|| {
        protocol_error(
            "ORB_P_UNKNOWN_PROTOCOL",
            format!(
                "unknown page protocol `{word}`: the protocols are {}",
                written_protocols()
            ),
        )
    })?;
    if matches!(spec.side, ProtocolSide::Client) {
        return Err(protocol_error(
            "ORB_P_UNKNOWN_PROTOCOL",
            format!("`{word}` is never written as a protocol word: give the page a string path (`page \"/path\" …`)"),
        ));
    }
    match (spec.address, &p.address) {
        (None, Some(_)) => Err(protocol_error(
            "ORB_P_ADDRESS_UNEXPECTED",
            format!("page protocol `{word}` takes no address: delete the `{{ … }}`"),
        )),
        (Some(fields), None) if p.reference.is_none() && fields.iter().any(|f| f.required) => {
            let required: Vec<&str> = fields
                .iter()
                .filter(|f| f.required)
                .map(|f| f.name)
                .collect();
            Err(protocol_error(
                "ORB_P_ADDRESS_REQUIRED",
                format!(
                    "page protocol `{word}` needs an address record with {}: write `page {word} {{ … }}`",
                    required.join(", ")
                ),
            ))
        }
        _ => Ok(()),
    }
}

fn written_protocols() -> String {
    protocols::PROTOCOLS
        .iter()
        .filter(|row| !matches!(row.side, ProtocolSide::Client))
        .map(|row| format!("`{}`", row.name))
        .collect::<Vec<_>>()
        .join(", ")
}

pub(crate) fn lower_page(p: &Page) -> Result<PageRef, LowerError> {
    let name = match (&p.name, &p.protocol) {
        (Some(name), _) => name.clone(),
        (None, None) => page_name_from_path(&p.path),
        (None, Some(word)) => {
            return Err(protocol_error(
                "ORB_P_PROTOCOL_PAGE_UNNAMED",
                format!("a `{word}` page has no path to name it from: write `page {word} … as Name -> …`"),
            ))
        }
    };
    Ok(PageRef::Inline(PageDefinition {
        protocol: p.protocol.clone(),
        address: p.address.clone(),
        translation_of: p.translation_of.clone(),
        source_page: None,
        name,
        path: p.path.clone(),
        view_type: p.view.clone(),
        primary_entity: p.primary_entity.clone(),
        is_initial: if p.is_initial { Some(true) } else { None },
        traits: Some(
            p.traits
                .iter()
                .map(|name| TraitReference::Reference {
                    dispatch: None,
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
        roles: p.roles.clone(),
        id: None,
        access: p.access,
        indexing: p.indexing,
        title: p.title.clone(),
        description: p.description.clone(),
    }))
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
