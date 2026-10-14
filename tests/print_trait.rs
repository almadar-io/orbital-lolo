//! `print_trait`: one trait as orb prints it, with the lines each of its IR transitions occupies,
//! so a page can show the code of the transition a running machine just took.

use orbital_core::schema::TraitDefinition;
use orbital_lolo::unlower::print_trait;

fn door() -> TraitDefinition {
    serde_json::from_value(serde_json::json!({
        "name": "DoorLife",
        "linkedEntity": "Door",
        "stateMachine": {
            "states": [{ "name": "closed", "isInitial": true }, { "name": "opened" }],
            "events": [{ "key": "OPEN", "name": "Open" }, { "key": "CLOSE", "name": "Close" }, { "key": "INIT", "name": "Initialize" }],
            "transitions": [
                { "from": "closed", "to": "opened", "event": "OPEN", "guard": ["not", "@entity.open"],
                  "effects": [["set", "@entity.open", true], ["emit", "OPENED"]] },
                { "from": "opened", "to": "closed", "event": "CLOSE", "effects": [["set", "@entity.open", false]] },
                { "from": "closed", "to": "closed", "event": "INIT" }
            ]
        }
    }))
    .expect("trait")
}

fn lines(text: &str, start: usize, end: usize) -> Vec<&str> {
    text.lines().skip(start - 1).take(end - start + 1).collect()
}

#[test]
fn each_transition_maps_to_the_lines_that_print_it() {
    let printed = print_trait(&door()).expect("prints");
    assert!(
        printed.text.starts_with("trait DoorLife"),
        "{}",
        printed.text
    );
    let by_index = |i: usize| {
        printed
            .transitions
            .iter()
            .find(|t| t.index == i)
            .expect("every transition is located")
    };

    let open = by_index(0);
    let open_lines = lines(&printed.text, open.start, open.end);
    assert!(
        open_lines[0]
            .trim_start()
            .starts_with("OPEN -> opened when (not @entity.open)"),
        "{open_lines:?}"
    );
    assert_eq!(
        open_lines.len(),
        3,
        "the arm and its two effects: {open_lines:?}"
    );
    assert!(open_lines[2].contains("(emit OPENED)"));

    // IR order interleaves states; each index still finds its own lines.
    let init = by_index(2);
    assert_eq!(init.start, init.end);
    assert!(lines(&printed.text, init.start, init.end)[0].contains("INIT -> closed"));
    let close = by_index(1);
    assert!(lines(&printed.text, close.start, close.end)[0].contains("CLOSE -> closed"));
    assert!(
        close.start > init.start,
        "printed under its own state, after `closed`'s arms"
    );
    assert_eq!(printed.transitions.len(), 3);
}

#[test]
fn control_ranges_never_overlap() {
    let printed = print_trait(&door()).expect("prints");
    let mut spans: Vec<(usize, usize)> = printed
        .transitions
        .iter()
        .map(|t| (t.start, t.end))
        .collect();
    spans.sort();
    for w in spans.windows(2) {
        assert!(w[0].1 < w[1].0, "{spans:?}");
    }
}
