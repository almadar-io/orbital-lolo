//! App and page modifiers share one `[key: value, …]` bracket grammar:
//! `app NAME "VER" [description: …, theme: …, locales: […], …]` and
//! `page "/p" -> A, B [view: …, access: …, title: …]`.

use orbital_core_base::schema::{PageAccess, PageIndexing, PageMeta, PageRef};
use orbital_lolo::{format_program, parse_and_lower_str, parse_str};

const BODY: &str = r#"
orbital Home {
  entity HomeState [runtime] {
    id : string!
  }

  trait HomeView -> HomeState [interaction, instance] {
    initial: idle

    state idle {
      INIT -> idle
        (render-ui main { type: typography, content: (i18n/t "hero.title") })
    }
  }

  page "/" as HomePage -> HomeView
}
"#;

const LEGACY_HEADER: &str = r#"app std-home "1.0.0"
"The home page"
theme "orb-light"
locales [en, ar, sl]
public "./public"
"#;

const BRACKET_HEADER: &str = r#"app std-home [
  version: "1.0.0",
  description: "The home page",
  theme: "orb-light",
  locales: [en, ar, sl],
  public: "./public"
]
"#;

/// The lowered program as JSON with minted ids and the ledger removed, so two
/// parses of equivalent source compare equal.
fn shape(source: &str) -> serde_json::Value {
    fn strip(v: &mut serde_json::Value) {
        match v {
            serde_json::Value::Object(map) => {
                map.remove("ledger");
                for value in map.values_mut() {
                    strip(value);
                }
            }
            serde_json::Value::Array(items) => items.iter_mut().for_each(strip),
            serde_json::Value::String(s) => {
                let minted = s.len() == 30
                    && ["orb_", "ent_", "trt_", "evt_", "pag_"]
                        .iter()
                        .any(|p| s.starts_with(p));
                if minted {
                    *s = "ID".to_string();
                }
            }
            _ => {}
        }
    }
    let mut value = serde_json::to_value(parse_and_lower_str(source).expect("lower")).unwrap();
    strip(&mut value);
    value
}

fn program(header: &str) -> String {
    format!("{header}{BODY}")
}

fn with_page(options: &str) -> String {
    program(BRACKET_HEADER).replace(
        r#"page "/" as HomePage -> HomeView"#,
        &format!(r#"page "/" as HomePage -> HomeView {options}"#),
    )
}

fn inline_page(source: &str) -> orbital_core_base::schema::PageDefinition {
    let schema = parse_and_lower_str(source).expect("lower");
    match &schema.orbitals[0].pages.as_ref().expect("pages")[0] {
        PageRef::Inline(p) => p.clone(),
        other => panic!("expected inline page, got {other:?}"),
    }
}

#[test]
fn the_bracket_header_lowers_like_the_legacy_lines() {
    assert_eq!(
        shape(&program(LEGACY_HEADER)),
        shape(&program(BRACKET_HEADER))
    );
}

#[test]
fn the_legacy_header_is_marked_and_the_bracket_is_not() {
    assert!(parse_str(&program(LEGACY_HEADER)).unwrap().app.legacy_lines);
    assert!(
        !parse_str(&program(BRACKET_HEADER))
            .unwrap()
            .app
            .legacy_lines
    );
}

#[test]
fn control_a_bare_app_line_is_not_legacy() {
    let p = parse_str(&program("app std-home\n")).unwrap();
    assert!(!p.app.legacy_lines);
    assert_eq!(p.app.description, None);
}

#[test]
fn site_modifiers_lower_into_schema_site() {
    let header = r#"app std-home [
  version: "1.0.0",
  origin: "https://orb.almadar.io",
  icon: "/favicon.svg",
  siteName: "Orb",
  signIn: "/login"
]
"#;
    let site = parse_and_lower_str(&program(header))
        .unwrap()
        .site
        .expect("site");
    assert_eq!(site.origin.as_deref(), Some("https://orb.almadar.io"));
    assert_eq!(site.icon.as_deref(), Some("/favicon.svg"));
    assert_eq!(site.name.as_deref(), Some("Orb"));
    assert_eq!(site.sign_in.as_deref(), Some("/login"));
}

const THEMES_HEADER: &str = r#"app std-home [
  version: "1.0.0",
  theme: "almadar-light",
  themes: ["almadar-dark", "clay-light"]
]
"#;

#[test]
fn the_themes_a_program_switches_to_lower_into_schema_themes() {
    let schema = parse_and_lower_str(&program(THEMES_HEADER)).expect("lower");
    assert_eq!(
        schema.themes,
        Some(vec!["almadar-dark".to_string(), "clay-light".to_string()])
    );
    assert!(matches!(
        schema.theme,
        Some(orbital_core_base::schema::ThemeRef::Reference(ref k)) if k == "almadar-light"
    ));
}

#[test]
fn control_no_themes_modifier_means_no_themes() {
    assert!(parse_and_lower_str(&program(BRACKET_HEADER))
        .unwrap()
        .themes
        .is_none());
}

#[test]
fn edge_a_themes_entry_is_a_key_not_an_inline_theme() {
    let header = "app std-home [version: \"1.0.0\", themes: [{ name: \"brand\" }]]\n";
    assert!(parse_and_lower_str(&program(header)).is_err());
}

#[test]
fn themes_survive_format_and_unlower() {
    let schema = parse_and_lower_str(&program(THEMES_HEADER)).expect("lower");
    let formatted = format_program(&parse_str(&program(THEMES_HEADER)).expect("parse"));
    assert!(
        formatted.contains("themes: [\"almadar-dark\", \"clay-light\"]"),
        "{formatted}"
    );
    assert_eq!(parse_and_lower_str(&formatted).expect("reparse").themes, schema.themes);
    let text = orbital_lolo::schema_to_lolo(&schema).expect("schema_to_lolo");
    assert_eq!(parse_and_lower_str(&text).expect("lower the unlowered text").themes, schema.themes);
}

#[test]
fn control_no_site_modifiers_means_no_site() {
    assert!(parse_and_lower_str(&program(BRACKET_HEADER))
        .unwrap()
        .site
        .is_none());
}

#[test]
fn the_bracket_takes_commas_or_newlines_and_one_line() {
    let one_line = "app std-home [version: \"1.0.0\", description: \"The home page\", theme: \"orb-light\", locales: [en, ar, sl], public: \"./public\"]\n";
    let no_commas = "app std-home [\n  version: \"1.0.0\",\n  description: \"The home page\"\n  theme: \"orb-light\"\n  locales: [en ar sl]\n  public: \"./public\"\n]\n";
    let expected = shape(&program(BRACKET_HEADER));
    for header in [one_line, no_commas] {
        assert_eq!(shape(&program(header)), expected, "{header}");
    }
}

#[test]
fn an_inline_theme_block_is_a_bracket_value() {
    let header = "app std-home [\n  version: \"1.0.0\",\n  theme: {\n    name: \"brand\"\n    tokens { colors { primary: \"#123456\" } }\n  },\n  locales: [en]\n]\n";
    let schema = parse_and_lower_str(&program(header)).expect("lower");
    assert!(matches!(
        schema.theme,
        Some(orbital_core_base::schema::ThemeRef::Inline(_))
    ));
    assert_eq!(schema.locales, Some(vec!["en".to_string()]));
}

#[test]
fn an_unknown_app_modifier_is_rejected() {
    let err =
        parse_and_lower_str(&program("app std-home [version: \"1.0.0\", colour: \"red\"]\n")).unwrap_err();
    assert!(err.to_string().contains("ORB_L_OPTION_UNKNOWN"), "{err}");
}

#[test]
fn a_duplicate_app_modifier_is_rejected() {
    let err = parse_and_lower_str(&program(
        "app std-home [version: \"1.0.0\", theme: \"a-light\", theme: \"b-light\"]\n",
    ))
    .unwrap_err();
    assert!(err.to_string().contains("ORB_L_OPTION_DUPLICATE"), "{err}");
}

#[test]
fn a_non_string_site_value_is_rejected() {
    let err = parse_and_lower_str(&program("app std-home [version: \"1.0.0\", origin: orb]\n")).unwrap_err();
    assert!(err.to_string().contains("ORB_L_OPTION_VALUE"), "{err}");
}

#[test]
fn page_modifiers_lower_onto_the_page() {
    let page = inline_page(&with_page(
        "[\n  access: public,\n  indexing: index,\n  title: \"Home\",\n  description: (i18n/t \"meta.description\")\n]",
    ));
    assert_eq!(page.access, Some(PageAccess::Public));
    assert_eq!(page.indexing, Some(PageIndexing::Index));
    assert_eq!(page.title, Some(PageMeta::Literal("Home".to_string())));
    assert_eq!(
        page.description,
        Some(PageMeta::message("std-home:meta.description"))
    );
}

#[test]
fn control_omitted_page_modifiers_stay_absent() {
    let page = inline_page(&program(BRACKET_HEADER));
    assert_eq!((page.access, page.indexing), (None, None));
    assert!(page.title.is_none() && page.description.is_none());
    let json = serde_json::to_string(&page).unwrap();
    for key in ["access", "indexing", "title", "description"] {
        assert!(!json.contains(&format!("\"{key}\"")), "{json}");
    }
}

#[test]
fn legacy_page_options_still_parse_alongside_new_ones() {
    let page = inline_page(&with_page(
        "[view: list, entity: HomeState, initial, access: authenticated]",
    ));
    assert_eq!(page.view_type.as_deref(), Some("list"));
    assert_eq!(page.primary_entity.as_deref(), Some("HomeState"));
    assert_eq!(page.is_initial, Some(true));
    assert_eq!(page.access, Some(PageAccess::Authenticated));
}

#[test]
fn an_unknown_page_modifier_is_rejected_not_dropped() {
    let err = parse_and_lower_str(&with_page("[layout: wide]")).unwrap_err();
    assert!(err.to_string().contains("ORB_L_OPTION_UNKNOWN"), "{err}");
}

#[test]
fn a_duplicate_page_modifier_is_rejected() {
    let err =
        parse_and_lower_str(&with_page("[access: public, access: authenticated]")).unwrap_err();
    assert!(err.to_string().contains("ORB_L_OPTION_DUPLICATE"), "{err}");
}

#[test]
fn an_invalid_access_value_is_rejected() {
    let err = parse_and_lower_str(&with_page("[access: private]")).unwrap_err();
    assert!(err.to_string().contains("ORB_P_ACCESS_INVALID"), "{err}");
}

#[test]
fn an_invalid_indexing_value_is_rejected() {
    let err = parse_and_lower_str(&with_page("[indexing: follow]")).unwrap_err();
    assert!(err.to_string().contains("ORB_P_INDEXING_INVALID"), "{err}");
}

#[test]
fn metadata_bound_to_an_entity_is_rejected() {
    for value in [
        "@entity.name",
        "(str/concat \"a\" \"b\")",
        "(i18n/t @entity.key)",
    ] {
        let err = parse_and_lower_str(&with_page(&format!("[title: {value}]"))).unwrap_err();
        assert!(
            err.to_string().contains("ORB_P_META_FRAME"),
            "{value}: {err}"
        );
    }
}

#[test]
fn many_traits_keep_their_roster_with_a_multiline_bracket() {
    let traits: Vec<String> = (0..18).map(|i| format!("T{i}")).collect();
    let source = format!(
        "{BRACKET_HEADER}orbital Home {{\n  page \"/\" as HomePage -> {} [\n    access: public,\n    title: \"Home\"\n  ]\n}}\n",
        traits.join(", ")
    );
    let prog = parse_str(&source).expect("parse");
    let page = &prog.orbitals[0].pages[0];
    assert_eq!(page.traits, traits);
    assert_eq!(page.access, Some(PageAccess::Public));
}

#[test]
fn a_page_reference_carries_explicit_overrides_and_omits_the_rest() {
    let source = format!(
        "{BRACKET_HEADER}orbital Home {{\n  uses Up from \"./up\"\n  entity HomeState [runtime] {{\n    id : string!\n  }}\n  page \"/x\" = Up.pages.Main [access: authenticated]\n}}\n"
    );
    let schema = parse_and_lower_str(&source).expect("lower");
    match &schema.orbitals[0].pages.as_ref().expect("pages")[0] {
        PageRef::Object(o) => {
            assert_eq!(o.access, Some(PageAccess::Authenticated));
            assert!(o.indexing.is_none() && o.title.is_none());
        }
        other => panic!("expected reference, got {other:?}"),
    }
}

#[test]
fn the_formatter_prints_brackets_and_round_trips() {
    for source in [
        program(LEGACY_HEADER),
        with_page("[access: public, indexing: noindex, title: (i18n/t \"meta.title\")]"),
    ] {
        let formatted = format_program(&parse_str(&source).expect("parse"));
        assert!(
            formatted.contains("app std-home [\n  version: \"1.0.0\",\n"),
            "{formatted}"
        );
        assert!(
            !formatted.contains("\ntheme \"") && !formatted.contains("\nlocales ["),
            "{formatted}"
        );
        assert_eq!(shape(&source), shape(&formatted), "{formatted}");
        let reparsed = parse_str(&formatted).unwrap();
        assert!(!reparsed.app.legacy_lines);
    }
}

#[test]
fn the_formatter_writes_page_metadata_as_source() {
    let formatted = format_program(
        &parse_str(&with_page(
            "[access: public, title: \"Say \\\"hi\\\"\", description: (i18n/t \"meta.d\")]",
        ))
        .unwrap(),
    );
    assert!(formatted.contains("access: public,"), "{formatted}");
    assert!(formatted.contains(r#"title: "Say \"hi\"","#), "{formatted}");
    assert!(
        formatted.contains(r#"description: (i18n/t "meta.d")"#),
        "{formatted}"
    );
}

#[test]
fn localized_bracket_keys_and_values_lower_like_english() {
    let english = "app Shop [\n  version: \"1.0.0\",\n  theme: \"orb-light\",\n  locales: [en, sl],\n  signIn: \"/prijava\"\n]\norbital Shop {\n  entity Item [runtime] {\n    id : string!\n  }\n  page \"/\" as Home -> View [access: authenticated, indexing: noindex, title: \"Domov\"]\n}\n";
    let slovene = "aplikacija Shop [\n  razlicica: \"1.0.0\",\n  tema: \"orb-light\",\n  jeziki: [en, sl],\n  prijava: \"/prijava\"\n]\norbitala Shop {\n  entiteta Item [runtime] {\n    id : string!\n  }\n  stran \"/\" as Home -> View [dostop: prijavljen, indeksiranje: brezIndeksa, naziv: \"Domov\"]\n}\n";
    assert_eq!(shape(slovene), shape(english));
}

#[test]
fn a_positional_version_is_legacy_and_lowers_like_the_bracket_version() {
    let positional = "app std-home \"1.0.0\" [\n  description: \"The home page\",\n  theme: \"orb-light\",\n  locales: [en, ar, sl],\n  public: \"./public\"\n]\n";
    assert!(parse_str(&program(positional)).unwrap().app.legacy_lines);
    assert_eq!(shape(&program(positional)), shape(&program(BRACKET_HEADER)));
    assert_eq!(parse_and_lower_str(&program(BRACKET_HEADER)).unwrap().version.as_deref(), Some("1.0.0"));
}

#[test]
fn a_version_both_positional_and_in_the_bracket_is_rejected() {
    let err = parse_and_lower_str(&program("app std-home \"1.0.0\" [version: \"2.0.0\"]\n")).unwrap_err();
    assert!(err.to_string().contains("ORB_L_OPTION_DUPLICATE"), "{err}");
}

const GRANTS_PROGRAM: &str = r#"app Shop [
  version: "1.0.0",
  egress: ["https://api.example.org"],
  fs: { "/data/**": [read] },
  stdio: [out],
  deployment: { api: [Orders, Billing], web: [Storefront] }
]

orbital Orders [
  fs: { "/data/orders/**": [read, write, create] },
  exec: ["git"],
  net: { "db.internal:5432": [tcp], "0.0.0.0:8080": [listen] },
  env: ["HOME"],
  stdio: [out, err],
  sys: [memory, power-state],
  desktop: [notify]
] {
  entity Order [runtime] {
    id : string!
  }
}

orbital Billing {
  entity Bill [runtime] {
    id : string!
  }
}

orbital Storefront {
  entity Item [runtime] {
    id : string!
  }
}
"#;

fn grants_error(source: &str) -> String {
    parse_and_lower_str(source).expect_err("must not lower").to_string()
}

#[test]
fn app_and_orbital_grants_lower_beside_egress_and_absent_stays_none() {
    use orbital_core_base::os_grants::{DesktopKind, FsMode, NetKind, StdioKind, SysKind};
    let schema = parse_and_lower_str(GRANTS_PROGRAM).expect("lower");
    let app = schema.os_grants.as_ref().expect("app-level grants");
    assert!(app.fs_allows(std::path::Path::new("/data/a"), FsMode::Read));
    assert!(app.stdio_allows(StdioKind::Out) && !app.stdio_allows(StdioKind::Err));
    let units = schema.deployment.as_ref().expect("deployment");
    assert_eq!(units["api"], ["Orders", "Billing"]);
    assert_eq!(units["web"], ["Storefront"]);
    assert_eq!(schema.egress.as_deref(), Some(&["https://api.example.org".to_string()][..]));

    let orders = schema.orbitals[0].os_grants.as_ref().expect("orbital grants");
    assert!(orders.fs_allows(std::path::Path::new("/data/orders/x"), FsMode::Create));
    assert!(!orders.fs_allows(std::path::Path::new("/data/x"), FsMode::Read), "the app level is not folded in");
    assert!(orders.exec_allows("git") && orders.env_allows("HOME"));
    assert!(orders.net_allows("db.internal", 5432, NetKind::Tcp));
    assert!(orders.net_allows("0.0.0.0", 8080, NetKind::Listen));
    assert!(orders.sys_allows(SysKind::PowerState) && orders.desktop_allows(DesktopKind::Notify));
    assert!(schema.orbitals[1].os_grants.is_none());

    let plain = parse_and_lower_str(&GRANTS_PROGRAM.replace("fs: { \"/data/**\": [read] },\n  stdio: [out],\n  deployment: { api: [Orders, Billing], web: [Storefront] }", "")).expect("lower");
    assert!(plain.os_grants.is_none() && plain.deployment.is_none());
}

#[test]
fn grants_survive_format_and_unlower() {
    let schema = parse_and_lower_str(GRANTS_PROGRAM).expect("lower");
    let formatted = format_program(&parse_str(GRANTS_PROGRAM).expect("parse"));
    for needle in ["fs: { \"/data/**\": [read] }", "deployment: { api: [Orders, Billing], web: [Storefront] }", "exec: [\"git\"]", "sys: [memory, power-state]"] {
        assert!(formatted.contains(needle), "{needle}\n{formatted}");
    }
    let again = parse_and_lower_str(&formatted).expect("reparse");
    assert_eq!(again.os_grants, schema.os_grants);
    assert_eq!(again.deployment, schema.deployment);
    assert_eq!(again.orbitals[0].os_grants, schema.orbitals[0].os_grants);
    let text = orbital_lolo::schema_to_lolo(&schema).expect("schema_to_lolo");
    let back = parse_and_lower_str(&text).expect("lower the unlowered text");
    assert_eq!(back.os_grants, schema.os_grants);
    assert_eq!(back.deployment, schema.deployment);
    assert_eq!(back.orbitals[0].os_grants, schema.orbitals[0].os_grants);
}

#[test]
fn a_grant_word_outside_the_closed_set_is_an_invalid_entry() {
    let err = grants_error(&GRANTS_PROGRAM.replace("[read, write, create]", "[read, wrte]"));
    assert!(err.contains("ORB_OS_GRANT_ENTRY_INVALID"), "{err}");
    let err = grants_error(&GRANTS_PROGRAM.replace("sys: [memory, power-state]", "sys: [gpu]"));
    assert!(err.contains("ORB_OS_GRANT_ENTRY_INVALID"), "{err}");
}

#[test]
fn grant_brackets_reject_unknown_and_repeated_keys_like_the_app_bracket() {
    let err = grants_error(&GRANTS_PROGRAM.replace("exec: [\"git\"],", "exec: [\"git\"], execs: [\"x\"],"));
    assert!(err.contains("ORB_L_OPTION_UNKNOWN"), "{err}");
    let err = grants_error(&GRANTS_PROGRAM.replace("exec: [\"git\"],", "exec: [\"git\"], exec: [\"ls\"],"));
    assert!(err.contains("ORB_L_OPTION_DUPLICATE"), "{err}");
    let err = grants_error(&GRANTS_PROGRAM.replace("stdio: [out],", "stdio: [out], stdio: [err],"));
    assert!(err.contains("ORB_L_OPTION_DUPLICATE"), "{err}");
    let err = grants_error(&GRANTS_PROGRAM.replace("web: [Storefront]", "api: [Storefront]"));
    assert!(err.contains("ORB_L_OPTION_DUPLICATE"), "{err}");
}

#[test]
fn a_kv_grant_lowers_per_store_and_mode_and_survives_format_and_unlower() {
    use orbital_core_base::os_grants::KvMode;
    let source = GRANTS_PROGRAM.replace("  desktop: [notify]\n", "  desktop: [notify],\n  kv: { rows: [read, write], \"rows-log*\": [read] }\n");
    let schema = parse_and_lower_str(&source).expect("lower");
    let orders = schema.orbitals[0].os_grants.as_ref().expect("orbital grants");
    assert!(orders.kv_allows("rows", KvMode::Write) && orders.kv_allows("rows-log-2", KvMode::Read));
    assert!(!orders.kv_allows("rows-log-2", KvMode::Write), "control: the glob grants read only");
    assert!(!orders.kv_allows("other", KvMode::Read), "control: an unnamed store");
    assert!(schema.os_grants.as_ref().expect("app grants").kv.is_empty(), "control: the app level declares none");

    let formatted = format_program(&parse_str(&source).expect("parse"));
    assert!(formatted.contains(r#"kv: { "rows": [read, write], "rows-log*": [read] }"#), "{formatted}");
    assert_eq!(parse_and_lower_str(&formatted).expect("reparse").orbitals[0].os_grants, schema.orbitals[0].os_grants);
    let text = orbital_lolo::schema_to_lolo(&schema).expect("schema_to_lolo");
    assert_eq!(parse_and_lower_str(&text).expect("lower the unlowered text").orbitals[0].os_grants, schema.orbitals[0].os_grants);

    let err = grants_error(&source.replace("[read, write]", "[read, wrte]"));
    assert!(err.contains("ORB_OS_GRANT_ENTRY_INVALID"), "{err}");
    let err = grants_error(&source.replace("kv: { rows: [read, write],", "kv: { rows: [read, write], rows: [read],"));
    assert!(err.contains("ORB_L_OPTION_DUPLICATE"), "edge: a store named twice: {err}");
}
