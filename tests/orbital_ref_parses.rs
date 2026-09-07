//! Orbital-import Stage A — `.lolo` surface tests for the orbital
//! REFERENCE form (`orbital Name = Alias.orbitals.Y { ... }`), file-scoped
//! `uses`, and the two `config {}` declaration sites (app-level, orbital-
//! level). See `docs/Almadar_Orbital_Import.md` §4 and the W2-R work item.

use orbital_core::schema::{
    ConfigField as SchemaConfigField, EntityRef, SExpression, TraitReference,
};
use orbital_lolo::ast::TypeExpr;
use orbital_lolo::{parse_and_lower_str, parse_str};

/// Drop the trailing `Span` (Stage C `.lolo` spans) from a 2- or 3-element
/// tuple so an assertion can compare just the semantic content — the parser
/// tests don't pin exact spans, only names/values.
fn strip_span2<A: Clone, B: Clone, S>(v: &[(A, B, S)]) -> Vec<(A, B)> {
    v.iter().map(|(a, b, _)| (a.clone(), b.clone())).collect()
}
fn strip_span1<A: Clone, S>(v: &[(A, S)]) -> Vec<A> {
    v.iter().map(|(a, _)| a.clone()).collect()
}

const SOURCE: &str = r#"
uses TimeTracking from "almadar-behaviors/std-time-tracking" {
  config {
    appName: "PF"
  }
}

orbital PfTimesheets = TimeTracking.orbitals.TimesheetPanelOrbital {
  entity Timesheet
  entities {
    Employee: Person
  }
  fields {
    periodStart: from
  }
  roles {
    approver: [owner, project_manager]
    employee: team_member
  }
  pages {
    "/timesheets": "/pf/timesheets" -> TimesheetApproval
  }
  omit {
    TimesheetDelete
  }
  config {
    pageSize: 50
  }
  events {
    TIMESHEET_APPROVED: PF_APPROVED
  }
  trait TimesheetApproval -> Timesheet [interaction, instance] {
    state draft {
      SUBMIT -> submitted
        (set @id "x")
    }
    state submitted {
    }
  }
}
"#;

#[test]
fn file_scoped_uses_parses_with_config_override() {
    let program = parse_str(SOURCE).expect("parse");
    assert_eq!(program.uses.len(), 1, "one file-scoped uses decl");
    let u = &program.uses[0];
    assert_eq!(u.alias, "TimeTracking");
    assert_eq!(u.path, "almadar-behaviors/std-time-tracking");
    assert_eq!(u.config.len(), 1);
    assert_eq!(u.config[0].name, "appName");
    match &u.config[0].default {
        Some(SExpression::String(s)) => assert_eq!(s, "PF"),
        other => panic!("expected string default, got {other:?}"),
    }
    // Plain-wiring override form: `field_type` stays the "unknown" sentinel.
    assert!(matches!(&u.config[0].type_expr, TypeExpr::Primitive(s) if s == "unknown"));
}

#[test]
fn orbital_reference_parses_every_override_block() {
    let program = parse_str(SOURCE).expect("parse");
    assert_eq!(program.orbitals.len(), 1);
    let orb = &program.orbitals[0];

    assert_eq!(orb.name, "PfTimesheets");
    assert_eq!(
        orb.reference.as_deref(),
        Some("TimeTracking.orbitals.TimesheetPanelOrbital")
    );
    // An inline-only orbital field must stay empty on a reference form.
    assert!(orb.entity.is_none());
    assert!(orb.pages.is_empty());
    assert!(
        orb.config.is_empty(),
        "reference form declares no own knobs"
    );
    // `traits` carries the ONE sibling trait declared in the body (Stage B) —
    // never the upstream's own traits, which arrive at inline.
    assert_eq!(orb.traits.len(), 1);
    assert_eq!(orb.traits[0].name, "TimesheetApproval");

    assert_eq!(orb.entity_rebind.as_deref(), Some("Timesheet"));
    assert_eq!(
        strip_span2(&orb.entities_remap),
        vec![("Employee".to_string(), "Person".to_string())]
    );
    assert_eq!(
        strip_span2(&orb.fields_remap),
        vec![("periodStart".to_string(), "from".to_string())]
    );
    assert_eq!(
        strip_span2(&orb.roles_remap),
        vec![
            (
                "approver".to_string(),
                vec!["owner".to_string(), "project_manager".to_string()]
            ),
            ("employee".to_string(), vec!["team_member".to_string()]),
        ]
    );
    assert_eq!(
        strip_span2(&orb.pages_remap),
        vec![("/timesheets".to_string(), "/pf/timesheets".to_string())]
    );
    assert_eq!(
        strip_span2(&orb.mounts),
        vec![(
            "/timesheets".to_string(),
            vec!["TimesheetApproval".to_string()]
        )]
    );
    assert_eq!(strip_span1(&orb.omit), vec!["TimesheetDelete".to_string()]);
    assert!(orb.only.is_empty());
    assert_eq!(orb.config_overrides.len(), 1);
    assert_eq!(orb.config_overrides[0].name, "pageSize");
    match &orb.config_overrides[0].default {
        Some(SExpression::Number(n)) => assert_eq!(*n, 50.0),
        other => panic!("expected numeric default, got {other:?}"),
    }
    assert_eq!(
        orb.events_remap,
        vec![("TIMESHEET_APPROVED".to_string(), "PF_APPROVED".to_string())]
    );
}

#[test]
fn orbital_reference_lowers_to_the_placeholder_and_ref_object() {
    let schema = parse_and_lower_str(SOURCE).expect("lower");
    assert_eq!(schema.orbitals.len(), 1);
    let orb = &schema.orbitals[0];

    assert_eq!(orb.name, "PfTimesheets");
    assert_eq!(
        orb.traits.len(),
        1,
        "the one sibling trait lowered into traits, not the empty placeholder"
    );
    match &orb.traits[0] {
        TraitReference::Inline(t) => assert_eq!(t.name, "TimesheetApproval"),
        other => panic!("expected an inline-lowered sibling trait, got {other:?}"),
    }
    assert!(orb.pages.is_none());
    assert!(
        orb.config.is_none(),
        "reference form's OWN declared config stays empty — reference.config is the override"
    );
    match &orb.entity {
        EntityRef::Reference(s) => {
            assert_eq!(s, "TimeTracking.orbitals.TimesheetPanelOrbital.entity")
        }
        other => panic!("expected the placeholder EntityRef::Reference, got {other:?}"),
    }

    let ref_object = orb
        .reference
        .as_ref()
        .expect("reference-form orbital must lower a Some(OrbitalRefObject)");
    assert_eq!(
        ref_object.reference,
        "TimeTracking.orbitals.TimesheetPanelOrbital"
    );
    assert!(ref_object.ref_id.is_none());
    assert_eq!(ref_object.entity.as_deref(), Some("Timesheet"));
    assert_eq!(
        ref_object
            .fields
            .as_ref()
            .unwrap()
            .get("periodStart")
            .map(String::as_str),
        Some("from")
    );
    assert_eq!(
        ref_object
            .pages
            .as_ref()
            .unwrap()
            .get("/timesheets")
            .map(String::as_str),
        Some("/pf/timesheets")
    );
    assert_eq!(
        ref_object.omit.as_ref().unwrap(),
        &vec!["TimesheetDelete".to_string()]
    );
    assert!(ref_object.only.is_none());
    let cfg = ref_object.config.as_ref().expect("config override map");
    assert_eq!(cfg.len(), 1);
    let page_size: &SchemaConfigField = &cfg["pageSize"];
    match &page_size.default {
        Some(SExpression::Number(n)) => assert_eq!(*n, 50.0),
        other => panic!("expected numeric default, got {other:?}"),
    }
    assert_eq!(
        ref_object
            .events
            .as_ref()
            .unwrap()
            .get("TIMESHEET_APPROVED")
            .map(String::as_str),
        Some("PF_APPROVED")
    );
    let roles = ref_object.roles.as_ref().expect("roles map");
    assert_eq!(roles.len(), 2);
    assert_eq!(
        roles.get("approver"),
        Some(&vec!["owner".to_string(), "project_manager".to_string()])
    );
    assert_eq!(
        roles.get("employee"),
        Some(&vec!["team_member".to_string()])
    );
    assert_eq!(
        ref_object
            .entities
            .as_ref()
            .unwrap()
            .get("Employee")
            .map(String::as_str),
        Some("Person")
    );
    assert_eq!(
        ref_object.mounts.as_ref().unwrap().get("/timesheets"),
        Some(&vec!["TimesheetApproval".to_string()])
    );

    // The file-scoped `uses` merged onto the orbital, carrying its config
    // override.
    let uses = orb.uses.as_ref().expect("merged uses");
    assert_eq!(uses.len(), 1);
    assert_eq!(uses[0].alias, "TimeTracking");
    assert_eq!(uses[0].from, "almadar-behaviors/std-time-tracking");
    let app_name = &uses[0].config.as_ref().expect("uses config")["appName"];
    match &app_name.default {
        Some(SExpression::String(s)) => assert_eq!(s, "PF"),
        other => panic!("expected string default, got {other:?}"),
    }
}

#[test]
fn orbital_ref_roles_single_and_list_forms() {
    let source = r#"
uses TimeTracking from "almadar-behaviors/std-time-tracking"

orbital PfTimesheets = TimeTracking.orbitals.TimesheetPanelOrbital {
  roles {
    approver: [owner, project_manager]
    employee: team_member
  }
}
"#;
    let program = parse_str(source).expect("parse");
    let orb = &program.orbitals[0];
    assert_eq!(
        strip_span2(&orb.roles_remap),
        vec![
            (
                "approver".to_string(),
                vec!["owner".to_string(), "project_manager".to_string()]
            ),
            ("employee".to_string(), vec!["team_member".to_string()]),
        ]
    );

    let schema = parse_and_lower_str(source).expect("lower");
    let ref_object = schema.orbitals[0].reference.as_ref().expect("ref object");
    let roles = ref_object.roles.as_ref().expect("roles map");
    assert_eq!(
        roles.get("approver"),
        Some(&vec!["owner".to_string(), "project_manager".to_string()])
    );
    assert_eq!(
        roles.get("employee"),
        Some(&vec!["team_member".to_string()])
    );
}

#[test]
fn orbital_ref_pages_mount_tail_lowers_to_mounts() {
    let source = r#"
uses TimeTracking from "almadar-behaviors/std-time-tracking"

orbital PfTimesheets = TimeTracking.orbitals.TimesheetPanelOrbital {
  pages {
    "/timesheets": "/pf/timesheets" -> TimesheetApproval, TimesheetHistory
    "/reports": "/pf/reports"
  }
}
"#;
    let program = parse_str(source).expect("parse");
    let orb = &program.orbitals[0];
    assert_eq!(
        strip_span2(&orb.pages_remap),
        vec![
            ("/timesheets".to_string(), "/pf/timesheets".to_string()),
            ("/reports".to_string(), "/pf/reports".to_string()),
        ]
    );
    assert_eq!(
        strip_span2(&orb.mounts),
        vec![(
            "/timesheets".to_string(),
            vec![
                "TimesheetApproval".to_string(),
                "TimesheetHistory".to_string()
            ]
        )]
    );

    let schema = parse_and_lower_str(source).expect("lower");
    let ref_object = schema.orbitals[0].reference.as_ref().expect("ref object");
    assert_eq!(
        ref_object.mounts.as_ref().unwrap().get("/timesheets"),
        Some(&vec![
            "TimesheetApproval".to_string(),
            "TimesheetHistory".to_string()
        ])
    );
    // The unmounted page fires no entry in `mounts` at all.
    assert!(!ref_object.mounts.as_ref().unwrap().contains_key("/reports"));
}

#[test]
fn orbital_ref_roles_missing_value_is_a_parse_error() {
    let source = r#"
uses TimeTracking from "almadar-behaviors/std-time-tracking"

orbital PfTimesheets = TimeTracking.orbitals.TimesheetPanelOrbital {
  roles {
    approver:
  }
}
"#;
    let err = parse_str(source).expect_err("a value-less roles entry must be a parse error");
    let message = format!("{err}");
    assert!(
        message.to_lowercase().contains("identifier") || message.contains("expected"),
        "error should point at the missing role target, got: {message}"
    );
}

#[test]
fn app_level_config_block_parses_and_lowers() {
    let source = r#"
app AcmeOrg "1.0.0"
config {
  appName : string = "Acme"
    @label "Shell title"
  navItems : [string] = ["home", "settings"]
}

orbital Shell {
  entity Widget [runtime] {
    id : string!
  }
}
"#;
    let program = parse_str(source).expect("parse");
    assert_eq!(program.app.config.len(), 2);
    assert_eq!(program.app.config[0].name, "appName");
    assert_eq!(program.app.config[0].label.as_deref(), Some("Shell title"));

    let schema = parse_and_lower_str(source).expect("lower");
    let config = schema.config.expect("app-level config map");
    assert_eq!(config.len(), 2);
    match &config["appName"].default {
        Some(SExpression::String(s)) => assert_eq!(s, "Acme"),
        other => panic!("expected string default, got {other:?}"),
    }
    assert_eq!(config["appName"].label.as_deref(), Some("Shell title"));
}

#[test]
fn orbital_level_config_block_parses_and_lowers() {
    let source = r#"
orbital Shell {
  config {
    pageSize : int = 10
      @label "Items per page"
  }
  entity Widget [runtime] {
    id : string!
  }
}
"#;
    let program = parse_str(source).expect("parse");
    let orb = &program.orbitals[0];
    assert_eq!(orb.config.len(), 1);
    assert_eq!(orb.config[0].name, "pageSize");

    let schema = parse_and_lower_str(source).expect("lower");
    let orb_schema = &schema.orbitals[0];
    let config = orb_schema
        .config
        .as_ref()
        .expect("orbital-level config map");
    assert_eq!(config.len(), 1);
    match &config["pageSize"].default {
        Some(SExpression::Number(n)) => assert_eq!(*n, 10.0),
        other => panic!("expected numeric default, got {other:?}"),
    }
    assert_eq!(config["pageSize"].label.as_deref(), Some("Items per page"));
}

// ---------------------------------------------------------------------
// Stage C: `extend { … }` — full entity-field declarations appended to
// the primary entity, distinct from `fields {}` (rename-only).
// ---------------------------------------------------------------------

#[test]
fn orbital_ref_extend_parses_scalar_default_and_relation() {
    let source = r#"
uses TimeTracking from "almadar-behaviors/std-time-tracking"

orbital WorkspaceOrbital {
  entity Workspace [persistent] {
    id : string!
  }
}

orbital PfTimesheets = TimeTracking.orbitals.TimesheetPanelOrbital {
  extend {
    overtimeHours : number = 0
    workspaceId : Workspace!
  }
}
"#;
    let program = parse_str(source).expect("parse");
    let orb = program
        .orbitals
        .iter()
        .find(|o| o.reference.is_some())
        .expect("the reference-form orbital");
    assert_eq!(orb.extend_fields.len(), 2);
    assert_eq!(orb.extend_fields[0].name, "overtimeHours");
    assert_eq!(orb.extend_fields[1].name, "workspaceId");
    assert!(orb.extend_fields[1].required, "Workspace! is required");

    let schema = parse_and_lower_str(source).expect("lower");
    let orb_schema = schema
        .orbitals
        .iter()
        .find(|o| o.reference.is_some())
        .expect("the reference-form orbital, lowered");
    let ref_object = orb_schema.reference.as_ref().expect("ref object");
    let extend = ref_object.extend.as_ref().expect("extend fields lowered");
    assert_eq!(extend.len(), 2);
    assert_eq!(extend[0].name.as_deref(), Some("overtimeHours"));
    match &extend[0].default {
        Some(SExpression::Number(n)) => assert_eq!(*n, 0.0),
        other => panic!("expected numeric default, got {other:?}"),
    }
    assert_eq!(extend[1].name.as_deref(), Some("workspaceId"));
    // A relation-typed `extend {}` field resolves its target in the
    // CONSUMER scope (this file's own `Workspace` entity), never upstream.
    let relation = extend[1]
        .relation
        .as_ref()
        .expect("Workspace! lowers to a relation field");
    assert_eq!(relation.entity, "Workspace");
}

#[test]
fn orbital_ref_extend_round_trips_through_format() {
    // The parser accepts `fields {}` (rename-only) and `extend {}` (new
    // fields) side by side without confusing the two surfaces.
    let source = r#"
uses TimeTracking from "almadar-behaviors/std-time-tracking"

orbital PfTimesheets = TimeTracking.orbitals.TimesheetPanelOrbital {
  fields {
    periodStart: from
  }
  extend {
    overtimeHours : number = 0
  }
}
"#;
    let program = parse_str(source).expect("parse");
    let orb = &program.orbitals[0];
    assert_eq!(
        strip_span2(&orb.fields_remap),
        vec![("periodStart".to_string(), "from".to_string())]
    );
    assert_eq!(orb.extend_fields.len(), 1);
    assert_eq!(orb.extend_fields[0].name, "overtimeHours");
}
