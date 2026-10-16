//! AST types for the .lolo language.
//!
//! This is the parser's output and the lowering pass's input. The shape
//! mirrors the v3 spec in `docs/Lolo_Language_Spec_v3.md` Section "Top-level
//! structure" and the EBNF grammar at the bottom of that document.

use orbital_core_base::schema::SExpression;

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
    /// File-scoped `uses Alias from "path"` declarations — legal ABOVE any
    /// `orbital` block (orbital-import Stage A). Lowering merges these into
    /// every orbital's own `uses` (local alias wins on collision), which is
    /// what lets a top-level `orbital X = Alias.orbitals.Y` reference resolve
    /// `Alias` through the same per-orbital `ctx` loop the compiler already
    /// runs — no change needed there. Distinct from `Orbital.uses`, which
    /// stays legal too (per-orbital, unchanged).
    pub uses: Vec<UseDecl>,
    /// Orbital declarations.
    pub orbitals: Vec<Orbital>,
    /// Source comments (trivia), in source order. Re-emitted by the formatter,
    /// interleaved by source line; ignored by lowering.
    pub comments: Vec<Comment>,
}

/// `app NAME "VERSION" [ modifiers ]` header.
#[derive(Debug, Clone)]
pub struct AppHeader {
    pub name: String,
    pub version: Option<String>,
    pub description: Option<String>,
    /// `theme "<key>"` on the line after the header — the APP-level skin
    /// every orbital's `@currentTheme` seeds from unless the orbital
    /// declares its own `theme`. A full `<base>-<light|dark>` registry key,
    /// or an inline `theme { … }` token definition.
    pub theme: Option<ThemeDecl>,
    /// `locales [en, ar, sl]` — the locales this program ships.
    pub locales: Vec<String>,
    /// `egress: ["https://host", …]` — the origins `http` may reach.
    pub egress: Vec<String>,
    /// `public: "./public"` — a folder copied verbatim to the built site's root.
    pub public: Option<String>,
    /// `origin:` / `icon:` / `siteName:` / `signIn:` site modifiers.
    pub site: orbital_core_base::schema::SiteMeta,
    /// The modifiers were written as loose lines after `app` (the pre-bracket
    /// form) — still parsed, reported as `ORB_L_APP_HEADER_LEGACY`.
    pub legacy_lines: bool,
    /// App-level `config { ... }` block, declared right after the header
    /// (orbital-import Stage A, `OrbitalSchema.config`) — ORGANISM-wide
    /// knobs every orbital's traits may forward with `@config.<knob>`.
    pub config: Vec<ConfigField>,
    pub span: Span,
}

/// A `theme` declaration — either a registry-key reference
/// (`theme "<key>"`, lowers to `ThemeRef::Reference`) or an inline token
/// definition (`theme { name: "…" tokens { … } variants { … } }`, lowers to
/// `ThemeRef::Inline`).
#[derive(Debug, Clone, PartialEq)]
pub enum ThemeDecl {
    Reference(String),
    Inline(InlineTheme),
}

/// Inline theme definition — the `.lolo` surface of `ThemeDefinition`.
#[derive(Debug, Clone, PartialEq)]
pub struct InlineTheme {
    pub name: String,
    pub display_name: Option<String>,
    pub tokens: ThemeTokenMaps,
    pub variants: Vec<(String, ThemeTokenMaps)>,
}

/// The theme token categories; entries keep source order. The five legacy
/// maps (`colors`/`radii`/`spacing`/`typography`/`shadows`) are free-form;
/// the typed axes `typeScale`/`geometry`/`elevation`/`surface` take only
/// their flat string fields (camelCase keys, see [`THEME_TYPED_AXIS_KEYS`]).
/// Nested typed fields (type scale/intents) and density/motion/iconography
/// stay `.orb`-JSON-only.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ThemeTokenMaps {
    pub colors: Vec<(String, String)>,
    pub radii: Vec<(String, String)>,
    pub spacing: Vec<(String, String)>,
    pub typography: Vec<(String, String)>,
    pub shadows: Vec<(String, String)>,
    pub type_scale: Vec<(String, String)>,
    pub geometry: Vec<(String, String)>,
    pub elevation: Vec<(String, String)>,
    pub surface: Vec<(String, String)>,
    pub motion: Vec<(String, String)>,
    pub diagram: Vec<(String, String)>,
    pub scene: Vec<(String, String)>,
}

/// The `.lolo`-authorable flat string keys of each typed theme axis — the
/// camelCase wire names of the matching `orbital_core::schema::*Tokens` fields.
pub const THEME_TYPED_AXIS_KEYS: [(&str, &[&str]); 7] = [
    (
        "typeScale",
        &[
            "displayFamily",
            "bodyFamily",
            "monoFamily",
            "fontImport",
            "headingWeight",
            "headingTransform",
            "headingTracking",
            "headingStyle",
            "headingShadow",
        ],
    ),
    (
        "geometry",
        &[
            "radiusContainer",
            "radiusInteractive",
            "radiusPill",
            "borderHairline",
            "borderStandard",
            "borderHeavy",
            "cornerShape",
            "cornerShapePill",
            "cornerShapeInteractive",
            "borderStyle",
            "borderStyleInteractive",
        ],
    ),
    (
        "elevation",
        &[
            "cardElevation",
            "popoverElevation",
            "dialogElevation",
            "toastElevation",
            "interactiveElevation",
            "pressedElevation",
        ],
    ),
    (
        "surface",
        &[
            "backdrop",
            "cardImage",
            "pageImage",
            "pageImageSize",
            "diagram",
            "edgeMask",
            "edgeHeight",
            "textureMask",
            "textureSize",
            "accentImage",
            "scrim",
        ],
    ),
    (
        "diagram",
        &[
            "strokeThin",
            "strokeNormal",
            "strokeBold",
            "lineCap",
            "lineJoin",
            "dash",
            "dot",
            "fillStyle",
            "fillOpacity",
            "roughness",
            "glow",
            "shadow",
            "marker",
            "labelFont",
            "labelSize",
            "labelWeight",
            "labelCase",
            "corner",
        ],
    ),
    (
        "scene",
        &[
            "materialRoughness",
            "materialMetalness",
            "materialFlat",
            "materialOutline",
            "lightAmbient",
            "lightKey",
            "lightKeyColor",
            "fog",
        ],
    ),
    // `motion` keys are the motion-shape fields; they lower into `motion.shapes`.
    (
        "motion",
        &[
            "modalEnter",
            "modalExit",
            "popoverEnter",
            "popoverExit",
            "toastEnter",
            "toastExit",
            "pageEnter",
        ],
    ),
];

/// Typed theme keys whose value is one of a closed set — checked at parse time so a
/// misspelled value is an error rather than an axis that silently fails to lower.
pub const THEME_TYPED_AXIS_VALUES: [(&str, &str, &[&str]); 6] = [
    ("diagram", "lineCap", &["butt", "round", "square"]),
    ("diagram", "lineJoin", &["miter", "round", "bevel"]),
    (
        "diagram",
        "fillStyle",
        &["solid", "tint", "outline", "hatch"],
    ),
    ("diagram", "marker", &["triangle", "open", "line", "dot"]),
    ("diagram", "labelFont", &["body", "display", "mono"]),
    ("diagram", "labelCase", &["none", "uppercase"]),
];

/// `type Name [T] = ...` declaration. The optional `type_param` is the single
/// entity-type variable used in parameterized shapes (`type Browsable T = ...`);
/// it's bound when the alias is applied via `::` annotation.
#[derive(Debug, Clone)]
pub struct TypeAlias {
    pub name: String,
    /// Single-ident type parameter — either the bare form (`T` in
    /// `type Browsable T = ...`) or the name half of the parenthesized
    /// kinded form (`E` in `type Foo (E : Entity) = ...`). `None` for
    /// non-parameterized aliases.
    pub type_param: Option<String>,
    /// The kind half of a parenthesized kinded type parameter (`Entity` in
    /// `type Foo (E : Entity) = ...`) — one of `Type | Entity | Trait | Page
    /// | Orbital | Event`. `None` for the bare form or when there's no
    /// parameter at all. Only `Entity` is acted on by the compiler today
    /// (increment 3-I: `(E : Entity)` on an `Event` alias monomorphizes to
    /// the using trait's bound entity); other kinds are stored but inert.
    pub type_param_kind: Option<String>,
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
    /// `expects ...` declarations — consumer-side requirements on sibling
    /// orbitals (docs/Almadar_LOLO_Expects_Proposal.md §3).
    pub expects: Vec<ExpectsDecl>,
    pub type_aliases: Vec<TypeAlias>,
    pub entity: Option<Entity>,
    /// Entity declarations beyond the first. The first `entity` block is the
    /// orbital's primary entity; later blocks lower into the IR's
    /// `auxiliary_entities` (previously the parser silently last-wins
    /// REPLACED the primary — C-VALIDATE-MULTI-ENTITY-ORBITAL).
    pub extra_entities: Vec<Entity>,
    pub traits: Vec<Trait>,
    pub pages: Vec<Page>,
    /// `theme "<name>"` / `theme { … }` — the orbital's active skin: a
    /// reference to a theme in the built-in `@almadar-ui/themes/` registry,
    /// or an inline token definition carried by the orbital itself.
    /// `@currentTheme` resolves to this at render time. `None` ⇒ no skin
    /// declared (the host omits `data-theme`, or the assembler sets it via
    /// the factory `theme` param).
    pub theme: Option<ThemeDecl>,
    /// Free-floating s-expression body items at the orbital level (e.g.,
    /// `(behavior/compose { ... })`). Lowered to top-level effects on the
    /// orbital that the Compose pass will resolve.
    pub body: Vec<SExpression>,
    /// `orbital Name = Alias.orbitals.Y { ... }` — the dotted reference path
    /// (`"Alias.orbitals.Y"`) when this orbital is a REFERENCE to an
    /// imported orbital, not an inline definition (orbital-import Stage A,
    /// mirrors `Page.reference`/`Trait.reference`). `entity`/`extra_entities`/
    /// `pages` MUST be empty when this is `Some` — the upstream organism owns
    /// them; the fields below are the override surface. `traits` (Stage B)
    /// holds SIBLING trait declarations authored in the reference body — the
    /// upstream's own traits arrive at inline, never here.
    pub reference: Option<String>,
    /// `entity NAME` override body — rebinds the imported orbital's primary
    /// entity, exactly as `-> Entity` does on a trait reference.
    pub entity_rebind: Option<String>,
    /// The entity-header bracket after `entity [Name]` in an import body
    /// (`[persistent: tbl]`, `[runtime]`, `[local]`, flags), overriding the
    /// materialized primary entity's persistence/collection/flags.
    pub entity_rebind_tags: Option<EntityRebindTags>,
    /// `fields { up: local }` — primary-entity field renames the imported
    /// traits read/write, lifted from the trait form's `fields {}` to
    /// orbital scope. Third tuple element is the KEY token's span (Stage C
    /// `.lolo` spans), for `ORB_O_*` diagnostics that name this key.
    pub fields_remap: Vec<(String, String, Span)>,
    /// `pages { "/up": "/local" }` — route-path remap. An unmapped upstream
    /// path keeps its own value; required whenever two imports of the same
    /// orbital would otherwise collide on route. Third element: KEY span.
    pub pages_remap: Vec<(String, String, Span)>,
    /// `pages { "/up": "/local" [access: …] }` — page modifiers per remap
    /// entry, keyed by the UPSTREAM path, in source order.
    pub page_modifiers: Vec<(String, orbital_core_base::schema::PageModifiers)>,
    /// `omit { A B }` — trait/aux-entity names to drop from the imported set.
    /// Omitting a trait some other imported trait embeds is an error, not a
    /// silent drop (checked post-inline). Second element: the name token's
    /// span.
    pub omit: Vec<(String, Span)>,
    /// `only { A B }` — trait/aux-entity names to KEEP; everything else is
    /// dropped. Mutually exclusive with `omit` in authoring practice, not
    /// enforced by the grammar.
    pub only: Vec<String>,
    /// `config { ... }` override body — overrides the referenced orbital's
    /// OWN declared knobs (`Orbital.config`) 1:1; a dotted `Trait.knob` is
    /// the upstream (pre-prefix) escape hatch.
    pub config_overrides: Vec<ConfigField>,
    /// `events { UP: LOCAL }` — orbital-scope event rename, applied to every
    /// trait in the imported set (emits AND listen keys), same rule as the
    /// trait reference form's `events {}`.
    pub events_remap: Vec<(String, String)>,
    /// `roles { approver: [owner, project_manager]  employee: team_member }`
    /// (Stage B) — upstream role vocabulary word → one or more consumer
    /// roster role names. Order preserved for the formatter. Third element:
    /// the KEY (upstream word) token's span.
    pub roles_remap: Vec<(String, Vec<String>, Span)>,
    /// `entities { Employee: Person }` (Stage B) — out-of-orbital upstream
    /// entity name → consumer entity name. Identity (`X: X`) is legal.
    /// Third element: the KEY token's span.
    pub entities_remap: Vec<(String, String, Span)>,
    /// `pages { "/up": "/local" -> TraitA, TraitB }` mount tails (Stage B),
    /// keyed by the UPSTREAM path — sibling trait names (declared in
    /// `traits` below) to mount on that page in addition to its own. Third
    /// element: the upstream path key's span (same span `pages_remap`
    /// records for the same path).
    pub mounts: Vec<(String, Vec<String>, Span)>,
    /// `config { ... }` — an INLINE orbital's own declared knobs
    /// (orbital-import Stage A, `Orbital.config`). Empty for a reference-form
    /// orbital, whose declared knobs live upstream; always empty together
    /// with `config_overrides`, which is the reference form's override
    /// surface for THIS field on the orbital it references.
    pub config: Vec<ConfigField>,
    /// `extend { <field-decl>+ }` (Stage C) — full entity-field declarations
    /// appended to the primary entity, AFTER `fields {}` renames apply.
    /// Unlike `fields {}` (rename-only), each entry is a whole new field.
    pub extend_fields: Vec<EntityField>,
    /// `mock { field: "a, b" }` — the import's own sample data for fields of
    /// the materialized primary entity, replacing the upstream `@mock`.
    pub mock_overrides: Vec<(String, String, Span)>,
    /// `retype { name : type = default }` — replaces the type/default of fields the
    /// upstream primary entity already declares (after `fields {}` renames).
    pub retype_fields: Vec<EntityField>,
    /// `traits { UpstreamTrait [-> Entity] { events fields config listens emitsScope } }`
    /// — per-imported-trait overrides, keyed by the UPSTREAM trait name.
    pub trait_overrides: Vec<TraitOverride>,
    /// Orbital-import `listens { Source.EVENT -> Target.TRIGGER }` — routes
    /// into the imported set; each carries its `target_trait`.
    pub import_listens: Vec<Listen>,
    pub span: Span,
}

/// Entity-header tags written on an import body's `entity` line.
#[derive(Debug, Clone, Default)]
pub struct EntityRebindTags {
    pub persistence: Option<Persistence>,
    pub collection: Option<String>,
    pub shared: bool,
    pub identity: bool,
    pub local: bool,
    pub mock: bool,
}

/// One `traits { UpstreamTrait [-> Entity] { ... } }` entry of an orbital-import body.
#[derive(Debug, Clone)]
pub struct TraitOverride {
    /// The trait's UPSTREAM name.
    pub trait_name: String,
    pub linked_entity: Option<String>,
    pub body: TraitRefBody,
    pub span: Span,
}

/// `uses Alias from "path"`.
#[derive(Debug, Clone)]
pub struct UseDecl {
    pub alias: String,
    pub path: String,
    /// `uses lazy A from "…"` — compiled to its own `.orb`, loaded when one of
    /// its pages is opened.
    pub lazy: bool,
    /// Optional `{ config { ... } }` body (orbital-import Stage A) —
    /// overrides the imported behavior's ORGANISM-wide `config {}` knobs for
    /// every orbital taken from this alias. Empty for the bare form.
    pub config: Vec<ConfigField>,
    pub span: Span,
}

/// One `expects ...` declaration inside an orbital block.
#[derive(Debug, Clone)]
pub struct ExpectsDecl {
    pub kind: ExpectsKind,
    pub span: Span,
}

/// The three expectation forms. `shape` reuses the entity field-declaration
/// grammar verbatim — it is the consumer's declared minimal contract.
#[derive(Debug, Clone)]
pub enum ExpectsKind {
    /// `expects identity [Name] [{ fields }]` — bare (neither name nor shape)
    /// is the hand-authoring escape hatch.
    Identity {
        name: Option<String>,
        shape: Option<Vec<EntityField>>,
    },
    /// `expects entity Name [{ fields }]` — shape omitted = opaque reference.
    Entity {
        name: String,
        shape: Option<Vec<EntityField>>,
    },
    /// `expects event Trait.EVENT` — names pass through verbatim.
    Event { trait_name: String, event: String },
    /// `expects page "/articles/:slug"` — a route a sibling orbital declares
    /// and this one navigates to. The provider's PATTERN, which a navigate
    /// effect's static prefix cannot supply.
    Page { path: String },
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
    /// `identity` header tag — orthogonal to both persistence and `shared`:
    /// this entity types the ambient `@user` viewer. An app-owned user
    /// directory is both persisted and the identity, so it cannot be a
    /// persistence mode.
    pub identity: bool,
    /// `local` header tag — `[persistent: x, local]`: the rows live in the browser.
    pub local: bool,
    /// `mock` header tag — seed an empty browser store from the shared mock seeder.
    pub seed_mock: bool,
    /// `instances [ { … }, … ]` — written-out rows the entity starts with.
    pub instances: Vec<orbital_core_base::schema::SObject>,
    /// `instances <lang> [ … ]` — the written-out rows for one declared locale.
    pub locale_instances:
        std::collections::BTreeMap<String, Vec<orbital_core_base::schema::SObject>>,
    /// `@read <sexpr>` — a per-row predicate evaluated with `@entity` bound to
    /// the candidate row and `@user` to the viewer. ANDed with any call-site
    /// `filter:`; enforced once, for every trait that fetches this entity —
    /// the read-access twin of `@create`/`@update`/`@delete` below.
    pub read_policy: Option<SExpression>,
    /// `@create <sexpr>` — evaluated with `@entity` bound to the INCOMING data
    /// about to become a row (no row exists yet) and `@user` to the viewer.
    pub create_policy: Option<SExpression>,
    /// `@update <sexpr>` — evaluated with `@entity` bound to the EXISTING row
    /// before mutation (fetched fresh) and `@user` to the viewer.
    pub update_policy: Option<SExpression>,
    /// `@delete <sexpr>` — same binding shape as `@update`.
    pub delete_policy: Option<SExpression>,
    /// `@<op> none "<reason>"` — a DECLARED omission, keyed by op name
    /// (`read`/`create`/`update`/`delete`) with the author's stated reason.
    /// Behaves exactly like leaving the directive out; it exists so a lint can
    /// tell a considered omission from a forgotten one.
    pub access_waivers: Vec<(String, String)>,
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
    /// Trailing `@mock "..."` annotation — a declared literal value (or
    /// comma-separated candidate list) the seeder uses AS-IS, never through a
    /// domain-name lookup table. Authoring metadata only, like `description`/
    /// `synonyms` — the runtime ignores it.
    pub mock: Option<String>,
    /// Default value `= sexpr`.
    pub default: Option<SExpression>,
    /// `derived` field flag with the derivation expression.
    pub derived: Option<SExpression>,
    pub span: Span,
}

/// What a `@payload <arg>` dependent type resolves against — the two
/// spellings `TypeExpr::EventPayloadOf` carries. Kept as one enum (not a
/// string convention) so every consumer switches on a closed shape instead
/// of re-parsing a leading sigil out of a string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PayloadOfTarget {
    /// `@payload @config.<knob>` — the payload of the event that config knob
    /// names. Resolved post-inline (L2), against the call site's folded
    /// config default — see `orbital-compiler::phases::inline::sigil_types`.
    ConfigKnob(String),
    /// `@payload <member>` — the payload of the event that a SIBLING member
    /// of the SAME struct body names. `member` must be declared alongside
    /// this field (same `type X = { ... }` / inline object body) and typed
    /// `event`. Resolved at L1, against literal struct instances authored in
    /// the same file — see `types::check_payload_sibling_refs`.
    SiblingMember(String),
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
    /// `[T1, T2, …]` — a fixed-length tuple (two or more slots).
    Tuple(Vec<TypeExpr>),
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
    /// `@entity` — the bound entity's ROW type (self-reference). The sigil
    /// already means "the bound entity" throughout the expression grammar; in
    /// a payload type position it names that entity's row shape, resolved at
    /// compose time against the trait's final `linkedEntity` (monomorphized
    /// exactly like an Entity-kinded type parameter's default). Only legal in
    /// event payload positions (Event alias bodies, emits/listens payloads).
    BoundEntityRow,
    /// `@payload <knob>` — the declared payload type of the event that the
    /// named config knob resolves to. A sigil type constructor applied by
    /// JUXTAPOSITION, like `Map K V`; the language has exactly two application
    /// forms (Lisp `(fn args)` in expressions, juxtaposition in types) and a
    /// third would be an inconsistency.
    ///
    /// The argument is a value REFERENCE, not a type — this is the language's
    /// first dependent type, so it is deliberately fenced: the knob must be
    /// declared on the same trait, resolution is compile-time and total
    /// (resolve or hard error), and there is no nesting.
    ///
    /// Distinct from the expression-position `@payload.x`, which reads the
    /// CURRENT event's payload. Same word, different grammar position,
    /// different referent — the parser change is scoped to `parse_type_atom`
    /// so the two can never overlap.
    EventPayloadOf(PayloadOfTarget),
    /// `@entity <knob>` — the row type of the entity that the named config
    /// knob resolves to. The unary sibling of `BoundEntityRow`: same sigil,
    /// arity distinguishes "the bound entity" from "the entity that knob names".
    NamedEntityRow(String),
    /// `@user` — the row type of the identity this orbital `expects`. The
    /// identity axis's counterpart to `BoundEntityRow`: lets a reusable atom
    /// say "the acting user" without naming whose identity entity that is.
    ExpectedIdentityRow,
    /// `T?` nullable.
    Optional(Box<TypeExpr>),
    /// `T.f` — the declared type of field `f` of the entity or struct type `T`,
    /// legal in every type position. `resolved` is `f`'s own type expression,
    /// filled by `lower::projection` before lowering; [`TypeExpr::peel`] reads
    /// through it, and field builders record the provenance (`projectedFrom`).
    FieldOf {
        ty: String,
        field: String,
        resolved: Option<Box<TypeExpr>>,
    },
    /// `T*` one-to-many relation.
    RelationMany(String),
    /// `T+` one-or-more relation.
    RelationManyNonEmpty(String),
    /// `trait` — value is a `@trait.X` reference. Substituted to embedded
    /// trait UI at render time. Allowed only in `config { ... }` blocks.
    Trait,
    /// `orbital` — an orbital value held as data (`<spec>.orbitals.<O>` plus a
    /// recorded §8b import body), or `none`. Never rendered.
    Orbital,
    /// `behavior` — a whole behavior held as data (`behavior/ref`), or `none`.
    /// Embedded in a render tree, it runs the program in place.
    Behavior,
    /// `entity` — value is an entity NAME reference (V4-W5). The stamp records
    /// the referenced entity's id on the config field so a declaration-only
    /// rename resolves the value by id. Allowed only in `config { ... }` blocks.
    EntityRef,
    /// `event` — value is an event NAME reference (V4-W5), id-carried by the
    /// stamp like `EntityRef`. Allowed only in `config { ... }` blocks.
    EventRef,
    /// `EventAddress` — a value addressing a declared external input
    /// (`Trait.EVENT`, `Orbital.Trait.EVENT`). Allowed only in `config { ... }` blocks.
    EventAddress,
    /// `slot` — value is a UI slot name. Validated against `VALID_SLOTS`.
    Slot,
    /// `pattern` — value is a pattern type name. Validated against `PATTERN_TYPES`.
    Pattern,
    /// `secret` — value is a credential REFERENCE (an env-var key name for
    /// `resolveCredentialRef`), never secret material itself. Config-only
    /// (core's `SECRET_CONFIG_TYPES` contract, I-29): UI renders the knob as
    /// a password input; traces/bakes mask it as defense-in-depth.
    Secret,
    /// `SExpr` — value is a compiled s-expression (the `SExpr` brand from
    /// `@almadar/core`). Used for config fields that hold an s-expression
    /// (e.g. a `conditionalFields` visibility condition). Emits the recursive
    /// `SExpr` TypeScript type (inlined as a local alias in codegen, like
    /// `TraitFieldRef`). Allowed only in `config { ... }` blocks.
    SExpr,
    /// `match @config.<knob> { <pattern> -> <type> ... }` — a compile-time
    /// conditional TYPE selected by a config knob's resolved literal (§25).
    /// Replaces `@typeWhen` outright: one concept, one spelling. The `_`
    /// arm is mandatory and must be last; it lowers to the field's base
    /// `field_type`, and every other arm lowers to one entry of the
    /// unchanged `PayloadTypeWhen` wire list (`lower_match_field_type`).
    /// Legal only as an event-payload field's WHOLE type (`emits`/local
    /// `listens`); forbidden everywhere else in the type grammar (entity
    /// fields, `expects`, config fields, type-alias/Event-alias bodies,
    /// nested inside another type) — see `check_match_types` +
    /// `type_expr_config_only_type`/`type_expr_contains_match`.
    Match { knob: String, arms: Vec<MatchArm> },
}

impl TypeExpr {
    /// The type this expression stands for: a resolved `T.f` is its field's
    /// type (recursively); every other form is itself.
    pub fn peel(&self) -> &TypeExpr {
        match self {
            TypeExpr::FieldOf {
                resolved: Some(inner),
                ..
            } => inner.peel(),
            other => other,
        }
    }

    /// A payload field's provenance: `T.f` itself, or for `[T.f]` each element's.
    pub fn payload_projection(&self) -> Option<orbital_core_base::schema::types::FieldProjection> {
        self.projection().or_else(|| match self.peel() {
            TypeExpr::Array(inner) => inner.projection(),
            _ => None,
        })
    }

    /// `T.f`'s provenance, for the field a projection types.
    pub fn projection(&self) -> Option<orbital_core_base::schema::types::FieldProjection> {
        match self {
            TypeExpr::FieldOf { ty, field, .. } => Some(orbital_core_base::schema::types::FieldProjection {
                type_name: ty.clone(),
                field: field.clone(),
            }),
            _ => None,
        }
    }
}

/// One arm of a type-level `match` (`TypeExpr::Match`). `ty` is restricted
/// to `Primitive`/`[Primitive]` in v1 — anything else is
/// `ELOLO_MATCH_ARM_TYPE_UNSUPPORTED` (`check_match_types`).
#[derive(Debug, Clone)]
pub struct MatchArm {
    pub pattern: MatchPattern,
    pub ty: TypeExpr,
    pub span: Span,
}

/// A type-level match arm's pattern. Compared against a resolved config
/// literal's canonical spelling (`orbital_core::schema::config_literal_spelling`)
/// via `spelling()` below — `Str`/`Word` are interchangeable spellings of the
/// same literal (`"number"` and `number` both spell `"number"`).
#[derive(Debug, Clone)]
pub enum MatchPattern {
    Str(String),
    Word(String),
    Bool(bool),
    Int(i64),
    Wildcard,
}

impl MatchPattern {
    /// Canonical spelling for duplicate/vocabulary-membership comparison,
    /// mirroring `config_literal_spelling`'s output exactly (`Str`/`Word`
    /// compare as their own text, `Bool` as `"true"`/`"false"`, `Int` as its
    /// decimal spelling). `None` for `Wildcard` — it has no literal spelling.
    pub fn spelling(&self) -> Option<String> {
        match self {
            MatchPattern::Str(s) | MatchPattern::Word(s) => Some(s.clone()),
            MatchPattern::Bool(b) => Some(b.to_string()),
            MatchPattern::Int(n) => Some(n.to_string()),
            MatchPattern::Wildcard => None,
        }
    }

    /// This pattern's literal value as the `SExpression` a synthesized `==`
    /// guard compares a scrutinee against — typed, not stringified
    /// (`Bool`/`Int` stay `SExpression::Bool`/`Number`), matching how
    /// hand-written guards compare typed fields (`(== @entity.playing
    /// true)`). `Wildcard` lowers to the `"_"` sentinel string — never used
    /// in a `==` guard (callers special-case `Wildcard` before reaching
    /// there), but needed verbatim by expression-match's arm encoding
    /// (Phase 3b, `parser::expression::parse_expr_match`), which embeds a
    /// pattern atom as one element of an ordinary `SExpression::Array` arm
    /// `[pattern, value]` — `"_"` can never collide with a `Word`/`Str`
    /// spelling because the parser always reads a bare `_` as `Wildcard`,
    /// never `Word("_")`. One converter, shared by term-level `match`
    /// (Phase 2, transition guards) and expression-level `match` (Phase 3b,
    /// value positions) — neither duplicates it.
    pub fn literal_sexpr(&self) -> SExpression {
        match self {
            MatchPattern::Str(s) | MatchPattern::Word(s) => SExpression::String(s.clone()),
            MatchPattern::Bool(b) => SExpression::Bool(*b),
            MatchPattern::Int(n) => SExpression::Number(*n as f64),
            MatchPattern::Wildcard => SExpression::String("_".to_string()),
        }
    }
}

/// One trait-level type parameter (Increment 3-II): `(V : Type)` in
/// `trait LookupRun (V : Type, R : Type) -> KeyLookup [lifecycle, instance] { ... }`.
/// Parsed immediately after the trait name, before `::`/`->`/`=`. `kind` is
/// validated against the same six kind names Increment 3-I's alias params
/// accept (`Type | Entity | Trait | Page | Orbital | Event`); only `Type` and
/// `Entity` are meaningfully acted on downstream. A param name is usable in
/// this trait's own `emits`/`listens`/config type positions, lowering each
/// reference to a `"$<name>"` sentinel (3-I's substitution machinery).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraitTypeParam {
    pub name: String,
    pub kind: String,
    /// Gap C: optional `= <ident>` declaration default — a primitive name,
    /// an entity name, or a struct-alias name, all
    /// resolved in the atom's own file.
    pub default: Option<String>,
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

/// A `trait` declaration: an inline state machine bound to an entity, or a
/// reference to an imported trait with call-site overrides.
#[derive(Debug, Clone)]
pub struct Trait {
    pub name: String,
    /// Increment 3-II: trait-level type parameters declared `(V : Type, ...)`
    /// immediately after the name. Empty for traits that declare none —
    /// which is every trait predating this increment, and always empty on a
    /// reference-form trait (a reference supplies ARGUMENTS via `::`, not a
    /// parameter declaration).
    pub type_params: Vec<TraitTypeParam>,
    /// Optional `::` type annotation above the trait declaration.
    pub type_annotation: Option<TypeAnnotation>,
    pub linked_entity: Option<String>,
    /// Where the entity name after `->` is written.
    pub linked_entity_span: Option<Span>,
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
    /// `local` marker parsed from the same trait-header bracket list.
    /// Declares client-only dispatch (see `docs/Almadar_LOLO.md` §7).
    /// Undeclared (`false`) is server-gated dispatch — never inferred.
    pub local: bool,
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
    /// When `true`, this trait declaration was synthesized by the parser from
    /// an inline `@(...)` reference. The formatter uses this marker to
    /// reconstruct the original sugar instead of emitting a top-level `trait`
    /// declaration.
    pub is_inline_synthetic: bool,
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
    /// `name: "InstanceName"` — instance-name override for the reference.
    pub name_override: Option<String>,
}

/// Override payload for a `trait Name = Alias.traits.Upstream { ... }` form.
#[derive(Debug, Clone, Default)]
pub struct TraitRefData {
    /// Dotted path: e.g. `"Modal.traits.ModalRecordModal"`.
    pub reference: String,
    /// `name: "InstanceName"` — instance-name override lowered to `name` on
    /// the trait reference. Disambiguates multiple rebinds of one source
    /// trait in a single orbital (the declaration identifier stays free for
    /// embed unification); absent, the declaration identifier is the name.
    pub name_override: Option<String>,
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
    /// L2 source-map spans captured while parsing `guard`, keyed by the
    /// JSON-pointer trail relative to the guard expression's own root (`""`
    /// for the guard's own span). Empty when there's no guard.
    pub guard_spans: Vec<(String, Span)>,
    /// Parallel to `effects`: `effect_spans[i]` holds the trail/span pairs
    /// captured while parsing `effects[i]`.
    pub effect_spans: Vec<Vec<(String, Span)>>,
    /// `EVENT -> target match @entity.<f> { … }` sugar (Phase 2, pattern-
    /// matching doc §1(a)) — mutually exclusive with `guard`/`effects`, which
    /// stay `None`/empty when this is `Some` (`parser::trait_::parse_transition`
    /// enforces the exclusion; `when` + `match` together is a parse error).
    /// Pure L1 sugar: `lower::expand_term_match` turns each arm into one
    /// ordinary guarded `Transition` before anything downstream ever sees it.
    pub term_match: Option<TermMatch>,
}

/// Transition-position term-level `match` (Phase 2). Dispatches on a VALUE
/// (`@entity.<f>`/`@payload.<f>`, v1) rather than Phase 1's config-resolved
/// TYPE — same `MatchPattern` arm vocabulary, reused as-is.
#[derive(Debug, Clone)]
pub struct TermMatch {
    pub scrutinee: MatchScrutinee,
    pub arms: Vec<TermMatchArm>,
    pub span: Span,
}

/// A term-match scrutinee: `@entity.<field>` / `@payload.<field>`, or the
/// Phase-3a dotted destructuring form `@entity.<field>.<member>` /
/// `@payload.<field>.<member>` where `<field>`'s declared type is a tagged
/// struct union and `<member>` is its discriminating member (each variant's
/// single-valued closed member, e.g. `type : "draw-shape"`). `@config.<knob>`
/// (degenerate constant dispatch; use type-level `match` or a plain guard)
/// and `@user.<field>` (fail-open class needs its own design pass) are parse
/// errors, see `parser::trait_::parse_term_match_scrutinee`.
#[derive(Debug, Clone)]
pub enum MatchScrutinee {
    Entity {
        field: String,
        member: Option<String>,
    },
    Payload {
        field: String,
        member: Option<String>,
    },
}

impl MatchScrutinee {
    /// The base field name this scrutinee names, irrespective of root and of
    /// any dotted `<member>` segment.
    pub fn field(&self) -> &str {
        match self {
            MatchScrutinee::Entity { field, .. } | MatchScrutinee::Payload { field, .. } => field,
        }
    }

    /// The dotted discriminant member (`type` in `@payload.item.type`), when
    /// this is the Phase-3a destructuring form.
    pub fn member(&self) -> Option<&str> {
        match self {
            MatchScrutinee::Entity { member, .. } | MatchScrutinee::Payload { member, .. } => {
                member.as_deref()
            }
        }
    }

    /// The OWNER value's sigil spelling — the base field without the
    /// discriminant segment (`@payload.item` for `@payload.item.type`).
    /// This is the value that flow-refines to the selected variant inside
    /// each arm (Phase 3c).
    pub fn owner_sigil(&self) -> String {
        match self {
            MatchScrutinee::Entity { field, .. } => format!("@entity.{field}"),
            MatchScrutinee::Payload { field, .. } => format!("@payload.{field}"),
        }
    }

    /// Full sigil spelling, discriminant segment included — used both by the
    /// synthesized `(== <sigil> <lit>)` guard and the formatter.
    pub fn sigil(&self) -> String {
        let owner = self.owner_sigil();
        match self.member() {
            Some(m) => format!("{owner}.{m}"),
            None => owner,
        }
    }
}

/// One arm of a term-level `match`. `effects` mirrors `Transition::effects`
/// (zero or more s-expressions, authoring order); `effect_spans` mirrors
/// `Transition::effect_spans`.
#[derive(Debug, Clone)]
pub struct TermMatchArm {
    pub pattern: MatchPattern,
    pub effects: Vec<SExpression>,
    pub effect_spans: Vec<Vec<(String, Span)>>,
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
    /// `-> Target.TRIGGER`: the imported trait that gains this route. Only
    /// an orbital import's `listens {}` carries one.
    pub target_trait: Option<String>,
    /// `EVENT -> external { … }`: a declared external input, an event a client
    /// outside the program may send this trait.
    pub external: bool,
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
    /// `[background]`: runs with no client mounted.
    pub background: bool,
    /// `[visible]`: a browser tick that pauses while its trait's output is off screen.
    pub visible: bool,
    pub span: Span,
    /// L2 source-map spans captured while parsing `guard`. See
    /// [`Transition::guard_spans`].
    pub guard_spans: Vec<(String, Span)>,
    /// Parallel to `effects`. See [`Transition::effect_spans`].
    pub effect_spans: Vec<Vec<(String, Span)>>,
}

/// How often a tick fires: a fixed interval or a cron expression.
#[derive(Debug, Clone)]
pub enum TickInterval {
    /// A fixed interval in milliseconds (`every 5s`).
    Millis(u64),
    /// A quoted cron expression (`every "0 9 * * *"`).
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
    /// `!` required marker — `fields : [FieldSpec]! = [...]`. Declares
    /// this knob must be supplied by every call site referencing the
    /// owning trait (the atom's own page binding is exempt). May
    /// coexist with a `default` (the default serves the demo page).
    pub required: bool,
    /// Dotted event atoms in `default`, keyed by their trail inside it — each
    /// must sit at an `event`-typed position.
    pub event_refs: Vec<(String, Span)>,
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
    /// Nav icon name — `@icon "..."` annotation below the page line.
    pub icon: Option<String>,
    /// Human nav label — `@label "..."` annotation below the page line.
    pub label: Option<String>,
    /// Nav entry roles — `@roles "a, b"` annotation below the page line.
    pub roles: Option<Vec<String>>,
    pub access: Option<orbital_core_base::schema::PageAccess>,
    pub indexing: Option<orbital_core_base::schema::PageIndexing>,
    pub title: Option<orbital_core_base::schema::PageMeta>,
    pub description: Option<orbital_core_base::schema::PageMeta>,
    /// `translationOf:` — the translation group this page belongs to.
    pub translation_of: Option<String>,
}

/// Top-level AST returned by the parser. The compiler-facing entry point is
/// `lower_to_orb` which takes one of these.
pub type Ast = Program;
