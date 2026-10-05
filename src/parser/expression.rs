//! S-expression / object-literal / array-literal / duration parsing for the
//! .lolo parser.

use super::*;

impl Parser {
    pub(crate) fn parse_sexpr(&mut self) -> Result<SExpression, ParseError> {
        let span = self.peek_span();
        self.expect_kind(&TokenKind::LParen, "(")?;
        // An expression is located by its operator, the token a diagnostic names.
        self.record_expr_span();
        if matches!(self.peek(), TokenKind::RParen) {
            self.advance();
            return Ok(SExpression::Array(Vec::new()));
        }
        // First element is the operator. Accept any identifier plus the
        // single-char comparison token `=` (tokenized as `Equals`; used in
        // legacy pre-V2 `.lolo` files as the Lisp-style equality operator).
        let op = match self.peek().clone() {
            TokenKind::Identifier(name) => {
                self.advance();
                // The head is either a built-in effect word or an
                // `ns/name` operator — canonicalize it here (once) so every
                // later `head.as_deref()` match in this function, and every
                // lower-phase match on the emitted `.orb`, sees only English.
                let name = self
                    .lang
                    .canon_head(&name)
                    .map(str::to_string)
                    .unwrap_or(name);
                SExpression::String(name)
            }
            TokenKind::Equals => {
                self.advance();
                SExpression::String("=".to_string())
            }
            TokenKind::Arrow | TokenKind::UnicodeArrow => {
                return Err(ParseError::InvalidSExpr {
                    message: "unexpected `->` inside an s-expression: transitions are written at state-block level as `EVENT -> target`, not inside parentheses; each effect under a transition is its own `(...)` line".to_string(),
                    span,
                })
            }
            other => {
                return Err(ParseError::InvalidSExpr {
                    message: format!("expected operator, got {other:?}"),
                    span,
                })
            }
        };
        let head = match &op {
            SExpression::String(s) => Some(s.clone()),
            _ => None,
        };
        // `(match <scrutinee> (<pattern> <value>) … (_ <value>))` — Phase 3b
        // expression-level match sugar. Reserved head, like `let`/`fn` above:
        // recognized before generic arg parsing so its arm grammar (`(pattern
        // value)` pairs, not a fixed arg budget) can differ from every other
        // call form. The opening `(` is already consumed; this returns the
        // whole expression, including its own closing `)`.
        if head.as_deref() == Some("match") {
            return self.parse_expr_match(span);
        }
        let mut items = vec![op];
        // Special case: `(let ((x v) (y w)) body)` — the second arg is a list of bindings.
        let is_let = matches!(items.first(), Some(SExpression::String(s)) if s == "let");
        let has_let_bindings = is_let && matches!(self.peek(), TokenKind::LParen);
        let is_lambda =
            matches!(items.first(), Some(SExpression::String(s)) if s == "fn" || s == "lambda");
        // Both forms open a lexical scope that stays live for the body args
        // parsed by the loop below.
        let mut opened_scope = false;
        if has_let_bindings {
            // Parse the binding list. Not wrapped in `with_trail_seg` — the
            // absolute `[1][pair][1]` trail for each binding VALUE is built
            // by `parse_let_bindings` itself (see its own doc comment).
            self.binding_scopes.push(Vec::new());
            opened_scope = true;
            let bindings = self.parse_let_bindings()?;
            items.push(bindings);
        } else if is_lambda {
            let params = self.parse_lambda_params()?;
            opened_scope = true;
            items.extend(params);
        }
        // Loop-arg budgets for heads whose args before this loop are already
        // consumed above (`let`'s binding list, `fn`/`lambda`'s params) — the
        // loop only sees what remains. `if`/`when` consume nothing above, so
        // their loop budget equals their total-arg budget.
        let loop_arg_max: Option<usize> = match head.as_deref() {
            Some("let") if has_let_bindings => Some(1),
            Some("if") => Some(3),
            Some("when") => Some(2),
            Some("quote") => Some(1),
            // `(fn (a b) body)` — parenthesized params aren't consumed by
            // `parse_lambda_params`, so they reach the loop as its first arg;
            // the budget then covers params + body.
            Some("fn") | Some("lambda") if opened_scope => {
                Some(if items.len() == 1 { 2 } else { 1 })
            }
            _ => None,
        };
        let mut arg_spans: Vec<Span> = Vec::new();
        loop {
            self.skip_newlines();
            if matches!(self.peek(), TokenKind::RParen) {
                self.advance();
                break;
            }
            if self.at_eof() {
                return Err(ParseError::UnclosedDelimiter {
                    delimiter: '(',
                    closer: ')',
                    open_span: span,
                    found: "end of file".to_string(),
                });
            }
            // A bare `}` here is always the enclosing block closing over an
            // unterminated s-expression — report the cause (the open `(`),
            // not the consequence.
            if matches!(self.peek(), TokenKind::RBrace) {
                return Err(ParseError::UnclosedDelimiter {
                    delimiter: '(',
                    closer: ')',
                    open_span: span,
                    found: "`}`".to_string(),
                });
            }
            arg_spans.push(self.peek_span());
            let arg_index = items.len();
            let arg =
                self.with_trail_seg(format!("[{arg_index}]"), |p| p.parse_sexpr_or_value())?;
            items.push(arg);
        }
        if opened_scope {
            self.binding_scopes.pop();
        }
        if let (Some(max), Some(h)) = (loop_arg_max, head.as_deref()) {
            if arg_spans.len() > max {
                let consumed_before_loop = items.len() - 1 - arg_spans.len();
                return Err(ParseError::TooManyArgs {
                    head: h.to_string(),
                    max: consumed_before_loop + max,
                    got: items.len() - 1,
                    span: arg_spans[max],
                });
            }
        }
        if let Some("set") = head.as_deref() {
            // args = items[1..] (target, value, [options]).
            let args_len = items.len() - 1;
            if args_len == 3 {
                let is_options_object = matches!(&items[3], SExpression::Object(_));
                if !is_options_object {
                    return Err(ParseError::SetExtraArgs {
                        got: args_len,
                        span: arg_spans[2],
                    });
                }
            } else if args_len > 3 {
                return Err(ParseError::SetExtraArgs {
                    got: args_len,
                    span: arg_spans[2],
                });
            }
            if args_len >= 2 {
                if let SExpression::Array(inner) = &items[2] {
                    if matches!(inner.first(), Some(SExpression::String(s)) if s == "set") {
                        return Err(ParseError::SetNestedValue { span: arg_spans[1] });
                    }
                }
            }
        }
        // `(quote x)` (G-CROSS-041): `x` becomes one encoded JSON string, so
        // nothing downstream walks, rewrites or resolves inside it.
        if head.as_deref() == Some("quote") {
            let Some(quoted) = items.get(1) else {
                return Err(ParseError::InvalidSExpr {
                    message: "`(quote x)` takes exactly one expression to hold as data".to_string(),
                    span,
                });
            };
            let body = orbital_core::schema::quote::encode_quote_body(quoted);
            return Ok(SExpression::Array(vec![
                SExpression::String("quote".to_string()),
                SExpression::String(body),
            ]));
        }
        // `(persist create|update|delete …)` — the action word (items[1])
        // is a `capabilities`-section word, canonicalized here rather than
        // at the generic bare-atom position (`parse_sexpr_or_value`'s
        // `Identifier` arm never canonicalizes: it can't tell an action
        // word from an ordinary event/entity/slot name at that position).
        if head.as_deref() == Some("persist") {
            if let Some(SExpression::String(s)) = items.get_mut(1) {
                if let Some(canon) = self.lang.canon_capability(s) {
                    *s = canon.to_string();
                }
            }
        }
        Ok(SExpression::Array(items))
    }

    /// Parse the body of an expression-level `match` (Phase 3b, pattern-
    /// matching doc §3b): `<scrutinee> (<pattern> <value>) … (_ <value>)`,
    /// including the closing `)`. The leading `(match` is already consumed;
    /// `open_span` is the whole expression's opening-paren span (for
    /// unclosed-delimiter reporting).
    ///
    /// Encodes the result as an ordinary `["match", scrutinee, [pattern,
    /// value], …]` `SExpression::Array` — reusing the exact grammar shape
    /// every other sexpr call already has — so every existing generic tree
    /// walker (the set/emit/button-action checks, `collect_entity_field_refs`,
    /// the formatter) recurses through a match's scrutinee and arm values
    /// unchanged; only the dedicated desugar (`lower::desugar_expr_match`)
    /// and check (`lower::checks::check_expr_match_types`) know the shape
    /// means anything. Arm-reachability/exhaustiveness needs the trait's own
    /// config/entity types, so that's an L1 *lowering*-phase check, not a
    /// parse one: this function only enforces what needs no such context —
    /// scrutinee purity and "at least one non-wildcard arm".
    pub(crate) fn parse_expr_match(&mut self, open_span: Span) -> Result<SExpression, ParseError> {
        let scrutinee_span = self.peek_span();
        let scrutinee = self.parse_sexpr_or_value()?;
        if !is_pure_match_scrutinee(&scrutinee) {
            return Err(ParseError::InvalidSExpr {
                message: "ELOLO_MATCH_EXPR_SCRUTINEE_IMPURE: a `match` scrutinee must be a pure \
                          reference (@entity.<f>, @payload.<f>, @config.<k>, or ?field) or a \
                          literal — re-evaluating an arbitrary expression once per arm isn't \
                          free, so this v1 requires a pure read; a general-expression scrutinee \
                          needs let-binding support in the desugar (a follow-up)"
                    .to_string(),
                span: scrutinee_span,
            });
        }

        let mut items = vec![SExpression::String("match".to_string()), scrutinee];
        let mut non_wildcard = 0usize;
        loop {
            self.skip_newlines();
            if matches!(self.peek(), TokenKind::RParen) {
                self.advance();
                break;
            }
            if self.at_eof() {
                return Err(ParseError::UnclosedDelimiter {
                    delimiter: '(',
                    closer: ')',
                    open_span,
                    found: "end of file".to_string(),
                });
            }
            self.expect_kind(&TokenKind::LParen, "(")?;
            let pattern = self.parse_match_pattern()?;
            if !matches!(pattern, MatchPattern::Wildcard) {
                non_wildcard += 1;
            }
            let value = self.parse_sexpr_or_value()?;
            self.expect_kind(&TokenKind::RParen, ")")?;
            items.push(SExpression::Array(vec![pattern.literal_sexpr(), value]));
        }
        if non_wildcard == 0 {
            return Err(ParseError::InvalidSExpr {
                message: "ELOLO_MATCH_EXPR_NO_ARMS: a `match` needs at least one non-wildcard \
                          arm"
                .to_string(),
                span: open_span,
            });
        }
        Ok(SExpression::Array(items))
    }

    /// Consume a lambda's parameters and open a binding scope for them.
    ///
    /// Two surface forms reach here: grouped `(fn [a b] body)` and flat
    /// `(fn a b body)`. In the flat form the trailing element is the body, not
    /// a parameter — the same rule `SExpression::normalize_fn_shapes` applies
    /// when it canonicalizes the two into one — so a run of bare identifiers
    /// that ends at `)` yields one fewer parameter.
    fn parse_lambda_params(&mut self) -> Result<Vec<SExpression>, ParseError> {
        let mut emitted = Vec::new();
        let mut names = Vec::new();
        if matches!(self.peek(), TokenKind::LBracket) {
            let grouped = self.parse_array_literal()?;
            if let SExpression::Array(entries) = &grouped {
                for entry in entries {
                    if let SExpression::String(name) = entry {
                        names.push(name.clone());
                    }
                }
            }
            emitted.push(grouped);
            self.binding_scopes.push(names);
            return Ok(emitted);
        }
        let mut flat: Vec<(String, Span)> = Vec::new();
        while let TokenKind::Identifier(name) = self.peek().clone() {
            // `<` heads a JSX element, never a parameter.
            if name == "<" {
                break;
            }
            // A literal is never a parameter name. English `true`/`false`/
            // `null` never reach here — the lexer gives them their own token
            // kinds — but a native rendering (`صحيح`, `drzi`) lexes as a plain
            // identifier, and swallowing it as a parameter would emit the body
            // of `(fn row صحيح)` as the STRING "صحيح" instead of `true`.
            // Stopping here hands it to the ordinary argument path, the one
            // place that canonicalizes a literal word.
            if self.lang.canon_literal(&name).is_some() {
                break;
            }
            let span = self.peek_span();
            self.advance();
            flat.push((name, span));
        }
        // A trailing bare identifier at `)` is the body. Emit it through the
        // same bare-reference check the ordinary argument path applies.
        let trailing_body = if matches!(self.peek(), TokenKind::RParen) {
            flat.pop()
        } else {
            None
        };
        for (name, _) in &flat {
            names.push(name.clone());
            emitted.push(SExpression::String(name.clone()));
        }
        self.binding_scopes.push(names);
        if let Some((body, span)) = trailing_body {
            if self.is_lexically_bound(&body) {
                return Err(ParseError::BareBindingReference { name: body, span });
            }
            emitted.push(SExpression::String(body));
        }
        Ok(emitted)
    }

    /// Parses `((x v) (y w))` — the outer list of `(name value)` pairs after
    /// `let`. Trail note: the call site (`parse_sexpr`) does NOT wrap this
    /// call in a `[1]` trail segment, so each binding's value span is
    /// recorded here under the FULL absolute path `[1][{pair_index}][1]` —
    /// index 1 (the bindings-list arg of the enclosing `let` sexpr), then the
    /// pair's index, then 1 (the value slot of the `(name value)` pair).
    pub(crate) fn parse_let_bindings(&mut self) -> Result<SExpression, ParseError> {
        // `((x v) (y w))` — outer list of inner pairs.
        let open_span = self.peek_span();
        self.expect_kind(&TokenKind::LParen, "(")?;
        let mut pairs = Vec::new();
        let mut pair_index = 0usize;
        loop {
            self.skip_newlines();
            if matches!(self.peek(), TokenKind::RParen) {
                self.advance();
                break;
            }
            if self.at_eof() {
                return Err(ParseError::UnclosedDelimiter {
                    delimiter: '(',
                    closer: ')',
                    open_span,
                    found: "end of file".to_string(),
                });
            }
            self.expect_kind(&TokenKind::LParen, "(")?;
            // name
            let name = match self.peek().clone() {
                TokenKind::Identifier(n) => {
                    self.advance();
                    SExpression::String(n)
                }
                other => {
                    return Err(ParseError::InvalidSExpr {
                        message: format!("let binding name expected, got {other:?}"),
                        span: self.peek_span(),
                    })
                }
            };
            let value = self.with_trail_seg(format!("[1][{pair_index}][1]"), |p| {
                p.parse_sexpr_or_value()
            })?;
            self.expect_kind(&TokenKind::RParen, ")")?;
            // `let` is sequential: a binding's value may read earlier bindings,
            // so the name only enters scope after its own value is parsed.
            if let SExpression::String(bound) = &name {
                if let Some(frame) = self.binding_scopes.last_mut() {
                    frame.push(bound.clone());
                }
            }
            pairs.push(SExpression::Array(vec![name, value]));
            pair_index += 1;
        }
        Ok(SExpression::Array(pairs))
    }

    /// Parse one s-expression argument: an atom, a paren list, an object literal, or an array literal.
    pub(crate) fn parse_sexpr_or_value(&mut self) -> Result<SExpression, ParseError> {
        if !matches!(self.peek(), TokenKind::LParen) {
            self.record_expr_span();
        }
        let pattern_type_value = std::mem::take(&mut self.pattern_type_value);
        match self.peek().clone() {
            TokenKind::AtParen => self.parse_inline_trait_ref(),
            TokenKind::LParen => self.parse_sexpr(),
            TokenKind::LBrace => self.parse_object_literal(),
            TokenKind::LBracket => self.parse_array_literal(),
            TokenKind::String(s) => {
                self.advance();
                Ok(SExpression::String(s))
            }
            TokenKind::Integer(n) => {
                self.advance();
                Ok(SExpression::Number(n as f64))
            }
            TokenKind::Number(n) => {
                self.advance();
                Ok(SExpression::Number(n))
            }
            TokenKind::Boolean(b) => {
                self.advance();
                Ok(SExpression::Bool(b))
            }
            TokenKind::Null => {
                self.advance();
                Ok(SExpression::Null)
            }
            TokenKind::Sigil(name) => {
                self.advance();
                let name = self.canon_sigil_root(&name);
                Ok(SExpression::String(format!("@{}", lower_self_sigil(&name))))
            }
            TokenKind::PayloadSigil(name) => {
                self.advance();
                if name.is_empty() {
                    Ok(SExpression::String("@payload".to_string()))
                } else {
                    Ok(SExpression::String(format!("@payload.{name}")))
                }
            }
            TokenKind::Identifier(name) => {
                if name == "<" {
                    // JSX element start (the comparison operator `<` only
                    // appears as an s-expression head, never in value
                    // position). Desugar to the existing render forms.
                    return self.parse_jsx_element();
                }
                let span = self.peek_span();
                self.advance();
                if let Some(dotted) = self.take_dotted_event_ref(&name, span) {
                    return Ok(SExpression::String(dotted));
                }
                // A native rendering of `true`/`false`/`null` lexes as a
                // plain identifier (the lexer stays language-free); the
                // parser is the one place that knows the current language,
                // so it canonicalizes here — same reservation English's
                // literal words already get at the lexer.
                if let Some(literal) = self.lang.canon_literal(&name) {
                    return Ok(match literal {
                        "true" => SExpression::Bool(true),
                        "false" => SExpression::Bool(false),
                        "null" => SExpression::Null,
                        other => unreachable!("unknown canonical literal: {other}"),
                    });
                }
                // A bare identifier that names an enclosing `let`/lambda
                // binding reads as the literal string of its own name in both
                // evaluators — silently, and green under every validator.
                // `type:` values are exempt: that position holds a pattern
                // name, so `type: card` inside `(fn card …)` is the pattern.
                if !pattern_type_value && self.is_lexically_bound(&name) {
                    return Err(ParseError::BareBindingReference { name, span });
                }
                // Bare identifiers in argument position are atoms (event names,
                // entity names, slot names, mode names). Special atoms `now`
                // and `state` (also translated via the `sigils` section) lower
                // to bindings.
                let atom = match self.lang.canon_sigil(&name).unwrap_or(name.as_str()) {
                    "now" => "@now".to_string(),
                    "state" => "@state".to_string(),
                    _ => name,
                };
                Ok(SExpression::String(atom))
            }
            TokenKind::Arrow | TokenKind::UnicodeArrow => Err(ParseError::InvalidSExpr {
                message: "unexpected `->` inside an s-expression: transitions are written at state-block level as `EVENT -> target`, not inside parentheses; each effect under a transition is its own `(...)` line".to_string(),
                span: self.peek_span(),
            }),
            other => Err(ParseError::InvalidSExpr {
                message: format!("unexpected token in s-expression: {other:?}"),
                span: self.peek_span(),
            }),
        }
    }

    /// Parse an inline trait-reference sugar `@(Alias.traits.Upstream [->
    /// Entity] { …body… })` in an s-expression value position. Hoists the
    /// body into a synthesized top-level `trait <Name> = Alias.traits.Upstream
    /// { … }` declaration (appended to the current orbital at the end of
    /// `parse_orbital`) and returns a `@trait.<Name>` string in its place, so
    /// the rest of the pipeline is byte-identical to the hand-written two-step
    /// form. Nested `@(…)` inside the body are handled naturally because the
    /// shared body parser recurses through `parse_sexpr_or_value`.
    ///
    /// On entry the next token is `AtParen`; on exit the matching `)` is
    /// consumed.
    pub(crate) fn parse_inline_trait_ref(&mut self) -> Result<SExpression, ParseError> {
        let span = self.peek_span();
        self.advance(); // consume `@(`

        let reference = self.parse_dotted_ident()?;

        // Optional `-> Entity` linkedEntity override.
        let linked_entity = if matches!(self.peek(), TokenKind::Arrow | TokenKind::UnicodeArrow) {
            self.advance();
            Some(self.expect_ident()?)
        } else {
            None
        };

        let mut body = self.parse_trait_ref_body()?;

        // Closing `)` of the `@(...)` wrapper.
        self.expect_kind(&TokenKind::RParen, ")")?;

        // Call-site payload capture (mirrors JSX `desugar_inline_ref`): a
        // `@payload.<field>` in this call-site config value is a reference to
        // the COMPOSING effect's event payload — tag it
        // `@callsitePayload.<field>` so it is resolved at the call site.
        for field in body.config.iter_mut() {
            if let Some(default) = field.default.as_mut() {
                orbital_core::schema::capture_callsite_payload(default);
            }
        }

        // Deterministic, collision-free synthesized name. Uses the upstream
        // short name (last segment of the dotted path) for readability plus a
        // per-orbital sequence. The `Inline_` prefix reserves a namespace the
        // validator forbids for hand-authored trait names.
        // Synthesized name must be PascalCase (`^[A-Z][a-zA-Z0-9]*$`) to
        // pass the trait-name validator — no underscores. The `Inline`
        // prefix reserves a namespace; the validator can forbid hand-authored
        // names starting with `Inline`.
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
            // Guard against colliding with a hand-authored trait name or an
            // earlier synthesis in this orbital.
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
            linked_entity_span: None,
            entity_rebindable: false,
            entity_binding_description: None,
            entity_binding_synonyms: None,
            category: None,
            scope: None,
            local: false,
            capabilities: Vec::new(),
            initial_state: None,
            states: Vec::new(),
            emits: Vec::new(),
            listens: body.listens,
            ticks: Vec::new(),
            config: Vec::new(),
            span,
            reference: Some(TraitRefData {
                name_override: None,
                reference,
                events: body.events,
                fields: body.fields,
                config: body.config,
                emits_scope: body.emits_scope,
            }),
            is_inline_synthetic: true,
        });
        self.pending_inline_producers
            .push(self.current_hoist_producer.clone());

        Ok(SExpression::String(format!("@trait.{}", name)))
    }

    pub(crate) fn parse_object_literal(&mut self) -> Result<SExpression, ParseError> {
        let open_span = self.peek_span();
        self.expect_kind(&TokenKind::LBrace, "{")?;
        let mut map: HashMap<String, SExpression> = HashMap::new();
        loop {
            self.skip_newlines();
            if matches!(self.peek(), TokenKind::RBrace) {
                self.advance();
                break;
            }
            if self.at_eof() {
                return Err(ParseError::UnclosedDelimiter {
                    delimiter: '{',
                    closer: '}',
                    open_span,
                    found: "end of file".to_string(),
                });
            }
            // Key: identifier or string.
            let key = match self.peek().clone() {
                TokenKind::Identifier(s) => {
                    self.advance();
                    s
                }
                TokenKind::String(s) => {
                    self.advance();
                    s
                }
                other => {
                    return Err(ParseError::InvalidSExpr {
                        message: format!("object key expected, got {other:?}"),
                        span: self.peek_span(),
                    })
                }
            };
            self.expect_kind(&TokenKind::Colon, ":")?;
            self.pattern_type_value = key == "type";
            let value = self.with_trail_seg(format!(".{key}"), |p| p.parse_sexpr_or_value())?;
            self.pattern_type_value = false;
            map.insert(key, value);
            if matches!(self.peek(), TokenKind::Comma) {
                self.advance();
            }
            self.skip_newlines();
        }
        Ok(SExpression::Object(map))
    }

    pub(crate) fn parse_array_literal(&mut self) -> Result<SExpression, ParseError> {
        let open_span = self.peek_span();
        self.expect_kind(&TokenKind::LBracket, "[")?;
        let mut items = Vec::new();
        loop {
            self.skip_newlines();
            if matches!(self.peek(), TokenKind::RBracket) {
                self.advance();
                break;
            }
            if self.at_eof() {
                return Err(ParseError::UnclosedDelimiter {
                    delimiter: '[',
                    closer: ']',
                    open_span,
                    found: "end of file".to_string(),
                });
            }
            let index = items.len();
            let value = self.with_trail_seg(format!("[{index}]"), |p| p.parse_sexpr_or_value())?;
            items.push(value);
            if matches!(self.peek(), TokenKind::Comma) {
                self.advance();
            }
            self.skip_newlines();
        }
        Ok(SExpression::Array(items))
    }

    pub(crate) fn parse_duration_ms(&mut self) -> Result<u64, ParseError> {
        let span = self.peek_span();
        let n = match self.peek().clone() {
            TokenKind::Integer(n) => {
                self.advance();
                n as u64
            }
            other => {
                return Err(ParseError::UnexpectedToken {
                    expected: "duration number".to_string(),
                    got: format!("{other:?}"),
                    span,
                })
            }
        };
        let suf = match self.peek().clone() {
            TokenKind::Identifier(s) if matches!(s.as_str(), "ms" | "s" | "m" | "h" | "d") => {
                self.advance();
                s
            }
            _ => {
                return Err(ParseError::InvalidDuration {
                    value: n.to_string(),
                    span,
                })
            }
        };
        Ok(duration_to_ms(n, &suf))
    }
}

/// Expression-match (Phase 3b) scrutinee purity gate: a pure reference
/// (`@entity.<f>` / `@payload.<f>` / `@config.<k>` — `?field` lexes to the
/// same `@payload.<f>` string, see `PayloadSigil` in `parse_sexpr_or_value`)
/// or a literal. Re-evaluating the scrutinee once per arm (the nested-`if`
/// desugar's cost) is only observationally free for these — an operator call
/// or other sigil (`@user.*`/`@row.*`) needs let-binding support in the
/// desugar first, so v1 rejects those at parse time rather than silently
/// re-running side-effecting reads. A bare word passes as a literal: unbound
/// it compares as a constant string, and a `let`-bound name resolves to its
/// already-evaluated binding — a pure read either way.
fn is_pure_match_scrutinee(expr: &SExpression) -> bool {
    match expr {
        SExpression::String(s) if s.starts_with('@') => {
            s.starts_with("@entity.") || s.starts_with("@payload.") || s.starts_with("@config.")
        }
        SExpression::String(_) => true,
        SExpression::Number(_) | SExpression::Bool(_) | SExpression::Null => true,
        SExpression::Array(_) | SExpression::Object(_) => false,
    }
}

/// Pass a self-sigil through to its `.orb` JSON form. Every `@<name>`
/// roots its own binding namespace — there is NO implicit `@<name>` →
/// `@entity.<name>` rewrite. Schema authors who want an entity field
/// write the qualifier explicitly (`@entity.title`), and lambda
/// parameters (`["fn", "metric", body]`) keep their bare names so
/// references like `@metric.label` resolve to the param at evaluation
/// time. The handful of well-known roots (`entity`, `now`, `state`,
/// `payload`, `config`, …) are preserved unchanged here for clarity;
/// every other name flows through verbatim so the runtime / codegen
/// can decide what to do with it (lambda binding, user-defined trait
/// scope, payload field, …).
pub(crate) fn lower_self_sigil(name: &str) -> String {
    if name.is_empty() {
        return "entity".to_string();
    }
    name.to_string()
}
