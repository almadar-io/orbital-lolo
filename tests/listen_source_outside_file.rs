//! A single orbital lowered on its own (rabit materializes one orbital of an organism for an
//! in-place edit) may listen to a sibling orbital the file does not contain. L1 cannot see that
//! sibling's emits, so — like the compiler's post-inline `validate_listens_integrity` — it defers;
//! the composed schema's validation is the gate. A source orbital the file DOES contain is still
//! checked here.

use orbital_lolo::parse_and_lower_str;

const LISTENER: &str = r#"
orbital CartItemOrbital {
  entity CartItem [runtime] {
    id : string!
    count : number
  }
  trait CartItemAddToCart -> CartItem [interaction] {
    state idle {
      ADD_TO_CART -> idle
    }
    listens {
      ProductOrbital.ProductBrowse.ADD_TO_CART -> ADD_TO_CART
    }
  }
}
"#;

fn product(emits_add_to_cart: bool) -> String {
    let emits = if emits_add_to_cart {
        "    emits {\n      ADD_TO_CART -> external { id: string }\n    }\n"
    } else {
        ""
    };
    format!(
        r#"
orbital ProductOrbital {{
  entity Product [runtime] {{
    id : string!
  }}
  trait ProductBrowse -> Product [interaction] {{
    state idle {{
      PICK -> idle
        (emit ADD_TO_CART {{ id: @id }})
    }}
{emits}  }}
}}
"#
    )
}

#[test]
fn a_listen_on_a_sibling_orbital_outside_the_file_is_deferred() {
    assert!(parse_and_lower_str(LISTENER).is_ok());
}

#[test]
fn control_a_source_orbital_in_the_file_that_declares_the_event_resolves() {
    let source = format!("{}{}", product(true), LISTENER);
    assert!(parse_and_lower_str(&source).is_ok());
}

#[test]
fn control_a_source_orbital_in_the_file_that_lacks_the_event_is_rejected() {
    let source = format!("{}{}", product(false), LISTENER);
    let err =
        parse_and_lower_str(&source).expect_err("undeclared source event in-file must be rejected");
    assert!(
        err.to_string().contains("not declared on the source trait"),
        "{err}"
    );
}
