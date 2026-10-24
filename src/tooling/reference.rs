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

    const DOOR: &str = "app Door [version: \"1.0.0\"]\norbital DoorOrbital {\n  entity Door [runtime] {\n    id : string!\n  }\n  trait DoorLife -> Door {\n    initial: closed\n    state closed {\n      INIT -> closed\n      OPEN -> open\n    }\n    state open {\n      CLOSE -> closed\n    }\n    emits {\n      OPENED\n    }\n  }\n  page \"/door\" -> DoorLife\n}\n";

    const SHED: &str = "app Shed [version: \"1.0.0\"]\norbital ShedOrbital {\n  entity Shed [runtime] {\n    id : string!\n    label : string\n    size : number\n  }\n  trait ShedLife -> Shed {\n    initial: idle\n    state idle {\n      INIT -> idle\n    }\n  }\n  page \"/shed\" -> ShedLife\n}\n";

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

/// The language reference as installed language data (G-ORB-442): `@almadar/core`
/// ships these files under `orbLanguage.{vocabulary,constructs,validationCodes}` so a
/// `.lolo` program reads the parser's own lists. The Rust here stays the source of
/// truth; `language_reference_json_is_current` fails when the committed copy drifts.
#[cfg(test)]
mod language_data_export {
    use std::path::PathBuf;

    #[derive(serde::Serialize)]
    struct ValidationCode {
        code: &'static str,
        category: &'static str,
        description: &'static str,
    }

    fn pretty<T: serde::Serialize + ?Sized>(value: &T) -> String {
        format!(
            "{}\n",
            serde_json::to_string_pretty(value).expect("serializes")
        )
    }

    fn vocabulary_with_orb() -> serde_json::Value {
        let mut value = serde_json::to_value(crate::keywords::vocabulary()).expect("serializes");
        let orb = &orbital_core_base::i18n::tables::english().orb;
        value["orb"] = serde_json::to_value(orb).expect("serializes");
        value
    }

    fn validation_codes() -> Vec<ValidationCode> {
        orbital_compiler::phases::validation::ErrorCode::ALL
            .iter()
            .map(|c| ValidationCode {
                code: c.as_str(),
                category: c.category(),
                description: c.description(),
            })
            .collect()
    }

    /// The codes as a TS `as const` module, so a key read like
    /// `VALIDATION_CODES.ORB_ID_KIND_MISMATCH` is checked at compile time.
    fn validation_codes_ts() -> String {
        let mut out = String::from(
            "// GENERATED by `cargo test -p orbital-lolo --lib language_reference_json` (LANGUAGE_REFERENCE_WRITE=1) from\n\
             // orbital-compiler `ErrorCode::ALL`. Do not edit: change the Rust enum and regenerate.\n\n\
             export const VALIDATION_CODES = {\n",
        );
        for c in validation_codes() {
            out.push_str(&format!("  {0}: {0:?},\n", c.code));
        }
        out.push_str(
            "} as const;\n\nexport type ValidationCode = keyof typeof VALIDATION_CODES;\n",
        );
        out
    }

    fn protocols() -> serde_json::Value {
        serde_json::to_value(orbital_core_base::protocols::PROTOCOLS).expect("serializes")
    }

    /// `(file name, generated text)` for the datasets and the codes' TS module.
    fn generated() -> Vec<(&'static str, String)> {
        vec![
            ("vocabulary.json", pretty(&vocabulary_with_orb())),
            ("constructs.json", pretty(super::constructs())),
            ("protocols.json", pretty(&protocols())),
            ("validation-codes.json", pretty(&validation_codes())),
            ("validation-codes.ts", validation_codes_ts()),
        ]
    }

    fn ts_keys(ts: &str) -> Vec<String> {
        ts.lines()
            .filter_map(|l| l.strip_prefix("  ")?.split_once(':'))
            .map(|(k, _)| k.to_string())
            .collect()
    }

    #[test]
    fn validation_codes_ts_and_json_carry_the_same_codes() {
        let json: Vec<String> = validation_codes()
            .iter()
            .map(|c| c.code.to_string())
            .collect();
        let ts = validation_codes_ts();
        assert_eq!(ts_keys(&ts), json);
        assert!(ts.contains("  ORB_T_TYPE_UNKNOWN: \"ORB_T_TYPE_UNKNOWN\",\n"));

        let dropped = ts.replace("  ORB_T_TYPE_UNKNOWN: \"ORB_T_TYPE_UNKNOWN\",\n", "");
        assert_ne!(ts_keys(&dropped), json);
    }

    #[test]
    fn protocols_json_carries_every_table_row_with_its_address() {
        let rows = protocols();
        let rows = rows.as_array().expect("array");
        assert_eq!(
            rows.len(),
            orbital_core_base::protocols::PROTOCOLS.len()
        );
        let http = rows
            .iter()
            .find(|r| r["name"] == "http")
            .expect("http row");
        let address = http["address"].as_array().expect("http address");
        assert!(address
            .iter()
            .any(|f| f["name"] == "port" && f["required"] == true));
        assert_eq!(http["grant"], "net-listen");
        let stdio = rows
            .iter()
            .find(|r| r["name"] == "stdio")
            .expect("stdio row");
        assert!(stdio["address"].is_null());
    }

    fn core_dir() -> PathBuf {
        PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../orbital-shell-typescript/core"
        ))
    }

    fn generated_dir() -> PathBuf {
        core_dir().join("src/generated/language")
    }

    /// Every file whose committed text differs from `generated`, with the reason.
    fn drift(generated: &[(&str, String)], dir: &std::path::Path) -> Vec<String> {
        generated
            .iter()
            .filter_map(
                |(name, text)| match std::fs::read_to_string(dir.join(name)) {
                    Ok(committed) if &committed == text => None,
                    Ok(_) => Some(format!("{name} is stale")),
                    Err(_) => Some(format!("{name} is missing")),
                },
            )
            .collect()
    }

    const REGENERATE: &str =
        "LANGUAGE_REFERENCE_WRITE=1 cargo test -p orbital-lolo --lib language_reference_json";

    #[test]
    fn language_reference_json_is_current() {
        let dir = generated_dir();
        let files = generated();
        if std::env::var("LANGUAGE_REFERENCE_WRITE").as_deref() == Ok("1") {
            std::fs::create_dir_all(&dir).unwrap();
            for (name, text) in &files {
                std::fs::write(dir.join(name), text).unwrap();
            }
            return;
        }
        // A standalone orbital-rust clone has no @almadar/core to check against.
        if !core_dir().join("package.json").exists() {
            return;
        }
        let problems = drift(&files, &dir);
        assert!(problems.is_empty(), "{problems:?}: {REGENERATE}");
    }

    #[test]
    fn language_reference_json_drift_catches_a_changed_keyword() {
        let dir = tempfile::tempdir().unwrap();
        let files = generated();
        for (name, text) in &files {
            std::fs::write(dir.path().join(name), text).unwrap();
        }
        assert!(drift(&files, dir.path()).is_empty());

        let vocabulary = crate::keywords::vocabulary();
        let mut keywords: Vec<&str> = vocabulary.keywords.to_vec();
        keywords.push("zzNotAKeyword");
        let mut changed = vocabulary_with_orb();
        changed["keywords"] = serde_json::json!(keywords);
        let mut mutated = files.clone();
        mutated[0].1 = pretty(&changed);
        assert_eq!(
            drift(&mutated, dir.path()),
            vec!["vocabulary.json is stale".to_string()]
        );

        std::fs::remove_file(dir.path().join("constructs.json")).unwrap();
        assert_eq!(
            drift(&files, dir.path()),
            vec!["constructs.json is missing".to_string()]
        );
    }
}
