//! L1 inference of the trait effect row (`TraitDefinition.effectRow`).
//!
//! The row answers "what does composing this trait bring along?" — the question
//! the riya open world had to answer by hand, splitting every level atom into an
//! emit-only data trait and a rendering play trait. Inferred, never authored, the
//! way `entityContract` is.

use orbital_core_base::effect_row::EffectSite;
use orbital_core_base::schema::{TraitDefinition, TraitReference};
use orbital_lolo::parse_and_lower_str;

fn lower_one(src: &str) -> TraitDefinition {
    let schema = parse_and_lower_str(src).expect("fixture must lower");
    match &schema.orbitals[0].traits[0] {
        TraitReference::Inline(td) => td.clone(),
        _ => panic!("expected an inline trait"),
    }
}

const RENDERING_TICK_TRAIT: &str = r#"app test-row "1.0.0"
"effect row inference"

orbital BoardOrbital {
  entity Board [runtime] {
    id     : string!
    active : boolean = false
    score  : number = 0
  }

  trait BoardPlay -> Board {
    initial: idle
    state idle {
      INIT -> idle
        (render-ui main { type: "box" })
      SCORE -> idle
        (set @entity.active true)
        (set @entity.score 1)
        (emit SCORED { value: 1 })
    }
    ticks {
      frame every "frame" when (== @entity.active true)
        (render-ui main { type: "box" })
    }
  }
}"#;

/// A rendering, ticking trait reports both, and the tick render is
/// distinguishable from the transition render — the distinction the whole
/// tick-contention check rests on.
#[test]
fn a_tick_render_is_recorded_at_the_tick_site() {
    let td = lower_one(RENDERING_TICK_TRAIT);
    let row = td.effect_row.as_ref().expect("a trait with effects has a row");

    let tick_slots: Vec<&str> = row.tick_rendered_slots().collect();
    assert_eq!(
        tick_slots,
        vec!["main"],
        "the ticks block repaints 'main' every frame; got {:?}",
        row.effects
    );
    assert!(row.has_ticks(), "the ticks block must be declared in the row");

    let transition_renders = row
        .effects
        .iter()
        .filter(|e| e.kind == "render-ui" && e.site == EffectSite::Transition)
        .count();
    assert_eq!(
        transition_renders, 1,
        "the INIT render is a transition render, not a tick render; got {:?}",
        row.effects
    );
}

/// Every effect kind lands in the row with its resource, not just the ones the
/// slot check cares about — the owner ruling was all kinds, resource-tagged.
#[test]
fn every_effect_kind_is_recorded_with_its_resource() {
    let td = lower_one(RENDERING_TICK_TRAIT);
    let row = td.effect_row.as_ref().expect("row present");

    let resources = |kind: &str| -> Vec<&str> {
        row.effects
            .iter()
            .filter(|e| e.kind == kind)
            .filter_map(|e| e.resolved_resource())
            .collect()
    };
    assert_eq!(
        resources("emit"),
        vec!["SCORED"],
        "an emit is addressed by its event, got {:?}",
        row.effects
    );
    assert_eq!(
        resources("set"),
        vec!["@entity.active", "@entity.score"],
        "a set is addressed by its binding path, and the row is sorted; got {:?}",
        row.effects
    );
}

/// The emit-only shape the riya `*LevelData` traits already use by hand: no
/// render, no tick, so composing 28 of them brings along nothing that collides.
#[test]
fn an_emit_only_trait_carries_no_render_and_no_tick() {
    let td = lower_one(
        r#"app test-row "1.0.0"
"emit-only data trait"

orbital LevelOrbital {
  entity Level [runtime] {
    id : string!
  }

  trait LevelData -> Level {
    initial: idle
    state idle {
      INIT -> idle
        (emit LEVEL_DATA { platforms: [] })
    }
  }
}"#,
    );
    let row = td.effect_row.as_ref().expect("row present");
    assert!(!row.has_ticks(), "no ticks block was declared");
    assert_eq!(
        row.tick_rendered_slots().count(),
        0,
        "an emit-only trait repaints nothing"
    );
    assert!(
        row.effects.iter().all(|e| e.kind != "render-ui"),
        "an emit-only trait renders nothing, got {:?}",
        row.effects
    );
}

/// L1 knows one file, so a `@config.*` slot has no identity yet. It is retained
/// as an unresolved binding for the compiler to substitute — never silently
/// treated as a literal, which would make it compare equal to a real slot name.
#[test]
fn a_config_bound_slot_stays_unresolved_at_l1() {
    let td = lower_one(
        r#"app test-row "1.0.0"
"config-bound slot"

orbital PanelOrbital {
  entity Panel [runtime] {
    id : string!
  }

  trait PanelRender -> Panel {
    initial: idle
    state idle {
      INIT -> idle
        (render-ui @config.surface { type: "box" })
    }
    config {
      surface : string = "main"
    }
  }
}"#,
    );
    let row = td.effect_row.as_ref().expect("row present");
    let render = row
        .effects
        .iter()
        .find(|e| e.kind == "render-ui")
        .expect("render recorded");
    assert!(
        !render.resolved,
        "L1 cannot see a call site's config, got {render:?}"
    );
    assert_eq!(render.resource.as_deref(), Some("@config.surface"));
    assert_eq!(
        render.resolved_resource(),
        None,
        "an unresolved slot must not compare equal to a literal slot"
    );
}

/// A trait that produces no effects at all carries no row, so traits without one
/// stay byte-identical in the emitted `.orb`.
#[test]
fn a_trait_with_no_effects_carries_no_row() {
    let td = lower_one(
        r#"app test-row "1.0.0"
"no effects"

orbital InertOrbital {
  entity Inert [runtime] {
    id : string!
  }

  trait InertFlow -> Inert {
    initial: idle
    state idle {
      GO -> done
    }
    state done {}
  }
}"#,
    );
    assert!(
        td.effect_row.is_none(),
        "no effects means no row, got {:?}",
        td.effect_row
    );
}
