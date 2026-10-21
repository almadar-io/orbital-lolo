//! `@<op> none "<reason>"` — a directive's omission, declared.
//!
//! Runtime behaviour is identical to leaving the directive out. What it buys is
//! the ability to tell a CONSIDERED omission from a forgotten one: without it a
//! lint must flag both, and a lint that flags correct code forever is one
//! authors learn to scroll past — costing the signal on the real gaps.
//!
//! The reason is mandatory, so the waiver can never become a quiet way to
//! silence a policy that IS expressible.

use orbital_core_base::schema::EntityRef;
use orbital_lolo::parse_and_lower_str;

fn entity_source(directives: &str) -> String {
    format!(
        r#"app fixture "1.0.0"
"Fixture."
orbital ReplyOrbital {{
  entity TicketReply [persistent: ticketreplies] {{
{directives}
    id : string!
    authorId : Person
  }}
}}
orbital PersonOrbital {{
  entity Person [persistent: people, identity] {{
    id : string!
    name : string!
    email : email
    role : "agent" | "supervisor" = agent
  }}
}}
"#
    )
}

/// The inline `TicketReply` definition, wherever it is declared.
fn ticket_reply(
    schema: &orbital_core_base::schema::OrbitalSchema,
) -> &orbital_core_base::schema::EntityDefinition {
    schema
        .orbitals
        .iter()
        .flat_map(|orb| std::iter::once(&orb.entity).chain(orb.auxiliary_entities.iter().flatten()))
        .find_map(|r| match r {
            EntityRef::Inline(def) if def.name == "TicketReply" => Some(def),
            _ => None,
        })
        .expect("TicketReply is declared")
}

#[test]
fn a_waiver_records_the_reason_and_leaves_the_policy_absent() {
    let schema = parse_and_lower_str(&entity_source(
        r#"    @read none "visibility follows the Ticket, which a per-row predicate cannot join""#,
    ))
    .expect("parses");
    let entity = ticket_reply(&schema);

    // Absent, exactly as if it had never been written — the runtime must not
    // start denying rows because the author explained themselves.
    assert!(entity.read_policy.is_none(), "a waiver is not a policy");
    let waivers = entity.access_waivers.as_ref().expect("waiver recorded");
    assert_eq!(
        waivers.read.as_deref(),
        Some("visibility follows the Ticket, which a per-row predicate cannot join")
    );
    assert!(waivers.create.is_none(), "only the waived op is recorded");
}

#[test]
fn a_waiver_without_a_reason_is_rejected() {
    let err = parse_and_lower_str(&entity_source("    @read none")).expect_err("must not parse");
    assert!(
        format!("{err}").contains("ELOLO_ACCESS_WAIVER_NEEDS_REASON"),
        "expected the missing-reason error, got: {err}"
    );
}

#[test]
fn an_empty_reason_is_rejected() {
    let err =
        parse_and_lower_str(&entity_source(r#"    @read none """#)).expect_err("must not parse");
    assert!(
        format!("{err}").contains("ELOLO_ACCESS_WAIVER_NEEDS_REASON"),
        "expected the missing-reason error, got: {err}"
    );
}

#[test]
fn a_waiver_and_a_real_policy_for_the_same_op_collide() {
    // Declaring both is a contradiction: either the rule is expressible or it
    // is not. Same error the duplicate-directive check already raises.
    let err = parse_and_lower_str(&entity_source(
        "    @read none \"cannot join\"\n    @read (= (object/get @entity authorId) @user.id)",
    ))
    .expect_err("must not parse");
    assert!(
        format!("{err}").contains("ELOLO_DUPLICATE_ACCESS_DIRECTIVE"),
        "expected the duplicate-directive error, got: {err}"
    );
}

#[test]
fn waiving_one_operation_leaves_the_others_enforced() {
    let schema = parse_and_lower_str(&entity_source(
        "    @read none \"cannot join across entities\"\n    @delete (= @user.role \"supervisor\")",
    ))
    .expect("parses");
    let entity = ticket_reply(&schema);
    assert!(entity.read_policy.is_none());
    assert!(
        entity.delete_policy.is_some(),
        "a waived @read must not disturb a declared @delete"
    );
}
