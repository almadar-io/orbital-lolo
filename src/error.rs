//! Error types for the .lolo lexer, parser, and lowering passes.

use crate::ast::Span;
use thiserror::Error;

/// Lexer error.
#[derive(Debug, Clone, Error)]
pub enum LexError {
    #[error("unexpected character {ch:?} at {line}:{col}")]
    UnexpectedChar { ch: char, line: u32, col: u32 },

    #[error("unterminated string starting at {line}:{col}")]
    UnterminatedString { line: u32, col: u32 },

    #[error("invalid string escape sequence \\{ch} at {line}:{col}")]
    InvalidEscape { ch: char, line: u32, col: u32 },

    #[error("invalid number {value:?} at {line}:{col}")]
    InvalidNumber { value: String, line: u32, col: u32 },

    #[error("unterminated block comment starting at {line}:{col}")]
    UnterminatedBlockComment { line: u32, col: u32 },
}

impl LexError {
    pub fn span(&self) -> Span {
        match self {
            LexError::UnexpectedChar { line, col, .. }
            | LexError::UnterminatedString { line, col }
            | LexError::InvalidEscape { line, col, .. }
            | LexError::InvalidNumber { line, col, .. }
            | LexError::UnterminatedBlockComment { line, col } => Span::new(*line, *col, 1),
        }
    }

    /// Human-readable teaching hint for the most common lexer mistakes.
    /// Returns `(suggestion, fix_guidance)`.
    pub fn hint(&self) -> (Option<String>, Option<String>) {
        match self {
            LexError::UnexpectedChar { ch: ';', .. } => (
                Some("a single `;` is not valid — `.lolo` comments are `;;` (double semicolon)".to_string()),
                Some("write `;; comment`, or remove the stray `;`".to_string()),
            ),
            LexError::UnexpectedChar { ch: '\'', .. } => (
                Some("strings are double-quoted in `.lolo` — `'` is not a valid quote".to_string()),
                Some("replace '…' with \"…\"".to_string()),
            ),
            // ```lolo fences written INTO the file (run 36: gpt-oss wrapped
            // the whole draft in a markdown code block).
            LexError::UnexpectedChar { ch: '`', .. } => (
                Some("`` ` `` has no meaning in `.lolo` — this usually means a markdown code fence (```) was written into the file; the file must contain raw source only".to_string()),
                Some("delete the ``` fence lines (and any prose around them) so the file starts directly with `orbital`/`app`".to_string()),
            ),
            // Inline prose annotation after a declaration (run 39: granite wrote
            // `date : date! – day of the review activity` — the dash starts free
            // text the lexer cannot read).
            LexError::UnexpectedChar { ch: '–' | '—', .. } => (
                Some("a dash starting free text — inline commentary is not part of the syntax; prose after a declaration must be a `;;` comment".to_string()),
                Some("replace the dash with `;;` (comments run to end of line) or delete the trailing prose".to_string()),
            ),
            LexError::UnexpectedChar { ch: '\u{2018}' | '\u{2019}' | '\u{201C}' | '\u{201D}', .. } => (
                Some("a curly “smart” quote — `.lolo` strings use straight double quotes only".to_string()),
                Some("replace the curly quote with a straight `\"` (and apostrophes in prose belong inside a double-quoted string or a `;;` comment)".to_string()),
            ),
            // `\uXXXX`-style escapes (run 39: StudyStreak) — the string escape
            // set is tiny; unicode goes in literally.
            LexError::InvalidEscape { ch, .. } => (
                Some(format!("`\\{ch}` is not a valid string escape — `.lolo` supports only `\\\"`, `\\\\`, `\\/`, `\\n`, `\\t`, `\\r`")),
                Some("write the character directly in the string (files are UTF-8; no `\\u` escapes needed)".to_string()),
            ),
            _ => (None, None),
        }
    }
}

/// Parser error.
#[derive(Debug, Clone, Error)]
pub enum ParseError {
    #[error("lex error: {0}")]
    Lex(#[from] LexError),

    #[error("unexpected token: expected {expected}, got {got} at {}:{}", span.line, span.col)]
    UnexpectedToken {
        expected: String,
        got: String,
        span: Span,
    },

    #[error("unexpected end of input at {}:{}: expected {expected}", span.line, span.col)]
    UnexpectedEof { expected: String, span: Span },

    /// An s-expression head accepts at most `max` argument(s) but the author
    /// wrote `got` — almost always a multi-statement body that needed
    /// `(do …)`. `span` points at the first extra argument.
    #[error(
        "too many expressions in `({head} …)` at {}:{}: `{head}` takes at most {max} argument(s), got {got}",
        span.line, span.col
    )]
    TooManyArgs {
        head: String,
        max: usize,
        got: usize,
        span: Span,
    },

    /// `(set target value …)` with more than the 2 required args (or a
    /// 3rd arg that isn't the `{ emit: … }` options object).
    #[error(
        "`(set …)` takes 2 arguments — a target and a value — but got {got} at {}:{}",
        span.line, span.col
    )]
    SetExtraArgs { got: usize, span: Span },

    /// `(set a (set b v))` — `set` is a statement, not an expression; it
    /// cannot be nested as the value of another `set`.
    #[error(
        "`(set …)` cannot be the value of another `(set …)` at {}:{}: set is a statement that returns nothing, not an expression",
        span.line, span.col
    )]
    SetNestedValue { span: Span },

    /// A block boundary (`found`: end of file, or a closing token like `}`)
    /// reached before the matching closer for a `(`, `{`, or `[` opened at
    /// `open_span`.
    #[error(
        "unclosed `{delimiter}` opened at {}:{}: reached {found} before the matching `{closer}`",
        open_span.line, open_span.col
    )]
    UnclosedDelimiter {
        delimiter: char,
        closer: char,
        open_span: Span,
        found: String,
    },

    #[error("unknown keyword {keyword:?} at {}:{}", span.line, span.col)]
    UnknownKeyword { keyword: String, span: Span },

    #[error("invalid persistence mode {value:?} at {}:{}: expected 'persistent', 'runtime', or 'shared'", span.line, span.col)]
    InvalidPersistenceMode { value: String, span: Span },

    #[error("invalid type expression at {}:{}: {message}", span.line, span.col)]
    InvalidType { message: String, span: Span },

    #[error("invalid s-expression at {}:{}: {message}", span.line, span.col)]
    InvalidSExpr { message: String, span: Span },

    #[error("invalid duration {value:?} at {}:{}", span.line, span.col)]
    InvalidDuration { value: String, span: Span },

    #[error("invalid sigil {value:?} at {}:{}", span.line, span.col)]
    InvalidSigil { value: String, span: Span },

    #[error("expected identifier at {}:{}, got {got}", span.line, span.col)]
    ExpectedIdentifier { got: String, span: Span },

    /// A `let` binding or `fn`/`lambda` parameter referenced without its `@`
    /// sigil. Both evaluators resolve lexical bindings only through the sigil,
    /// so a bare occurrence silently evaluates to its own name as a string —
    /// green under every validator, wrong at runtime.
    #[error("bare reference to the bound name {name:?} at {}:{}", span.line, span.col)]
    BareBindingReference { name: String, span: Span },
}

impl ParseError {
    pub fn span(&self) -> Span {
        match self {
            ParseError::Lex(e) => e.span(),
            ParseError::UnexpectedToken { span, .. }
            | ParseError::UnknownKeyword { span, .. }
            | ParseError::InvalidPersistenceMode { span, .. }
            | ParseError::InvalidType { span, .. }
            | ParseError::InvalidSExpr { span, .. }
            | ParseError::InvalidDuration { span, .. }
            | ParseError::InvalidSigil { span, .. }
            | ParseError::BareBindingReference { span, .. }
            | ParseError::TooManyArgs { span, .. }
            | ParseError::SetExtraArgs { span, .. }
            | ParseError::SetNestedValue { span }
            | ParseError::ExpectedIdentifier { span, .. } => *span,
            ParseError::UnclosedDelimiter { open_span, .. } => *open_span,
            ParseError::UnexpectedEof { span, .. } => *span,
        }
    }

    /// Human-readable teaching hint for the most common parser mistakes.
    /// Returns `(suggestion, fix_guidance)`. Delegates to [`LexError::hint`]
    /// for lexer errors so the teaching logic lives next to the error types.
    pub fn hint(&self) -> (Option<String>, Option<String>) {
        match self {
            ParseError::Lex(e) => e.hint(),
            ParseError::InvalidSExpr { message, .. } if message.contains("Comma") => (
                Some(".lolo arrays and objects are whitespace-separated, not comma-separated".to_string()),
                Some("remove the `,` and keep a space or newline between items".to_string()),
            ),
            // `(transition loading -> ready)` — an invented transition EFFECT
            // (run 30: ReviewHeatmap, 4 identical validates: the message
            // stated the rule but not the concrete edit for THIS line).
            ParseError::InvalidSExpr { message, .. }
                if message.contains("unexpected `->` inside an s-expression") =>
            (
                Some("there is no `(transition …)` effect — the state change lives on the ARM line itself".to_string()),
                Some("DELETE this parenthesized line; the transition target goes after the arrow on the arm above the effects: `EVENT -> ready`".to_string()),
            ),
            ParseError::UnexpectedToken { expected, got, .. }
                if expected == "]" && got == "Bang" =>
            (
                Some("array type syntax is `[ElementType]`; put `!` on the field, not inside the array type".to_string()),
                Some("change `[string!]` to `[string]` and make the field required as `field : [string]!`".to_string()),
            ),
            ParseError::UnexpectedToken { got, .. } if got.contains("Identifier(\"when\")") => (
                Some("`when` is not a `.lolo` keyword for transition guards".to_string()),
                Some("write the guard in parentheses on the same arm: `EVENT -> nextState (guard-expr)`".to_string()),
            ),
            ParseError::BareBindingReference { name, .. } => (
                Some(format!(
                    "`{name}` is bound by an enclosing `let` or `fn`, but bare names are string literals here — both evaluators resolve lexical bindings only through the `@` sigil"
                )),
                Some(format!(
                    "write `@{name}` to read the binding, or `\"{name}\"` if you really meant the literal string"
                )),
            ),
            ParseError::UnexpectedToken { got, .. } if got.contains("Identifier(\"/\")") => (
                Some("`//` is not a comment in `.lolo` — comments start with `;;`".to_string()),
                Some("replace the `//` with `;;`, or delete the comment line".to_string()),
            ),
            ParseError::InvalidSExpr { message, .. }
                if message.contains("object key expected, got LParen") =>
            (
                Some("`{ … }` opens an OBJECT literal, not a code block — code blocks don't exist in `.lolo`".to_string()),
                Some("for several effects in one slot (e.g. a lambda body) write `(do effect1 effect2 …)`; if you meant an object, its entries are `key: value`".to_string()),
            ),
            ParseError::InvalidSExpr { message, .. }
                if message.contains("object key expected, got Sigil") =>
            (
                Some("object literals have no shorthand keys — every entry is written `key: value`".to_string()),
                Some("write the pair explicitly: `field: @entity.field`".to_string()),
            ),
            // Observed live: a closer pile-up like `}])` `}))` where one `{`
            // was never closed — the parser is still inside an object when a
            // `)` arrives (C-PAREN mid-file shape; the EOF shape is
            // UnclosedDelimiter).
            ParseError::InvalidSExpr { message, .. }
                if message.contains("object key expected, got RParen")
                    || message.contains("object key expected, got RBracket") =>
            (
                Some("a `)` or `]` arrived while an object `{ … }` was still open — a `{` above is missing its `}`".to_string()),
                Some("close the object first: every `{` needs its `}` before the enclosing `(`/`[` can close; re-count the closer sequence at this line".to_string()),
            ),
            ParseError::UnexpectedToken { expected, .. } if expected == "from" => (
                Some("`uses` needs a source path: `uses <name> from \"std/behaviors/<name>\"`".to_string()),
                Some("std imports use the flat canonical form `std/behaviors/<name>` — topic folders never appear in import paths".to_string()),
            ),
            ParseError::UnexpectedToken { expected, got, .. }
                if expected.contains("{ or . or newline") && got.contains("Arrow") =>
            (
                Some("a `listens` entry is `EVENT { … }` — the `EVENT -> target { payload }` direction form belongs to `emits`, not `listens`".to_string()),
                Some("drop the `-> …` part; declare just the event name with its payload block".to_string()),
            ),
            // `payload: { … }` under an emits entry (run 33: ActivityHeatmap
            // — the listens brace-block shape guessed with a keyword; the old
            // silent loop-break surfaced a trait-level keyword error instead).
            ParseError::InvalidSExpr { message, .. }
                if message.contains("unexpected token inside `emits")
                    && message.contains("Identifier(\"payload\")") =>
            (
                Some("there is no `payload:` keyword — an emits entry declares its payload as a brace block directly after the event name".to_string()),
                Some("merge it into the entry: `EVENT_NAME { field : type }` (annotations like `@tier` go on the lines after the block)".to_string()),
            ),
            ParseError::InvalidSExpr { message, .. }
                if message.contains("unexpected token inside `emits") =>
            (
                Some("an `emits { … }` block holds only event entries: `EVENT_NAME [-> internal|external] [{ field : type }]` plus `@description`/`@tier` annotation lines".to_string()),
                Some("remove or rewrite this token so each entry starts with an UPPER_SNAKE event name".to_string()),
            ),
            // `ADD_TOPIC -> presentation` — the @tier vocabulary written as a
            // route (run 33: TopicManager, never converted without a hint).
            ParseError::UnexpectedToken { expected, got, .. }
                if expected == "internal or external after `->`" =>
            (
                Some(format!(
                    "`{got}` is not a route — after `->` an emits entry routes `internal` or `external` only; words like `presentation`/`domain` are TIER annotations, not routes"
                )),
                Some(format!(
                    "delete `-> {got}` (or route it `-> external` if a sibling orbital consumes it) and put the tier on its own line after the entry: `@tier \"{got}\"`"
                )),
            ),
            ParseError::UnexpectedToken { expected, got, .. }
                if expected == ":"
                    && (got.contains("Comma") || got.contains("RBrace") || got.contains("Newline")) =>
            (
                Some("an object entry is `key: value` — a multi-word text value must be quoted, and entries are whitespace-separated".to_string()),
                Some("quote the text (`label: \"Next Review\"`) and drop any `,` between entries".to_string()),
            ),
            ParseError::UnexpectedToken { expected, got, .. }
                if expected == ":" && got.contains("Sigil(") =>
            (
                Some("object literals have no shorthand keys — every entry is written `key: value`".to_string()),
                Some("write the pair explicitly: `field: @entity.field`".to_string()),
            ),
            ParseError::ExpectedIdentifier { got, .. }
                if ["description", "tier", "synonyms", "category", "capabilities"]
                    .iter()
                    .any(|a| got.contains(&format!("Sigil(\"{a}\")"))) =>
            (
                Some("event annotations (`@description`, `@tier`, …) go AFTER the payload block's closing `}`, not inside it".to_string()),
                Some("inside `{ … }` only `field : type` lines are allowed; move the annotation lines to just below the closing `}`".to_string()),
            ),
            ParseError::UnexpectedToken { expected, got, .. }
                if expected.contains("uses, expects, type, entity, trait, page")
                    && ["state", "emits", "listens", "ticks", "initial", "config"]
                        .iter()
                        .any(|kw| got.contains(&format!("Identifier(\"{kw}\")"))) =>
            (
                Some("this block belongs INSIDE a `trait <Name> { … }`, but the parser is at orbital level — an extra `}` above closed the trait early".to_string()),
                Some("remove the surplus `}` above this line (re-count braces from the trait header down), or move this block back inside the trait".to_string()),
            ),
            // Same failure one scope up (C-EARLY-TRAIT-CLOSE's orbital-level
            // cousin, observed live: `page` at file level after a surplus `}`
            // closed the orbital — the snippet shows the stranded declaration
            // but not the causal brace, so the hint must name it).
            ParseError::UnexpectedToken { expected, got, .. }
                if expected.contains("type or orbital")
                    && ["page", "trait", "entity", "uses", "expects"]
                        .iter()
                        .any(|kw| got.contains(&format!("Identifier(\"{kw}\")"))) =>
            (
                Some("this declaration belongs INSIDE the `orbital <Name> { … }` block, but the parser is at FILE level — an extra `}` above closed the orbital early".to_string()),
                Some("remove the surplus `}` above this line (re-count braces from the orbital header down), or move this declaration back inside the orbital's `{ … }`".to_string()),
            ),
            // Observed live (Run 18, 4 identical stalls): a stray `]` (or
            // `)`) left over from a render-ui block falls through to trait
            // level, and the default state-arm suggestion misdirects — the
            // model edits transitions instead of deleting one closer.
            ParseError::UnexpectedToken { expected, got, .. }
                if expected.contains("state, initial, emits, listens, ticks, or config")
                    && (got.contains("RBracket") || got.contains("RParen")) =>
            (
                Some("a stray `]` or `)` — the s-expression/array above already closed, so this extra closer falls through to trait level".to_string()),
                Some("delete this surplus closer (re-count `[`/`]` and `(`/`)` in the effect above; each opener closes exactly once)".to_string()),
            ),
            ParseError::UnexpectedToken { expected, got, .. }
                if expected.contains("state, initial, emits, listens, ticks, or config")
                    && looks_like_event_identifier(got) =>
            (
                Some("this looks like a transition arm (`EVENT -> target`) sitting OUTSIDE any `state { … }` block — transitions live inside states".to_string()),
                Some("move the arm into the right `state <name> { … }` block; write its guard inline as `EVENT -> target (guard-expr)`, never with `when`".to_string()),
            ),
            ParseError::UnexpectedToken { expected, got, .. }
                if expected.contains("state, initial, emits, listens, ticks, or config")
                    && got.contains("RParen") =>
            (
                Some("stray `)` — an s-expression in the effects above this line closes one paren too many".to_string()),
                Some("re-count parens in the preceding effect lines; each `(…)` effect must balance".to_string()),
            ),
            // An effect `(…)` where trait MEMBERS are expected: the effect is
            // fine — a surplus `}` above ejected it from its `state { … }`
            // block (run 21: TopicManager burned 8+ validates while the
            // catch-all arm below told it to rewrite the state body).
            ParseError::UnexpectedToken { expected, got, .. }
                if expected.contains("state, initial, emits, listens, ticks, or config")
                    && got.contains("LParen") =>
            (
                Some("this effect `(…)` is sitting at TRAIT level — a `}` above this line closed its `state { … }` block too early".to_string()),
                Some("delete the surplus `}` between the last transition arm and this effect (or move the effect back under its `EVENT -> target` arm inside the state block)".to_string()),
            ),
            ParseError::UnexpectedToken { expected, .. }
                if expected.contains("state, initial, emits, listens, ticks, or config") =>
            (
                Some("inside a `state { ... }` body each arm must start with `EVENT -> nextState` or an effect like `(set ...)`".to_string()),
                Some("remove or rewrite the stray token so the state body matches the grammar".to_string()),
            ),
            ParseError::TooManyArgs { head, .. } if head == "if" => (
                Some(format!(
                    "`({head} …)` allows one expression per argument position — everything after the last one is an extra argument"
                )),
                Some("write `(if cond then else)`; to run several effects in a branch wrap them as `(do effect1 effect2 …)`".to_string()),
            ),
            ParseError::TooManyArgs { head, .. } => (
                Some(format!(
                    "`({head} …)` allows one expression per argument position — everything after the last one is an extra argument"
                )),
                Some("wrap the body's multiple effects in a single `(do effect1 effect2 …)`".to_string()),
            ),
            ParseError::SetExtraArgs { .. } => (
                Some("each `(set …)` writes one binding".to_string()),
                Some("write one `(set target value)` per effect line, or run several in sequence with `(do (set …) (set …))`".to_string()),
            ),
            ParseError::SetNestedValue { .. } => (
                Some("`set` performs a write and produces no value".to_string()),
                Some("write the writes in sequence: `(do (set a …) (set b …))`".to_string()),
            ),
            ParseError::UnclosedDelimiter { .. } => (
                Some("every `(`, `{`, `[` must balance; this one was never closed".to_string()),
                Some("add the matching closer, then re-count delimiters in the block starting at the reported line".to_string()),
            ),
            ParseError::InvalidSExpr { message, .. }
                if message.contains("expected operator, got Newline")
                    || message.contains("expected operator, got LParen") =>
            (
                Some("bare `( … )` is not a grouping block — every `(` must start an operator call".to_string()),
                Some("to run several effects in one slot write `(do effect1 effect2 …)`; delete the bare `(`".to_string()),
            ),
            ParseError::InvalidSExpr { message, .. }
                if message.contains("expected operator, got Sigil") =>
            (
                Some("a value never needs its own parentheses — parens always start an operator call like `(set …)` or `(+ …)`".to_string()),
                Some("write the value bare (`@entity.field`), or complete the operator call you meant".to_string()),
            ),
            ParseError::InvalidSExpr { message, .. }
                if message.contains("unexpected token in s-expression: RBrace") =>
            (
                Some("missing `)` — the s-expression above never closed before this `}`".to_string()),
                Some("re-count parens in the effect lines just above; add the missing `)`".to_string()),
            ),
            _ => (None, None),
        }
    }
}

/// Lowering error.
#[derive(Debug, Clone, Error)]
pub enum LowerError {
    #[error("unknown type alias {name:?}")]
    UnknownTypeAlias { name: String },

    #[error("orbital must define an entity")]
    MissingEntity,

    #[error("transition references unknown state {state:?}")]
    UnknownState { state: String },

    #[error("invalid time-bound state: missing TIMEOUT target for state {state:?}")]
    MissingTimeoutTarget { state: String },

    #[error("invalid duration {duration:?}: {message}")]
    InvalidDuration { duration: String, message: String },

    #[error("invalid persistence {value:?}")]
    InvalidPersistence { value: String },

    #[error("invalid trait category {value:?}")]
    InvalidCategory { value: String },

    #[error("internal lowering bug: {0}")]
    Internal(String),

    /// Emit payload references a named type that cannot be resolved to an
    /// Entity- or Struct-shaped type alias or to the orbital's own entity.
    /// The type-system resolver catches most of these before lowering; this
    /// covers edge cases where the alias exists but is not field-shaped.
    #[error("emit payload references unknown or non-field-shaped type {name:?}")]
    UnknownPayloadType { name: String },

    /// Form B (`name :: TypeName`) flattened payload field collides with a
    /// sibling primitive field of the same name, or with another Form B
    /// expansion on the same emit.
    #[error("emit {event:?} has duplicate payload field {field:?}")]
    PayloadFieldCollision { event: String, field: String },

    /// An emits/listens entry restates its event's payload inline while an
    /// orbital-scope `type X = Event { … }` alias also types the same event,
    /// and the two shapes DISAGREE. Restating is allowed (it keeps the
    /// emits/listens block self-documenting) but must match the alias —
    /// silent drift between the two was the old "inline always wins"
    /// precedence's failure mode (syntax revision 2026-08-18).
    #[error(
        "event {event:?}: the inline payload shape disagrees with the `type {event} = Event {{ … }}` alias ({detail});          restating the shape inline is allowed but it must match the alias — fix one side (or delete the inline restatement to use the alias alone)"
    )]
    PayloadAliasMismatch { event: String, detail: String },

    /// L1 rung of the payload invariant: an event declares a field the effect
    /// that fires it does not emit, so every read of it is `undefined` at
    /// runtime. Raised only for the cases one file can decide on its own — a
    /// `persist create|update` naming a literal, locally-declared entity. The
    /// contract comes from `orbital_core::effects::emit_contract`, shared with
    /// the compiler's post-inline check so the two layers cannot disagree.
    #[error(
        "event {event:?} declares payload field {field:?}, but `{operator} {entity}` emits the persisted row FLAT — {entity} has no {field:?} field, so `?{field}` reads undefined at runtime. Declare the event as the bare entity (`type {event} = Event {entity}`), or drop the field."
    )]
    PayloadNotProduced {
        event: String,
        field: String,
        operator: String,
        entity: String,
    },

    /// Trait reads `@entity.<field>` in render-ui / a guard / an effect but
    /// no transition runs `(set @entity.<field> <value>)`. Per the
    /// explicit-binding rule, every `@entity.X` read must be backed by an
    /// explicit setter; otherwise the value is undefined at runtime and the
    /// component renders empty. `first_event` names the first transition
    /// where the bare read was observed (for diagnostics).
    #[error(
        "trait {trait_name:?} reads @entity.{field} (first seen on event {first_event:?}) but \
         no transition runs (set @entity.{field} <value>); per the explicit-binding rule, \
         every @entity.X must be set explicitly or it stays undefined"
    )]
    EntityFieldNeverSet {
        trait_name: String,
        field: String,
        first_event: String,
    },

    /// Cross-trait or cross-orbital `listens` block references a source
    /// trait that doesn't exist in the schema, or an event the source
    /// trait doesn't emit. Detects bus-topology drift at parse time so the
    /// authoring layer fails fast instead of producing IR that the
    /// validator rejects post-resolve. Carries the dispatching trait
    /// (`origin_orbital` / `origin_trait`), the declared source
    /// (`source_orbital` / `source_trait`), and the event name.
    #[error(
        "listens entry in {origin_orbital:?}.{origin_trait:?} references \
         source {source_orbital:?}.{source_trait:?} event {event:?} which is \
         not declared on the source trait"
    )]
    ListenSourceUnresolved {
        origin_orbital: String,
        origin_trait: String,
        source_orbital: String,
        source_trait: String,
        event: String,
    },

    /// A trait's own `config { key : T = @config.key }` declaration
    /// defaults to itself — resolving the key would recurse forever.
    /// L1-only: after lowering, a call-site rebind's same-name forward
    /// (`cols: @config.cols`, wiring the composing atom's key into a
    /// wrapper) becomes textually identical, so L2 cannot check this.
    #[error(
        "config key {key:?} in trait {trait_name:?} defaults to @config.{key} — \
         a self-referential default; use a concrete literal"
    )]
    ConfigDefaultSelfReferential { trait_name: String, key: String },

    /// Button rendered in trait A with action X targets sibling trait B
    /// (same orbital) which has a transition on X, but B has no
    /// `listens { A X -> ... }` entry. Detects the inverse of
    /// `ListenSourceUnresolved` at parse time: a dispatch path exists from
    /// producer to consumer but the consumer never subscribed.
    #[error(
        "button action {action:?} in {orbital:?}.{source_trait:?} targets trait \
         {target_trait:?} which has a matching transition but no \
         `listens {{ {source_trait} {action} -> ... }}` entry"
    )]
    ButtonTransitionUnreachable {
        orbital: String,
        source_trait: String,
        action: String,
        target_trait: String,
    },

    /// `(emit EVT {...})` literal payload omits a field declared as
    /// required on the trait's `emits { EVT { field : T! } }` block.
    /// Parse-time mirror of L2's `EmitPayloadMissingRequired` — the
    /// validator catches the same case post-resolve, but landing it at
    /// parse means the author sees the error in their editor the moment
    /// they save, not on the next `orbital validate` run.
    #[error(
        "emit {event:?} in {orbital:?}.{trait_name:?} is missing required payload field \
         {field:?} (declared as {expected_type})"
    )]
    EmitPayloadMissingRequired {
        orbital: String,
        trait_name: String,
        event: String,
        field: String,
        expected_type: String,
    },

    /// `(emit EVT { field: ... })` literal payload includes a field NOT
    /// in the trait's declared `emits { EVT { ... } }` block. Catches
    /// typos (`titel` vs `title`) and stale field names left over after
    /// schema renames.
    #[error(
        "emit {event:?} in {orbital:?}.{trait_name:?} includes payload field {field:?} \
         which isn't declared on the event's payload schema — declare the field on \
         the `emits` block or remove it from the literal"
    )]
    EmitPayloadExtraField {
        orbital: String,
        trait_name: String,
        event: String,
        field: String,
    },

    /// `(emit EVT { field: <literal> })` literal value's primitive type
    /// (number / boolean / string / object / array) doesn't match the
    /// declared `field_type` on the event's payload schema. Bindings
    /// (`@payload.X`, `@entity.X`, etc.) are skipped — those are typed by
    /// the binding-resolver and reported separately.
    #[error(
        "emit {event:?} in {orbital:?}.{trait_name:?} field {field:?} is declared as \
         {declared:?} but the literal value has type {actual:?}"
    )]
    EmitPayloadFieldTypeMismatch {
        orbital: String,
        trait_name: String,
        event: String,
        field: String,
        declared: String,
        actual: String,
    },

    /// `(set @entity.X <value>)` value's resolved type doesn't match the
    /// linked entity's declared field type for `X`. Catches the foot-gun
    /// where authors write `(set @entity.count "5")` and the runtime
    /// silently coerces a string into a number-typed field.
    ///
    /// Resolution sources covered at parse time (mirrors L2 F3):
    /// - **Literal** (number / boolean / string / object / array) →
    ///   primitive type from the SExpression node.
    /// - **`@payload.<F>`** → field type from the trait's `listens { EVENT
    ///   { F : T } }` block matching the transition's event.
    /// - **`@config.<F>`** → field type from the trait's `config { F : T }`
    ///   block.
    /// - **`@entity.<F>`** → field type from the trait's linked entity.
    /// - **SExpression `["op", ...]`** → operator's `returnType` from
    ///   canonical-operators (sharpened by F2's `returnSemantics` for
    ///   input-dependent operators).
    ///
    /// Cross-orbital bindings (`@payload.<F>` whose F is declared on a
    /// listened-from-other-orbital event) defer to the L2 validator.
    #[error(
        "(set @entity.{field}): expected {expected_type} (entity field type on {orbital:?}.{trait_name:?}'s linked entity) \
         but got {actual_type} (from {value_source})"
    )]
    SetValueTypeMismatch {
        orbital: String,
        trait_name: String,
        field: String,
        expected_type: String,
        actual_type: String,
        /// Where the actual type came from — `"literal"`, `"@payload.X"`,
        /// `"@config.X"`, `"@entity.X"`, or the operator name `"or"` / etc.
        /// Lets authors trace the mismatch back to the binding site.
        value_source: String,
    },

    /// `(set @entity.<F> "literal")` assigns a string literal that is not one
    /// of the declared enum members of F (an inline `"a" | "b"` union or a
    /// named string-enum alias). The union validated + emitted before but was
    /// never enforced, so an out-of-set value passed silently
    /// (Gap G-LOLO-ENUM-UNENFORCED). The optional `did you mean` names the
    /// closest declared member.
    #[error(
        "(set @entity.{field} {value:?}): {value:?} is not a declared member of \
         {field}'s enum [{members}]{}",
        suggestion.as_ref().map(|s| format!(" — did you mean {s:?}?")).unwrap_or_default()
    )]
    EnumValueNotMember {
        orbital: String,
        trait_name: String,
        field: String,
        value: String,
        /// Comma-joined declared members, for the diagnostic.
        members: String,
        suggestion: Option<String>,
    },

    /// Entity name collides with a sibling trait's locally-emitted
    /// state/event union type. The TS codegen emits `type
    /// <TraitName>State = '<s1>' | …` and `type <TraitName>Event = '<e1>' | …`
    /// alongside an `import type { <EntityName> } from '@app/shared';`
    /// at the top of every handler/hook/view file. When an entity is
    /// named exactly `<TraitName>State` or `<TraitName>Event` for any
    /// trait in the same orbital, the import collides with the local
    /// type declaration and tsc raises TS2440 "Import declaration
    /// conflicts with local declaration".
    ///
    /// The fix is purely a naming choice in the lolo: rename the entity
    /// (typical convention: `<TraitName>Data` instead of
    /// `<TraitName>State`). Caught here at lower time so authors see
    /// the issue in their editor before the .orb hits the validator or
    /// codegen.
    #[error(
        "entity {entity_name:?} in {orbital:?} collides with trait {trait_name:?}'s \
         generated {kind} union — rename the entity (e.g. {entity_name}Data) so the \
         TS codegen import doesn't shadow the local type declaration"
    )]
    EntityNameShadowsTraitUnion {
        orbital: String,
        entity_name: String,
        trait_name: String,
        /// "state" or "event" — which union the entity name would shadow.
        kind: String,
    },

    /// `uses`-imported trait reference `Alias.traits.X` names a trait the
    /// imported behavior doesn't declare (typo / stale name). Caught at parse
    /// time from the baked behavior-contract slice; the compiler's inline phase
    /// would otherwise fail later with an opaque ref-resolution error.
    #[error(
        "trait reference {reference:?} names trait {member:?}, but behavior {behavior:?} \
         does not declare it — check the name against `signatures.lolo`"
    )]
    UnknownTraitRef {
        reference: String,
        member: String,
        behavior: String,
    },

    /// `uses`-imported page reference `Alias.pages.X` names a page the imported
    /// behavior doesn't declare.
    #[error(
        "page reference {reference:?} names page {member:?}, but behavior {behavior:?} \
         does not declare it"
    )]
    UnknownPageRef {
        reference: String,
        member: String,
        behavior: String,
    },

    /// A rebind target entity is missing field(s) the imported trait READS
    /// (`entityContract.requires`). Pre-inline mirror of the compiler's
    /// post-inline `ORB_T_REBIND_MISSING_FIELDS`; only fires for inline-entity
    /// targets (an `extends` target's inherited fields aren't visible until
    /// inline, so those are left to the compiler backstop).
    #[error(
        "rebind target entity {entity:?} is missing field(s) {missing:?} required by trait \
         {member:?} from behavior {behavior:?} — add them to the entity or remap via fields"
    )]
    RebindMissingFields {
        entity: String,
        member: String,
        behavior: String,
        missing: Vec<String>,
    },

    /// A `config { key = value }` override on a `uses`-imported trait reference
    /// supplies a value whose type doesn't match the atom's declared config-key
    /// type (from the baked slice). Only clear scalar mismatches are flagged;
    /// bindings and array/object/SExpr-typed knobs are skipped.
    #[error(
        "config override {key:?} on {reference:?} expects type {expected:?} but the value \
         is {got:?}"
    )]
    ConfigTypeMismatch {
        reference: String,
        key: String,
        expected: String,
        got: String,
    },

    /// A `config { key = value }` override on a `uses`-imported trait reference
    /// names a key the imported atom does not declare in its `config { }` block.
    /// Silently dropping such a key (the prior behavior) hid typos and renamed
    /// knobs; this surfaces them at author time with a nearest-key suggestion.
    #[error(
        "config override {key:?} on {reference:?} is not a declared config key of the \
         imported trait{}",
        hint_suggestion(suggestion)
    )]
    UnknownConfigKey {
        reference: String,
        key: String,
        suggestion: Option<String>,
    },

    /// Increment 3-II: a trait reference's `::` annotation, once reinterpreted
    /// as call-site type-parameter arguments (`[type_name] ++ type_args`
    /// chunked into name/value pairs), has an odd total count — one name has
    /// no paired value. Structurally malformed regardless of what the
    /// referenced source trait declares, so this is caught at L1.
    #[error(
        "trait {trait_name:?}'s `::` annotation {args:?} has an odd number of identifiers — \
         reinterpreted as type-parameter call-site arguments (`:: V string R Booking`), each \
         name needs a paired value"
    )]
    OddTypeArgCount { trait_name: String, args: Vec<String> },

    /// Error from the type-system resolver. See `TypeError` for variants.
    /// Carries a span so the CLI can surface `file:line:col: …`.
    #[error("{0}")]
    Type(TypeError),
}

/// Type-system errors from the `.lolo` resolver.
///
/// Each variant carries a `Span` so the CLI can produce precise
/// `file:line:col` diagnostics. The `ORB_T_TYPE_*` code mapping lives in
/// Stage 3 (validator + compile) — at the lolo layer we only produce the
/// structured error.
#[derive(Debug, Clone, Error)]
pub enum TypeError {
    #[error("unknown type {name:?}{}", hint_suggestion(suggestion))]
    UnknownType {
        name: String,
        suggestion: Option<String>,
        span: Span,
    },

    #[error("type composition forms a cycle: {}", cycle.join(" -> "))]
    CircularType { cycle: Vec<String>, span: Span },

    #[error("type kind mismatch: expected {expected:?}, got {actual:?}")]
    KindMismatch {
        expected: TypeKindName,
        actual: TypeKindName,
        span: Span,
    },

    #[error("composition conflict: field {field:?} is declared in multiple composed entities ({})", types.join(", "))]
    CompositionConflict {
        field: String,
        types: Vec<String>,
        span: Span,
    },

    #[error("type-annotation mismatch for {type_name:?}: {what}")]
    AnnotationMismatch {
        type_name: String,
        what: String,
        span: Span,
    },

    #[error("unknown type parameter {name:?} in alias {alias:?}")]
    UnknownTypeParam {
        name: String,
        alias: String,
        span: Span,
    },

    #[error("type {alias:?} expects {expected} type argument(s), got {got}")]
    WrongTypeArgCount {
        alias: String,
        expected: usize,
        got: usize,
        span: Span,
    },

    #[error("`emits: [{name:?}]` references {name:?} which is a {actual} type, not an Event type")]
    EventTypeWrongKind {
        name: String,
        actual: TypeKindName,
        span: Span,
    },
}

/// String label for the `TypeBody` kind — used only in error messages so we
/// can talk about kind-mismatches without exporting the full `ast::TypeKind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeKindName {
    Enum,
    Struct,
    Alias,
    Entity,
    Trait,
    Page,
    Orbital,
    Event,
}

impl std::fmt::Display for TypeKindName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            TypeKindName::Enum => "enum",
            TypeKindName::Struct => "struct",
            TypeKindName::Alias => "alias",
            TypeKindName::Entity => "Entity",
            TypeKindName::Trait => "Trait",
            TypeKindName::Page => "Page",
            TypeKindName::Orbital => "Orbital",
            TypeKindName::Event => "Event",
        };
        f.write_str(s)
    }
}

impl TypeError {
    pub fn span(&self) -> Span {
        match self {
            TypeError::UnknownType { span, .. }
            | TypeError::CircularType { span, .. }
            | TypeError::KindMismatch { span, .. }
            | TypeError::CompositionConflict { span, .. }
            | TypeError::AnnotationMismatch { span, .. }
            | TypeError::UnknownTypeParam { span, .. }
            | TypeError::WrongTypeArgCount { span, .. }
            | TypeError::EventTypeWrongKind { span, .. } => *span,
        }
    }
}

/// True when a rendered token debug string is `Identifier("NAME")` with NAME
/// in ALL_CAPS — the shape of an event name heading a transition arm.
fn looks_like_event_identifier(got: &str) -> bool {
    let Some(start) = got.find("Identifier(\"") else {
        return false;
    };
    let name = &got[start + "Identifier(\"".len()..];
    let Some(end) = name.find('"') else {
        return false;
    };
    let name = &name[..end];
    !name.is_empty()
        && name.chars().any(|c| c.is_ascii_uppercase())
        && name
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

fn hint_suggestion(s: &Option<String>) -> String {
    match s {
        Some(suggestion) => format!(" — did you mean {suggestion:?}?"),
        None => String::new(),
    }
}

impl LowerError {
    /// Span on lowering errors — available for the Type variant only; older
    /// variants predate span tracking and return a dummy span.
    pub fn span(&self) -> Option<Span> {
        match self {
            LowerError::Type(te) => Some(te.span()),
            _ => None,
        }
    }
}

/// Top-level error returned from `parse_and_lower`.
#[derive(Debug, Clone, Error)]
pub enum LoloError {
    #[error("io error reading {path}: {message}")]
    Io { path: String, message: String },

    #[error("{path}: {inner}")]
    Parse { path: String, inner: ParseError },

    #[error("{path}: {inner}")]
    Lower { path: String, inner: LowerError },
}

impl LoloError {
    /// Render as `path:line:col: message` for CLI output.
    pub fn render(&self) -> String {
        match self {
            LoloError::Io { path, message } => format!("{path}: {message}"),
            LoloError::Parse { path, inner } => {
                let span = inner.span();
                format!("{path}:{}:{}: {inner}", span.line, span.col)
            }
            LoloError::Lower { path, inner } => match inner.span() {
                Some(span) => format!("{path}:{}:{}: {inner}", span.line, span.col),
                None => format!("{path}: {inner}"),
            },
        }
    }

    /// Teaching hint for CLI/editor surfaces. Delegates to the inner parse
    /// error's `hint()`; lowering errors carry no hint table yet.
    pub fn hint(&self) -> (Option<String>, Option<String>) {
        match self {
            LoloError::Parse { inner, .. } => inner.hint(),
            LoloError::Io { .. } | LoloError::Lower { .. } => (None, None),
        }
    }

    /// [`Self::render`] plus the 💡 suggestion / → fix-guidance lines when
    /// `hint()` has something to say.
    pub fn render_with_hint(&self) -> String {
        let mut out = self.render();
        let (suggestion, fix) = self.hint();
        if let Some(s) = suggestion {
            out.push_str(&format!("\n  \u{1f4a1} {s}"));
        }
        if let Some(f) = fix {
            out.push_str(&format!("\n  \u{2192} {f}"));
        }
        out
    }
}
