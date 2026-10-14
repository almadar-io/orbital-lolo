//! The `.lolo` language reference, generated from the parser's own code: every AST
//! construct with its doc comment and fields (from `ast.rs`), and the vocabulary
//! (from `keywords.rs`).

/// One field of an AST construct.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct ConstructField {
    pub name: &'static str,
    #[serde(rename = "type")]
    pub ty: &'static str,
    pub doc: &'static str,
}

/// One AST construct: a struct (fields) or an enum (variants, listed as fields with no type).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct Construct {
    pub name: &'static str,
    pub kind: &'static str,
    pub doc: &'static str,
    pub fields: &'static [ConstructField],
}

include!(concat!(env!("OUT_DIR"), "/ast_reference.rs"));

/// Every construct of the `.lolo` AST, in declaration order.
pub fn constructs() -> &'static [Construct] {
    CONSTRUCTS
}

/// The construct named `name` (`Entity`, `Trait`, …).
pub fn construct(name: &str) -> Option<&'static Construct> {
    CONSTRUCTS.iter().find(|c| c.name == name)
}

/// Real occurrences of the construct `name` in `programs`, printed by the formatter,
/// shortest first.
pub fn examples(name: &str, programs: &[crate::ast::Program]) -> Vec<String> {
    use crate::format::{format_node, Node};
    let mut out: Vec<String> = Vec::new();
    let mut add = |node: Node<'_>| out.push(format_node(node));
    for program in programs {
        let orbitals = program.orbitals.iter();
        match name {
            "Orbital" => orbitals.for_each(|o| add(Node::Orbital(o))),
            "UseDecl" => program
                .uses
                .iter()
                .chain(orbitals.flat_map(|o| o.uses.iter()))
                .for_each(|u| add(Node::UseDecl(u))),
            "ExpectsDecl" => orbitals
                .flat_map(|o| o.expects.iter())
                .for_each(|e| add(Node::ExpectsDecl(e))),
            "TypeAlias" => program
                .type_aliases
                .iter()
                .chain(orbitals.flat_map(|o| o.type_aliases.iter()))
                .for_each(|t| add(Node::TypeAlias(t))),
            "Entity" => orbitals
                .flat_map(|o| o.entity.iter().chain(o.extra_entities.iter()))
                .for_each(|e| add(Node::Entity(e))),
            "EntityField" => orbitals
                .flat_map(|o| o.entity.iter().chain(o.extra_entities.iter()))
                .flat_map(|e| e.fields.iter())
                .for_each(|f| add(Node::EntityField(f))),
            "Page" => orbitals
                .flat_map(|o| o.pages.iter())
                .for_each(|p| add(Node::Page(p))),
            _ => {
                for t in orbitals.flat_map(|o| o.traits.iter()) {
                    match name {
                        "Trait" => add(Node::Trait(t)),
                        "State" => t.states.iter().for_each(|s| add(Node::State(s))),
                        "Transition" => t
                            .states
                            .iter()
                            .flat_map(|s| s.transitions.iter())
                            .for_each(|tr| add(Node::Transition(tr))),
                        "Emit" => t.emits.iter().for_each(|e| add(Node::Emit(e))),
                        "Listen" => t.listens.iter().for_each(|l| add(Node::Listen(l))),
                        "Tick" => t.ticks.iter().for_each(|k| add(Node::Tick(k))),
                        "ConfigField" => t.config.iter().for_each(|c| add(Node::ConfigField(c))),
                        _ => {}
                    }
                }
            }
        }
    }
    out.sort_by_key(String::len);
    out.dedup();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_construct_carries_its_doc_comment() {
        let undocumented: Vec<&str> = constructs()
            .iter()
            .filter(|c| c.doc.trim().is_empty())
            .map(|c| c.name)
            .collect();
        assert!(
            undocumented.is_empty(),
            "undocumented AST constructs: {undocumented:?}"
        );
    }

    #[test]
    fn a_struct_lists_its_fields_with_types() {
        let trait_ = construct("Trait").expect("Trait");
        assert_eq!(trait_.kind, "struct");
        let name = trait_
            .fields
            .iter()
            .find(|f| f.name == "name")
            .expect("Trait.name");
        assert_eq!(name.ty, "String");
    }

    #[test]
    fn an_enum_lists_its_variants() {
        let persistence = construct("Persistence").expect("Persistence");
        assert_eq!(persistence.kind, "enum");
        assert!(!persistence.fields.is_empty());
    }

    const DOOR: &str = "app Door [version: \"1.0.0\"]\norbital DoorOrbital {\n  entity Door [runtime] {\n    id : string!\n  }\n  trait DoorLife -> Door [interaction] {\n    initial: closed\n    state closed {\n      INIT -> closed\n      OPEN -> open\n    }\n    state open {\n      CLOSE -> closed\n    }\n    emits {\n      OPENED\n    }\n  }\n  page \"/door\" -> DoorLife\n}\n";

    const SHED: &str = "app Shed [version: \"1.0.0\"]\norbital ShedOrbital {\n  entity Shed [runtime] {\n    id : string!\n    label : string\n    size : number\n  }\n  trait ShedLife -> Shed [interaction] {\n    initial: idle\n    state idle {\n      INIT -> idle\n    }\n  }\n  page \"/shed\" -> ShedLife\n}\n";

    fn program(source: &str) -> crate::ast::Program {
        crate::parse_str(source).expect("parses")
    }

    #[test]
    fn examples_are_real_nodes_printed_by_the_formatter() {
        let programs = [program(DOOR)];
        let entities = examples("Entity", &programs);
        assert_eq!(entities.len(), 1);
        assert!(
            entities[0].starts_with("entity Door [runtime]"),
            "{}",
            entities[0]
        );
        let states = examples("State", &programs);
        assert!(
            states.iter().any(|s| s.contains("OPEN -> open")),
            "{states:?}"
        );
    }

    #[test]
    fn examples_pick_the_shortest_occurrence_first() {
        let programs = [program(SHED), program(DOOR)];
        let entities = examples("Entity", &programs);
        assert!(entities[0].starts_with("entity Door"), "{entities:?}");
    }

    #[test]
    fn a_construct_with_no_occurrence_has_no_example() {
        assert!(examples("Tick", &[program(DOOR)]).is_empty());
        assert!(examples("NoSuchConstruct", &[program(DOOR)]).is_empty());
    }

    #[test]
    fn an_unknown_construct_is_none() {
        assert!(construct("NoSuchConstruct").is_none());
    }
}
