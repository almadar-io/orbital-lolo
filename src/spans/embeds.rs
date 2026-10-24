//! Where each inline embed sits in its `.lolo`: the lowering names an inline
//! behavior embed (`<Button.traits.ButtonRender …/>`) `Inline<Short><N>`, a name
//! that appears only in the `.orb`. This maps every such trait back to the
//! embed's opening `<` so an editor can change the embed's attributes.

use serde::Serialize;

use crate::error::ParseError;
use crate::parser::parse;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InlineEmbed {
    pub orbital: String,
    /// The trait name the lowered `.orb` carries.
    #[serde(rename = "trait")]
    pub trait_name: String,
    /// The embedded behavior's trait path, e.g. `Button.traits.ButtonRender`.
    pub behavior: String,
    /// 1-based line and column (in characters) of the embed's first token.
    pub line: u32,
    pub col: u32,
}

pub fn inline_embeds(source: &str) -> Result<Vec<InlineEmbed>, ParseError> {
    let program = parse(source)?;
    Ok(program
        .orbitals
        .iter()
        .flat_map(|orbital| {
            orbital
                .traits
                .iter()
                .filter(|t| t.is_inline_synthetic)
                .filter_map(move |t| {
                    t.reference.as_ref().map(|r| InlineEmbed {
                        orbital: orbital.name.clone(),
                        trait_name: t.name.clone(),
                        behavior: r.reference.clone(),
                        line: t.span.line,
                        col: t.span.col,
                    })
                })
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = r#"app a "1.0.0"
"Á block — with non-ASCII before a tag."
orbital NoteOrbital {
  uses Button from "std/behaviors/std-button"
  entity Note [runtime] {
    id : string!
  }

  trait NoteView -> Note {
    initial: idle
    state idle {
      INIT -> idle
        (render-ui main <Stack direction={vertical}>
          <Box title={"«»"}><Button.traits.ButtonRender label={"New Note"} /></Box>
          <Button.traits.ButtonRender label={"Help"} />
        </Stack>)
    }
  }
}

orbital TagOrbital {
  uses Button from "std/behaviors/std-button"
  entity Tag [runtime] {
    id : string!
  }

  trait TagView -> Tag {
    initial: idle
    state idle {
      INIT -> idle
        (render-ui main <Button.traits.ButtonRender label={"Tags"} />)
    }
  }
}
"#;

    fn char_at(source: &str, line: u32, col: u32) -> Option<char> {
        source
            .lines()
            .nth(line as usize - 1)?
            .chars()
            .nth(col as usize - 1)
    }

    #[test]
    fn every_embed_is_named_as_the_orb_names_it_and_points_at_its_tag() {
        let embeds = inline_embeds(SRC).expect("parses");
        assert_eq!(embeds.len(), 3);
        let lowered = crate::parse_and_lower_str(SRC).expect("lowers");
        let orb_traits: Vec<String> = lowered
            .orbitals
            .iter()
            .flat_map(|o| {
                o.traits.iter().filter_map(|t| match t {
                    orbital_core_base::schema::types::TraitReference::Inline(d) => Some(d.name.clone()),
                    orbital_core_base::schema::types::TraitReference::Reference { name, .. } => name.clone(),
                })
            })
            .collect();
        for e in &embeds {
            assert!(
                orb_traits.contains(&e.trait_name),
                "{} not in {orb_traits:?}",
                e.trait_name
            );
            assert_eq!(e.behavior, "Button.traits.ButtonRender");
            assert_eq!(char_at(SRC, e.line, e.col), Some('<'), "{e:?}");
        }
    }

    #[test]
    fn edge_the_counter_runs_across_orbitals_and_columns_count_characters() {
        let embeds = inline_embeds(SRC).expect("parses");
        assert_eq!(
            embeds
                .iter()
                .map(|e| e.orbital.as_str())
                .collect::<Vec<_>>(),
            ["NoteOrbital", "NoteOrbital", "TagOrbital"]
        );
        let names: std::collections::BTreeSet<&str> =
            embeds.iter().map(|e| e.trait_name.as_str()).collect();
        assert_eq!(names.len(), 3, "names are unique across the file");
        let first = &embeds[0];
        assert_eq!(
            (first.line, first.col),
            (14, 29),
            "«» before the tag are two characters, four bytes"
        );
    }

    #[test]
    fn control_a_named_trait_and_a_trait_reference_are_not_embeds() {
        let src = "app a \"1.0.0\"\norbital O {\n  uses Button from \"std/behaviors/std-button\"\n  entity E [runtime] {\n    id : string!\n  }\n  trait B = Button.traits.ButtonRender -> E {\n    config {\n      label: \"Go\"\n    }\n  }\n}\n";
        assert_eq!(inline_embeds(src).expect("parses"), vec![]);
    }
}
