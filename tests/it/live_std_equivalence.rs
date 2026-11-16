//! The installed `@almadar/std` publishes no contracts sidecar: its contracts are
//! derived from its `.lolo`. The derivation equals what the retired generated
//! contracts file held, pinned here for two behaviors.

use orbital_lolo::registry::LoloContracts;
use std::path::Path;

fn std_contracts() -> std::sync::Arc<LoloContracts> {
    let std = orbital_test_support::installed_package("almadar-std", Path::new(env!("CARGO_MANIFEST_DIR")));
    LoloContracts::load_shared(&[std]).expect("std contracts load from its .lolo")
}

fn config(trait_config: &std::collections::HashMap<String, String>) -> Vec<(String, String)> {
    let mut v: Vec<_> = trait_config.iter().map(|(k, t)| (k.clone(), t.clone())).collect();
    v.sort();
    v
}

#[test]
fn std_modal_contract_is_derived_from_its_lolo() {
    let modal = std_contracts().get_behavior("std-modal").unwrap().expect("std-modal");
    assert_eq!(modal.pages, vec!["ModalRecordModalPage".to_string()]);
    assert!(modal.has_orbital("ModalRecordOrbital"));
    let t = modal.trait_contract("ModalRecordModal").expect("trait");
    assert!(t.requires.is_empty());
    let expect = [
        ("detailPattern", "pattern"),
        ("detailSlot", "slot"),
        ("fieldOverrides", "[ModalFieldOverride]"),
        ("fields", "[string]"),
        ("icon", "string"),
        ("maxRows", "number"),
        ("mode", "string"),
        ("title", "string"),
    ];
    let expect: Vec<(String, String)> = expect.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect();
    assert_eq!(config(&t.config), expect);
}

#[test]
fn std_agent_assistant_contract_is_derived_from_its_lolo() {
    let a = std_contracts().get_behavior("std-agent-assistant").unwrap().expect("assistant");
    let mut pages = a.pages.clone();
    pages.sort();
    assert_eq!(pages, vec!["AssistantPage".to_string(), "AssistantRunsPage".to_string()]);
    assert!(a.has_orbital("AgentAssistantOrbital") && a.has_orbital("AgentRecordsOrbital"));
    for name in [
        "AgentRunHistory", "AssistantChatKeeper", "AssistantContext", "AssistantConversation",
        "AssistantLayout", "AssistantLoop", "AssistantMemory", "AssistantThread", "AssistantTrace",
    ] {
        assert!(a.has_trait(name), "{name}");
    }
    let memory = a.trait_contract("AssistantMemory").unwrap();
    assert_eq!(config(&memory.config), vec![("recallLimit".to_string(), "number".to_string())]);
    let loop_ = a.trait_contract("AssistantLoop").unwrap();
    assert_eq!(loop_.config.get("tools").map(String::as_str), Some("[AgentTool]"));
    assert!(a.trait_contract("AgentRunHistory").unwrap().config.is_empty());
    assert!(!std_contracts().get_behavior("std-no-such-behavior").unwrap().is_some());
}

fn std_organism_path(file_name: &str) -> std::path::PathBuf {
    let std = orbital_test_support::installed_package("almadar-std", Path::new(env!("CARGO_MANIFEST_DIR")));
    let mut pending = vec![std.lolo_source.expect("std ships its .lolo")];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir).expect("a readable std dir").flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else if path.file_name().and_then(|n| n.to_str()) == Some(file_name) {
                return path;
            }
        }
    }
    panic!("{file_name} is not in the installed std");
}

/// Lower and resolve a std program on a thread with `stack` bytes (G-ORB-478: deep finite recursion outgrew 2 MiB).
fn lower_and_resolve_on_stack(path: std::path::PathBuf, stack: usize) -> Result<usize, String> {
    std::thread::Builder::new()
        .stack_size(stack)
        .spawn(move || {
            let lowered = orbital_lolo::parse_and_lower(&path).map_err(|e| e.to_string())?;
            orbital_compiler::loader::register_source_lowerer(|path, _, _| orbital_test_support::lower_lolo_file(path));
            let start = Path::new(env!("CARGO_MANIFEST_DIR"));
            let user_store = orbital_core_base::behavior_packages::user_store_dir().unwrap_or_else(|| start.to_path_buf());
            let packages = orbital_core_base::behavior_packages::discover(start, &user_store).map_err(|e| e.to_string())?;
            let options = orbital_compiler::CompilerOptions {
                inline: orbital_compiler::phases::inline::InlineConfig {
                    loader_config: orbital_compiler::loader::LoaderConfig { packages, ..Default::default() },
                    ..Default::default()
                },
                ..Default::default()
            };
            let resolved = orbital_compiler::phases::resolve_schema_with_path(lowered, &options, Some(&path))
            .map_err(|e| e.to_string())?;
            Ok(resolved.orbitals.len())
        })
        .expect("a test thread")
        .join()
        .map_err(|_| "the thread panicked".to_string())?
}

#[test]
fn the_deepest_std_organism_lowers_and_resolves_on_a_two_mebibyte_stack() {
    let orbitals = lower_and_resolve_on_stack(std_organism_path("std-tactics-board-2d.lolo"), 2 * 1024 * 1024)
        .expect("lowered and resolved");
    assert!(orbitals > 0);
}

#[test]
fn control_a_shallow_std_atom_lowers_and_resolves_on_the_same_stack() {
    let orbitals =
        lower_and_resolve_on_stack(std_organism_path("std-modal.lolo"), 2 * 1024 * 1024).expect("lowered and resolved");
    assert!(orbitals > 0);
}
