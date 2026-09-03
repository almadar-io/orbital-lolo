//! Behavior-contract registry slice for the lolo resolver.
//!
//! orbital-lolo runs BEFORE orbital-compiler and can't import the compiler's
//! embedded `factory-signatures-*.json` (circular dep). The sync tool emits a
//! slim slice — per behavior (`organism`), each trait's `entityContract.requires`
//! + overridable config-key types, and the page names — into
//! `baked/lolo-behavior-contracts.json`, embedded here via `include_str!`
//! (same pattern as `patterns_meta.rs`).
//!
//! Used by the resolver for parse-time `uses`-import checks: unknown
//! trait/page references, rebind row-constraints (target entity ⊇ requires),
//! and typed config overrides. The compiler re-verifies post-inline as a
//! backstop; this surfaces the error at the call site instead.

use serde::Deserialize;
use std::collections::HashMap;
use std::sync::OnceLock;

const CONTRACTS_JSON: &str = include_str!("baked/lolo-behavior-contracts.json");

#[derive(Deserialize)]
struct Contracts {
    behaviors: HashMap<String, Behavior>,
}

/// One behavior (`std-modal`, `std-browse`, …) and its trait/page contracts.
#[derive(Deserialize)]
pub struct Behavior {
    #[serde(default)]
    pub traits: HashMap<String, TraitContract>,
    #[serde(default)]
    pub pages: Vec<String>,
}

/// A single trait's parse-time-checkable contract.
#[derive(Deserialize)]
pub struct TraitContract {
    /// Entity fields the trait READS (`@entity.<field>`); a rebind target must
    /// supply all of these.
    #[serde(default)]
    pub requires: Vec<String>,
    /// Overridable config keys → their lolo type, for typed config-override
    /// checks.
    #[serde(default)]
    pub config: HashMap<String, String>,
}

impl Behavior {
    pub fn has_trait(&self, name: &str) -> bool {
        self.traits.contains_key(name)
    }
    pub fn has_page(&self, name: &str) -> bool {
        self.pages.iter().any(|p| p == name)
    }
    pub fn trait_contract(&self, name: &str) -> Option<&TraitContract> {
        self.traits.get(name)
    }
}

/// True when `ALMADAR_DEV` is set to a truthy value (anything but empty/`"0"`)
/// — the same gate the compiler uses for live dev-workspace registries. The
/// baked contracts slice is a publish-time artifact and can lag the live
/// registries mid-dev-loop (a freshly authored trait is absent from it), so
/// dev mode degrades unknown-member contract errors to skips; the compiler's
/// post-inline validation reads the LIVE registries and stays authoritative.
pub fn dev_registries_live() -> bool {
    match std::env::var("ALMADAR_DEV") {
        Ok(v) => !v.is_empty() && v != "0",
        Err(_) => false,
    }
}

static CONTRACTS: OnceLock<HashMap<String, Behavior>> = OnceLock::new();

fn contracts() -> &'static HashMap<String, Behavior> {
    // A malformed embedded slice degrades to "no contracts known" — the
    // resolver then skips the registry-backed checks rather than emitting
    // false errors (the compiler backstop still runs). A CI sanity test
    // catches a malformed slice.
    CONTRACTS.get_or_init(|| {
        serde_json::from_str::<Contracts>(CONTRACTS_JSON)
            .map(|c| c.behaviors)
            .unwrap_or_default()
    })
}

/// Behavior key from a `uses … from "<path>"` path: the last path segment
/// minus an `.orb`/`.lolo` extension (`std/behaviors/std-modal` → `std-modal`,
/// browse form `@std/ui/core/atoms/std-stats.lolo` → `std-stats`). Local-file
/// imports (`./x.orb`) have no registry entry; callers treat a `None` lookup
/// as "unknown, skip".
pub fn behavior_key_from_path(path: &str) -> &str {
    let trimmed = path.trim_end_matches(".orb");
    let trimmed = trimmed.trim_end_matches(".lolo");
    trimmed.rsplit('/').next().unwrap_or(trimmed)
}

/// Look up a behavior's contract by its key (e.g. `std-modal`).
pub fn get_behavior(key: &str) -> Option<&'static Behavior> {
    contracts().get(key)
}

/// Whether the embedded slice parsed and is non-empty (used by the CI sanity
/// test and to gate checks off entirely if the slice is missing).
pub fn is_loaded() -> bool {
    !contracts().is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn behavior_key_from_canonical_and_browse_paths() {
        assert_eq!(
            behavior_key_from_path("std/behaviors/std-modal"),
            "std-modal"
        );
        assert_eq!(behavior_key_from_path("./local/thing.orb"), "thing");
        assert_eq!(
            behavior_key_from_path("@std/ui/core/atoms/std-stats.lolo"),
            "std-stats"
        );
        assert_eq!(
            behavior_key_from_path("@behaviors/app/molecules/app-crud-manager.lolo"),
            "app-crud-manager"
        );
    }
}
