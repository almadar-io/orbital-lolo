//! JSX surface parsing for render element trees.
//!
//! JSX is **pure sugar**: every element desugars to one of the three
//! pre-existing render S-expression forms — an anonymous pattern object
//! (`{ type: … }`), a named trait reference (`@trait.X`), or an inline
//! trait-reference sugar (hoisted synthetic `trait` + `@trait.Inline…`).
//! On exit from [`Parser::parse_jsx_element`] the result is a plain
//! [`SExpression`], so the lowerer, validator, codegen, and runtime are
//! byte-identical to the hand-written object-literal form.
//!
//! Tag shapes (all Capitalized, per React convention — the disambiguator is
//! the tag's *shape*, not its case):
//!
//! | tag | desugars to |
//! |-----|-------------|
//! | `<Name …/>` (single ident) | `{ type: <kebab(Name)>, …props, children: […] }` |
//! | `<trait.X />` | `"@trait.X"` (named reference; props forbidden) |
//! | `<Alias.traits.Name …>` | `@(Alias.traits.Name …)` hoisted synthetic trait |
//!
//! The `<` token is the comparison operator `Identifier("<")`; in *value*
//! position a bare `<` can only start a JSX element (comparison `<` only
//! appears as the head of a parenthesized s-expression, which is consumed by
//! the operator-position arm of `parse_sexpr`).

use super::*;
use crate::ast::{ConfigField, Listen, Trait, TraitRefData, TypeExpr};
use orbital_core::schema::SExpression;
use std::collections::HashMap;

/// The three JSX tag kinds, distinguished by tag shape after the first
/// identifier segment is read.
enum JsxTag {
    /// `<Name …>` — anonymous pattern object; `type` is `kebab(Name)`.
    Pattern(String),
    /// `<trait.X />` — named trait reference; carries no props.
    NamedRef(String),
    /// `<Alias.traits.Name …>` — inline trait-reference sugar.
    InlineRef(String),
}

impl Parser {
    /// Parse a single JSX element starting at the current `<` token.
    /// Returns the desugared [`SExpression`] (object / `@trait.X` string).
    pub(crate) fn parse_jsx_element(&mut self) -> Result<SExpression, ParseError> {
        let span = self.peek_span();
        // consume `<`
        self.advance();

        let first = self.expect_ident()?;
        let dotted = matches!(self.peek(), TokenKind::Dot);

        let (tag, open_text) = if first == "trait" {
            if !dotted {
                return Err(ParseError::InvalidSExpr {
                    message: "JSX `<trait.…>` requires a dotted member name, e.g. `<trait.FileTextIcon />`".to_string(),
                    span,
                });
            }
            let path = self.parse_dotted_tail(&first)?;
            // path is `trait.<member>`
            let member = path.strip_prefix("trait.").unwrap_or(&path).to_string();
            (
                JsxTag::NamedRef(member.clone()),
                format!("trait.{}", member),
            )
        } else if dotted {
            let reference = self.parse_dotted_tail(&first)?;
            (JsxTag::InlineRef(reference.clone()), reference)
        } else {
            (JsxTag::Pattern(first.clone()), first)
        };

        let attrs = self.parse_jsx_attrs()?;
        let self_closing = self.consume_jsx_opening_end(span)?;

        match tag {
            JsxTag::NamedRef(member) => {
                if !attrs.is_empty() {
                    return Err(ParseError::InvalidSExpr {
                        message: format!(
                            "`<trait.{member}>` is a named reference and takes no props; \
                             author overrides on the `trait {member} = …` declaration"
                        ),
                        span,
                    });
                }
                if !self_closing {
                    return Err(ParseError::InvalidSExpr {
                        message: format!(
                            "`<trait.{member}>` must be self-closing: `<trait.{member} />`"
                        ),
                        span,
                    });
                }
                Ok(SExpression::String(format!("@trait.{}", member)))
            }
            JsxTag::Pattern(name) => {
                let children = if self_closing {
                    Vec::new()
                } else {
                    let ch = self.parse_jsx_children()?;
                    self.consume_jsx_closing_tag(&open_text, span)?;
                    ch
                };
                Ok(desugar_pattern(&name, attrs, children))
            }
            JsxTag::InlineRef(reference) => {
                let children = if self_closing {
                    Vec::new()
                } else {
                    let ch = self.parse_jsx_children()?;
                    self.consume_jsx_closing_tag(&open_text, span)?;
                    ch
                };
                self.desugar_inline_ref(&reference, attrs, children, span)
            }
        }
    }

    /// Read `name` / `name=value` / `name="str"` attribute pairs until the
    /// opening tag's closing `>` or `/>`. Does not consume the closing token.
    fn parse_jsx_attrs(&mut self) -> Result<Vec<(String, SExpression)>, ParseError> {
        let mut attrs = Vec::new();
        loop {
            self.skip_newlines();
            match self.peek().clone() {
                TokenKind::Identifier(s) if s == ">" || s == "/" => break,
                TokenKind::Identifier(name) => {
                    self.advance();
                    let value = if matches!(self.peek(), TokenKind::Equals) {
                        self.advance();
                        self.parse_jsx_attr_value()?
                    } else {
                        // Boolean shorthand: bare `disabled` ⇒ `true`.
                        SExpression::Bool(true)
                    };
                    attrs.push((name, value));
                }
                other => {
                    return Err(ParseError::InvalidSExpr {
                        message: format!(
                            "expected a JSX attribute name, `>`, or `/>`, got {other:?}"
                        ),
                        span: self.peek_span(),
                    })
                }
            }
        }
        Ok(attrs)
    }

    /// An attribute value: a quoted string (`label="Back"`) or an expression
    /// container (`action={PAY_BILL}`).
    fn parse_jsx_attr_value(&mut self) -> Result<SExpression, ParseError> {
        match self.peek().clone() {
            TokenKind::String(s) => {
                self.advance();
                Ok(SExpression::String(s))
            }
            TokenKind::LBrace => {
                self.advance();
                self.skip_newlines();
                let v = self.parse_sexpr_or_value()?;
                self.skip_newlines();
                self.expect_kind(&TokenKind::RBrace, "}")?;
                Ok(v)
            }
            other => Err(ParseError::InvalidSExpr {
                message: format!(
                    "JSX attribute value must be a quoted string or `{{expr}}`, got {other:?}"
                ),
                span: self.peek_span(),
            }),
        }
    }

    /// Consume the `>` (open) or `/>` (self-closing) that ends an opening
    /// tag. Returns `true` when self-closing.
    fn consume_jsx_opening_end(&mut self, span: Span) -> Result<bool, ParseError> {
        self.skip_newlines();
        match self.peek().clone() {
            TokenKind::Identifier(s) if s == "/" => {
                self.advance();
                if !matches!(self.peek(), TokenKind::Identifier(t) if t == ">") {
                    return Err(ParseError::InvalidSExpr {
                        message: "expected `>` after `/` in a self-closing JSX tag".to_string(),
                        span: self.peek_span(),
                    });
                }
                self.advance();
                Ok(true)
            }
            TokenKind::Identifier(s) if s == ">" => {
                self.advance();
                Ok(false)
            }
            other => Err(ParseError::InvalidSExpr {
                message: format!(
                    "expected `>` or `/>` to close the JSX opening tag, got {other:?}"
                ),
                span,
            }),
        }
    }

    /// Parse child elements / expression containers until the matching closing
    /// tag `</…>`. Raw text children are rejected in v1 — the idiomatic form
    /// carries text on a prop (`content=`, `label=`) or via `{"text"}`.
    fn parse_jsx_children(&mut self) -> Result<Vec<SExpression>, ParseError> {
        let mut children = Vec::new();
        loop {
            self.skip_newlines();
            match self.peek().clone() {
                // Closing tag start: `<` `/`.
                TokenKind::Identifier(s) if s == "<" => {
                    if matches!(self.peek_n(1), TokenKind::Identifier(t) if t == "/") {
                        break;
                    }
                    let child = self.parse_jsx_element()?;
                    children.push(child);
                }
                TokenKind::LBrace => {
                    self.advance();
                    self.skip_newlines();
                    let v = self.parse_sexpr_or_value()?;
                    self.skip_newlines();
                    self.expect_kind(&TokenKind::RBrace, "}")?;
                    children.push(v);
                }
                // An inline `@(...)` trait reference is itself a render
                // element, so it's a valid JSX child (the formatter emits it
                // for inline refs that carry a `listens` override, which JSX
                // can't express).
                TokenKind::AtParen => {
                    children.push(self.parse_inline_trait_ref()?);
                }
                other => {
                    return Err(ParseError::InvalidSExpr {
                        message: format!(
                            "JSX children must be elements `<…>` or `{{expr}}`; raw text is not \
                             supported (use a `content=`/`label=` prop or `{{\"text\"}}`). Got {other:?}"
                        ),
                        span: self.peek_span(),
                    })
                }
            }
        }
        Ok(children)
    }

    /// Consume `</open_text>` and verify the name matches the opening tag.
    fn consume_jsx_closing_tag(&mut self, expected: &str, span: Span) -> Result<(), ParseError> {
        if !matches!(self.peek(), TokenKind::Identifier(s) if s == "<") {
            return Err(ParseError::InvalidSExpr {
                message: format!("expected JSX closing tag `</{expected}>`"),
                span,
            });
        }
        self.advance();
        if !matches!(self.peek(), TokenKind::Identifier(s) if s == "/") {
            return Err(ParseError::InvalidSExpr {
                message: "expected `</` to start the JSX closing tag".to_string(),
                span: self.peek_span(),
            });
        }
        self.advance();
        let first = self.expect_ident()?;
        let tag = self.parse_dotted_tail(&first)?;
        if !matches!(self.peek(), TokenKind::Identifier(s) if s == ">") {
            return Err(ParseError::InvalidSExpr {
                message: "expected `>` to end the JSX closing tag".to_string(),
                span: self.peek_span(),
            });
        }
        self.advance();
        if tag != expected {
            return Err(ParseError::InvalidSExpr {
                message: format!(
                    "JSX closing tag `</{tag}>` does not match opening `<{expected}>`"
                ),
                span,
            });
        }
        Ok(())
    }

    /// Read a dotted identifier tail (`.seg.seg…`) starting after `first` has
    /// already been consumed. Returns `first` + appended `.seg` segments.
    fn parse_dotted_tail(&mut self, first: &str) -> Result<String, ParseError> {
        let mut buf = first.to_string();
        while matches!(self.peek(), TokenKind::Dot) {
            self.advance();
            let seg = self.expect_ident()?;
            buf.push('.');
            buf.push_str(&seg);
        }
        Ok(buf)
    }

    /// Desugar an inline-trait JSX element `<Alias.traits.Name …>` into a
    /// hoisted synthetic `trait` declaration + an `@trait.Inline…` string,
    /// reusing the exact same mechanism as the `@(...)` sugar so the inline
    /// phase, validator, and formatter treat it identically.
    fn desugar_inline_ref(
        &mut self,
        reference: &str,
        attrs: Vec<(String, SExpression)>,
        children: Vec<SExpression>,
        span: Span,
    ) -> Result<SExpression, ParseError> {
        let mut linked_entity: Option<String> = None;
        let mut listens: Vec<Listen> = Vec::new();
        let mut config: Vec<ConfigField> = Vec::new();

        for (key, value) in attrs {
            match key.as_str() {
                "linkedEntity" => {
                    linked_entity = match &value {
                        SExpression::String(s) => Some(s.clone()),
                        other => {
                            return Err(ParseError::InvalidSExpr {
                                message: format!(
                                    "JSX `linkedEntity=` must be an entity name (e.g. `linkedEntity=\"ApBill\"` or `linkedEntity={{ApBill}}`), got {other:?}"
                                ),
                                span,
                            });
                        }
                    };
                }
                "listens" => {
                    listens = listens_from_object(&value, span)?;
                }
                _ => config.push(ConfigField {
                    name: key,
                    type_expr: TypeExpr::Primitive("unknown".to_string()),
                    default: Some(value),
                    label: None,
                    description: None,
                    synonyms: None,
                    tier: None,
                    required: false,
                    span,
                }),
            }
        }

        if !children.is_empty() {
            config.push(ConfigField {
                name: "children".to_string(),
                type_expr: TypeExpr::Primitive("unknown".to_string()),
                default: Some(SExpression::Array(children)),
                label: None,
                description: None,
                synonyms: None,
                tier: None,
                required: false,
                span,
            });
        }

        Ok(self.hoist_inline_trait(reference.to_string(), linked_entity, config, listens, span))
    }

    /// Hoist a synthetic inline `trait` declaration (shared by the `@(…)` and
    /// JSX inline-ref sugar paths) and return the `@trait.<Name>` reference
    /// string that stands in for it in the render tree.
    fn hoist_inline_trait(
        &mut self,
        reference: String,
        linked_entity: Option<String>,
        mut config: Vec<ConfigField>,
        listens: Vec<Listen>,
        span: Span,
    ) -> SExpression {
        // Call-site payload capture: a `@payload.<field>` in an inline-ref
        // config value is, by position, a reference to the COMPOSING effect's
        // event payload — tag it `@callsitePayload.<field>` so it is resolved
        // at the call site and never in the child's own INIT scope.
        for field in config.iter_mut() {
            if let Some(default) = field.default.as_mut() {
                orbital_core::schema::capture_callsite_payload(default);
            }
        }
        let short = reference
            .rsplit('.')
            .next()
            .filter(|s| !s.is_empty())
            .unwrap_or("Ref");
        let short = {
            let mut c = short.chars();
            match c.next() {
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                None => String::from("Ref"),
            }
        };
        let name = loop {
            self.inline_ref_counter += 1;
            let candidate = format!("Inline{}{}", short, self.inline_ref_counter);
            let collides = self
                .pending_inline_traits
                .iter()
                .any(|t| t.name == candidate);
            if !collides {
                break candidate;
            }
        };

        self.pending_inline_traits.push(Trait {
            name: name.clone(),
            type_params: Vec::new(),
            type_annotation: None,
            linked_entity,
            entity_rebindable: false,
            entity_binding_description: None,
            entity_binding_synonyms: None,
            category: None,
            scope: None,
            capabilities: Vec::new(),
            initial_state: None,
            states: Vec::new(),
            emits: Vec::new(),
            listens,
            ticks: Vec::new(),
            config: Vec::new(),
            span,
            reference: Some(TraitRefData {
                name_override: None,
                reference,
                events: Vec::new(),
                fields: Vec::new(),
                config,
                emits_scope: None,
            }),
            is_inline_synthetic: true,
        });

        SExpression::String(format!("@trait.{}", name))
    }
}

/// Build the anonymous pattern object `{ type: <kebab(name) or type= override>, …props, children: […] }`.
fn desugar_pattern(
    name: &str,
    attrs: Vec<(String, SExpression)>,
    children: Vec<SExpression>,
) -> SExpression {
    let mut map: HashMap<String, SExpression> = HashMap::new();
    // `type` — explicit `type=` override wins; otherwise kebab-normalize the
    // PascalCase tag name. A non-string `type=` is passed through verbatim.
    let type_value = attrs
        .iter()
        .find(|(k, _)| k == "type")
        .map(|(_, v)| v.clone())
        .unwrap_or_else(|| SExpression::String(kebab_case(name)));
    map.insert("type".to_string(), type_value);
    for (key, value) in attrs {
        if key == "type" {
            continue;
        }
        map.insert(key, value);
    }
    if !children.is_empty() {
        map.insert("children".to_string(), SExpression::Array(children));
    }
    SExpression::Object(map)
}

/// Convert a `listens={…}` attribute value (an object literal `{ KEY: TRIGGER }`)
/// into [`Listen`] entries in the local bare-event shape.
fn listens_from_object(value: &SExpression, span: Span) -> Result<Vec<Listen>, ParseError> {
    let map = match value {
        SExpression::Object(m) => m,
        other => {
            return Err(ParseError::InvalidSExpr {
                message: format!(
                    "JSX `listens=` must be an object `{{ EVENT: TRIGGER }}`, got {other:?}"
                ),
                span,
            })
        }
    };
    // `SExpression::Object` is HashMap-backed, so iteration order is
    // nondeterministic per process — sort by event name so re-emits are
    // byte-stable (the authored attribute order is already lost at parse).
    let mut entries: Vec<(&String, &SExpression)> = map.iter().collect();
    entries.sort_by(|a, b| a.0.cmp(b.0));
    let mut out = Vec::new();
    for (event, trigger) in entries {
        let triggers = match trigger {
            SExpression::String(s) => s.clone(),
            other => {
                return Err(ParseError::InvalidSExpr {
                    message: format!(
                        "JSX `listens` trigger for `{event}` must be an event name, got {other:?}"
                    ),
                    span,
                })
            }
        };
        out.push(Listen {
            event: event.clone(),
            triggers: Some(triggers),
            guard: None,
            payload_mapping: Vec::new(),
            payload: Vec::new(),
            description: None,
            synonyms: None,
            tier: None,
            span,
        });
    }
    Ok(out)
}

/// PascalCase / MixedCase → kebab-case (idempotent): insert `-` before each
/// interior uppercase letter, then lowercase. `Stack` → `stack`,
/// `DataGrid` → `data-grid`, `UiSpinner` → `ui-spinner`.
fn kebab_case(name: &str) -> String {
    let mut out = String::new();
    for (i, c) in name.chars().enumerate() {
        if c.is_ascii_uppercase() {
            if i > 0 && !out.ends_with('-') {
                out.push('-');
            }
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_value(src: &str) -> SExpression {
        let mut p = Parser::new(crate::lexer::tokenize(src).unwrap());
        p.skip_newlines();
        p.parse_sexpr_or_value().expect("parse value")
    }

    #[test]
    fn kebab_basic() {
        assert_eq!(kebab_case("Stack"), "stack");
        assert_eq!(kebab_case("DataGrid"), "data-grid");
        assert_eq!(kebab_case("SimpleGrid"), "simple-grid");
        assert_eq!(kebab_case("UiSpinner"), "ui-spinner");
        assert_eq!(kebab_case("StatDisplay"), "stat-display");
        // idempotent on already-kebab input
        assert_eq!(kebab_case("data-grid"), "data-grid");
    }

    #[test]
    fn pattern_self_close() {
        let v = parse_value("<Stack direction=\"vertical\" />");
        let map = match v {
            SExpression::Object(m) => m,
            other => panic!("expected object, got {other:?}"),
        };
        assert_eq!(map.get("type").and_then(|t| t.as_str()), Some("stack"));
        assert_eq!(
            map.get("direction").and_then(|t| t.as_str()),
            Some("vertical")
        );
        assert!(map.get("children").is_none());
    }

    #[test]
    fn pattern_with_children() {
        let v = parse_value("<Stack direction=\"vertical\">\n  <Badge label=\"x\" />\n</Stack>");
        let outer = match v {
            SExpression::Object(m) => m,
            other => panic!("expected object, got {other:?}"),
        };
        let children = outer.get("children").expect("children");
        let arr = match children {
            SExpression::Array(a) => a,
            other => panic!("expected children array, got {other:?}"),
        };
        assert_eq!(arr.len(), 1);
        let badge = match &arr[0] {
            SExpression::Object(m) => m,
            other => panic!("expected badge object, got {other:?}"),
        };
        assert_eq!(badge.get("type").and_then(|t| t.as_str()), Some("badge"));
    }

    #[test]
    fn named_trait_ref() {
        let v = parse_value("<trait.FileTextIcon />");
        assert_eq!(v.as_str(), Some("@trait.FileTextIcon"));
    }

    #[test]
    fn named_trait_ref_rejects_props() {
        let mut p =
            Parser::new(crate::lexer::tokenize("<trait.FileTextIcon label=\"x\" />").unwrap());
        assert!(p.parse_jsx_element().is_err());
    }

    #[test]
    fn inline_trait_ref_with_entity_and_children() {
        let v = parse_value(
            "<UiButton.traits.ButtonRender linkedEntity=\"ApBill\" action={PAY_BILL} label=\"Back\">\n  <trait.FileTextIcon />\n</UiButton.traits.ButtonRender>",
        );
        // Desugars to an @trait.Inline… string; the synthetic trait is hoisted.
        let s = v.as_str().expect("@trait string");
        assert!(s.starts_with("@trait.InlineButtonRender"));
    }

    /// Inline-ref props carrying binding expressions (`@entity.X`, a lambda
    /// param like `@party.X`) desugar to the documented form: a hoisted
    /// synthetic `TraitReference::Reference` whose config-field DEFAULTS hold
    /// the author's expressions verbatim, plus the `@trait.InlineN` string at
    /// the authoring site.
    #[test]
    fn inline_ref_binding_props_desugar_to_hoisted_config_defaults() {
        let src = r#"orbital O {
  entity E [runtime] {
    id : string!
    displayIndex : number
    rows : [object]
  }
  trait T -> E {
    state s {
      INIT -> s
        (set @entity.rows [])
        (set @entity.displayIndex 0)
        (render-ui main {
          type: data-list
          items: @entity.rows
          renderItem: (fn party
            <UiBadge.traits.BadgeRender linkedEntity="E" amount={@entity.displayIndex} label={@party.title} />)
        })
    }
  }
}"#;
        let schema = crate::parse_and_lower_str(src).expect("lower");
        let orbital = &schema.orbitals[0];

        let (name, linked, config) = orbital
            .traits
            .iter()
            .find_map(|t| match t {
                orbital_core::schema::TraitReference::Reference {
                    reference: Some(r),
                    name,
                    linked_entity,
                    config,
                    ..
                } if r == "UiBadge.traits.BadgeRender" => {
                    Some((name.clone(), linked_entity.clone(), config.clone()))
                }
                _ => None,
            })
            .expect("hoisted synthetic trait reference must exist");

        assert!(
            name.as_deref()
                .unwrap_or("")
                .starts_with("InlineBadgeRender"),
            "synthetic name must be InlineBadgeRenderN, got {name:?}"
        );
        assert_eq!(linked.as_deref(), Some("E"));
        let config = config.expect("config map");
        assert_eq!(
            config.get("amount").and_then(|f| f.default.as_ref()),
            Some(&SExpression::String("@entity.displayIndex".to_string())),
            "@entity prop expression must land verbatim in the config default"
        );
        assert_eq!(
            config.get("label").and_then(|f| f.default.as_ref()),
            Some(&SExpression::String("@party.title".to_string())),
            "lambda-param prop expression must land verbatim in the config default"
        );

        // The authoring site inside the lambda holds the @trait.InlineN string.
        let json = serde_json::to_string(&schema).expect("serialize");
        assert!(
            json.contains("@trait.InlineBadgeRender"),
            "render tree must reference the hoisted synthetic: {json}"
        );
    }

    /// A `?field` (= `@payload.field`) prop on an inline-ref call site is a
    /// capture from the COMPOSING effect's event — it desugars to the hoisted
    /// synthetic's config default as `@callsitePayload.field`, NOT a raw
    /// `@payload.field` (which would be evaluated in the child's own INIT
    /// scope). Non-payload prop expressions are unaffected.
    #[test]
    fn inline_ref_payload_prop_captured_as_callsite() {
        let src = r#"orbital O {
  type Failed = Event { error : string } "e"
  entity E [runtime] { id : string! }
  trait T -> E {
    state s {
      INIT -> s
        (render-ui main {
          type: stack
          children: [@trait.WidgetTitle]
        })
      Failed -> s
        (render-ui main <Stack direction={vertical}>
          <Typography.traits.TypographyRender content={?error} variant={body} />
        </Stack>)
    }
    emits { Failed }
  }
  trait WidgetTitle = Typography.traits.TypographyRender -> E {
    config { content: Widget }
  }
}"#;
        let schema = crate::parse_and_lower_str(src).expect("lower");
        let orbital = &schema.orbitals[0];
        let config = orbital
            .traits
            .iter()
            .find_map(|t| match t {
                orbital_core::schema::TraitReference::Reference {
                    reference: Some(r),
                    name: Some(n),
                    config,
                    ..
                } if r == "Typography.traits.TypographyRender"
                    && n.starts_with("InlineTypographyRender") =>
                {
                    config.clone()
                }
                _ => None,
            })
            .expect("hoisted synthetic Typography reference must exist");
        assert_eq!(
            config
                .get("content")
                .and_then(|f| f.default.as_ref())
                .and_then(|d| d.as_str()),
            Some("@callsitePayload.error"),
            "a `?error` capture prop must lower to @callsitePayload.error on the reference config"
        );
        assert_eq!(
            config
                .get("variant")
                .and_then(|f| f.default.as_ref())
                .and_then(|d| d.as_str()),
            Some("body"),
            "non-payload props are untouched"
        );
    }

    #[test]
    fn expression_container_attrs() {
        let v = parse_value("<StatDisplay value={@entity.amountTotal} label=\"Total\" />");
        let map = match v {
            SExpression::Object(m) => m,
            other => panic!("expected object, got {other:?}"),
        };
        assert_eq!(
            map.get("value").and_then(|t| t.as_str()),
            Some("@entity.amountTotal")
        );
    }

    #[test]
    fn boolean_shorthand() {
        let v = parse_value("<Spinner centered />");
        let map = match v {
            SExpression::Object(m) => m,
            other => panic!("expected object, got {other:?}"),
        };
        assert_eq!(map.get("type").and_then(|t| t.as_str()), Some("spinner"));
        assert_eq!(map.get("centered"), Some(&SExpression::Bool(true)));
    }

    #[test]
    fn raw_text_child_rejected() {
        let mut p = Parser::new(crate::lexer::tokenize("<Stack>raw text</Stack>").unwrap());
        assert!(p.parse_jsx_element().is_err());
    }

    #[test]
    fn mismatched_closing_tag() {
        let mut p = Parser::new(crate::lexer::tokenize("<Stack>\n  <Badge />\n</Badge>").unwrap());
        assert!(p.parse_jsx_element().is_err());
    }

    /// The defining property of JSX sugar: a JSX-authored render tree must
    /// lower to byte-identical `.orb` JSON as the hand-written object-literal
    /// form. We compare the canonicalized lowered schemas.
    fn lower_json(src: &str) -> serde_json::Value {
        let schema = crate::parse_and_lower_str(src).expect("lower");
        let v = serde_json::to_value(&schema).unwrap();
        // Canonicalize: sort object keys recursively so HashMap ordering
        // differences don't cause false mismatches, and drop the V4 identity
        // fields (opaque per-lowering ids + ledger) — this test asserts
        // render-tree structural identity, not id identity.
        fn canon(v: serde_json::Value) -> serde_json::Value {
            fn is_identity_key(k: &str) -> bool {
                k == "id"
                    || k == "ledger"
                    || k == "schemaVersion"
                    || k.ends_with("Id")
                    || k.ends_with("Ids")
            }
            match v {
                serde_json::Value::Object(map) => {
                    let mut sorted: std::collections::BTreeMap<String, serde_json::Value> =
                        std::collections::BTreeMap::new();
                    for (k, val) in map {
                        if is_identity_key(&k) {
                            continue;
                        }
                        sorted.insert(k, canon(val));
                    }
                    let mut out = serde_json::Map::new();
                    for (k, val) in sorted {
                        out.insert(k, val);
                    }
                    serde_json::Value::Object(out)
                }
                serde_json::Value::Array(items) => {
                    serde_json::Value::Array(items.into_iter().map(canon).collect())
                }
                other => other,
            }
        }
        canon(v)
    }

    #[test]
    fn jsx_lowers_identically_to_object_literal() {
        let header = "orbital O {\n  entity E [runtime] { id : string! }\n  trait T -> E {\n    state s {\n      INIT -> s\n        (render-ui main ";
        let footer = ")\n    }\n  }\n}\n";

        // Object-literal form.
        let obj = format!(
            "{header}{{ type: stack, direction: vertical, gap: md, children: [@trait.A, {{ type: badge, label: \"x\" }}] }}{footer}"
        );
        // JSX form (semantically identical).
        let jsx = format!(
            "{header}<Stack direction=\"vertical\" gap=\"md\">\n  <trait.A />\n  <Badge label=\"x\" />\n</Stack>{footer}"
        );

        let a = lower_json(&obj);
        let b = lower_json(&jsx);
        assert_eq!(
            a, b,
            "JSX must lower to identical .orb as object-literal form"
        );
    }
}
