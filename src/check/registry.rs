//! Behavior-contract registry slice for the lolo resolver. Every package
//! publishes its contracts from its own `.lolo` files (`orb.source`), lowered on
//! demand and cached by content hash; there is no contracts sidecar.
//!
//! orbital-lolo runs BEFORE orbital-compiler and can't import the
//! compiler's data (circular dep). Each behavior's slim slice — per behavior
//! (`organism`), each trait's `entityContract.requires` + overridable
//! config-key types, and the page names — is derived from its `.lolo`.
//! `@almadar/std` is an installed behavior package like any other (no
//! embedded fallback, Phase 2) — `LoloContracts::load` errors when none is
//! found on the walk.
//!
//! Used by the resolver for parse-time `uses`-import checks: unknown
//! trait/page references, rebind row-constraints (target entity ⊇ requires),
//! and typed config overrides. The compiler re-verifies post-inline as a
//! backstop; this surfaces the error at the call site instead.

use orbital_core_base::behavior_packages::{BehaviorPackage, RegistrySource};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::SystemTime;
use thiserror::Error;

/// The prefix `@almadar/std`'s `orb` block declares — the std source on the
/// walk, ruling 3.
const STD_PREFIX: &str = "almadar-std";

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
    /// The package declares no `orb.source`: its contracts are derived from its
    /// `.lolo` files, so it has none to read.
    #[error(
        "ORB_BEHAVIOR_PACKAGE_NO_SOURCE: behavior package {package} declares no `orb.source` \
         (its `.lolo` tree), so its behavior contracts cannot be derived; reinstall a version \
         that ships `behaviors/lolo` and declares `orb.source`"
    )]
    NoLoloSource { package: String },
    /// No `almadar-std`-prefixed package was found among `packages` — the
    /// std contracts have no embedded fallback (Phase 2).
    #[error(
        "{}",
        orbital_core_base::behavior_packages::not_installed_message(
            orbital_core_base::behavior_packages::STD_PACKAGE
        )
    )]
    StdNotInstalled,
}

/// Where one package's contracts come from: its own `.lolo` files, lowered on
/// demand (`Live`), or an in-memory map (tests, [`LoloContracts::from_parts`]).
enum Slice {
    Declared(HashMap<String, Arc<Behavior>>),
    Live(LiveSlice),
}

/// A live package's `.lolo` tree: behavior key → file, indexed on first use. A miss re-indexes only when the tree
/// changed since: a file added or removed changes its directory's modified time, so a miss re-reads the directories'
/// times instead of walking every file.
struct LiveSlice {
    root: PathBuf,
    index: Mutex<Option<LiveIndex>>,
}

struct LiveIndex {
    files: HashMap<String, PathBuf>,
    /// Every directory walked, with its modified time when it was read.
    dirs: Vec<(PathBuf, Option<SystemTime>)>,
    scanned_at: SystemTime,
}

impl LiveIndex {
    /// The directories this index answers from count as read by whoever asks, cached or not.
    fn note_read(&self) {
        for (dir, _) in &self.dirs {
            orbital_core_base::read_set::note(dir);
        }
    }

    /// A directory changed since the scan, or was modified too close to it for its time to tell (a filesystem with
    /// coarse timestamps can stamp a later addition with the same time).
    fn stale(&self) -> bool {
        self.dirs.iter().any(|(dir, seen)| {
            let now = std::fs::metadata(dir).and_then(|m| m.modified()).ok();
            now != *seen || now.is_some_and(|t| self.scanned_at.duration_since(t).map_or(true, |age| age < RACY_WINDOW))
        })
    }
}

/// How close to a scan a directory's modified time must be for the time not to prove the directory unchanged.
const RACY_WINDOW: std::time::Duration = std::time::Duration::from_secs(2);

impl LiveSlice {
    fn new(root: PathBuf) -> Self {
        Self {
            root,
            index: Mutex::new(None),
        }
    }

    fn scan(&self) -> LiveIndex {
        let scanned_at = SystemTime::now();
        let mut files = HashMap::new();
        let mut dirs = Vec::new();
        let mut stack = vec![self.root.clone()];
        while let Some(dir) = stack.pop() {
            dirs.push((dir.clone(), std::fs::metadata(&dir).and_then(|m| m.modified()).ok()));
            for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.extension().is_some_and(|e| e == "lolo") {
                    if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                        files.insert(stem.to_string(), path);
                    }
                }
            }
        }
        LiveIndex { files, dirs, scanned_at }
    }

    fn file(&self, key: &str) -> Option<PathBuf> {
        let mut index = self.index.lock().expect("live contract index poisoned");
        let current = index.get_or_insert_with(|| self.scan());
        current.note_read();
        if let Some(path) = current.files.get(key) {
            return Some(path.clone());
        }
        if !current.stale() {
            return None;
        }
        let rescanned = index.insert(self.scan());
        rescanned.note_read();
        rescanned.files.get(key).cloned()
    }

    fn keys(&self) -> Vec<String> {
        let mut index = self.index.lock().expect("live contract index poisoned");
        let current = index.get_or_insert_with(|| self.scan());
        current.note_read();
        current.files.keys().cloned().collect()
    }
}

/// The loaded contracts ladder for one lower: std first (authoritative — a
/// miss there never falls through to the other packages' std-named entries,
/// since there are none by construction), then every other installed
/// package in walk order. See [`LoloContracts::get_behavior`].
pub struct LoloContracts {
    std: Slice,
    /// (`name@version` label, the package's `orb.prefix`, its contracts).
    others: Vec<(String, String, Slice)>,
    /// The packages a live behavior is lowered against.
    packages: Vec<BehaviorPackage>,
}

/// One live behavior file, lowered: its program and the contract it publishes.
enum LiveLowering {
    Lowered(Arc<orbital_core_base::schema::OrbitalSchema>, Arc<Behavior>),
    /// An import cycle reached this file while it was being lowered: neither is known yet.
    Cycle,
}

/// A live behavior file lowered against `packages` once per content: the contract check and the loader that inlines
/// the behavior read the same lowering, keyed by path, content hash and package set, so a file is lowered once
/// whichever asks first and again only when it, or a file its lowering read, changes.
fn live_lowering(path: &Path, packages: &[BehaviorPackage]) -> Result<LiveLowering, crate::LoloError> {
    type Key = (PathBuf, Vec<(String, String, PathBuf, Option<PathBuf>)>);
    type Entry = ([u8; 32], Arc<orbital_core_base::schema::OrbitalSchema>, Arc<Behavior>, orbital_core_base::read_set::ReadSet);
    static CACHE: OnceLock<Mutex<HashMap<Key, Entry>>> = OnceLock::new();
    thread_local! {
        static LOWERING: std::cell::RefCell<std::collections::HashSet<PathBuf>> = Default::default();
    }
    let label = path.display().to_string();
    let source = orbital_core_base::read_set::read_to_string(path)
        .map_err(|e| crate::LoloError::Io { path: label.clone(), message: e.to_string() })?;
    let hash: [u8; 32] = Sha256::digest(source.as_bytes()).into();
    let key: Key = (
        path.to_path_buf(),
        packages.iter().map(|p| (p.name.clone(), p.version.clone(), p.root.clone(), p.lolo_source.clone())).collect(),
    );
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some((h, schema, behavior, reads)) = cache.lock().expect("live lowering cache poisoned").get(&key) {
        if *h == hash && reads.is_current() {
            reads.replay();
            return Ok(LiveLowering::Lowered(Arc::clone(schema), Arc::clone(behavior)));
        }
    }
    if !LOWERING.with(|l| l.borrow_mut().insert(path.to_path_buf())) {
        return Ok(LiveLowering::Cycle);
    }
    orbital_core_base::read_set::begin();
    let lowered = crate::traced_parse(&source, &label).and_then(|ast| {
        tracing::debug_span!(target: orbital_core_base::logging::ns::LOLO, "lower", file = %label)
            .in_scope(|| {
                crate::lower::lower_to_orb_with_prior_with(
                    &ast,
                    None,
                    &crate::LowerOptions { packages: packages.to_vec(), ..Default::default() },
                )
            })
            .map_err(|inner| crate::LoloError::Lower { path: label.clone(), inner })
    });
    let reads = orbital_core_base::read_set::end();
    LOWERING.with(|l| l.borrow_mut().remove(path));
    let schema = Arc::new(lowered?);
    let behavior = Arc::new(Behavior::from_program(&schema));
    cache
        .lock()
        .expect("live lowering cache poisoned")
        .insert(key, (hash, Arc::clone(&schema), Arc::clone(&behavior), reads));
    Ok(LiveLowering::Lowered(schema, behavior))
}

/// The contract a live behavior file publishes; `None` while an import cycle is lowering it, or when it does not
/// lower (the compiler re-verifies the call site).
fn live_contract(path: &Path, packages: &[BehaviorPackage]) -> Option<Arc<Behavior>> {
    match live_lowering(path, packages) {
        Ok(LiveLowering::Lowered(_, behavior)) => Some(behavior),
        Ok(LiveLowering::Cycle) => None,
        Err(e) => {
            tracing::warn!(target: orbital_core_base::logging::ns::LOLO, path = %path.display(), error = %e, "live behavior did not lower; its contract is unknown");
            None
        }
    }
}

/// The program the `.lolo` file at `path` lowers to against `packages`: the one lowering the contract check shares
/// ([`live_lowering`]), or, while an import cycle is lowering the file, a lowering of its own.
pub fn lower_live_file(path: &Path, packages: &[BehaviorPackage]) -> Result<orbital_core_base::schema::OrbitalSchema, crate::LoloError> {
    match live_lowering(path, packages)? {
        LiveLowering::Lowered(schema, _) => Ok((*schema).clone()),
        LiveLowering::Cycle => {
            let label = path.display().to_string();
            let source = orbital_core_base::read_set::read_to_string(path)
                .map_err(|e| crate::LoloError::Io { path: label.clone(), message: e.to_string() })?;
            let ast = crate::traced_parse(&source, &label)?;
            crate::lower::lower_to_orb_with_prior_with(&ast, None, &crate::LowerOptions { packages: packages.to_vec(), ..Default::default() })
                .map_err(|inner| crate::LoloError::Lower { path: label, inner })
        }
    }
}

impl Slice {
    fn load(pkg: &BehaviorPackage) -> Result<Slice, LoadError> {
        let root = match (&pkg.registry, &pkg.lolo_source) {
            (RegistrySource::Live(root), _) => root.clone(),
            (_, Some(source)) => source.clone(),
            (_, None) => {
                return Err(LoadError::NoLoloSource {
                    package: format!("{}@{}", pkg.name, pkg.version),
                })
            }
        };
        Ok(Slice::Live(LiveSlice::new(root)))
    }

    fn get(&self, key: &str, packages: &[BehaviorPackage]) -> Option<Arc<Behavior>> {
        match self {
            Slice::Declared(map) => map.get(key).cloned(),
            Slice::Live(live) => live_contract(&live.file(key)?, packages),
        }
    }

    fn keys(&self) -> Vec<String> {
        match self {
            Slice::Declared(map) => map.keys().cloned().collect(),
            Slice::Live(live) => live.keys(),
        }
    }
}

impl LoloContracts {
    /// Load every package's contracts once: its `.lolo` tree, read per behavior
    /// on demand. A package whose `orb.prefix` is `almadar-std` supplies the
    /// std slice; every other package is searched in walk order on a std miss.
    /// No `almadar-std` package among `packages` → [`LoadError::StdNotInstalled`].
    /// Parsed once per process for one set of contracts files: the cache key is
    /// each file's path, size and modified time, so an edited file is read again.
    pub fn load_shared(packages: &[BehaviorPackage]) -> Result<Arc<LoloContracts>, LoadError> {
        type Key = Vec<(PathBuf, String, String, String, PathBuf, Option<PathBuf>)>;
        static CACHE: OnceLock<Mutex<HashMap<Key, Arc<LoloContracts>>>> = OnceLock::new();
        let key: Key = packages
            .iter()
            .map(|pkg| {
                (
                    pkg.root.clone(),
                    pkg.prefix.clone(),
                    pkg.name.clone(),
                    pkg.version.clone(),
                    pkg.registry.root().to_path_buf(),
                    pkg.lolo_source.clone(),
                )
            })
            .collect();
        let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
        if let Some(hit) = cache.lock().expect("contracts cache poisoned").get(&key) {
            return Ok(Arc::clone(hit));
        }
        let loaded = Arc::new(LoloContracts::load(packages)?);
        let mut map = cache.lock().expect("contracts cache poisoned");
        map.retain(|k, _| k.iter().map(|e| &e.0).ne(key.iter().map(|e| &e.0)));
        map.insert(key, Arc::clone(&loaded));
        Ok(loaded)
    }

    pub fn load(packages: &[BehaviorPackage]) -> Result<LoloContracts, LoadError> {
        let mut std_source: Option<Slice> = None;
        let mut others = Vec::new();
        for pkg in packages {
            let slice = Slice::load(pkg)?;
            if pkg.prefix == STD_PREFIX {
                std_source = Some(slice);
            } else {
                others.push((
                    format!("{}@{}", pkg.name, pkg.version),
                    pkg.prefix.clone(),
                    slice,
                ));
            }
        }
        Ok(LoloContracts {
            std: std_source.ok_or(LoadError::StdNotInstalled)?,
            others,
            packages: packages.to_vec(),
        })
    }

    /// Look up a behavior's contract by its key (e.g. `std-modal`), applying
    /// ruling 3: std first (authoritative — a miss there does NOT fall
    /// through to the other packages' std-named entries, since there are
    /// none by construction); on a std miss, search every other package: one
    /// hit resolves it, two is an [`Ambiguity`], none is `Ok(None)`.
    pub fn get_behavior(&self, key: &str) -> Result<Option<Arc<Behavior>>, Ambiguity> {
        if let Some(behavior) = self.std.get(key, &self.packages) {
            return Ok(Some(behavior));
        }

        let mut found: Option<(&str, Arc<Behavior>)> = None;
        for (name_version, _, slice) in &self.others {
            if let Some(behavior) = slice.get(key, &self.packages) {
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

    /// Contracts assembled in memory (tests; callers that already hold the maps).
    pub fn from_parts(
        std: HashMap<String, Behavior>,
        others: Vec<(String, String, HashMap<String, Behavior>)>,
    ) -> LoloContracts {
        let declared = |m: HashMap<String, Behavior>| {
            Slice::Declared(m.into_iter().map(|(k, v)| (k, Arc::new(v))).collect())
        };
        LoloContracts {
            std: declared(std),
            others: others
                .into_iter()
                .map(|(n, p, m)| (n, p, declared(m)))
                .collect(),
            packages: Vec::new(),
        }
    }

    /// Every installed behavior as `(import path, key)`: std's as
    /// `std/behaviors/<key>`, another package's as `<orb.prefix>/<key>` —
    /// the spellings `uses … from "…"` resolves (see [`behavior_key_from_path`]).
    pub fn import_paths(&self) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = self
            .std
            .keys()
            .into_iter()
            .map(|k| (format!("std/behaviors/{k}"), k))
            .collect();
        for (_, prefix, slice) in &self.others {
            out.extend(
                slice
                    .keys()
                    .into_iter()
                    .map(|k| (format!("{prefix}/{k}"), k)),
            );
        }
        out.sort();
        out
    }
}

/// `@almadar/core`'s `jsToLolo`: a JS runtime type name as its lolo type.
fn js_to_lolo(t: &str) -> String {
    match t {
        "string" | "number" | "boolean" | "array" | "object" => t.to_string(),
        "integer" => "number".to_string(),
        "" => "string".to_string(),
        other => other.to_string(),
    }
}

/// `@almadar/core`'s `inferConfigType`: the knob's declared type, else its
/// default's runtime type; then [`js_to_lolo`].
fn config_types(
    config: Option<&orbital_core_base::collections::IndexMap<String, orbital_core_base::schema::ConfigField>>,
) -> HashMap<String, String> {
    use orbital_core_base::schema::SExpression;
    config
        .into_iter()
        .flatten()
        .map(|(key, field)| {
            let inferred = if !field.field_type.is_empty() {
                field.field_type.clone()
            } else {
                match &field.default {
                    Some(SExpression::Array(_)) => "array".to_string(),
                    Some(SExpression::String(_)) => "string".to_string(),
                    Some(SExpression::Number(_)) => "number".to_string(),
                    Some(SExpression::Bool(_)) => "boolean".to_string(),
                    Some(SExpression::Object(_)) => "object".to_string(),
                    _ => String::new(),
                }
            };
            (key.clone(), js_to_lolo(&inferred))
        })
        .collect()
}

/// A composed trait's config: a knob it only wires (`knob: @config.X`, tagged
/// `unknown` — it declares no type) takes the type of the orbital's or app's
/// own declared knob `X`; an unresolved wiring stays `unknown` (unchecked).
fn wired_config_types(
    config: Option<&orbital_core_base::collections::IndexMap<String, orbital_core_base::schema::ConfigField>>,
    orbital: Option<&orbital_core_base::collections::IndexMap<String, orbital_core_base::schema::ConfigField>>,
    app: Option<&orbital_core_base::collections::IndexMap<String, orbital_core_base::schema::ConfigField>>,
) -> HashMap<String, String> {
    use orbital_core_base::schema::SExpression;
    let mut types = config_types(config);
    let own = config_types(orbital);
    let shared = config_types(app);
    for (key, field) in config.into_iter().flatten() {
        if field.field_type != "unknown" {
            continue;
        }
        let Some(SExpression::String(binding)) = &field.default else {
            continue;
        };
        let Some(source) = binding.strip_prefix("@config.") else {
            continue;
        };
        if let Some(t) = own.get(source).or_else(|| shared.get(source)) {
            types.insert(key.clone(), t.clone());
        }
    }
    types
}

impl Behavior {
    /// The contract a lowered program publishes — `@almadar/core`'s
    /// `buildLoloBehaviorContracts` rule for a behavior read from its own
    /// program: each trait's `entityContract.requires` and config types (a
    /// composed trait by its local name), the page names, each orbital's
    /// config and the app config.
    pub fn from_program(schema: &orbital_core_base::schema::types::OrbitalSchema) -> Behavior {
        use orbital_core_base::schema::{PageRef, TraitReference};
        let mut traits = HashMap::new();
        let mut pages = Vec::new();
        let mut orbitals = HashMap::new();
        for orbital in &schema.orbitals {
            for t in &orbital.traits {
                let (name, contract) = match t {
                    TraitReference::Inline(def) => (
                        def.name.clone(),
                        TraitContract {
                            requires: def
                                .entity_contract
                                .as_ref()
                                .map(|c| c.requires.clone())
                                .unwrap_or_default(),
                            config: config_types(def.config.as_ref()),
                        },
                    ),
                    TraitReference::Reference {
                        name,
                        reference,
                        config,
                        ..
                    } => {
                        let Some(name) = name.clone().or_else(|| {
                            reference
                                .as_deref()
                                .and_then(|r| r.rsplit('.').next())
                                .map(str::to_string)
                        }) else {
                            continue;
                        };
                        (
                            name,
                            TraitContract {
                                requires: Vec::new(),
                                config: wired_config_types(
                                    config.as_ref(),
                                    orbital.config.as_ref(),
                                    schema.config.as_ref(),
                                ),
                            },
                        )
                    }
                };
                traits.entry(name).or_insert(contract);
            }
            for p in orbital.pages.iter().flatten() {
                let name = match p {
                    PageRef::Inline(def) => Some(def.name.clone()),
                    other => other
                        .reference_string()
                        .and_then(|r| r.rsplit('.').next())
                        .map(str::to_string),
                };
                if let Some(name) = name.filter(|n| !n.is_empty() && !pages.contains(n)) {
                    pages.push(name);
                }
            }
            orbitals.insert(
                orbital.name.clone(),
                OrbitalContract {
                    config: config_types(orbital.config.as_ref()),
                },
            );
        }
        pages.sort();
        Behavior {
            traits,
            pages,
            orbitals,
            app_config: config_types(schema.config.as_ref()),
        }
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

    fn live_tree(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("orb-live-slice-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("atoms")).expect("tree");
        std::fs::write(root.join("atoms").join("std-a.lolo"), "").expect("file");
        root
    }

    fn age(dir: &std::path::Path) {
        let past = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        std::fs::File::open(dir).and_then(|f| f.set_modified(past)).expect("set dir time");
    }

    #[test]
    fn a_miss_in_an_unchanged_tree_does_not_walk_it_again() {
        let root = live_tree("unchanged");
        age(&root);
        age(&root.join("atoms"));
        let slice = LiveSlice::new(root.clone());
        assert!(slice.file("std-a").is_some(), "an indexed key is found");
        assert!(slice.file("std-b").is_none(), "a key no file names is a miss");
        std::fs::write(root.join("atoms").join("std-b.lolo"), "").expect("file");
        age(&root.join("atoms"));
        assert!(slice.file("std-b").is_none(), "a directory whose time did not move is not walked again");
        std::fs::write(root.join("atoms").join("std-c.lolo"), "").expect("file");
        assert!(slice.file("std-c").is_some(), "control: a file added since the scan is found");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_directory_stamped_close_to_the_scan_is_read_again_on_a_miss() {
        let root = live_tree("racy");
        let slice = LiveSlice::new(root.clone());
        assert!(slice.file("std-a").is_some());
        std::fs::write(root.join("atoms").join("std-d.lolo"), "").expect("file");
        assert!(slice.file("std-d").is_some(), "a recently stamped directory cannot prove itself unchanged");
        let _ = std::fs::remove_dir_all(&root);
    }


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
