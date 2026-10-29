//! Page declaration parsing for the .lolo parser.

use super::*;

impl Parser {
    pub(crate) fn parse_page(&mut self) -> Result<Page, ParseError> {
        let span = self.peek_span();
        self.expect_keyword("page")?;
        // A string literal is a route path; a bare identifier is a protocol word
        // (checked against the core protocol table at lowering) followed by its
        // `{ … }` address record.
        let mut protocol = None;
        let mut address = None;
        let path = match self.peek().clone() {
            TokenKind::String(s) => {
                self.advance();
                s
            }
            TokenKind::Identifier(word) => {
                self.advance();
                protocol = Some(self.lang.canon_keyword(&word).map(str::to_string).unwrap_or(word));
                if matches!(self.peek(), TokenKind::LBrace) {
                    address = Some(self.parse_object_literal()?);
                }
                String::new()
            }
            _ => {
                return Err(ParseError::UnexpectedToken {
                    expected: "page path or protocol word".to_string(),
                    got: format!("{:?}", self.peek()),
                    span: self.peek_span(),
                })
            }
        };
        // Optional `:: TypeName [args]` annotation between path and the
        // reference / traits / options sections.
        let type_annotation = self.try_parse_type_annotation()?;
        // Optional `as PageName` explicit name override.
        let name = if self.check_ident("as") {
            self.advance();
            Some(self.expect_ident()?)
        } else {
            None
        };
        // Optional `= Alias.pages.Upstream` reference form. The reference
        // turns this declaration into a `PageRef::Object` at lowering time;
        // any subsequent `-> Trait, Trait...` becomes the traits override.
        let reference = if matches!(self.peek(), TokenKind::Equals) {
            self.advance();
            Some(self.parse_dotted_ident()?)
        } else {
            None
        };
        // `-> Trait[, Trait...]`
        let mut traits = Vec::new();
        if matches!(self.peek(), TokenKind::Arrow | TokenKind::UnicodeArrow) {
            self.advance();
            traits.push(self.expect_ident()?);
            // Optional commas between page traits: `-> A, B` and `-> A B` both
            // parse (uniform-comma consistency — see docs/Almadar_LOLO_Audit.md).
            // No newline skip, so the list still ends at end-of-line / `[`.
            loop {
                if matches!(self.peek(), TokenKind::Comma) {
                    self.advance();
                }
                if matches!(self.peek(), TokenKind::Identifier(_)) {
                    traits.push(self.expect_ident()?);
                } else {
                    break;
                }
            }
        }
        // A page has no `{ }` body: its traits follow `->` (agents often write a block of
        // `@trait.X` lines instead).
        let page_line = |extra: Option<&str>| -> String {
            let mut names = traits.clone();
            if let Some(x) = extra {
                names.push(x.to_string());
            }
            let as_name = name
                .as_deref()
                .map(|n| format!(" as {n}"))
                .unwrap_or_default();
            let head = match &protocol {
                Some(word) => word.clone(),
                None => format!("\"{path}\""),
            };
            format!("page {head}{as_name} -> {}", names.join(", "))
        };
        if matches!(self.peek(), TokenKind::LBrace) {
            return Err(ParseError::UnexpectedToken {
                expected: format!(
                    "end of page declaration — a page has no `{{ }}` body; its traits follow `->`, \
                     comma-separated (`{}`)",
                    page_line(None)
                ),
                got: format!("{:?}", self.peek()),
                span: self.peek_span(),
            });
        }
        // Optional `[modifiers]` (shared grammar with the app header).
        let mut view = None;
        let mut primary_entity = None;
        let mut is_initial = false;
        let mut is_not_found = false;
        let mut head = orbital_core_base::schema::PageModifiers::default();
        if matches!(self.peek(), TokenKind::LBracket) {
            self.parse_modifier_bracket("page", PAGE_MODIFIERS, |p, key| {
                if key == "initial" {
                    is_initial = true;
                    return Ok(());
                }
                if key == "notFound" {
                    is_not_found = true;
                    return Ok(());
                }
                p.expect_kind(&TokenKind::Colon, "':' after a page modifier")?;
                match key {
                    "view" => view = Some(p.expect_ident()?),
                    "entity" => primary_entity = Some(p.expect_ident()?),
                    _ => p.parse_head_modifier(key, &mut head)?,
                }
                Ok(())
            })?;
        }
        let orbital_core_base::schema::PageModifiers {
            access,
            indexing,
            title,
            description,
            translation_of,
            label: _,
            icon: _,
            roles: _,
        } = head;
        // Trailing `@icon "..."` / `@label "..."` annotations (one or more
        // indented lines below the page declaration). Mirrors the config-knob
        // annotation reader (`parse_config_annotations`).
        let mut icon = None;
        let mut label = None;
        let mut roles = None;
        loop {
            self.skip_newlines();
            let annotation = match self.peek().clone() {
                TokenKind::Sigil(raw) => {
                    if let Some(mounted) = raw.strip_prefix("trait.") {
                        return Err(ParseError::UnexpectedToken {
                            expected: format!(
                                "@icon, @label or @roles — a page's traits follow `->`, comma-separated, \
                                 not `@trait.` lines: write `{}`",
                                page_line(Some(mounted))
                            ),
                            got: format!("{:?}", self.peek()),
                            span: self.peek_span(),
                        });
                    }
                    let s = self.canon_annotation_sigil(&raw);
                    if s == "icon" || s == "label" || s == "roles" {
                        s
                    } else {
                        break;
                    }
                }
                _ => break,
            };
            self.advance();
            if annotation == "roles" {
                roles = Some(self.parse_role_list("@roles")?);
                continue;
            }
            let value = match self.peek().clone() {
                TokenKind::String(s) => {
                    self.advance();
                    s
                }
                other => {
                    return Err(ParseError::UnexpectedToken {
                        expected: format!("string literal after @{}", annotation),
                        got: format!("{:?}", other),
                        span: self.peek_span(),
                    });
                }
            };
            match annotation.as_str() {
                "icon" => icon = Some(value),
                "label" => label = Some(value),
                _ => unreachable!(),
            }
        }
        Ok(Page {
            protocol,
            address,
            path,
            type_annotation,
            traits,
            view,
            primary_entity,
            is_initial,
            is_not_found,
            span,
            reference,
            name,
            icon,
            label,
            roles,
            access,
            indexing,
            title,
            description,
            translation_of,
        })
    }
}
