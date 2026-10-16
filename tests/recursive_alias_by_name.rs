//! A self-referential alias (`type Node = { children : [Node] }`) cannot be expanded forever, so the lowerer
//! leaves the back-reference as a by-name reference (`FieldDefinition::by_name_reference`): a typed reference
//! to the alias, never a shapeless carrier. The std draw-item vocabulary (`DrawGroupItem.items : [DrawItem]`
//! where `DrawItem` includes `DrawGroupItem`) is the case this pins.

use orbital_core_base::schema::FieldDefinition;
use orbital_lolo::parse_and_lower_str;
use serde_json::Value;

fn program(types: &str, config: &str) -> String {
    let knob = config.split(':').next().unwrap().trim();
    format!(
        r#"
{types}

orbital Tree {{
  entity Doc [runtime] {{
    id : string!
  }}

  trait Walker -> Doc [interaction] {{
    state idle {{
      INIT -> idle
        (set @entity.id (json/stringify @config.{knob}))
    }}
    config {{ {config} }}
  }}
}}
"#
    )
}

fn config_field(source: &str, knob: &str) -> Value {
    let schema = parse_and_lower_str(source).unwrap_or_else(|e| panic!("{e}"));
    let json = serde_json::to_value(&schema).unwrap();
    json.pointer(&format!("/orbitals/0/traits/0/config/{knob}"))
        .unwrap_or_else(|| panic!("no config knob {knob} in {json}"))
        .clone()
}

fn field_at(value: &Value, pointer: &str) -> FieldDefinition {
    let found = value
        .pointer(pointer)
        .unwrap_or_else(|| panic!("no {pointer} in {value}"));
    serde_json::from_value(found.clone()).unwrap_or_else(|e| panic!("{pointer}: {e}"))
}

#[test]
fn a_self_referential_alias_leaves_a_by_name_reference_for_the_back_edge() {
    let source = program(
        "type Node = {\n  label : string!\n  children : [Node]\n}",
        "tree : Node",
    );
    let tree = config_field(&source, "tree");
    let back = field_at(&tree, "/properties/children/items");
    assert_eq!(back.by_name_reference(), Some("Node"), "{tree}");
    let label = field_at(&tree, "/properties/label");
    assert_eq!(label.by_name_reference(), None, "control: an ordinary field names no type");
}

#[test]
fn a_mutually_recursive_pair_names_the_type_it_would_expand() {
    let source = program(
        "type Leaf = {\n  v : number!\n}\ntype Branch = {\n  kids : [Item]\n}\ntype Item = Leaf | Branch",
        "root : Item",
    );
    let root = config_field(&source, "root");
    let kids = field_at(&root, "/properties/Branch/properties/kids/items");
    assert_eq!(kids.by_name_reference(), Some("Item"), "{root}");
}

#[test]
fn a_non_recursive_alias_is_expanded_in_place() {
    let source = program("type Point = {\n  x : number!\n  y : number!\n}", "p : Point");
    let p = config_field(&source, "p");
    let x = field_at(&p, "/properties/x");
    assert_eq!(x.by_name_reference(), None);
    assert!(p.pointer("/properties/y").is_some(), "{p}");
}

#[test]
fn a_map_or_list_alias_is_recorded_as_one_unnamed_entry_for_json_parse() {
    let source = program(
        "type Row = Map string string\ntype Rows = [Row]\ntype Point = {\n  x : number!\n}",
        "p : Point",
    );
    let schema = parse_and_lower_str(&source).unwrap();
    let types = serde_json::to_value(&schema).unwrap();
    let types = types.pointer("/orbitals/0/types").expect("types").clone();
    assert_eq!(types["Row"][0]["name"], "");
    assert_eq!(types["Row"][0]["type"], "Map<string,string>");
    assert_eq!(types["Rows"][0]["type"], "[Row]");
    assert_eq!(types["Point"][0]["name"], "x", "control: a struct alias keeps its fields");
}
