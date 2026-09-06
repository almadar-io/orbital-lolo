//! Orbital-import Stage A — `.lolo` surface tests for the orbital
//! REFERENCE form (`orbital Name = Alias.orbitals.Y { ... }`), file-scoped
//! `uses`, and the two `config {}` declaration sites (app-level, orbital-
//! level). See `docs/Almadar_Orbital_Import.md` §4 and the W2-R work item.

use orbital_core::schema::{ConfigField as SchemaConfigField, EntityRef, SExpression};
use orbital_lolo::ast::TypeExpr;
use orbital_lolo::{parse_and_lower_str, parse_str};

const SOURCE: &str = r#"
uses TimeTracking from "almadar-behaviors/std-time-tracking" {
  config {
    appName: "PF"
  }
}

orbital PfTimesheets = TimeTracking.orbitals.TimesheetPanelOrbital {
  entity Timesheet
  fields {
    periodStart: from
  }
  pages {
    "/timesheets": "/pf/timesheets"
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
    assert!(orb.traits.is_empty());
    assert!(orb.pages.is_empty());
    assert!(orb.config.is_empty(), "reference form declares no own knobs");

    assert_eq!(orb.entity_rebind.as_deref(), Some("Timesheet"));
    assert_eq!(
        orb.fields_remap,
        vec![("periodStart".to_string(), "from".to_string())]
    );
    assert_eq!(
        orb.pages_remap,
        vec![("/timesheets".to_string(), "/pf/timesheets".to_string())]
    );
    assert_eq!(orb.omit, vec!["TimesheetDelete".to_string()]);
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
    assert!(orb.traits.is_empty(), "reference form ships no traits pre-inline");
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
    assert_eq!(ref_object.reference, "TimeTracking.orbitals.TimesheetPanelOrbital");
    assert!(ref_object.ref_id.is_none());
    assert_eq!(ref_object.entity.as_deref(), Some("Timesheet"));
    assert_eq!(
        ref_object.fields.as_ref().unwrap().get("periodStart").map(String::as_str),
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
    assert_eq!(ref_object.omit.as_ref().unwrap(), &vec!["TimesheetDelete".to_string()]);
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
fn orbital_ref_rejects_roles() {
    let source = r#"
uses TimeTracking from "almadar-behaviors/std-time-tracking"

orbital PfTimesheets = TimeTracking.orbitals.TimesheetPanelOrbital {
  roles {
    approver: project_manager
  }
}
"#;
    let err = parse_str(source).expect_err("roles {} must be a parse error");
    let message = format!("{err}");
    assert!(
        message.contains("Stage B"),
        "error should name Stage B, got: {message}"
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
    let config = orb_schema.config.as_ref().expect("orbital-level config map");
    assert_eq!(config.len(), 1);
    match &config["pageSize"].default {
        Some(SExpression::Number(n)) => assert_eq!(*n, 10.0),
        other => panic!("expected numeric default, got {other:?}"),
    }
    assert_eq!(config["pageSize"].label.as_deref(), Some("Items per page"));
}
