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

    #[error("unexpected end of input: expected {expected}")]
    UnexpectedEof { expected: String },

    #[error("unknown keyword {keyword:?} at {}:{}", span.line, span.col)]
    UnknownKeyword { keyword: String, span: Span },

    #[error("invalid type expression at {}:{}: {message}", span.line, span.col)]
    InvalidType { message: String, span: Span },

    #[error("invalid s-expression at {}:{}: {message}", span.line, span.col)]
    InvalidSExpr { message: String, span: Span },

    #[error("invalid duration {value:?} at {}:{}", span.line, span.col)]
    InvalidDuration { value: String, span: Span },

    #[error("invalid sigil {value:?} at {}:{}", span.line, span.col)]
    InvalidSigil { value: String, span: Span },

    #[error("expected identifier at {}:{}", span.line, span.col)]
    ExpectedIdentifier { span: Span },
}

impl ParseError {
    pub fn span(&self) -> Span {
        match self {
            ParseError::Lex(e) => e.span(),
            ParseError::UnexpectedToken { span, .. }
            | ParseError::UnknownKeyword { span, .. }
            | ParseError::InvalidType { span, .. }
            | ParseError::InvalidSExpr { span, .. }
            | ParseError::InvalidDuration { span, .. }
            | ParseError::InvalidSigil { span, .. }
            | ParseError::ExpectedIdentifier { span } => *span,
            ParseError::UnexpectedEof { .. } => Span::new(0, 0, 0),
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

    /// `(navigate "/path")` effect references a route that no orbital
    /// declares as a `pages[].path`. Catches typos and dangling links at
    /// parse time before they show up as silent client-side router
    /// no-ops at runtime (gap #19.A's author-side detection in
    /// `docs/Almadar_Std_Verification.md`).
    ///
    /// Path matching is canonicalised: `:foo` placeholders on the page
    /// side and `@payload.X` / `@entity.X` interpolations on the navigate
    /// side both reduce to `<seg>` so the structural shape compares
    /// 1:1. Static segments must match verbatim.
    #[error(
        "navigate target {path:?} in {orbital:?}.{trait_name:?} doesn't resolve to any \
         declared `pages[].path` — declare a matching page or correct the path"
    )]
    NavigateTargetUnreachable {
        orbital: String,
        trait_name: String,
        path: String,
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
}
