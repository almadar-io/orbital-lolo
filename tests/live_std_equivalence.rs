//! The installed `@almadar/std` publishes no contracts sidecar: its contracts are
//! derived from its `.lolo`. The derivation equals what the generated
//! `lolo-behavior-contracts.json` held, pinned here for two behaviors.

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
