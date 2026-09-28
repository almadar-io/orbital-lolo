//! `.lolo` completion: what is valid at a cursor position, from the language's
//! own tables — keywords and effect operators ([`crate::keywords`]), pattern
//! tags and props (the baked pattern registry), and installed behaviors with
//! their traits, orbitals and typed config keys ([`LoloContracts`]).
//!
//! Text-context driven so it works on the partial, unparseable source a person
//! is in the middle of typing. It only offers names the language or an
//! installed package declares; ranking them is the caller's (embeddings).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

use serde::Serialize;

use crate::keywords::vocabulary;
use crate::registry::{behavior_key_from_path, LoloContracts};

const PATTERNS_JSON: &str = include_str!("baked/patterns-registry.json");

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum CompletionKind {
    Keyword,
    ImportPath,
    Trait,
    Orbital,
    ConfigKey,
    EntityField,
    Pattern,
    Prop,
    Operator,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Completion {
    pub label: String,
    pub kind: CompletionKind,
    /// A type, a behavior, or what the name refers to.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CompletionResult {
    /// The context the cursor is in (`import-path`, `trait-ref`, `config-key`, …); `none` when nothing applies.
    pub context: &'static str,
    /// The partial word being completed; every candidate starts with it.
    pub prefix: String,
    pub candidates: Vec<Completion>,
}

/// Pattern type → its prop names, from the baked registry.
fn pattern_props() -> &'static BTreeMap<String, Vec<String>> {
    static PROPS: OnceLock<BTreeMap<String, Vec<String>>> = OnceLock::new();
    PROPS.get_or_init(|| {
        let mut out = BTreeMap::new();
        let Ok(parsed) = serde_json::from_str::<serde_json::Value>(PATTERNS_JSON) else {
            return out;
        };
        let Some(patterns) = parsed.get("patterns").and_then(|v| v.as_object()) else {
            return out;
        };
        for (name, def) in patterns {
            let props = def
                .get("propsSchema")
                .and_then(|p| p.as_object())
                .map(|p| p.keys().cloned().collect())
                .unwrap_or_default();
            out.insert(name.clone(), props);
        }
        out
    })
}

/// `data-grid` → `DataGrid` — the JSX tag a render tree writes for a pattern.
fn tag_of(pattern: &str) -> String {
    pattern
        .split('-')
        .map(|part| {
            let mut chars = part.chars();
            chars
                .next()
                .map(|c| c.to_ascii_uppercase().to_string() + chars.as_str())
                .unwrap_or_default()
        })
        .collect()
}

/// The pattern a JSX tag names, if the registry has it.
fn pattern_of_tag(tag: &str) -> Option<&'static str> {
    pattern_props()
        .keys()
        .find(|p| tag_of(p) == tag)
        .map(|p| p.as_str())
}

fn is_word_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-'
}

/// The partial word ending at the cursor.
fn trailing_word(before: &str) -> &str {
    let start = before
        .char_indices()
        .rev()
        .take_while(|(_, c)| is_word_char(*c))
        .last()
        .map(|(i, _)| i)
        .unwrap_or(before.len());
    &before[start..]
}

/// `uses <Alias> from "<path>"` declarations in the source → alias → behavior key.
fn uses_aliases(source: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for line in source.lines() {
        let words: Vec<&str> = line.split_whitespace().collect();
        if words.len() >= 4 && words[0] == "uses" && words[2] == "from" {
            let path = words[3].trim_matches(|c| c == '"' || c == '{');
            out.insert(
                words[1].to_string(),
                behavior_key_from_path(path).to_string(),
            );
        }
    }
    out
}

/// Byte offsets of the `{` still open at the end of `before`, innermost last.
fn open_braces(before: &str) -> Vec<usize> {
    let mut stack = Vec::new();
    let mut in_string = false;
    for (i, c) in before.char_indices() {
        match c {
            '"' => in_string = !in_string,
            '{' if !in_string => stack.push(i),
            '}' if !in_string => {
                stack.pop();
            }
            _ => {}
        }
    }
    stack
}

/// The last non-blank text on the line(s) before a `{` — its block header.
fn header_before(before: &str, brace: usize) -> &str {
    let head = &before[..brace];
    let line_start = head.rfind('\n').map(|i| i + 1).unwrap_or(0);
    head[line_start..].trim()
}

const TOP_LEVEL: &[&str] = &["app", "type", "uses", "orbital", "theme", "config"];
const ORBITAL_BODY: &[&str] = &[
    "uses", "expects", "type", "entity", "trait", "page", "theme", "config",
];
const ORBITAL_REF_BODY: &[&str] = &[
    "entity", "entities", "fields", "roles", "events", "pages", "omit", "only", "config", "extend",
    "listens", "trait",
];
const TRAIT_BODY: &[&str] = &["initial", "state", "emits", "listens", "ticks", "config"];
const TRAIT_REF_BODY: &[&str] = &[
    "events",
    "fields",
    "config",
    "on",
    "name",
    "listens",
    "emitsScope",
];
const ENTITY_BODY: &[&str] = &["derived"];

/// The statements the parser dispatches on at the start of a line inside the block
/// headed `header` (`""` = top level) — `parser/mod.rs`, `orbital.rs`, `trait.rs`,
/// `entity.rs`. `None` where a line starts with a free name (a state's events, an
/// entity's fields, emits/listens/ticks/config entries).
fn block_keywords(header: &str) -> Option<&'static [&'static str]> {
    let words: Vec<&str> = header.split_whitespace().collect();
    let is_ref = words.get(2) == Some(&"=");
    match words.first() {
        None => Some(TOP_LEVEL),
        Some(&"orbital") => Some(if is_ref {
            ORBITAL_REF_BODY
        } else {
            ORBITAL_BODY
        }),
        Some(&"trait") => Some(if is_ref { TRAIT_REF_BODY } else { TRAIT_BODY }),
        Some(&"entity") => Some(ENTITY_BODY),
        _ => None,
    }
}

/// The entity fields declared by the orbital enclosing `offset`.
fn enclosing_entity_fields(source: &str, offset: usize) -> Vec<(String, String)> {
    let before = &source[..offset];
    let Some(orbital_start) = before.rfind("orbital ") else {
        return Vec::new();
    };
    let rest = &source[orbital_start..];
    let Some(entity_at) = rest.find("\n  entity ").or_else(|| rest.find("entity ")) else {
        return Vec::new();
    };
    let entity = &rest[entity_at..];
    let Some(open) = entity.find('{') else {
        return Vec::new();
    };
    let Some(close) = entity[open..].find('}') else {
        return Vec::new();
    };
    entity[open + 1..open + close]
        .lines()
        .filter_map(|line| {
            let (name, ty) = line.trim().split_once(':')?;
            let name = name.trim();
            (!name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
                .then(|| (name.to_string(), ty.trim().to_string()))
        })
        .collect()
}

fn filtered(
    prefix: &str,
    context: &'static str,
    candidates: impl IntoIterator<Item = Completion>,
) -> CompletionResult {
    let lower = prefix.to_ascii_lowercase();
    let mut seen = BTreeSet::new();
    let mut out: Vec<Completion> = candidates
        .into_iter()
        .filter(|c| c.label.to_ascii_lowercase().starts_with(&lower))
        .filter(|c| seen.insert(c.label.clone()))
        .collect();
    out.sort_by(|a, b| a.label.cmp(&b.label));
    CompletionResult {
        context,
        prefix: prefix.to_string(),
        candidates: out,
    }
}

fn none(prefix: &str) -> CompletionResult {
    CompletionResult {
        context: "none",
        prefix: prefix.to_string(),
        candidates: Vec::new(),
    }
}

/// Completions at byte `offset` of `source`. `contracts` supplies installed
/// behaviors; without it, behavior-backed contexts offer nothing.
pub fn complete(
    source: &str,
    offset: usize,
    contracts: Option<&LoloContracts>,
) -> CompletionResult {
    let mut offset = offset.min(source.len());
    while !source.is_char_boundary(offset) {
        offset -= 1;
    }
    let before = &source[..offset];
    let line = &before[before.rfind('\n').map(|i| i + 1).unwrap_or(0)..];

    // Inside `uses X from "…` — the import path typed so far.
    if let Some(quote) = line.rfind('"') {
        let words: Vec<&str> = line[..quote].split_whitespace().collect();
        if line[quote + 1..].chars().all(|c| c != '"')
            && words.len() == 3
            && words[0] == "uses"
            && words[2] == "from"
        {
            let prefix = &line[quote + 1..];
            let Some(contracts) = contracts else {
                return none(prefix);
            };
            return filtered(
                prefix,
                "import-path",
                contracts
                    .import_paths()
                    .into_iter()
                    .map(|(path, key)| Completion {
                        label: path,
                        kind: CompletionKind::ImportPath,
                        detail: Some(key.to_string()),
                    }),
            );
        }
    }

    let word = trailing_word(before);
    let lead = &before[..before.len() - word.len()];

    // `Alias.traits.` / `Alias.orbitals.`
    for (member, kind, context) in [
        (".traits.", CompletionKind::Trait, "trait-ref"),
        (".orbitals.", CompletionKind::Orbital, "orbital-ref"),
    ] {
        if let Some(alias_part) = lead.strip_suffix(member) {
            let alias = trailing_word(alias_part);
            let Some(behavior) = contracts
                .zip(uses_aliases(source).get(alias).cloned())
                .and_then(|(c, key)| c.get_behavior(&key).ok().flatten().cloned())
            else {
                return none(word);
            };
            let names: Vec<String> = if kind == CompletionKind::Trait {
                behavior.traits.keys().cloned().collect()
            } else {
                behavior.orbitals.keys().cloned().collect()
            };
            return filtered(
                word,
                context,
                names.into_iter().map(|n| Completion {
                    label: n,
                    kind,
                    detail: Some(alias.to_string()),
                }),
            );
        }
    }

    // `@entity.` — the enclosing orbital's entity fields.
    if lead.ends_with("@entity.") {
        return filtered(
            word,
            "entity-field",
            enclosing_entity_fields(source, offset)
                .into_iter()
                .map(|(name, ty)| Completion {
                    label: name,
                    kind: CompletionKind::EntityField,
                    detail: Some(ty),
                }),
        );
    }

    // `<trait.` — a trait declared in this file, embedded in a render tree.
    if lead.ends_with("<trait.") {
        let traits = source.lines().filter_map(|l| {
            let mut w = l.split_whitespace();
            (w.next() == Some("trait"))
                .then(|| {
                    w.next()
                        .map(|n| n.trim_end_matches(|c: char| !is_word_char(c)).to_string())
                })
                .flatten()
        });
        return filtered(
            word,
            "trait-embed",
            traits.map(|t| Completion {
                label: t,
                kind: CompletionKind::Trait,
                detail: None,
            }),
        );
    }

    // A JSX tag being opened, or a prop inside one.
    if let Some(lt) = before.rfind('<') {
        let inside = &before[lt + 1..];
        if !inside.contains('>') && !inside.starts_with('/') && !inside.starts_with("trait.") {
            if !inside.contains(char::is_whitespace) {
                return filtered(
                    word,
                    "pattern-tag",
                    pattern_props().keys().map(|p| Completion {
                        label: tag_of(p),
                        kind: CompletionKind::Pattern,
                        detail: Some(p.clone()),
                    }),
                );
            }
            let tag = inside.split_whitespace().next().unwrap_or("");
            let braces_open = inside.matches('{').count() > inside.matches('}').count();
            let in_value = lead.trim_end().ends_with('=') || braces_open;
            if let Some(pattern) = pattern_of_tag(tag).filter(|_| !in_value) {
                let used: BTreeSet<&str> = inside
                    .split_whitespace()
                    .filter_map(|a| a.split_once('=').map(|(k, _)| k))
                    .collect();
                return filtered(
                    word,
                    "pattern-prop",
                    pattern_props()[pattern]
                        .iter()
                        .filter(|p| !used.contains(p.as_str()))
                        .map(|p| Completion {
                            label: p.clone(),
                            kind: CompletionKind::Prop,
                            detail: Some(pattern.to_string()),
                        }),
                );
            }
        }
    }

    // A key inside `config { … }` of a reference: that reference's typed config.
    let braces = open_braces(before);
    if let [.., outer, inner] = braces.as_slice() {
        let at_key = !line.contains(':');
        if header_before(before, *inner).ends_with("config") && at_key {
            let header = header_before(before, *outer);
            let aliases = uses_aliases(source);
            let config = contracts.and_then(|c| {
                let target = header.split('=').nth(1)?.trim();
                let mut parts = target.split('.');
                let (alias, member, name) = (
                    parts.next()?.trim(),
                    parts.next()?,
                    parts.next()?.split_whitespace().next()?,
                );
                let behavior = c.get_behavior(aliases.get(alias)?).ok().flatten()?;
                match member {
                    "traits" => behavior.traits.get(name).map(|t| t.config.clone()),
                    "orbitals" => behavior.orbitals.get(name).map(|o| o.config.clone()),
                    _ => None,
                }
            });
            if let Some(config) = config {
                return filtered(
                    word,
                    "config-key",
                    config.into_iter().map(|(k, ty)| Completion {
                        label: k,
                        kind: CompletionKind::ConfigKey,
                        detail: Some(ty),
                    }),
                );
            }
        }
    }

    // `(` opens an effect.
    if lead.trim_end().ends_with('(') {
        return filtered(
            word,
            "effect",
            vocabulary().effects.iter().map(|e| Completion {
                label: (*e).to_string(),
                kind: CompletionKind::Operator,
                detail: None,
            }),
        );
    }

    // `name : ` inside an entity body: a field type.
    let entity_body = open_braces(before)
        .last()
        .is_some_and(|&brace| header_before(before, brace).starts_with("entity "));
    if entity_body {
        let head = line[..line.len() - word.len()].trim_start();
        if let Some(name) = head.trim_end().strip_suffix(':') {
            if !name.trim().is_empty() && name.trim().chars().all(is_word_char) {
                return filtered(
                    word,
                    "field-type",
                    vocabulary().types.iter().map(|t| Completion {
                        label: (*t).to_string(),
                        kind: CompletionKind::Keyword,
                        detail: Some("type".to_string()),
                    }),
                );
            }
        }
    }

    // At the start of a line: a statement keyword of the enclosing block.
    if lead.trim().is_empty() || line[..line.len() - word.len()].trim().is_empty() {
        let header = open_braces(before)
            .last()
            .map(|&brace| header_before(before, brace))
            .unwrap_or("");
        let Some(allowed) = block_keywords(header) else {
            return none(word);
        };
        return filtered(
            word,
            "keyword",
            allowed.iter().map(|k| Completion {
                label: (*k).to_string(),
                kind: CompletionKind::Keyword,
                detail: None,
            }),
        );
    }

    none(word)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::{Behavior, OrbitalContract, TraitContract};
    use std::collections::HashMap;

    fn contracts() -> LoloContracts {
        let mut std = HashMap::new();
        std.insert(
            "std-app-layout".to_string(),
            Behavior {
                traits: HashMap::from([(
                    "AppLayout".to_string(),
                    TraitContract {
                        requires: vec![],
                        config: HashMap::from([
                            ("navItems".to_string(), "[NavItem]".to_string()),
                            ("appName".to_string(), "string".to_string()),
                        ]),
                    },
                )]),
                pages: vec![],
                orbitals: HashMap::new(),
                app_config: HashMap::new(),
            },
        );
        let mut io = HashMap::new();
        io.insert(
            "std-crm".to_string(),
            Behavior {
                traits: HashMap::new(),
                pages: vec![],
                orbitals: HashMap::from([(
                    "ContactOrbital".to_string(),
                    OrbitalContract {
                        config: HashMap::from([("pageSize".to_string(), "number".to_string())]),
                    },
                )]),
                app_config: HashMap::new(),
            },
        );
        LoloContracts::from_parts(
            std,
            vec![(
                "almadar-behaviors@1".to_string(),
                "almadar-behaviors".to_string(),
                io,
            )],
        )
    }

    fn at(source: &str) -> (String, usize) {
        let offset = source.find('|').expect("cursor");
        (source.replacen('|', "", 1), offset)
    }

    fn labels(r: &CompletionResult) -> Vec<&str> {
        r.candidates.iter().map(|c| c.label.as_str()).collect()
    }

    #[test]
    fn import_paths_in_uses_spell_std_and_packages() {
        let (src, off) = at("orbital O {\n  uses Shell from \"std/beh|");
        let r = complete(&src, off, Some(&contracts()));
        assert_eq!(r.context, "import-path");
        assert_eq!(labels(&r), vec!["std/behaviors/std-app-layout"]);
        let (src, off) = at("orbital O {\n  uses Crm from \"almadar|");
        assert_eq!(
            labels(&complete(&src, off, Some(&contracts()))),
            vec!["almadar-behaviors/std-crm"]
        );
    }

    #[test]
    fn trait_and_orbital_refs_come_from_the_used_behavior() {
        let (src, off) = at("orbital O {\n  uses Shell from \"std/behaviors/std-app-layout\"\n  trait L = Shell.traits.|");
        let r = complete(&src, off, Some(&contracts()));
        assert_eq!((r.context, labels(&r)), ("trait-ref", vec!["AppLayout"]));
        let (src, off) =
            at("uses Crm from \"almadar-behaviors/std-crm\"\norbital C = Crm.orbitals.Con|");
        assert_eq!(
            labels(&complete(&src, off, Some(&contracts()))),
            vec!["ContactOrbital"]
        );
    }

    #[test]
    fn config_keys_are_the_referenced_traits_typed_knobs_minus_nothing_else() {
        let (src, off) = at("orbital O {\n  uses Shell from \"std/behaviors/std-app-layout\"\n  trait L = Shell.traits.AppLayout {\n    config {\n      nav|");
        let r = complete(&src, off, Some(&contracts()));
        assert_eq!(r.context, "config-key");
        assert_eq!(
            r.candidates,
            vec![Completion {
                label: "navItems".into(),
                kind: CompletionKind::ConfigKey,
                detail: Some("[NavItem]".into())
            }]
        );
    }

    #[test]
    fn entity_fields_of_the_enclosing_orbital() {
        let (src, off) = at("orbital O {\n  entity Note [runtime] {\n    id : string!\n    title : string\n  }\n  trait T {\n    state s { INIT -> s (render-ui main <Typography content=@entity.ti|");
        let r = complete(&src, off, Some(&contracts()));
        assert_eq!((r.context, labels(&r)), ("entity-field", vec!["title"]));
    }

    #[test]
    fn pattern_tags_then_their_props_minus_ones_already_set() {
        let (src, off) = at("(render-ui main <Butt|");
        let r = complete(&src, off, None);
        assert_eq!(r.context, "pattern-tag");
        assert!(labels(&r).contains(&"Button"));
        let (src, off) = at("(render-ui main <Button label=\"Go\" var|");
        let r = complete(&src, off, None);
        assert_eq!(r.context, "pattern-prop");
        assert!(labels(&r).contains(&"variant"));
        let (src, off) = at("(render-ui main <Button label=\"Go\" lab|");
        assert!(
            !labels(&complete(&src, off, None)).contains(&"label"),
            "a prop already set is not offered again"
        );
    }

    #[test]
    fn effects_after_an_open_paren_and_keywords_at_line_start() {
        let (src, off) = at("    INIT -> s (pers|");
        let r = complete(&src, off, None);
        assert_eq!((r.context, labels(&r)), ("effect", vec!["persist"]));
        let (src, off) = at("orbital O {\n  ent|");
        let r = complete(&src, off, None);
        assert_eq!(r.context, "keyword");
        assert!(labels(&r).contains(&"entity"));
    }

    #[test]
    fn line_start_keywords_are_the_ones_the_enclosing_block_parses() {
        let (src, off) = at("orbital O {\n  trait T -> E [interaction] {\n    em|");
        let r = complete(&src, off, None);
        assert_eq!((r.context, labels(&r)), ("keyword", vec!["emits"]));
        let (src, off) = at("orbital O {\n  trait T -> E [interaction] {\n    e|");
        assert!(
            !labels(&complete(&src, off, None)).contains(&"entity"),
            "entity is not a trait statement"
        );
        let (src, off) = at("orbital O {\n  entity E [runtime] {\n    id : string!\n    de|");
        assert_eq!(labels(&complete(&src, off, None)), vec!["derived"]);
        let (src, off) = at("orbital O {\n  trait L = Browse.traits.B -> E {\n    na|");
        assert_eq!(labels(&complete(&src, off, None)), vec!["name"]);
    }

    #[test]
    fn a_field_type_after_the_colon_in_an_entity() {
        let (src, off) = at("orbital O {\n  entity E [runtime] {\n    title : str|");
        let r = complete(&src, off, None);
        assert_eq!((r.context, labels(&r)), ("field-type", vec!["string"]));
        let (src, off) = at("orbital O {\n  entity E [runtime] {\n    total : |");
        assert!(labels(&complete(&src, off, None)).contains(&"money"));
    }

    #[test]
    fn control_a_colon_outside_an_entity_is_not_a_field_type() {
        let (src, off) = at("orbital O {\n  trait T -> E [interaction] {\n    initial: id|");
        assert_ne!(complete(&src, off, None).context, "field-type");
    }

    #[test]
    fn control_a_name_position_offers_no_keyword() {
        let (src, off) = at("orbital O {\n  entity E [runtime] {\n    id : string!\n    ti|");
        let r = complete(&src, off, None);
        assert_eq!(
            r.candidates,
            vec![],
            "a field name is free — `ticks` is not an entity statement"
        );
        let (src, off) =
            at("orbital O {\n  trait T -> E [interaction] {\n    state idle {\n      IN|");
        assert_eq!(complete(&src, off, None).candidates, vec![]);
    }

    #[test]
    fn edge_every_scoped_keyword_is_a_parser_keyword() {
        let all = vocabulary().keywords;
        for header in [
            "",
            "orbital O",
            "orbital O = A.orbitals.X",
            "trait T -> E",
            "trait T = A.traits.X -> E",
            "entity E [runtime]",
        ] {
            for k in block_keywords(header).unwrap_or(&[]) {
                assert!(
                    all.contains(k),
                    "{k} (in `{header}`) is not in the parser's vocabulary"
                );
            }
        }
    }

    #[test]
    fn edge_nothing_is_invented_unknown_alias_and_value_positions_offer_nothing() {
        let (src, off) = at("trait L = Nope.traits.|");
        assert_eq!(complete(&src, off, Some(&contracts())).candidates, vec![]);
        let (src, off) = at("(render-ui main <Button variant={|");
        assert_ne!(complete(&src, off, None).context, "pattern-prop");
        let (src, off) = at("|");
        assert_eq!(complete(&src, off, None).context, "keyword");
    }

    #[test]
    fn edge_an_offset_inside_a_multibyte_char_snaps_back_to_its_boundary() {
        let src = "\"A — b\"\n  uses Shell from \"std/beh";
        let dash = src.find('—').expect("dash");
        for off in [dash + 1, dash + 2] {
            let r = complete(src, off, Some(&contracts()));
            assert_eq!(r.context, "none");
            assert_eq!(r.prefix, "");
        }
        let r = complete(src, src.len(), Some(&contracts()));
        assert_eq!(r.prefix, "std/beh");
    }
}
