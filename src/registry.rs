//! Behavior-contract registry slice for the lolo resolver.
//!
//! orbital-lolo runs BEFORE orbital-compiler and can't import the compiler's
//! embedded `factory-signatures-*.json` (circular dep). Each behavior package
//! ships a slim slice — per behavior (`organism`), each trait's
//! `entityContract.requires` + overridable config-key types, and the page
//! names — at the path its `package.json` `orb.loloContracts` block declares.
//! `@almadar/std` is an installed behavior package like any other (no
//! embedded fallback, Phase 2) — `LoloContracts::load` errors when none is
//! found on the walk.
//!
//! Used by the resolver for parse-time `uses`-import checks: unknown
//! trait/page references, rebind row-constraints (target entity ⊇ requires),
//! and typed config overrides. The compiler re-verifies post-inline as a
//! backstop; this surfaces the error at the call site instead.

use orbital_core::behavior_packages::BehaviorPackage;
use serde::Deserialize;
use std::collections::HashMap;
use thiserror::Error;

/// The prefix `@almadar/std`'s `orb` block declares — the std source on the
/// walk, ruling 3.
const STD_PREFIX: &str = "almadar-std";

#[derive(Deserialize, Clone)]
struct Contracts {
    behaviors: HashMap<String, Behavior>,
}

/// One behavior (`std-modal`, `std-browse`, …) and its trait/page/orbital
/// contracts.
#[derive(Deserialize, Clone)]
pub struct Behavior {
    #[serde(default)]
    pub traits: HashMap<String, TraitContract>,
    #[serde(default)]
    pub pages: Vec<String>,
    /// Orbital contracts, keyed by orbital name — the `Alias.orbitals.X`
    /// surface (orbital-import Stage B). `#[serde(default)]` so a behavior
    /// baked before this field existed degrades to "no orbitals known"
    /// instead of failing to deserialize.
    #[serde(default)]
    pub orbitals: HashMap<String, OrbitalContract>,
    /// This behavior's app-level (organism-wide) declared config knobs →
    /// their lolo type, for typed `uses … { config { … } }` override checks.
    #[serde(default, rename = "appConfig")]
    pub app_config: HashMap<String, String>,
}

/// A single trait's parse-time-checkable contract.
#[derive(Deserialize, Clone)]
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

/// A single orbital's parse-time-checkable contract (orbital-import Stage B):
/// what `Alias.orbitals.X` publishes for a `config { … }` override to check
/// against.
#[derive(Deserialize, Clone)]
pub struct OrbitalContract {
    /// Overridable config keys → their lolo type, for typed config-override
    /// checks on an `orbital Local = Alias.orbitals.X { config { … } }`
    /// reference.
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
    pub fn has_orbital(&self, name: &str) -> bool {
        self.orbitals.contains_key(name)
    }
    pub fn trait_contract(&self, name: &str) -> Option<&TraitContract> {
        self.traits.get(name)
    }
    pub fn orbital_contract(&self, name: &str) -> Option<&OrbitalContract> {
        self.orbitals.get(name)
    }
}

/// A behavior name found in two installed (non-std) packages — never a
/// precedence rule, ruling 3.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ambiguity {
    pub name: String,
    pub package_a: String,
    pub package_b: String,
}

#[derive(Debug, Error)]
pub enum LoadError {
    #[error("io error reading behavior contracts at {path}: {message}")]
    Io { path: String, message: String },
    #[error("invalid behavior contracts JSON at {path}: {message}")]
    Parse { path: String, message: String },
    /// No `almadar-std`-prefixed package was found among `packages` — the
    /// std contracts have no embedded fallback (Phase 2).
    #[error(
        "{}",
        orbital_core::behavior_packages::not_installed_message(
            orbital_core::behavior_packages::STD_PACKAGE
        )
    )]
    StdNotInstalled,
}

/// The loaded contracts ladder for one lower: std first (authoritative — a
/// miss there never falls through to the other packages' std-named entries,
/// since there are none by construction), then every other installed
/// package in walk order. See [`LoloContracts::get_behavior`].
pub struct LoloContracts {
    std: HashMap<String, Behavior>,
    others: Vec<(String, HashMap<String, Behavior>)>,
}

impl LoloContracts {
    /// Read every package's `lolo_contracts` file once. A package whose
    /// `orb.prefix` is `almadar-std` supplies the std slice; every other
    /// package is searched in walk order on a std miss. No `almadar-std`
    /// package among `packages` → [`LoadError::StdNotInstalled`].
    pub fn load(packages: &[BehaviorPackage]) -> Result<LoloContracts, LoadError> {
        let mut std_source: Option<HashMap<String, Behavior>> = None;
        let mut others = Vec::new();
        for pkg in packages {
            let path = pkg.lolo_contracts.display().to_string();
            let raw = std::fs::read_to_string(&pkg.lolo_contracts).map_err(|e| LoadError::Io {
                path: path.clone(),
                message: e.to_string(),
            })?;
            let parsed: Contracts = serde_json::from_str(&raw).map_err(|e| LoadError::Parse {
                path,
                message: e.to_string(),
            })?;
            if pkg.prefix == STD_PREFIX {
                std_source = Some(parsed.behaviors);
            } else {
                others.push((format!("{}@{}", pkg.name, pkg.version), parsed.behaviors));
            }
        }
        Ok(LoloContracts {
            std: std_source.ok_or(LoadError::StdNotInstalled)?,
            others,
        })
    }

    /// Look up a behavior's contract by its key (e.g. `std-modal`), applying
    /// ruling 3: std first (authoritative — a miss there does NOT fall
    /// through to the other packages' std-named entries, since there are
    /// none by construction); on a std miss, search every other package: one
    /// hit resolves it, two is an [`Ambiguity`], none is `Ok(None)`.
    pub fn get_behavior(&self, key: &str) -> Result<Option<&Behavior>, Ambiguity> {
        if let Some(behavior) = self.std.get(key) {
            return Ok(Some(behavior));
        }

        let mut found: Option<(&str, &Behavior)> = None;
        for (name_version, map) in &self.others {
            if let Some(behavior) = map.get(key) {
                match found {
                    None => found = Some((name_version.as_str(), behavior)),
                    Some((first, _)) => {
                        return Err(Ambiguity {
                            name: key.to_string(),
                            package_a: first.to_string(),
                            package_b: name_version.clone(),
                        });
                    }
                }
            }
        }
        Ok(found.map(|(_, behavior)| behavior))
    }
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
