//! Orbital-import body additions that retire the factory parameter layer:
//! the entity-header bracket on the `entity` line (persistence + collection),
//! `retype { … }`, and per-imported-trait `traits { Up { … } }` overrides.

use orbital_core_base::schema::{EventScope, Persistence};
use orbital_lolo::{format_program, parse_and_lower_str, parse_str};

const HEAD: &str = r#"uses Notes from "almadar-behaviors/std-notes"

"#;

fn import(body: &str) -> String {
    format!("{HEAD}orbital NoteImport = Notes.orbitals.NoteOrbital {{\n{body}}}\n")
}

fn reference(source: &str) -> orbital_core_base::schema::OrbitalRefObject {
    parse_and_lower_str(source).expect("lower").orbitals[0]
        .reference
        .clone()
        .expect("reference object")
}

fn round_trips(source: &str) -> String {
    let program = parse_str(source).expect("parse");
    let printed = format_program(&program);
    parse_str(&printed).unwrap_or_else(|e| panic!("formatted source must parse: {e:?}\n{printed}"));
    let reprinted = format_program(&parse_str(&printed).expect("reparse"));
    assert_eq!(printed, reprinted, "formatting is idempotent");
    printed
}

#[test]
fn entity_line_bracket_overrides_persistence_and_collection() {
    let r = reference(&import("  entity Note [persistent: notes]\n"));
    assert_eq!(r.entity.as_deref(), Some("Note"));
    assert_eq!(r.persistence, Some(Persistence::Persistent));
    assert_eq!(r.collection.as_deref(), Some("notes"));
    assert!(!r.browser && !r.shared && !r.identity);
}

#[test]
fn entity_line_bracket_runtime_flags_and_bare_persistent() {
    let r = reference(&import("  entity Note [runtime]\n"));
    assert_eq!(r.persistence, Some(Persistence::Runtime));
    assert_eq!(r.collection, None);

    let r = reference(&import("  entity Note [persistent]\n"));
    assert_eq!(r.persistence, Some(Persistence::Persistent));
    assert_eq!(r.collection, None);

    let r = reference(&import(
        "  entity Note [persistent: notes, browser, shared]\n",
    ));
    assert_eq!(r.collection.as_deref(), Some("notes"));
    assert!(r.browser && r.shared && !r.identity);

    let r = reference(&import("  entity Note [browser]\n"));
    assert_eq!(r.persistence, None);
    assert!(r.browser);
}

#[test]
fn entity_line_bracket_without_a_name_keeps_the_default_rebind() {
    let r = reference(&import("  entity [persistent: notes]\n"));
    assert_eq!(r.entity, None);
    assert_eq!(r.collection.as_deref(), Some("notes"));
}

#[test]
fn control_entity_line_without_bracket_lowers_no_persistence() {
    let r = reference(&import("  entity Note\n"));
    assert_eq!(r.entity.as_deref(), Some("Note"));
    assert_eq!(r.persistence, None);
    assert_eq!(r.collection, None);
    let json = serde_json::to_value(&r).expect("serializes");
    for key in [
        "persistence",
        "collection",
        "shared",
        "identity",
        "browser",
        "retype",
        "traits",
    ] {
        assert!(json.get(key).is_none(), "{key} must be absent: {json}");
    }
}

#[test]
fn entity_line_bracket_retired_words_error_with_their_fix_its() {
    for (word, fixit) in [("local", "write `browser`"), ("mock", "delete it")] {
        let err = orbital_lolo::parse_and_lower_str(&import(&format!("  entity Note [{word}]\n")))
            .unwrap_err()
            .to_string();
        assert!(err.contains("ORB_E_HEADER_WORD_RETIRED") && err.contains(fixit), "{word}: {err}");
    }
}

#[test]
fn entity_line_bracket_rejects_an_unknown_tag_and_conflicting_modes() {
    assert!(parse_str(&import("  entity Note [bogus]\n")).is_err());
    assert!(parse_str(&import("  entity Note [persistent, runtime]\n")).is_err());
}

#[test]
fn entity_line_bracket_round_trips_through_format() {
    for line in [
        "entity Note [persistent: notes]",
        "entity Note [runtime]",
        "entity Note [persistent: notes, browser]",
        "entity [persistent: notes]",
    ] {
        let printed = round_trips(&import(&format!("  {line}\n")));
        assert!(
            printed.contains(line),
            "{line} not printed back:\n{printed}"
        );
    }
}

#[test]
fn retype_parses_lowers_and_round_trips() {
    let source =
        import("  retype {\n    title : string = \"untitled\"\n    pinned : boolean = true\n  }\n");
    let program = parse_str(&source).expect("parse");
    assert_eq!(program.orbitals[0].retype_fields.len(), 2);
    let r = reference(&source);
    let retype = r.retype.expect("retype lowered");
    assert_eq!(retype.len(), 2);
    assert_eq!(retype[0].name.as_deref(), Some("title"));
    assert_eq!(retype[1].name.as_deref(), Some("pinned"));
    assert!(r.extend.is_none());
    let printed = round_trips(&source);
    assert!(printed.contains("retype {"), "{printed}");
}

#[test]
fn extend_and_retype_are_independent_blocks() {
    let source =
        import("  extend {\n    dueDate : date = null\n  }\n  retype {\n    title : string\n  }\n");
    let r = reference(&source);
    assert_eq!(r.extend.expect("extend").len(), 1);
    assert_eq!(r.retype.expect("retype").len(), 1);
}

#[test]
fn control_an_import_without_retype_lowers_none() {
    assert!(reference(&import("")).retype.is_none());
}

const TRAITS: &str = "  traits {
    NoteList {
      events {
        NOTE_OPEN: NOTE_VIEW
      }
      fields {
        title: subject
      }
      config {
        pageSize: 50
      }
      emitsScope internal
    }
    NoteForm -> Draft {
    }
  }
";

#[test]
fn traits_block_lowers_each_entry_keyed_by_upstream_name() {
    let r = reference(&import(TRAITS));
    let traits = r.traits.expect("traits lowered");
    assert_eq!(traits.len(), 2);

    let list = &traits["NoteList"];
    assert_eq!(
        list.events.as_ref().expect("events")["NOTE_OPEN"],
        "NOTE_VIEW"
    );
    assert_eq!(list.fields.as_ref().expect("fields")["title"], "subject");
    assert!(list
        .config
        .as_ref()
        .expect("config")
        .contains_key("pageSize"));
    assert_eq!(list.emits_scope, Some(EventScope::Internal));
    assert_eq!(list.linked_entity, None);
    assert!(list.listens.is_none());

    let form = &traits["NoteForm"];
    assert_eq!(form.linked_entity.as_deref(), Some("Draft"));
    assert!(form.config.is_none() && form.events.is_none() && form.fields.is_none());
}

#[test]
fn traits_block_round_trips_through_format() {
    let printed = round_trips(&import(TRAITS));
    assert!(printed.contains("traits {"), "{printed}");
    assert!(printed.contains("NoteForm -> Draft {"), "{printed}");
    let program = parse_str(&printed).expect("reparse");
    assert_eq!(program.orbitals[0].trait_overrides.len(), 2);
    assert_eq!(
        program.orbitals[0].trait_overrides[0].trait_name,
        "NoteList"
    );
}

#[test]
fn traits_block_rejects_a_duplicate_trait_and_a_name_override() {
    let dup = import("  traits {\n    NoteList {\n    }\n    NoteList {\n    }\n  }\n");
    assert!(parse_str(&dup).is_err());
    let named = import("  traits {\n    NoteList {\n      name: \"Other\"\n    }\n  }\n");
    assert!(parse_str(&named).is_err());
}

#[test]
fn traits_block_rejects_an_unknown_body_key() {
    assert!(parse_str(&import(
        "  traits {\n    NoteList {\n      bogus { }\n    }\n  }\n"
    ))
    .is_err());
}

const TRAIT_MODES: &str = "  traits {
    NoteList [optimistic]
    NoteForm -> Draft [server] {
      config {
        pageSize: 5
      }
    }
    NoteView [client]
    NoteBare {
    }
  }
";

#[test]
fn traits_block_entries_take_a_dispatch_word_with_or_without_a_body() {
    use orbital_core_base::schema::DispatchMode;
    let traits = reference(&import(TRAIT_MODES)).traits.expect("traits lowered");
    assert_eq!(traits["NoteList"].dispatch, Some(DispatchMode::Optimistic));
    assert_eq!(traits["NoteForm"].dispatch, Some(DispatchMode::Server));
    assert_eq!(traits["NoteForm"].linked_entity.as_deref(), Some("Draft"));
    assert!(traits["NoteForm"].config.is_some());
    assert_eq!(traits["NoteView"].dispatch, Some(DispatchMode::Client));
    assert_eq!(traits["NoteBare"].dispatch, None);
}

#[test]
fn traits_block_dispatch_serializes_and_round_trips_through_format() {
    let json = serde_json::to_value(reference(&import(TRAIT_MODES))).unwrap();
    assert_eq!(json["traits"]["NoteList"]["dispatch"], "optimistic", "{json}");
    assert!(json["traits"]["NoteBare"].get("dispatch").is_none(), "{json}");
    let printed = round_trips(&import(TRAIT_MODES));
    assert!(printed.contains("NoteList [optimistic]"), "{printed}");
    assert!(printed.contains("NoteForm -> Draft [server] {"), "{printed}");
    assert!(printed.contains("NoteView [client]"), "{printed}");
    let again = reference(&printed).traits.expect("traits");
    assert_eq!(again["NoteList"].dispatch, Some(orbital_core_base::schema::DispatchMode::Optimistic));
}

#[test]
fn traits_block_dispatch_refuses_two_modes_and_retired_words() {
    assert!(parse_str(&import("  traits {\n    NoteList [client, server]\n  }\n")).is_err());
    let err = parse_and_lower_str(&import("  traits {\n    NoteList [local]\n  }\n"))
        .unwrap_err()
        .to_string();
    assert!(err.contains("ORB_T_HEADER_WORD_RETIRED") && err.contains("write `[client]`"), "{err}");
}

#[test]
fn control_an_import_without_traits_lowers_none() {
    assert!(reference(&import("  entity Note\n")).traits.is_none());
}
