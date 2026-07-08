//! AST types for the .lolo language.
//!
//! This is the parser's output and the lowering pass's input. The shape
//! mirrors the v3 spec in `docs/Lolo_Language_Spec_v3.md` Section "Top-level
//! structure" and the EBNF grammar at the bottom of that document.

use orbital_core::schema::SExpression;

/// Source span for error reporting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Span {
    pub line: u32,
    pub col: u32,
    pub len: u16,
}

impl Span {
    pub fn new(line: u32, col: u32, len: u16) -> Self {
        Self { line, col, len }
    }

    pub fn dummy() -> Self {
        Self::default()
    }
}

/// A source comment (`;;`, `#`, or `#= … =#`), captured by the lexer as trivia
/// so the formatter can re-emit it. `line` is the 1-based source line; `text` is
/// the verbatim comment text starting at its marker (leading indentation is
/// dropped — the formatter re-indents).
#[derive(Debug, Clone)]
pub struct Comment {
    pub line: u32,
    pub text: String,
}

/// Top-level program. One `.lolo` file is one `Program`.
#[derive(Debug, Clone)]
pub struct Program {
    /// `app NAME [vVERSION]` line.
    pub app: AppHeader,
    /// Top-level type aliases (scoped to the whole file).
    pub type_aliases: Vec<TypeAlias>,
    /// Orbital declarations.
    pub orbitals: Vec<Orbital>,
    /// Source comments (trivia), in source order. Re-emitted by the formatter,
    /// interleaved by source line; ignored by lowering.
    pub comments: Vec<Comment>,
}

/// `app NAME vVERSION` header.
#[derive(Debug, Clone)]
pub struct AppHeader {
    pub name: String,
    pub version: Option<String>,
    pub description: Option<String>,
    pub span: Span,
}

/// `type Name [T] = ...` declaration. The optional `type_param` is the single
/// entity-type variable used in parameterized shapes (`type Browsable T = ...`);
/// it's bound when the alias is applied via `::` annotation.
#[derive(Debug, Clone)]
pub struct TypeAlias {
    pub name: String,
    /// Single-ident type parameter (the `T` in `type Browsable T = ...`).
    /// `None` for non-parameterized aliases.
    pub type_param: Option<String>,
    pub body: TypeBody,
    pub span: Span,
}

/// Body of a type alias. In addition to the legacy sum/struct/alias forms,
/// aliases can name composable shapes: `Entity`, `Trait`, `Page`, `Orbital`.
/// Composition via `&` merges two bodies of the same kind.
#[derive(Debug, Clone)]
pub enum TypeBody {
    /// `type Status = a | b | c` enum form.
    Enum(Vec<String>),
    /// `type Coord = { lat: float, lng: float! }` struct form. Each entry
    /// is `(field_name, type, required)` — `required` mirrors the optional
    /// `!` marker after the field's type (`name : T` → false, `name : T!`
    /// → true). Carrying it through the AST instead of dropping it (per
    /// the old `Vec<(String, TypeExpr)>` shape) is what lets the codegen
    /// emit lolo struct fields as optional-by-default in TS, matching
    /// lolo's authoring semantics.
    Struct(Vec<(String, TypeExpr, bool)>),
    /// `type Money = number` nominal alias form.
    Alias(TypeExpr),
    /// `type Patient = Entity [persistent: patients] { id : string! ... }`.
    Entity {
        persistence: Option<Persistence>,
        collection: Option<String>,
        fields: Vec<EntityField>,
    },
    /// `type Browsable T = Trait Browse.traits.BrowseItemBrowse -> T`.
    /// `linked_entity_param` is the name of the type parameter that binds to
    /// the trait's linkedEntity slot (e.g. `T`). Nominal references to named
    /// entities can instead be expressed via `TypeExpr::Named` in
    /// `linked_entity`.
    Trait {
        upstream: Option<String>,
        linked_entity: Option<TypeExpr>,
    },
    /// `type ListPage T = Page -> Trait[Browsable T]`.
    Page {
        path: Option<String>,
        trait_types: Vec<TypeExpr>,
    },
    /// `type CrudOrbital T = Orbital { entity: T; traits: ...; page: ... }`.
    Orbital {
        entity: Option<TypeExpr>,
        trait_types: Vec<TypeExpr>,
        page_type: Option<TypeExpr>,
    },
    /// `type BrowseItemLoaded = Event BrowseItem` or
    /// `type OrderShipped = Event OrderShipment [external]` or
    /// `type Err = Event { message: string }` or
    /// `type XLoaded = Event X "Fired when X finishes loading"`.
    /// The payload type can be any TypeExpr: a named entity / type alias, an
    /// anonymous struct body, or a primitive. The optional bracket modifier
    /// sets the event's cross-orbital scope; unmarked defaults to `internal`.
    /// The optional trailing string literal sets the event description; it
    /// flows through to `EmitDefinition.description` on every emit reference.
    /// Lowers to one `EmitDefinition { event, scope, description, payload }`
    /// per reference in a trait's `emits: [...]` list.
    Event {
        payload: TypeExpr,
        scope: Option<EventScopeAst>,
        description: Option<String>,
    },
    /// `A & B & C` composition. Resolver flattens, checks kind compatibility,
    /// merges fields / traits / pages depending on the kind of the members.
    Composition(Vec<TypeBody>),
}

/// Event scope marker parsed from `[internal]` / `[external]` after an
/// `Event ...` type body. `None` in the outer `Option<EventScopeAst>` means
/// the author did not annotate; the lowerer defaults to Internal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventScopeAst {
    Internal,
    External,
}

/// Kind tag used by the type resolver for composition-compatibility checks.
/// Not parsed from source — derived from the `TypeBody` variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeKind {
    Enum,
    Struct,
    Alias,
    Entity,
    Trait,
    Page,
    Orbital,
    Event,
}

/// `Name :: TypeName [Arg1 Arg2 ...]` annotation above a declaration.
///
/// Applied to entity / trait / page / orbital blocks. The parser attaches this
/// to the corresponding AST node; the type resolver checks the body against
/// the named type (with `type_args` substituted for the alias's type parameters).
#[derive(Debug, Clone)]
pub struct TypeAnnotation {
    /// Dotted name of the type alias to apply (e.g. `"Auditable"` or
    /// `"Lib.CrudOrbital"` once cross-file resolution is wired).
    pub type_name: String,
    /// Positional type arguments. Each is a single identifier that resolves
    /// to another type alias or entity name.
    pub type_args: Vec<String>,
    pub span: Span,
}

/// `orbital Name [:: TypeAnnotation] { ... }` declaration.
#[derive(Debug, Clone)]
pub struct Orbital {
    pub name: String,
    /// Optional `::` type annotation above the orbital declaration.
    pub type_annotation: Option<TypeAnnotation>,
    pub uses: Vec<UseDecl>,
    pub type_aliases: Vec<TypeAlias>,
    pub entity: Option<Entity>,
    pub traits: Vec<Trait>,
    pub pages: Vec<Page>,
    /// Free-floating s-expression body items at the orbital level (e.g.,
    /// `(behavior/compose { ... })`). Lowered to top-level effects on the
    /// orbital that the Compose pass will resolve.
    pub body: Vec<SExpression>,
    pub span: Span,
}

/// `uses Alias from "path"`.
#[derive(Debug, Clone)]
pub struct UseDecl {
    pub alias: String,
    pub path: String,
    pub span: Span,
}

/// `entity Name [:: TypeAnnotation] [persistence] { fields... }`.
#[derive(Debug, Clone)]
pub struct Entity {
    pub name: String,
    /// Optional `::` type annotation above the entity declaration.
    pub type_annotation: Option<TypeAnnotation>,
    pub persistence: Option<Persistence>,
    pub collection: Option<String>,
    /// `shared` header tag — orthogonal to persistence: entity state shared
    /// across all bound traits vs copied per-trait.
    pub shared: bool,
    pub fields: Vec<EntityField>,
    pub span: Span,
}

/// Persistence tag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Persistence {
    Persistent,
    Runtime,
}

/// One field on an entity.
#[derive(Debug, Clone)]
pub struct EntityField {
    pub name: String,
    pub type_expr: TypeExpr,
    /// `!` modifier on the type.
    pub required: bool,
    /// `?` modifier on the type.
    pub nullable: bool,
    /// Trailing `@key` annotation.
    pub is_key: bool,
    /// Trailing `@unique` annotation.
    pub is_unique: bool,
    /// Trailing `@intrinsic` annotation. Marks the field as machinery/view-state
    /// the owning trait computes internally; it travels WITH the trait on a
    /// `linkedEntity` rebind and is NOT required to pre-exist on the target.
    pub is_intrinsic: bool,
    /// Trailing `@description "..."` annotation — human/semantic description of
    /// the field. Authoring/build-time metadata (factory-signature catalog,
    /// embeddings, curation remap); the runtime ignores it.
    pub description: Option<String>,
    /// Trailing `@synonyms "..."` annotation — user-vocabulary synonyms (free
    /// text) feeding catalog search / curation field-matching.
    pub synonyms: Option<String>,
    /// Default value `= sexpr`.
    pub default: Option<SExpression>,
    /// `derived` field flag with the derivation expression.
    pub derived: Option<SExpression>,
    pub span: Span,
}

/// Type expression in field declarations.
#[derive(Debug, Clone)]
pub enum TypeExpr {
    /// Primitive type (`string`, `int`, `bool`, ...).
    Primitive(String),
    /// User-defined named type (e.g. `Status`, `Order`).
    Named(String),
    /// String-literal singleton type (`"active"`). Only meaningful inside a
    /// `Union` that spells out a finite set of strings — the lowerer detects
    /// such unions and emits them as `"type": "string"` + `"values": [...]`
    /// on the field, matching the committed `.orb` shape.
    StringLiteral(String),
    /// `[T]` array type.
    Array(Box<TypeExpr>),
    /// `Map K V` dynamic-key map type — a uniform-value dictionary with
    /// arbitrary keys (e.g. `Map string SimulationParameter`). Reserved
    /// built-in binary constructor applied by juxtaposition, mirroring
    /// Haskell's `Map k v`. Lowers to `Record<K, V>` in codegen. Distinct from
    /// the fixed-key `Object` struct: keys aren't known at author time.
    Map(Box<TypeExpr>, Box<TypeExpr>),
    /// `{ k: T, ... }` / `{ k: T! }` inline object type. Each entry is
    /// `(field_name, type, required)`; the optional `!` marker tracks
    /// whether the field is required so the lowerer can emit the matching
    /// `required` flag on the resulting `FieldDefinition`.
    Object(Vec<(String, TypeExpr, bool)>),
    /// `T | U` union (lowering inlines this to enum values when all members
    /// are bare names or string literals).
    Union(Vec<TypeExpr>),
    /// `T?` nullable.
    Optional(Box<TypeExpr>),
    /// `T*` one-to-many relation.
    RelationMany(String),
    /// `T+` one-or-more relation.
    RelationManyNonEmpty(String),
    /// `trait` — value is a `@trait.X` reference. Substituted to embedded
    /// trait UI at render time. Allowed only in `config { ... }` blocks.
    Trait,
    /// `slot` — value is a UI slot name. Validated against `VALID_SLOTS`.
    Slot,
    /// `pattern` — value is a pattern type name. Validated against `PATTERN_TYPES`.
    Pattern,
    /// `SExpr` — value is a compiled s-expression (the `SExpr` brand from
    /// `@almadar/core`). Used for config fields that hold an s-expression
    /// (e.g. a `conditionalFields` visibility condition). Emits the recursive
    /// `SExpr` TypeScript type (inlined as a local alias in codegen, like
    /// `TraitFieldRef`). Allowed only in `config { ... }` blocks.
    SExpr,
}

/// `trait Name [:: TypeAnnotation] -> Entity [category] { ... }` or
/// `trait Name [:: TypeAnnotation] = Alias.traits.Upstream -> Entity { ... }` (reference form).
/// Instance vs collection scope, parsed from the trait modifier list
/// (`[interaction, instance]` / `[interaction, collection]`). Optional in this
/// phase; unmarked traits pass. The type-checker will require one of these in
/// a later phase (see Almadar_Entity_V2_Plan.md Phase 4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TraitScope {
    Instance,
    Collection,
}

#[derive(Debug, Clone)]
pub struct Trait {
    pub name: String,
    /// Optional `::` type annotation above the trait declaration.
    pub type_annotation: Option<TypeAnnotation>,
    pub linked_entity: Option<String>,
    /// Set when the binding was authored as `-> @rebindable Entity`. Opts
    /// the trait into consumer entity-rebinding via `linkedEntity`. Only
    /// meaningful on inline trait definitions (atoms); a reference's `->`
    /// IS the rebind, so this is always false there.
    pub entity_rebindable: bool,
    /// `@description "..."` annotation on the rebindable binding (body-level).
    /// Surfaced to the catalog/knob-embeddings so the LLM can discover the
    /// binding from intent vocabulary. Only emitted when `entity_rebindable`.
    pub entity_binding_description: Option<String>,
    /// `@synonyms "..."` annotation on the rebindable binding (body-level).
    pub entity_binding_synonyms: Option<String>,
    pub category: Option<String>,
    /// Instance or collection scope marker parsed from the modifier list.
    /// `None` today means the trait opted out / predates the modifier. Phase 4
    /// of the V2 migration turns `None` into a warning.
    pub scope: Option<TraitScope>,
    /// Author-supplied capability tags parsed from the trait header's bracket
    /// list. Anything beyond the scope marker (`instance`/`collection`) and
    /// the first category identifier collects here. Free-form strings — the
    /// parser doesn't validate against a known set. Source for
    /// `OirTrait.capabilities` and downstream `FactorySignature` matching.
    /// See `docs/Almadar_Domain_Language.md` Phase 3.
    pub capabilities: Vec<String>,
    pub initial_state: Option<String>,
    pub states: Vec<State>,
    pub emits: Vec<Emit>,
    pub listens: Vec<Listen>,
    pub ticks: Vec<Tick>,
    pub config: Vec<ConfigField>,
    pub span: Span,
    /// When `Some`, this trait is a reference to an imported trait, not an
    /// inline definition. The reference data carries the dotted path
    /// (`Alias.traits.X`) plus per-event override maps. `states`,
    /// `initial_state`, `ticks`, and `config` MUST be empty when this is set;
    /// the upstream atom owns state-machine topology. `linked_entity`,
    /// `listens`, and `emits` may still be set: they translate into the
    /// reference's `linkedEntity`, `listens`, and (via `emits_scope`)
    /// `emitsScope` overrides.
    pub reference: Option<TraitRefData>,
}

/// Parsed body of a trait-reference call site — the `{ events {} fields {}
/// config {} listens {} emitsScope {} }` shared between the `trait Name =
/// Alias.traits.X { … }` declaration form and the inline `@(Alias.traits.X {
/// … })` sugar. Produced by `Parser::parse_trait_ref_body`.
#[derive(Debug, Clone, Default)]
pub struct TraitRefBody {
    pub events: Vec<(String, String)>,
    pub fields: Vec<(String, String)>,
    pub config: Vec<ConfigField>,
    pub listens: Vec<Listen>,
    pub emits_scope: Option<String>,
}

/// Override payload for a `trait Name = Alias.traits.Upstream { ... }` form.
#[derive(Debug, Clone, Default)]
pub struct TraitRefData {
    /// Dotted path: e.g. `"Modal.traits.ModalRecordModal"`.
    pub reference: String,
    /// Event-key renames in source order: `[("OPEN", "ADD_ITEM"), ...]`.
    /// Lowered to `events: { OPEN: "ADD_ITEM" }` on the trait reference.
    pub events: Vec<(String, String)>,
    /// Entity-field renames in source order: `[("title", "dishName"), ...]`.
    /// Lowered to `fields: { title: "dishName" }` on the trait reference. The
    /// inline phase rewrites `@entity.title` reads to `@entity.dishName`.
    pub fields: Vec<(String, String)>,
    /// Caller-supplied `config: { param: value, ... }` values at the
    /// trait-ref call site. Inline phase substitutes `@config.<param>` in the
    /// imported trait's effects/guards with these values. May carry full
    /// annotations (`@label`/`@description`/`@synonyms`/`@tier`) when the
    /// organism explicitly re-documents a knob. Lowered to `config: { ... }`
    /// on the trait reference.
    pub config: Vec<ConfigField>,
    /// `emitsScope external|internal` override. Lowered to `emitsScope` on
    /// the trait reference.
    pub emits_scope: Option<String>,
}

/// `state name [for duration] { transitions... }`.
#[derive(Debug, Clone)]
pub struct State {
    pub name: String,
    /// Duration in milliseconds for `state name for 30s` form.
    pub time_bound: Option<u64>,
    /// All transitions out of this state, in source order.
    pub transitions: Vec<Transition>,
    pub span: Span,
}

/// `EVENT -> target [when (guard)] { effects... }`.
#[derive(Debug, Clone)]
pub struct Transition {
    pub event: String,
    pub target: String,
    pub guard: Option<SExpression>,
    pub effects: Vec<SExpression>,
    pub span: Span,
}

/// One entry in an `emits { }` block.
///
/// Two surface shapes (post-Phase 10):
///   `EVENT [-> internal|external] [{ fields }]`
///
/// When `payload` is empty, the lower step resolves `event` against the
/// orbital-scope `type EVENT = Event { ... }` alias registry and uses the
/// alias's fields. Bare names with no matching alias produce a payloadless
/// emit definition.
#[derive(Debug, Clone)]
pub struct Emit {
    pub event: String,
    pub scope: Option<String>,
    pub payload: Vec<EventField>,
    /// Semantic annotations (trailing `@description`/`@synonyms`/`@tier`), mirror
    /// of the entity-field + config-knob annotation layer. Feed the event matcher.
    pub description: Option<String>,
    pub synonyms: Option<String>,
    pub tier: Option<String>,
    pub span: Span,
}

/// One entry in a `listens { }` block.
///
/// Two shapes (post-Phase 10):
///
/// - **Cross-orbital**: `Source.EVENT -> TRIGGER [when (...)] [with { ... }]`.
///   `event` is `"Source.EVENT"`, `triggers` names the local event to fire.
///   Source and event are dot-separated.
///
/// - **Local (incoming trigger metadata)**: `EVENT [{ fields }]`. No source,
///   no `->`. `event` is the bare key. When `payload` is empty, the lower
///   step resolves against the orbital-scope alias registry (same rule as
///   `Emit`).
#[derive(Debug, Clone)]
pub struct Listen {
    /// Source orbital and event in `Source.EVENT` form for the cross-orbital
    /// shape; bare key (`"REQUEST"`) for the local shape.
    pub event: String,
    pub triggers: Option<String>,
    pub guard: Option<SExpression>,
    pub payload_mapping: Vec<(String, SExpression)>,
    /// Inline payload field list for the local shape:
    /// `REQUEST { id : string!, foo : string }`. Empty for cross-orbital or
    /// payloadless locals.
    pub payload: Vec<EventField>,
    /// Semantic annotations (trailing `@description`/`@synonyms`/`@tier`).
    pub description: Option<String>,
    pub synonyms: Option<String>,
    pub tier: Option<String>,
    pub span: Span,
}

/// One entry in a `ticks { }` block.
#[derive(Debug, Clone)]
pub struct Tick {
    pub name: String,
    /// Interval in milliseconds (or original cron string when not numeric).
    pub interval: TickInterval,
    pub guard: Option<SExpression>,
    pub effects: Vec<SExpression>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum TickInterval {
    Millis(u64),
    Cron(String),
}

/// One entry in a `config { }` block.
#[derive(Debug, Clone)]
pub struct ConfigField {
    pub name: String,
    pub type_expr: TypeExpr,
    pub default: Option<SExpression>,
    /// Human-readable question prompt — `@label "..."` annotation on
    /// the line below the field declaration. Threads through to the
    /// .orb config block so the studio questionnaire can render it
    /// without inventing labels.
    pub label: Option<String>,
    /// Long-form help text — `@description "..."` annotation on the
    /// line below the field declaration.
    pub description: Option<String>,
    /// User-vocabulary synonyms — `@synonyms "..."` annotation on
    /// the line below the field declaration. Comma-separated free
    /// text that downstream consumers (the agent's catalog summary,
    /// the knob-embeddings bake) use to widen vocabulary recall for
    /// user prompts. e.g. `@synonyms "taller, shorter, pixel height"`
    /// on a `height` field lets the LLM connect "make it taller" to
    /// the `height` knob without inferring cold.
    pub synonyms: Option<String>,
    /// Decision-kind tier — `@tier "..."` annotation on the line below
    /// the field declaration. One of `domain`, `presentation`,
    /// `internal`. Drives the studio Questionnaire's filter +
    /// disclosure: `internal` knobs are hidden entirely, `presentation`
    /// knobs collapse under the "Polish wording" panel, `domain` knobs
    /// render inline. Untagged knobs are treated as `presentation` on
    /// the consumer side (safe default — most un-audited knobs are
    /// presentation polish). The L2 validator
    /// (`ORB_T_CONFIG_TIER_INVALID`) rejects any other value.
    pub tier: Option<String>,
    pub span: Span,
}

/// One field in an event payload schema.
#[derive(Debug, Clone)]
pub struct EventField {
    pub name: String,
    pub type_expr: TypeExpr,
    pub required: bool,
    pub span: Span,
}

/// `page /path [:: TypeAnnotation] -> Trait [, Trait...] [options]` or
/// `page /path [:: TypeAnnotation] = Alias.pages.Upstream [-> Trait [, Trait...]]` (reference form).
#[derive(Debug, Clone)]
pub struct Page {
    pub path: String,
    /// Optional `::` type annotation above the page declaration.
    pub type_annotation: Option<TypeAnnotation>,
    pub traits: Vec<String>,
    pub view: Option<String>,
    pub primary_entity: Option<String>,
    pub is_initial: bool,
    pub span: Span,
    /// When `Some`, this page is a reference to an imported page. The string
    /// is the dotted path (`Alias.pages.PageName`). When set, `traits` and
    /// `primary_entity` (if present) act as overrides on the imported page.
    pub reference: Option<String>,
    /// Explicit page name override from `page "/path" as PageName -> ...`.
    /// When `Some`, the lowerer uses it as the .orb `name` field verbatim;
    /// when `None`, a name is derived from the path. Explicit naming avoids
    /// fragile path-to-name heuristics — every committed .orb was authored
    /// with a hand-picked name that no single derivation rule reproduces.
    pub name: Option<String>,
}

/// Top-level AST returned by the parser. The compiler-facing entry point is
/// `lower_to_orb` which takes one of these.
pub type Ast = Program;
