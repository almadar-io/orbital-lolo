//! End-to-end: lowering a `.lolo` source with `parse_and_lower_str_with_map`
//! populates the L2 source map with the exact line of a known transition
//! effect.

use orbital_lolo::parse_and_lower_str_with_map;

#[test]
fn transition_effect_span_is_tracked() {
    let src = "orbital O {\n  entity E [runtime] {\n    x : number = 0\n  }\n  trait T -> E {\n    state s {\n      EVT -> s\n        (set @entity.x 1)\n    }\n  }\n}\n";
    let (_schema, map) =
        parse_and_lower_str_with_map(src, "probe.lolo").expect("parse+lower must succeed");

    // `(set @entity.x 1)` is the sole effect of the sole transition of the
    // sole trait of the sole orbital — line 8 in the source above.
    let span = map
        .lookup("orbitals[0].traits[0].stateMachine.transitions[0].effects[0]")
        .expect("effect span must be tracked");
    assert_eq!(span.line, 8);

    // The name-annotated form L2 actually emits (`orbitals[0 (O)]…`) must
    // resolve identically — normalization strips the annotation.
    let annotated = map
        .lookup("orbitals[0 (O)].traits[0 (T)].stateMachine.transitions[0].effects[0]")
        .expect("annotated path must normalize to the same entry");
    assert_eq!(annotated.line, 8);
}

#[test]
fn guard_span_is_tracked() {
    let src = "orbital O {\n  entity E [runtime] {\n    x : number = 0\n  }\n  trait T -> E {\n    state s {\n      EVT -> s when (> @entity.x 0)\n        (set @entity.x 1)\n    }\n  }\n}\n";
    let (_schema, map) =
        parse_and_lower_str_with_map(src, "probe.lolo").expect("parse+lower must succeed");

    let span = map
        .lookup("orbitals[0].traits[0].stateMachine.transitions[0].guard")
        .expect("guard span must be tracked");
    assert_eq!(span.line, 7);
}

#[test]
fn deeper_pointer_falls_back_to_tracked_ancestor() {
    let src = "orbital O {\n  entity E [runtime] {\n    x : number = 0\n  }\n  trait T -> E {\n    state s {\n      EVT -> s\n        (set @entity.x 1)\n    }\n  }\n}\n";
    let (_schema, map) =
        parse_and_lower_str_with_map(src, "probe.lolo").expect("parse+lower must succeed");

    // A path one level deeper than anything tracked (as L2 would report for
    // a nested sub-expression inside the effect) falls back to the tracked
    // effect-level span rather than missing outright.
    let span = map
        .lookup("orbitals[0].traits[0].stateMachine.transitions[0].effects[0][1].extra")
        .expect("must fall back to the nearest tracked ancestor");
    assert_eq!(span.line, 8);
}

#[test]
fn orb_json_input_has_no_map_entries_worth_finding() {
    // Sanity: a schema built from nothing (empty map) never resolves — the
    // CLI layer is what wires "None map for .orb JSON" (see detect.rs); at
    // this layer we just confirm a fresh map is a clean miss.
    let map = orbital_lolo::LoloSourceMap::new();
    assert_eq!(map.lookup("orbitals[0].traits[0]"), None);
}
