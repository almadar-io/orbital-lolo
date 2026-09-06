//! The `.lolo` parser's own vocabulary — one list per B1 section, verified
//! against every `expect_keyword`/`check_ident` call site, `is_primitive_type`,
//! the `type.rs` special type names, sigil roots, tag/category/capability
//! idents, tick units, and literal words. This is the parser's SOURCE OF
//! TRUTH: `orbital_core::i18n` tables are checked against this list (never
//! the other way around) so a new inline keyword literal is caught here
//! first, at the parser's own call sites (`debug_assert!` in
//! `expect_keyword`/`check_ident`).

pub struct Vocabulary {
    pub keywords: &'static [&'static str],
    pub shapes: &'static [&'static str],
    pub tags: &'static [&'static str],
    pub categories: &'static [&'static str],
    pub capabilities: &'static [&'static str],
    pub annotations: &'static [&'static str],
    pub sigils: &'static [&'static str],
    pub effects: &'static [&'static str],
    pub types: &'static [&'static str],
    pub units: &'static [&'static str],
    pub literals: &'static [&'static str],
    pub reserved_events: &'static [&'static str],
}

/// `app orbital uses expects entity type derived trait initial state for
/// page as theme emits listens ticks config when with from to every
/// capabilities match external internal on name fields events event
/// emitsScope identity pages omit only roles` — every
/// `expect_keyword`/`check_ident` literal in `parser/*.rs` (`parser/mod.rs`,
/// `parser/entity.rs`, `parser/trait.rs`, `parser/type.rs`, `parser/page.rs`,
/// `parser/orbital.rs`) that is not itself a shape/tag/category/capability/
/// annotation word. `pages`/`omit`/`only`/`roles` are the orbital-reference
/// body arms (orbital-import Stage A); `roles` parses ONLY to reject with a
/// Stage-B error, never lowered.
const KEYWORDS: &[&str] = &[
    "app",
    "orbital",
    "uses",
    "expects",
    "entity",
    "type",
    "derived",
    "trait",
    "initial",
    "state",
    "for",
    "page",
    "as",
    "theme",
    "emits",
    "listens",
    "ticks",
    "config",
    "when",
    "with",
    "from",
    "to",
    "every",
    "capabilities",
    "match",
    "external",
    "internal",
    "on",
    "name",
    "fields",
    "events",
    "event",
    "emitsScope",
    "identity",
    "pages",
    "omit",
    "only",
    "roles",
];

/// `Entity Trait Page Orbital Event` — `check_ident` in `parser/type.rs`
/// (`parse_type_body_atom`) plus `VALID_TYPE_PARAM_KINDS`.
const SHAPES: &[&str] = &["Entity", "Trait", "Page", "Orbital", "Event"];

/// `persistent runtime shared identity instance collection` — the entity
/// `[...]` persistence-tag list (`parser/entity.rs parse_persistence_tag`)
/// plus the trait `[category, scope, ...]` modifier list's `instance`/
/// `collection` scope markers (`parser/trait.rs parse_trait`). `identity`
/// is shared with `KEYWORDS` (same English word, same translation, B1
/// rule 3).
const TAGS: &[&str] = &[
    "persistent",
    "runtime",
    "shared",
    "identity",
    "instance",
    "collection",
];

/// `interaction integration lifecycle game-core game-entity game-ui` — the
/// trait `[category]` modifier's first non-scope word
/// (`parser/trait.rs parse_trait`, interpreted by `lower/trait.rs`).
const CATEGORIES: &[&str] = &[
    "interaction",
    "integration",
    "lifecycle",
    "game-core",
    "game-entity",
    "game-ui",
];

/// `read write browse create update delete notify schedule invoke` — the
/// trait `[category, scope, ...capabilities]` modifier's trailing words
/// (`parser/trait.rs parse_trait`), and the `(persist create|update|delete
/// …)` effect's action argument. `read`/`create`/`update`/`delete` are
/// shared with `ANNOTATIONS` (the `@read`/`@create`/`@update`/`@delete`
/// entity access-policy directives) — same English word, same translation.
const CAPABILITIES: &[&str] = &[
    "read", "write", "browse", "create", "update", "delete", "notify", "schedule", "invoke",
];

/// `key unique intrinsic label description synonyms tier rebindable read
/// create update delete` — the `@x` annotation sigils at
/// `parser/entity.rs` (`@key`/`@unique`/`@intrinsic`/`@description`/
/// `@synonyms`, `@read`/`@create`/`@update`/`@delete`), `parser/trait.rs`
/// (`@rebindable`, `@description`/`@synonyms` binding annotations,
/// `@label`/`@description`/`@synonyms`/`@tier` config annotations), and
/// `parser/page.rs` (`@label`). NOTE (discrepancy, reported — see the
/// program report): `parser/page.rs` also accepts `@icon` at the same
/// call site as `@label`, but `icon` is not a B1 annotations-section
/// word; listed here anyway since this table must stay faithful to the
/// parser, not to B1/en.json.
const ANNOTATIONS: &[&str] = &[
    "key",
    "unique",
    "intrinsic",
    "label",
    "description",
    "synonyms",
    "tier",
    "rebindable",
    "read",
    "create",
    "update",
    "delete",
    "icon",
];

/// `entity payload state now config user trait` — sigil roots (the text
/// before the first `.` of a `@x`/`@x.y` token) recognized at value
/// position (`parser/expression.rs`), type position (`parser/type.rs`),
/// term-match scrutinees (`parser/trait.rs parse_term_match_scrutinee`),
/// and the `@config.<knob>` event/type reference form
/// (`parser/mod.rs config_ref_knob`).
const SIGILS: &[&str] = &[
    "entity", "payload", "state", "now", "config", "user", "trait",
];

/// `set fetch persist emit navigate navigate-back render-ui notify ref
/// call-service invoke do if let fn lambda match when` — s-expression
/// heads special-cased in `parser/expression.rs` (`parse_sexpr`'s
/// `loop_arg_max` dispatch, the `let`/`fn`/`lambda`/`match` sugar, the
/// `set` post-check) plus effect heads read by `lower/*.rs`.
const EFFECTS: &[&str] = &[
    "set",
    "fetch",
    "persist",
    "emit",
    "navigate",
    "navigate-back",
    "render-ui",
    "notify",
    "ref",
    "call-service",
    "invoke",
    "do",
    "if",
    "let",
    "fn",
    "lambda",
    "match",
    "when",
];

/// `string number int float bool boolean timestamp date datetime duration
/// money scalar opaque file email url phone uuid image icon asset
/// component object void null any` (`parser/mod.rs is_primitive_type`) plus
/// `trait entity event slot pattern Map secret SExpr` (`parser/type.rs`
/// special type names). `entity`/`event`/`trait` are shared with
/// `KEYWORDS`/`SIGILS` (same English word).
const TYPES: &[&str] = &[
    "string",
    "number",
    "int",
    "float",
    "bool",
    "boolean",
    "timestamp",
    "date",
    "datetime",
    "duration",
    "money",
    "scalar",
    "opaque",
    "file",
    "email",
    "url",
    "phone",
    "uuid",
    "image",
    "icon",
    "asset",
    "component",
    "object",
    "void",
    "null",
    "any",
    "trait",
    "entity",
    "event",
    "slot",
    "pattern",
    "Map",
    "secret",
    "SExpr",
];

/// `ms s m h d` — tick-interval duration suffixes (`parser/trait.rs
/// parse_tick_interval`).
const UNITS: &[&str] = &["ms", "s", "m", "h", "d"];

/// `true false null` — the lexer's language-free literal words
/// (`lexer.rs lex_identifier_or_keyword`); a non-English rendering of
/// these lexes as a plain `Identifier` and is canonicalized by the parser
/// (`parse_sexpr_or_value`) via `Lang::canon_literal`.
const LITERALS: &[&str] = &["true", "false", "null"];

/// `INIT` — the reserved lifecycle event name, canonicalized at every
/// event-name capture site (`parse_transition`,
/// `expect_event_name_or_config_ref`).
const RESERVED_EVENTS: &[&str] = &["INIT"];

pub fn vocabulary() -> &'static Vocabulary {
    static V: Vocabulary = Vocabulary {
        keywords: KEYWORDS,
        shapes: SHAPES,
        tags: TAGS,
        categories: CATEGORIES,
        capabilities: CAPABILITIES,
        annotations: ANNOTATIONS,
        sigils: SIGILS,
        effects: EFFECTS,
        types: TYPES,
        units: UNITS,
        literals: LITERALS,
        reserved_events: RESERVED_EVENTS,
    };
    &V
}

/// A word the parser treats as reserved at some keyword/shape/tag/
/// category/capability/annotation call site — used by
/// `expect_keyword`/`check_ident`'s `debug_assert!` so a new inline
/// literal fails the parser's own tests until it is added here.
pub fn is_known_keyword(word: &str) -> bool {
    let v = vocabulary();
    v.keywords.contains(&word)
        || v.shapes.contains(&word)
        || v.tags.contains(&word)
        || v.categories.contains(&word)
        || v.capabilities.contains(&word)
        || v.annotations.contains(&word)
}

fn section_by_name<'a>(
    def: &'a orbital_core::i18n::tables::I18nDefinition,
    name: &str,
) -> &'a std::collections::BTreeMap<String, String> {
    match name {
        "keywords" => &def.keywords,
        "shapes" => &def.shapes,
        "tags" => &def.tags,
        "categories" => &def.categories,
        "capabilities" => &def.capabilities,
        "annotations" => &def.annotations,
        "sigils" => &def.sigils,
        "effects" => &def.effects,
        "types" => &def.types,
        "units" => &def.units,
        "literals" => &def.literals,
        "reservedEvents" => &def.reserved_events,
        other => panic!("unknown vocabulary section: {other}"),
    }
}

fn vocab_sections() -> [(&'static str, &'static [&'static str]); 12] {
    let v = vocabulary();
    [
        ("keywords", v.keywords),
        ("shapes", v.shapes),
        ("tags", v.tags),
        ("categories", v.categories),
        ("capabilities", v.capabilities),
        ("annotations", v.annotations),
        ("sigils", v.sigils),
        ("effects", v.effects),
        ("types", v.types),
        ("units", v.units),
        ("literals", v.literals),
        ("reservedEvents", v.reserved_events),
    ]
}

/// Section-set diffs, both directions, between `en.json`'s tables and this
/// parser's `Vocabulary` — one problem string per disagreeing key. Empty
/// when they agree exactly. Shared by the `keywords_match_tables` test and
/// `orb i18n check`.
pub fn vocabulary_vs_en_problems() -> Vec<String> {
    let en = &orbital_core::i18n::tables::definitions()["en"];
    let mut problems = Vec::new();
    for (name, vocab) in vocab_sections() {
        let en_map = section_by_name(en, name);
        for key in en_map.keys() {
            if !vocab.contains(&key.as_str()) {
                problems.push(format!(
                    "{name}.{key}: in orbital_core::i18n en.json but not orbital-lolo Vocabulary"
                ));
            }
        }
        for word in vocab {
            if !en_map.contains_key(*word) {
                problems.push(format!(
                    "{name}.{word}: in orbital-lolo Vocabulary but not orbital_core::i18n en.json"
                ));
            }
        }
    }
    problems
}

/// Every non-English language must cover (non-empty) every entry in every
/// section this parser actually uses — one problem string per missing/empty
/// entry. Shared by `non_english_langs_cover_parser_vocabulary` and
/// `orb i18n check`.
pub fn non_english_coverage_problems() -> Vec<String> {
    let defs = orbital_core::i18n::tables::definitions();
    let mut problems = Vec::new();
    for (code, def) in defs {
        if code == "en" {
            continue;
        }
        for (name, words) in vocab_sections() {
            let map = section_by_name(def, name);
            for word in words {
                match map.get(*word) {
                    None => problems.push(format!("{code}.{name}.{word}: missing")),
                    Some(v) if v.is_empty() => {
                        problems.push(format!("{code}.{name}.{word}: empty"))
                    }
                    Some(_) => {}
                }
            }
        }
    }
    problems
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keywords_match_tables() {
        let problems = vocabulary_vs_en_problems();
        assert!(
            problems.is_empty(),
            "keywords_match_tables: {} problem(s) (never truncated): {:?}",
            problems.len(),
            problems
        );
    }

    /// Every non-English language must cover every entry in every section
    /// this parser actually uses. Expected to fail (reported, not silently
    /// passed) until `ar.json`/`sl.json` are complete — see the program
    /// report.
    #[test]
    fn non_english_langs_cover_parser_vocabulary() {
        let problems = non_english_coverage_problems();
        assert!(
            problems.is_empty(),
            "non_english_langs_cover_parser_vocabulary: {} problem(s) — expected to fail \
             until ar.json/sl.json are complete (report verbatim, never truncated): {:?}",
            problems.len(),
            problems
        );
    }
}
