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

    #[derive(serde::Serialize, Clone, PartialEq, Debug)]
    struct ValidationCode {
        code: String,
        category: String,
        description: String,
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

    const STD_ATOMS: &str = "packages/almadar-std/behaviors/lolo/cli/atoms";

    /// The std atoms directory of the installed language packages: `ALMADAR_ROOT` when set, else the nearest
    /// ancestor of this checkout that carries the monorepo `packages/`.
    fn std_atoms_dir() -> Option<PathBuf> {
        if let Ok(root) = std::env::var("ALMADAR_ROOT") {
            let dir = PathBuf::from(root).join(STD_ATOMS);
            return dir.is_dir().then_some(dir);
        }
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .map(|a| a.join(STD_ATOMS))
            .find(|p| p.is_dir())
    }

    /// Every code declaration in a lowered program: the `default` of a trait's `codes` knob, one
    /// `{ code, category, description }` per entry.
    fn declared_codes(program: &serde_json::Value) -> Vec<ValidationCode> {
        let mut out = Vec::new();
        for orbital in program["orbitals"].as_array().into_iter().flatten() {
            for t in orbital["traits"].as_array().into_iter().flatten() {
                let Some(entries) = t["config"]["codes"]["default"].as_object() else {
                    continue;
                };
                for decl in entries.values() {
                    out.push(ValidationCode {
                        code: decl["code"].as_str().unwrap_or_default().to_string(),
                        category: decl["category"].as_str().unwrap_or_default().to_string(),
                        description: decl["description"].as_str().unwrap_or_default().to_string(),
                    });
                }
            }
        }
        out
    }

    /// `(atom file name, lowered program)` for every std cli atom, in name order.
    fn std_programs(dir: &std::path::Path) -> Vec<(String, serde_json::Value)> {
        let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.extension().is_some_and(|e| e == "lolo"))
            .collect();
        files.sort();
        files
            .into_iter()
            .map(|p| {
                let schema = crate::parse_and_lower(&p)
                    .unwrap_or_else(|e| panic!("{} does not lower: {e:?}", p.display()));
                (
                    p.file_name().unwrap().to_string_lossy().to_string(),
                    serde_json::to_value(&schema).expect("serializes"),
                )
            })
            .collect()
    }

    /// The registry: the codes Rust phases raise that no std atom declares, then every std-declared
    /// code sorted by code. Two atoms may declare one code only with an identical declaration.
    fn registry(
        rust: Vec<ValidationCode>,
        std_decls: Vec<(String, ValidationCode)>,
    ) -> Result<Vec<ValidationCode>, String> {
        let mut declared: std::collections::BTreeMap<String, (String, ValidationCode)> =
            std::collections::BTreeMap::new();
        for (atom, decl) in std_decls {
            if decl.code.is_empty()
                || decl.category.is_empty()
                || decl.description.trim().is_empty()
            {
                return Err(format!(
                    "{atom}: an incomplete declaration of {:?}",
                    decl.code
                ));
            }
            match declared.get(&decl.code) {
                Some((first, seen)) if seen != &decl => {
                    return Err(format!(
                        "{} is declared differently by {first} and {atom}",
                        decl.code
                    ))
                }
                Some(_) => {}
                None => {
                    declared.insert(decl.code.clone(), (atom, decl));
                }
            }
        }
        let mut out: Vec<ValidationCode> = rust
            .into_iter()
            .filter(|c| !declared.contains_key(&c.code))
            .collect();
        out.extend(declared.into_values().map(|(_, d)| d));
        Ok(out)
    }

    fn rust_codes() -> Vec<ValidationCode> {
        orbital_compiler::phases::validation::ErrorCode::ALL
            .iter()
            .map(|c| ValidationCode {
                code: c.as_str().to_string(),
                category: c.category().to_string(),
                description: c.description().to_string(),
            })
            .collect()
    }

    fn validation_codes() -> Vec<ValidationCode> {
        let std_decls = std_atoms_dir()
            .map(|dir| {
                std_programs(&dir)
                    .iter()
                    .flat_map(|(name, program)| {
                        declared_codes(program)
                            .into_iter()
                            .map(|d| (name.clone(), d))
                    })
                    .collect()
            })
            .unwrap_or_default();
        registry(rust_codes(), std_decls).unwrap_or_else(|e| panic!("{e}"))
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
            out.push_str(&format!("  {0:?}: {0:?},\n", c.code));
        }
        out.push_str(
            "} as const;\n\nexport type ValidationCode = keyof typeof VALIDATION_CODES;\n",
        );
        out
    }

    /// `(file name, generated text)` for the datasets and the codes' TS module.
    fn generated() -> Vec<(&'static str, String)> {
        vec![
            ("vocabulary.json", pretty(&vocabulary_with_orb())),
            ("constructs.json", pretty(super::constructs())),
            ("validation-codes.json", pretty(&validation_codes())),
            ("validation-codes.ts", validation_codes_ts()),
        ]
    }

    fn ts_keys(ts: &str) -> Vec<String> {
        ts.lines()
            .filter_map(|l| l.strip_prefix("  ")?.split_once(':'))
            .map(|(k, _)| k.trim_matches('"').to_string())
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
        assert!(ts.contains("  \"ORB_T_TYPE_UNKNOWN\": \"ORB_T_TYPE_UNKNOWN\",\n"));

        let dropped = ts.replace("  \"ORB_T_TYPE_UNKNOWN\": \"ORB_T_TYPE_UNKNOWN\",\n", "");
        assert_ne!(ts_keys(&dropped), json);
    }

    fn code_shaped(s: &str) -> bool {
        s.contains('_')
            && s.starts_with(|c: char| c.is_ascii_uppercase())
            && s.chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
    }

    fn code_literals(value: &serde_json::Value, out: &mut Vec<String>) {
        match value {
            serde_json::Value::String(s) if code_shaped(s) => out.push(s.clone()),
            serde_json::Value::Array(a) => a.iter().for_each(|v| code_literals(v, out)),
            serde_json::Value::Object(o) => o.values().for_each(|v| code_literals(v, out)),
            _ => {}
        }
    }

    fn finding_code_problems(
        value: &serde_json::Value,
        declared: &[String],
        out: &mut Vec<String>,
    ) {
        match value {
            serde_json::Value::String(s) => {
                if let Some(key) = s
                    .strip_prefix("@config.codes.")
                    .and_then(|c| c.strip_suffix(".code"))
                {
                    if !declared.iter().any(|d| d == key) {
                        out.push(format!("{s} (not declared)"));
                    }
                }
            }
            serde_json::Value::Object(o) => {
                if let Some(code) = o.get("code") {
                    let mut literals = Vec::new();
                    code_literals(code, &mut literals);
                    out.extend(literals.into_iter().map(|l| format!("{l} (literal)")));
                }
                o.values()
                    .for_each(|v| finding_code_problems(v, declared, out));
            }
            serde_json::Value::Array(a) => a
                .iter()
                .for_each(|v| finding_code_problems(v, declared, out)),
            _ => {}
        }
    }

    fn trait_problems(t: &serde_json::Value) -> Vec<String> {
        let declared: Vec<String> = t["config"]["codes"]["default"]
            .as_object()
            .map(|m| m.keys().cloned().collect())
            .unwrap_or_default();
        let mut out = Vec::new();
        let mut walked = t.clone();
        walked["config"] = serde_json::Value::Null;
        finding_code_problems(&walked, &declared, &mut out);
        out
    }

    #[test]
    fn a_finding_reads_its_code_from_its_atoms_declared_codes_knob() {
        let Some(dir) = std_atoms_dir() else { return };
        let mut bad = Vec::new();
        for (name, program) in std_programs(&dir) {
            for orbital in program["orbitals"].as_array().into_iter().flatten() {
                for t in orbital["traits"].as_array().into_iter().flatten() {
                    for p in trait_problems(t) {
                        bad.push(format!("{name}: {p}"));
                    }
                }
            }
        }
        assert!(
            bad.is_empty(),
            "findings whose code is not a declared codes-knob entry: {bad:?}"
        );
    }

    #[test]
    fn control_a_literal_or_undeclared_finding_code_is_refused() {
        let finding = |code: &str| {
            serde_json::json!({ "config": { "codes": { "default": { "known": {} } } },
                "fx": [{ "code": code, "path": "p", "message": "m", "severity": "error" }] })
        };
        assert!(trait_problems(&finding("@config.codes.known.code")).is_empty());
        assert_eq!(trait_problems(&finding("ORB_LITERAL")).len(), 1);
        let branched = serde_json::json!({ "config": { "codes": { "default": {} } },
            "fx": { "code": ["if", ["=", "@k", "name"], "ORB_ONE_X", "ORB_TWO_Y"] } });
        assert_eq!(trait_problems(&branched).len(), 2);
        let row = serde_json::json!({ "config": { "codes": { "default": {} } }, "fx": { "code": ["object/get", "@h", "code"] } });
        assert!(trait_problems(&row).is_empty());
        assert_eq!(
            trait_problems(&finding("@config.codes.other.code")).len(),
            1
        );
    }

    #[test]
    fn every_std_declared_code_is_in_the_registry_exactly_once() {
        let Some(dir) = std_atoms_dir() else { return };
        let codes = validation_codes();
        let mut seen = std::collections::BTreeSet::new();
        for c in &codes {
            assert!(seen.insert(c.code.clone()), "{} appears twice", c.code);
        }
        let mut declared = 0;
        for (name, program) in std_programs(&dir) {
            for d in declared_codes(&program) {
                declared += 1;
                let entry = codes.iter().find(|c| c.code == d.code);
                assert_eq!(
                    entry,
                    Some(&d),
                    "{name}: {} is not in the registry as declared",
                    d.code
                );
            }
        }
        assert!(declared > 0, "no std atom declares a code");
    }

    /// Every code a Rust phase once raised and a std lint atom now owns. Retiring a producer adds its codes here, so
    /// the finding cannot silently vanish: the generated registry must list each one, declared by a std atom.
    const RETIRED_RUST_CODES: &[&str] = &[
        "ORB_CALL_KEY_UNUSED",
        "ORB_CALL_SERVICE_CANCELLED_WITHOUT_KEY",
        "ORB_CALL_SERVICE_KEY_TYPE",
        "ORB_CONFIG_DANGLING_REF",
        "ORB_EFF_FETCH_INVALID_ENTITY",
        "ORB_EFF_NAVIGATE_MISSING_PAYLOAD",
        "ORB_EFF_SET_PAYLOAD_NOT_ALLOWED",
        "ORB_E_INVALID_RELATION",
        "ORB_GEN_INVALID_VALUE",
        "ORB_INBOUND_ANSWER_FOREIGN",
        "ORB_O_CONFIG_UNKNOWN_KEY",
        "ORB_O_ENTITY_EXPECT_AMBIGUOUS",
        "ORB_O_ENTITY_PERSISTENCE_LOCKED",
        "ORB_O_ENTITY_TARGET_UNKNOWN",
        "ORB_O_ENTITY_UNKNOWN_KEY",
        "ORB_O_ENTITY_UNMAPPED",
        "ORB_O_FIELD_RENAME_UNAPPLIED",
        "ORB_O_MOCK_UNKNOWN_FIELD",
        "ORB_O_OMIT_EMBEDDED_TRAIT",
        "ORB_O_PAGE_UNKNOWN_PATH",
        "ORB_O_RETYPE_FIELD_CONFLICT",
        "ORB_O_RETYPE_UNKNOWN_FIELD",
        "ORB_O_ROLE_TARGET_NOT_MEMBER",
        "ORB_O_ROLE_UNKNOWN_UPSTREAM",
        "ORB_O_TRAITS_OMITTED_TRAIT",
        "ORB_O_TRAITS_UNKNOWN_TRAIT",
        "ORB_P_MISSING_PATH",
        "ORB_RUI_CHILD_NOT_DRAWABLE",
        "ORB_RUI_FORM_FIELD_INTRINSIC",
        "ORB_RUI_INVALID_ITEM_ACTION",
        "ORB_RUI_INVALID_SLOT",
        "ORB_SLOT_MAIN_NOT_COVERED",
        "ORB_S_EXPECTATION_UNSATISFIED",
        "ORB_S_EXPECTED_FIELD_TYPE_MISMATCH",
        "ORB_S_OWNER_FIELD_NOT_IDENTITY_TYPED",
        "ORB_S_ROLE_LITERAL_NOT_MEMBER",
        "ORB_S_UNKNOWN_IDENTITY_FIELD",
        "ORB_TAB_REQUEST_INVALID",
        "ORB_THEME_UNDECLARED",
        "ORB_T_EVT_INVALID_NAME",
        "ORB_T_GENERIC_TYPE_DEPRECATED",
        "ORB_T_INVALID_EVENT_DEFAULT",
        "ORB_T_NO_INITIAL_STATE",
        "ORB_UI_ICON_EMPTY",
        "ORB_UI_ICON_INVALID_FORMAT",
        "ORB_U_CONFIG_UNKNOWN_KEY",
        "ORB_X_CROSS_EMITTER_DECLARED_PAYLOAD_MISMATCH",
        "ORB_X_ENUM_VALUE_NOT_MEMBER",
        "ORB_X_MIXED_RESIDENCE",
        "ORB_X_UNRESOLVED_RELATION_TARGET",
        "ORB_X_UNTYPED_ARRAY",
    ];

    fn retired_without_owner(retired: &[&str], codes: &[ValidationCode]) -> Vec<String> {
        retired
            .iter()
            .filter(|r| !codes.iter().any(|c| c.code == **r))
            .map(|r| r.to_string())
            .collect()
    }

    #[test]
    fn every_code_retired_from_the_rust_enum_has_a_std_owner() {
        let Some(_) = std_atoms_dir() else { return };
        let still_in_rust: Vec<&str> = RETIRED_RUST_CODES
            .iter()
            .copied()
            .filter(|r| orbital_compiler::phases::validation::ErrorCode::ALL.iter().any(|c| c.as_str() == *r))
            .collect();
        assert!(still_in_rust.is_empty(), "listed as retired but still in the Rust enum: {still_in_rust:?}");
        assert_eq!(retired_without_owner(RETIRED_RUST_CODES, &validation_codes()), Vec::<String>::new());
    }

    #[test]
    fn control_a_retired_code_no_std_atom_declares_is_reported() {
        let codes = vec![ValidationCode { code: "ORB_KEPT".into(), category: "c".into(), description: "d".into() }];
        assert_eq!(retired_without_owner(&["ORB_KEPT", "ORB_DROPPED"], &codes), vec!["ORB_DROPPED".to_string()]);
    }

    #[test]
    fn registry_has_one_declaration_per_code() {
        let v = |code: &str, description: &str| ValidationCode {
            code: code.into(),
            category: "c".into(),
            description: description.into(),
        };
        let rust = vec![v("ORB_RUST_ONLY", "r"), v("ORB_BOTH", "rust copy")];
        let merged = registry(
            rust.clone(),
            vec![
                ("a".into(), v("ORB_BOTH", "d")),
                ("b".into(), v("ORB_BOTH", "d")),
            ],
        )
        .unwrap();
        assert_eq!(
            merged.iter().map(|c| c.code.as_str()).collect::<Vec<_>>(),
            ["ORB_RUST_ONLY", "ORB_BOTH"]
        );
        assert_eq!(merged[1].description, "d");
        let conflict = registry(
            rust.clone(),
            vec![
                ("a".into(), v("ORB_X", "one")),
                ("b".into(), v("ORB_X", "two")),
            ],
        );
        assert!(conflict
            .unwrap_err()
            .contains("ORB_X is declared differently"));
        assert!(registry(rust, vec![("a".into(), v("ORB_Y", " "))]).is_err());
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
        // A standalone orbital-rust clone has no @almadar/core or std to check against.
        if !core_dir().join("package.json").exists() || std_atoms_dir().is_none() {
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
