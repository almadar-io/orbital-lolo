//! Pretty-printer for the `.lolo` surface language.
//!
//! Takes a parsed `ast::Program` and emits canonical, consistently-indented
//! `.lolo` source. Source comments (`;;`, `#`, `#= … =#`) are preserved: the
//! lexer captures them as trivia and the formatter re-interleaves them by source
//! line before the node that follows them (re-indented to the output).

use orbital_core_base::os_grants::{DesktopKind, FsMode, NetKind, OsGrants, StdioKind, SysKind};
use orbital_core_base::schema::SExpression;
use std::collections::HashMap;

use crate::ast::{
    AppHeader, Comment, ConfigField, Emit, Entity, EntityField, EventField, EventScopeAst,
    ExpectedAccess, ExpectsDecl, ExpectsKind, Listen, MatchPattern, Orbital, Page, PayloadOfTarget, Persistence,
    Program, State, TermMatch, ThemeDecl, ThemeTokenMaps, Tick, TickInterval, Trait, TraitRefData,
    TraitScope, Transition, TypeAlias, TypeAnnotation, TypeBody, TypeExpr, UseDecl,
};

/// Format a `Program` AST into canonical `.lolo` source text.
pub fn format_program(program: &Program) -> String {
    let mut f = Formatter::new(program.comments.clone());
    f.fmt_program(program);
    f.finish()
}

fn theme_maps_empty(maps: &ThemeTokenMaps) -> bool {
    maps.colors.is_empty()
        && maps.radii.is_empty()
        && maps.spacing.is_empty()
        && maps.typography.is_empty()
        && maps.shadows.is_empty()
        && maps.type_scale.is_empty()
        && maps.geometry.is_empty()
        && maps.elevation.is_empty()
        && maps.surface.is_empty()
        && maps.motion.is_empty()
        && maps.diagram.is_empty()
        && maps.scene.is_empty()
}

/// One AST node, printed on its own (the language reference's examples).
pub enum Node<'a> {
    Orbital(&'a Orbital),
    UseDecl(&'a UseDecl),
    ExpectsDecl(&'a ExpectsDecl),
    TypeAlias(&'a TypeAlias),
    Entity(&'a Entity),
    EntityField(&'a EntityField),
    Trait(&'a Trait),
    State(&'a State),
    Transition(&'a Transition),
    Emit(&'a Emit),
    Listen(&'a Listen),
    Tick(&'a Tick),
    ConfigField(&'a ConfigField),
    Page(&'a Page),
}

/// Format one AST node as canonical `.lolo` source, without comments.
pub fn format_node(node: Node<'_>) -> String {
    let mut f = Formatter::new(Vec::new());
    match node {
        Node::Orbital(n) => f.fmt_orbital(n),
        Node::UseDecl(n) => f.fmt_use_decl(n),
        Node::ExpectsDecl(n) => f.fmt_expects_decl(n),
        Node::TypeAlias(n) => f.fmt_type_alias(n),
        Node::Entity(n) => f.fmt_entity(n),
        Node::EntityField(n) => f.fmt_entity_field(n),
        Node::Trait(n) => f.fmt_trait(n),
        Node::State(n) => f.fmt_state(n),
        Node::Transition(n) => f.fmt_transition(n),
        Node::Emit(n) => f.fmt_emit(n),
        Node::Listen(n) => f.fmt_listen(n),
        Node::Tick(n) => f.fmt_tick(n),
        Node::ConfigField(n) => f.fmt_config_field(n),
        Node::Page(n) => f.fmt_page(n),
    }
    f.finish().trim().to_string()
}

/// [`format_node`], plus the 1-based first and last line of every transition it printed, in
/// print order.
pub fn format_node_with_transition_lines(node: Node<'_>) -> (String, Vec<(usize, usize)>) {
    let mut f = Formatter::new(Vec::new());
    match node {
        Node::Trait(n) => f.fmt_trait(n),
        Node::State(n) => f.fmt_state(n),
        Node::Transition(n) => f.fmt_transition(n),
        other => return (format_node(other), Vec::new()),
    }
    let ranges = std::mem::take(&mut f.transition_ranges);
    let raw = f.finish();
    let lead = raw.len() - raw.trim_start().len();
    let line_at = |offset: usize| raw[lead..offset.max(lead)].matches('\n').count() + 1;
    let lines = ranges
        .into_iter()
        .map(|(start, end)| (line_at(start), line_at(end)))
        .collect();
    (raw.trim().to_string(), lines)
}

/// One `key: value` entry of a modifier bracket; the closure writes the value.
type ModifierEntry<'a> = (&'a str, Box<dyn Fn(&mut Formatter) + 'a>);

struct Formatter {
    indent: usize,
    output: String,
    comments: Vec<Comment>,
    next_comment: usize,
    /// Synthesized inline trait references in the orbital currently being
    /// formatted, keyed by trait name (without the `@trait.` prefix). Used to
    /// reconstruct `@(...)` sugar and to suppress the desugared top-level
    /// `trait` declarations.
    inline_traits: HashMap<String, Trait>,
    /// The output byte range of every transition printed, in print order.
    transition_ranges: Vec<(usize, usize)>,
    /// Lexical bindings opened by enclosing `let` / `fn` forms. A bare atom
    /// naming one of these must print quoted: the parser rejects the bare
    /// spelling, and re-reading our own output has to succeed. Quoting is
    /// always value-preserving — `x` and `"x"` both evaluate to `"x"`.
    binding_scopes: Vec<Vec<String>>,
}

impl Formatter {
    fn new(comments: Vec<Comment>) -> Self {
        Self {
            indent: 0,
            output: String::new(),
            comments,
            next_comment: 0,
            inline_traits: HashMap::new(),
            transition_ranges: Vec::new(),
            binding_scopes: Vec::new(),
        }
    }

    fn is_lexically_bound(&self, name: &str) -> bool {
        self.binding_scopes
            .iter()
            .any(|frame| frame.iter().any(|bound| bound == name))
    }

    /// Binding names declared by a `let` binding list (`((a v) (b w))`).
    /// The authored template of a `(quasiquote x)` IR call: the decoded body
    /// with each marker `["unquote", i]` put back as `(unquote <hole i>)`.
    fn quasiquote_template(arr: &[SExpression]) -> Option<SExpression> {
        let call = SExpression::Array(arr.to_vec());
        let (body, holes) = orbital_core_base::schema::quote::quasiquote_of(&call)?;
        let template = orbital_core_base::schema::quote::decode_quote_body(body)?;
        fn restore(e: SExpression, holes: &[SExpression]) -> SExpression {
            match e {
                SExpression::Array(items) => {
                    if let [SExpression::String(h), SExpression::Number(n)] = items.as_slice() {
                        if h == orbital_core_base::schema::quote::UNQUOTE_HEAD && n.fract() == 0.0 {
                            if let Some(hole) = holes.get(*n as usize) {
                                return SExpression::Array(vec![
                                    SExpression::String(h.clone()),
                                    hole.clone(),
                                ]);
                            }
                        }
                    }
                    SExpression::Array(items.into_iter().map(|i| restore(i, holes)).collect())
                }
                SExpression::Object(map) => SExpression::Object(
                    map.into_iter()
                        .map(|(k, v)| (k, restore(v, holes)))
                        .collect(),
                ),
                other => other,
            }
        }
        Some(restore(template, holes))
    }

    fn let_binding_names(bindings: &SExpression) -> Vec<String> {
        let SExpression::Array(pairs) = bindings else {
            return Vec::new();
        };
        pairs
            .iter()
            .filter_map(|pair| match pair {
                SExpression::Array(kv) => match kv.first() {
                    Some(SExpression::String(name)) => Some(name.clone()),
                    _ => None,
                },
                _ => None,
            })
            .collect()
    }

    /// Emit any pending comments whose source line precedes `line`, each on its
    /// own line at the current indent. Call before printing a node that starts a
    /// fresh line, to interleave its leading comments.
    fn emit_comments_before(&mut self, line: u32) {
        while self.next_comment < self.comments.len()
            && self.comments[self.next_comment].line < line
        {
            let text = self.comments[self.next_comment].text.clone();
            self.write_indent();
            self.output.push_str(&text);
            self.write_newline();
            self.next_comment += 1;
        }
    }

    fn finish(self) -> String {
        self.output
    }

    fn write_indent(&mut self) {
        for _ in 0..self.indent {
            self.output.push_str("  ");
        }
    }

    fn indent(&mut self) {
        self.indent += 1;
    }

    fn dedent(&mut self) {
        self.indent -= 1;
    }

    fn write_newline(&mut self) {
        self.output.push('\n');
    }

    fn fmt_program(&mut self, program: &Program) {
        self.fmt_app_header(&program.app);
        if !program.type_aliases.is_empty()
            || !program.uses.is_empty()
            || !program.orbitals.is_empty()
        {
            self.write_newline();
        }
        for (i, alias) in program.type_aliases.iter().enumerate() {
            if i > 0 {
                self.write_newline();
            }
            self.emit_comments_before(alias.span.line);
            self.fmt_type_alias(alias);
        }
        if !program.type_aliases.is_empty()
            && (!program.uses.is_empty() || !program.orbitals.is_empty())
        {
            self.write_newline();
        }
        // File-scoped `uses` (orbital-import Stage A) — legal above any
        // `orbital` block; EBNF places them after type aliases, before
        // orbitals.
        for (i, u) in program.uses.iter().enumerate() {
            if i > 0 {
                self.write_newline();
            }
            self.emit_comments_before(u.span.line);
            self.write_indent();
            self.fmt_use_decl(u);
            self.write_newline();
        }
        if !program.uses.is_empty() && !program.orbitals.is_empty() {
            self.write_newline();
        }
        for (i, orbital) in program.orbitals.iter().enumerate() {
            if i > 0 {
                self.write_newline();
                self.write_newline();
            }
            self.emit_comments_before(orbital.span.line);
            self.fmt_orbital(orbital);
        }
        // Trailing / EOF comments.
        self.emit_comments_before(u32::MAX);
    }

    fn fmt_app_header(&mut self, app: &AppHeader) {
        if app.name.is_empty() {
            return;
        }
        self.write_indent();
        self.output.push_str("app ");
        self.output.push_str(&app.name);
        let mut entries: Vec<ModifierEntry<'_>> = Vec::new();
        if let Some(v) = &app.version {
            entries.push((
                "version",
                Box::new(move |f: &mut Self| f.write_quoted_string(v)),
            ));
        }
        if let Some(desc) = &app.description {
            entries.push((
                "description",
                Box::new(move |f: &mut Self| f.write_quoted_string(desc)),
            ));
        }
        if let Some(theme) = &app.theme {
            entries.push((
                "theme",
                Box::new(move |f: &mut Self| f.fmt_theme_value(theme)),
            ));
        }
        if !app.themes.is_empty() {
            entries.push((
                "themes",
                Box::new(move |f: &mut Self| {
                    f.output.push('[');
                    for (i, key) in app.themes.iter().enumerate() {
                        if i > 0 {
                            f.output.push_str(", ");
                        }
                        f.write_quoted_string(key);
                    }
                    f.output.push(']');
                }),
            ));
        }
        if !app.locales.is_empty() {
            entries.push((
                "locales",
                Box::new(move |f: &mut Self| {
                    f.output.push('[');
                    f.output.push_str(&app.locales.join(", "));
                    f.output.push(']');
                }),
            ));
        }
        if !app.egress.is_empty() {
            entries.push((
                "egress",
                Box::new(move |f: &mut Self| {
                    f.output.push('[');
                    for (i, origin) in app.egress.iter().enumerate() {
                        if i > 0 {
                            f.output.push_str(", ");
                        }
                        f.write_quoted_string(origin);
                    }
                    f.output.push(']');
                }),
            ));
        }
        if !app.inbound.is_empty() {
            entries.push((
                "inbound",
                Box::new(move |f: &mut Self| {
                    f.output.push('[');
                    for (i, name) in app.inbound.iter().enumerate() {
                        if i > 0 {
                            f.output.push_str(", ");
                        }
                        f.write_quoted_string(name);
                    }
                    f.output.push(']');
                }),
            ));
        }
        entries.extend(Self::grant_entries(&app.os_grants));
        if !app.deployment.is_empty() {
            entries.push((
                "deployment",
                Box::new(move |f: &mut Self| {
                    f.output.push_str("{ ");
                    for (i, (unit, orbitals)) in app.deployment.iter().enumerate() {
                        if i > 0 {
                            f.output.push_str(", ");
                        }
                        f.output.push_str(unit);
                        f.output.push_str(": [");
                        f.output.push_str(&orbitals.join(", "));
                        f.output.push(']');
                    }
                    f.output.push_str(" }");
                }),
            ));
        }
        if let Some(public) = &app.public {
            entries.push((
                "public",
                Box::new(move |f: &mut Self| f.write_quoted_string(public)),
            ));
        }
        for (key, value) in [
            ("origin", &app.site.origin),
            ("icon", &app.site.icon),
            ("siteName", &app.site.name),
            ("signIn", &app.site.sign_in),
        ] {
            if let Some(v) = value {
                entries.push((key, Box::new(move |f: &mut Self| f.write_quoted_string(v))));
            }
        }
        self.fmt_modifier_bracket(entries);
        if !app.config.is_empty() {
            self.write_newline();
            self.write_indent();
            self.output.push_str("config {");
            self.write_newline();
            self.indent();
            for cf in &app.config {
                self.write_indent();
                self.fmt_config_field(cf);
                self.write_newline();
            }
            self.dedent();
            self.write_indent();
            self.output.push('}');
        }
    }

    /// The `fs` … `desktop` entries of a grant set, in the contract's order; an empty family writes nothing.
    fn grant_entries(grants: &OsGrants) -> Vec<ModifierEntry<'_>> {
        fn names<T: Copy>(kinds: &[T], name: fn(T) -> &'static str) -> String {
            kinds.iter().map(|k| name(*k)).collect::<Vec<_>>().join(", ")
        }
        fn strings(f: &mut Formatter, items: &[String]) {
            f.output.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    f.output.push_str(", ");
                }
                f.write_quoted_string(item);
            }
            f.output.push(']');
        }
        let mut entries: Vec<ModifierEntry<'_>> = Vec::new();
        if !grants.fs.is_empty() {
            entries.push((
                "fs",
                Box::new(move |f: &mut Self| {
                    f.output.push_str("{ ");
                    for (i, g) in grants.fs.iter().enumerate() {
                        if i > 0 {
                            f.output.push_str(", ");
                        }
                        f.write_quoted_string(&g.glob);
                        f.output.push_str(": [");
                        f.output.push_str(&names(&g.modes, FsMode::name));
                        f.output.push(']');
                    }
                    f.output.push_str(" }");
                }),
            ));
        }
        if !grants.exec.is_empty() {
            entries.push(("exec", Box::new(move |f: &mut Self| strings(f, &grants.exec))));
        }
        if !grants.net.is_empty() {
            entries.push((
                "net",
                Box::new(move |f: &mut Self| {
                    f.output.push_str("{ ");
                    for (i, g) in grants.net.iter().enumerate() {
                        if i > 0 {
                            f.output.push_str(", ");
                        }
                        f.write_quoted_string(&g.target);
                        f.output.push_str(": [");
                        f.output.push_str(&names(&g.kinds, NetKind::name));
                        f.output.push(']');
                    }
                    f.output.push_str(" }");
                }),
            ));
        }
        if !grants.env.is_empty() {
            entries.push(("env", Box::new(move |f: &mut Self| strings(f, &grants.env))));
        }
        if !grants.stdio.is_empty() {
            entries.push((
                "stdio",
                Box::new(move |f: &mut Self| {
                    f.output.push_str(&format!("[{}]", names(&grants.stdio, StdioKind::name)))
                }),
            ));
        }
        if !grants.sys.is_empty() {
            entries.push((
                "sys",
                Box::new(move |f: &mut Self| {
                    f.output.push_str(&format!("[{}]", names(&grants.sys, SysKind::name)))
                }),
            ));
        }
        if !grants.desktop.is_empty() {
            entries.push((
                "desktop",
                Box::new(move |f: &mut Self| {
                    f.output
                        .push_str(&format!("[{}]", names(&grants.desktop, DesktopKind::name)))
                }),
            ));
        }
        entries
    }

    fn fmt_theme_decl(&mut self, decl: &ThemeDecl) {
        match decl {
            ThemeDecl::Reference(_) | ThemeDecl::Inline(_) => {
                self.output.push_str("theme ");
                self.fmt_theme_value(decl);
            }
        }
    }

    /// One `[` … `]` modifier bracket, one entry per line (shared by the app
    /// header and multi-entry pages). Writes nothing when `entries` is empty.
    fn fmt_modifier_bracket(&mut self, entries: Vec<ModifierEntry<'_>>) {
        if entries.is_empty() {
            return;
        }
        self.output.push_str(" [");
        self.write_newline();
        self.indent();
        let last = entries.len() - 1;
        for (i, (key, value)) in entries.into_iter().enumerate() {
            self.write_indent();
            self.output.push_str(key);
            self.output.push_str(": ");
            value(self);
            if i != last {
                self.output.push(',');
            }
            self.write_newline();
        }
        self.dedent();
        self.write_indent();
        self.output.push(']');
    }

    /// A theme VALUE: a quoted registry key, or the inline `{ … }` block.
    fn fmt_theme_value(&mut self, decl: &ThemeDecl) {
        match decl {
            ThemeDecl::Reference(key) => {
                self.write_quoted_string(key);
            }
            ThemeDecl::Inline(inline) => {
                self.output.push('{');
                self.write_newline();
                self.indent();
                self.write_indent();
                self.output.push_str("name: ");
                self.write_quoted_string(&inline.name);
                if let Some(display_name) = &inline.display_name {
                    self.write_newline();
                    self.write_indent();
                    self.output.push_str("displayName: ");
                    self.write_quoted_string(display_name);
                }
                if !theme_maps_empty(&inline.tokens) {
                    self.write_newline();
                    self.write_indent();
                    self.output.push_str("tokens ");
                    self.fmt_theme_token_maps(&inline.tokens);
                }
                if !inline.variants.is_empty() {
                    self.write_newline();
                    self.write_indent();
                    self.output.push_str("variants {");
                    self.write_newline();
                    self.indent();
                    for (i, (vname, maps)) in inline.variants.iter().enumerate() {
                        if i > 0 {
                            self.write_newline();
                        }
                        self.write_indent();
                        self.output.push_str(vname);
                        self.output.push(' ');
                        self.fmt_theme_token_maps(maps);
                        self.write_newline();
                    }
                    self.dedent();
                    self.write_indent();
                    self.output.push('}');
                }
                self.write_newline();
                self.dedent();
                self.write_indent();
                self.output.push('}');
            }
        }
    }

    /// `{ colors { … } … }` — a token-category block; the closing `}` is left
    /// on its own line and the caller owns the trailing newline.
    fn fmt_theme_token_maps(&mut self, maps: &ThemeTokenMaps) {
        self.output.push('{');
        self.write_newline();
        self.indent();
        let categories: [(&str, &[(String, String)]); 12] = [
            ("colors", &maps.colors),
            ("radii", &maps.radii),
            ("spacing", &maps.spacing),
            ("typography", &maps.typography),
            ("shadows", &maps.shadows),
            ("typeScale", &maps.type_scale),
            ("geometry", &maps.geometry),
            ("elevation", &maps.elevation),
            ("surface", &maps.surface),
            ("motion", &maps.motion),
            ("diagram", &maps.diagram),
            ("scene", &maps.scene),
        ];
        let mut first = true;
        for (cat, entries) in categories {
            if entries.is_empty() {
                continue;
            }
            if !first {
                self.write_newline();
            }
            first = false;
            self.write_indent();
            self.output.push_str(cat);
            self.output.push_str(" {");
            self.write_newline();
            self.indent();
            for (i, (key, value)) in entries.iter().enumerate() {
                if i > 0 {
                    self.write_newline();
                }
                self.write_indent();
                self.output.push_str(key);
                self.output.push_str(": ");
                self.write_quoted_string(value);
            }
            self.write_newline();
            self.dedent();
            self.write_indent();
            self.output.push('}');
        }
        self.write_newline();
        self.dedent();
        self.write_indent();
        self.output.push('}');
    }

    fn fmt_type_alias(&mut self, alias: &TypeAlias) {
        self.write_indent();
        self.output.push_str("type ");
        self.output.push_str(&alias.name);
        if let Some(tp) = &alias.type_param {
            self.output.push(' ');
            if let Some(kind) = &alias.type_param_kind {
                self.output.push('(');
                self.output.push_str(tp);
                self.output.push_str(" : ");
                self.output.push_str(kind);
                self.output.push(')');
            } else {
                self.output.push_str(tp);
            }
        }
        self.output.push_str(" = ");
        self.fmt_type_body(&alias.body);
    }

    fn fmt_type_body(&mut self, body: &TypeBody) {
        match body {
            TypeBody::Enum(variants) => {
                for (i, v) in variants.iter().enumerate() {
                    if i > 0 {
                        self.output.push_str(" | ");
                    }
                    self.output.push_str(v);
                }
            }
            TypeBody::Struct(fields) => {
                self.output.push_str("{ ");
                for (i, (name, ty, req)) in fields.iter().enumerate() {
                    if i > 0 {
                        self.output.push_str(", ");
                    }
                    self.output.push_str(&shape_field_name(name));
                    self.output.push_str(" : ");
                    self.fmt_type_expr(ty);
                    if *req {
                        self.output.push('!');
                    }
                }
                self.output.push_str(" }");
            }
            TypeBody::Alias(ty) => {
                self.fmt_type_expr(ty);
            }
            TypeBody::Entity {
                persistence,
                collection,
                fields,
            } => {
                self.output.push_str("Entity");
                // The `type X = Entity [...]` alias form carries neither
                // `shared` nor `identity` (see parse_type_entity_body).
                self.fmt_persistence_tag(
                    persistence.as_ref(),
                    collection.as_ref().map(|s| s.as_str()),
                    &[],
                );
                self.output.push_str(" {");
                if !fields.is_empty() {
                    self.write_newline();
                    self.indent();
                    for field in fields {
                        self.write_indent();
                        self.fmt_entity_field(field);
                        self.write_newline();
                    }
                    self.dedent();
                    self.write_indent();
                }
                self.output.push('}');
            }
            TypeBody::Trait {
                upstream,
                linked_entity,
            } => {
                self.output.push_str("Trait");
                if let Some(up) = upstream {
                    self.output.push(' ');
                    self.output.push_str(up);
                }
                if let Some(le) = linked_entity {
                    self.output.push_str(" -> ");
                    self.fmt_type_expr(le);
                }
            }
            TypeBody::Page { path, trait_types } => {
                self.output.push_str("Page");
                if let Some(p) = path {
                    self.output.push_str(" -> ");
                    self.output.push_str(p);
                }
                if !trait_types.is_empty() {
                    self.output.push_str(" -> ");
                    for (i, tt) in trait_types.iter().enumerate() {
                        if i > 0 {
                            self.output.push_str(", ");
                        }
                        self.fmt_type_expr(tt);
                    }
                }
            }
            TypeBody::Orbital {
                entity,
                trait_types,
                page_type,
            } => {
                self.output.push_str("Orbital {");
                if entity.is_some() || !trait_types.is_empty() || page_type.is_some() {
                    self.write_newline();
                    self.indent();
                    if let Some(e) = entity {
                        self.write_indent();
                        self.output.push_str("entity: ");
                        self.fmt_type_expr(e);
                        self.write_newline();
                    }
                    if !trait_types.is_empty() {
                        self.write_indent();
                        self.output.push_str("traits: ");
                        for (i, tt) in trait_types.iter().enumerate() {
                            if i > 0 {
                                self.output.push_str(", ");
                            }
                            self.fmt_type_expr(tt);
                        }
                        self.write_newline();
                    }
                    if let Some(p) = page_type {
                        self.write_indent();
                        self.output.push_str("page: ");
                        self.fmt_type_expr(p);
                        self.write_newline();
                    }
                    self.dedent();
                    self.write_indent();
                }
                self.output.push('}');
            }
            TypeBody::Event {
                payload,
                scope,
                description,
            } => {
                self.output.push_str("Event ");
                self.fmt_type_expr(payload);
                if let Some(s) = scope {
                    self.output.push(' ');
                    self.fmt_event_scope(s);
                }
                if let Some(d) = description {
                    self.output.push(' ');
                    self.write_quoted_string(d);
                }
            }
            TypeBody::Composition(parts) => {
                for (i, part) in parts.iter().enumerate() {
                    if i > 0 {
                        self.output.push_str(" & ");
                    }
                    self.fmt_type_body(part);
                }
            }
        }
    }

    /// A type in a position that takes one atom: a union is parenthesized.
    fn fmt_grouped_type_expr(&mut self, ty: &TypeExpr) {
        if matches!(ty, TypeExpr::Union(_)) {
            self.output.push('(');
            self.fmt_type_expr(ty);
            self.output.push(')');
        } else {
            self.fmt_type_expr(ty);
        }
    }

    fn fmt_type_expr(&mut self, ty: &TypeExpr) {
        match ty {
            TypeExpr::EventPayloadOf(PayloadOfTarget::ConfigKnob(knob)) => {
                self.output.push_str(&format!("@payload @config.{knob}"));
            }
            TypeExpr::EventPayloadOf(PayloadOfTarget::SiblingMember(member)) => {
                self.output.push_str(&format!("@payload {member}"));
            }
            TypeExpr::NamedEntityRow(knob) => {
                self.output.push_str(&format!("@entity @config.{knob}"));
            }
            TypeExpr::ExpectedIdentityRow => {
                self.output.push_str("@user");
            }
            TypeExpr::Primitive(name) | TypeExpr::Named(name) => {
                self.output.push_str(name);
            }
            TypeExpr::StringLiteral(s) => {
                self.write_quoted_string(s);
            }
            TypeExpr::BoundEntityRow => {
                self.output.push_str("@entity");
            }
            TypeExpr::Array(inner) => {
                self.output.push('[');
                self.fmt_grouped_type_expr(inner);
                self.output.push(']');
            }
            TypeExpr::Tuple(slots) => {
                self.output.push('[');
                for (i, slot) in slots.iter().enumerate() {
                    if i > 0 {
                        self.output.push_str(", ");
                    }
                    self.fmt_type_expr(slot);
                }
                self.output.push(']');
            }
            TypeExpr::Map(key, value) => {
                // Surface syntax: `Map K V` (juxtaposition).
                self.output.push_str("Map ");
                self.fmt_grouped_type_expr(key);
                self.output.push(' ');
                self.fmt_grouped_type_expr(value);
            }
            TypeExpr::Object(fields) => {
                self.output.push_str("{ ");
                for (i, (name, fty, req)) in fields.iter().enumerate() {
                    if i > 0 {
                        self.output.push_str(", ");
                    }
                    self.output.push_str(&shape_field_name(name));
                    self.output.push_str(" : ");
                    self.fmt_type_expr(fty);
                    if *req {
                        self.output.push('!');
                    }
                }
                self.output.push_str(" }");
            }
            TypeExpr::Union(parts) => {
                for (i, p) in parts.iter().enumerate() {
                    if i > 0 {
                        self.output.push_str(" | ");
                    }
                    self.fmt_type_expr(p);
                }
            }
            TypeExpr::Optional(inner) => {
                self.fmt_grouped_type_expr(inner);
                self.output.push('?');
            }
            TypeExpr::FieldOf { ty, field, .. } => {
                self.output.push_str(ty);
                self.output.push('.');
                self.output.push_str(field);
            }
            TypeExpr::RelationMany(name) => {
                self.output.push_str(name);
                self.output.push('*');
            }
            TypeExpr::RelationManyNonEmpty(name) => {
                self.output.push_str(name);
                self.output.push('+');
            }
            TypeExpr::Trait => {
                self.output.push_str("trait");
            }
            TypeExpr::Orbital => {
                self.output.push_str("orbital");
            }
            TypeExpr::Behavior => {
                self.output.push_str("behavior");
            }
            TypeExpr::EntityRef => {
                self.output.push_str("entity");
            }
            TypeExpr::EventRef => {
                self.output.push_str("event");
            }
            TypeExpr::EventAddress => {
                self.output.push_str("EventAddress");
            }
            TypeExpr::Slot => {
                self.output.push_str("slot");
            }
            TypeExpr::Pattern => {
                self.output.push_str("pattern");
            }
            TypeExpr::Secret => {
                self.output.push_str("secret");
            }
            TypeExpr::SExpr => {
                self.output.push_str("SExpr");
            }
            TypeExpr::Match { knob, arms } => {
                self.output.push_str(&format!("match @config.{knob} {{"));
                self.write_newline();
                self.indent();
                let rendered: Vec<String> = arms
                    .iter()
                    .map(|a| render_match_pattern(&a.pattern))
                    .collect();
                let width = rendered
                    .iter()
                    .map(|s| s.chars().count())
                    .max()
                    .unwrap_or(0);
                for (arm, pat) in arms.iter().zip(rendered.iter()) {
                    self.write_indent();
                    self.output.push_str(pat);
                    for _ in pat.chars().count()..width {
                        self.output.push(' ');
                    }
                    self.output.push_str(" -> ");
                    self.fmt_type_expr(&arm.ty);
                    self.write_newline();
                }
                self.dedent();
                self.write_indent();
                self.output.push('}');
            }
        }
    }

    fn fmt_persistence_tag(
        &mut self,
        persistence: Option<&Persistence>,
        collection: Option<&str>,
        flags: &[(&str, bool)],
    ) {
        let mut tags = Vec::new();
        match (persistence, collection) {
            (Some(Persistence::Persistent), Some(c)) => tags.push(format!("persistent: {c}")),
            (Some(Persistence::Persistent), None) => tags.push("persistent".to_string()),
            (Some(Persistence::Runtime), _) => tags.push("runtime".to_string()),
            (None, _) => {}
        }
        tags.extend(
            flags
                .iter()
                .filter(|(_, on)| *on)
                .map(|(tag, _)| tag.to_string()),
        );
        if !tags.is_empty() {
            self.output.push_str(" [");
            self.output.push_str(&tags.join(", "));
            self.output.push(']');
        }
    }

    fn fmt_entity_field(&mut self, field: &EntityField) {
        self.output.push_str(&field.name);
        self.output.push_str(" : ");
        self.fmt_type_expr(&field.type_expr);
        if field.required {
            self.output.push('!');
        }
        if field.nullable {
            self.output.push('?');
        }
        if field.is_key {
            self.output.push_str(" @key");
        }
        if field.is_unique {
            self.output.push_str(" @unique");
        }
        if field.is_intrinsic {
            self.output.push_str(" @intrinsic");
        }
        if let Some(desc) = &field.description {
            self.output.push_str(" @description ");
            self.write_quoted_string(desc);
        }
        if let Some(syn) = &field.synonyms {
            self.output.push_str(" @synonyms ");
            self.write_quoted_string(syn);
        }
        if let Some(mock) = &field.mock {
            self.output.push_str(" @mock ");
            self.write_quoted_string(mock);
        }
        if let Some(def) = &field.default {
            self.output.push_str(" = ");
            self.fmt_sexpr(def);
        }
        if let Some(derived) = &field.derived {
            self.output.push_str(" = derived ");
            self.fmt_sexpr(derived);
        }
    }

    fn fmt_orbital(&mut self, orbital: &Orbital) {
        // Collect synthesized inline trait references so `@trait.InlineXxxN`
        // strings can be reprinted as `@(...)` sugar and the desugared trait
        // declarations are hidden.
        self.inline_traits = orbital
            .traits
            .iter()
            .filter(|t| t.is_inline_synthetic)
            .map(|t| (t.name.clone(), t.clone()))
            .collect();

        self.write_indent();
        self.output.push_str("orbital ");
        self.output.push_str(&orbital.name);
        self.fmt_modifier_bracket(Self::grant_entries(&orbital.os_grants));
        if let Some(ann) = &orbital.type_annotation {
            self.output.push(' ');
            self.fmt_type_annotation(ann);
        }

        // Reference form (orbital-import Stage A): `orbital Name =
        // Alias.orbitals.Upstream { ... }` — mirrors `fmt_page`'s reference
        // branch. Body braces are always present, even when every override
        // block is empty.
        if let Some(reference) = &orbital.reference {
            self.output.push_str(" = ");
            self.output.push_str(reference);
            self.output.push_str(" {");
            self.write_newline();
            self.indent();
            self.fmt_orbital_ref_body(orbital);
            self.dedent();
            self.write_indent();
            self.output.push('}');
            return;
        }

        self.output.push_str(" {");
        self.write_newline();
        self.indent();

        let mut first = true;
        for u in &orbital.uses {
            if !first {
                self.write_newline();
            }
            first = false;
            self.emit_comments_before(u.span.line);
            self.write_indent();
            self.fmt_use_decl(u);
            self.write_newline();
        }
        for e in &orbital.expects {
            if !first {
                self.write_newline();
            }
            first = false;
            self.emit_comments_before(e.span.line);
            self.write_indent();
            self.fmt_expects_decl(e);
            self.write_newline();
        }
        for ta in &orbital.type_aliases {
            if !first {
                self.write_newline();
            }
            first = false;
            self.emit_comments_before(ta.span.line);
            self.write_indent();
            self.fmt_type_alias(ta);
            self.write_newline();
        }
        for entity in orbital.entity.iter().chain(orbital.extra_entities.iter()) {
            if !first {
                self.write_newline();
            }
            first = false;
            self.emit_comments_before(entity.span.line);
            self.write_indent();
            self.fmt_entity(entity);
            self.write_newline();
        }
        for t in &orbital.traits {
            if t.is_inline_synthetic {
                continue;
            }
            if !first {
                self.write_newline();
            }
            first = false;
            self.emit_comments_before(t.span.line);
            self.write_indent();
            self.fmt_trait(t);
            self.write_newline();
        }
        for p in &orbital.pages {
            if !first {
                self.write_newline();
            }
            first = false;
            self.emit_comments_before(p.span.line);
            self.write_indent();
            self.fmt_page(p);
            self.write_newline();
        }
        if let Some(theme) = &orbital.theme {
            if !first {
                self.write_newline();
            }
            first = false;
            self.write_indent();
            self.fmt_theme_decl(theme);
            self.write_newline();
        }
        if !orbital.config.is_empty() {
            if !first {
                self.write_newline();
            }
            first = false;
            self.write_indent();
            self.output.push_str("config {");
            self.write_newline();
            self.indent();
            for cf in &orbital.config {
                self.write_indent();
                self.fmt_config_field(cf);
                self.write_newline();
            }
            self.dedent();
            self.write_indent();
            self.output.push('}');
            self.write_newline();
        }
        for expr in &orbital.body {
            if !first {
                self.write_newline();
            }
            first = false;
            self.write_indent();
            self.fmt_sexpr(expr);
            self.write_newline();
        }

        self.dedent();
        self.write_indent();
        self.output.push('}');
    }

    /// Body of an orbital-reference declaration (orbital-import Stage A + B)
    /// — `entity` / `entities` / `fields` / `roles` / `pages` (with mount
    /// tails) / `omit` / `only` / `config` / `events` / sibling `trait`
    /// declarations, in EBNF order, each printed only when populated.
    /// Mirrors `fmt_trait_ref_overrides` one scope up.
    fn fmt_orbital_ref_body(&mut self, orbital: &Orbital) {
        let mut wrote_any = false;
        if orbital.entity_rebind.is_some() || orbital.entity_rebind_tags.is_some() {
            self.write_indent();
            self.output.push_str("entity");
            if let Some(entity) = &orbital.entity_rebind {
                self.output.push(' ');
                self.output.push_str(entity);
            }
            if let Some(tags) = &orbital.entity_rebind_tags {
                self.fmt_persistence_tag(
                    tags.persistence.as_ref(),
                    tags.collection.as_deref(),
                    &[
                        ("shared", tags.shared),
                        ("identity", tags.identity),
                        ("local", tags.local),
                        ("mock", tags.mock),
                    ],
                );
            }
            self.write_newline();
            wrote_any = true;
        }
        for e in &orbital.expects {
            self.write_indent();
            self.fmt_expects_decl(e);
            self.write_newline();
            wrote_any = true;
        }
        if !orbital.entities_remap.is_empty() {
            if wrote_any {
                self.write_newline();
            }
            wrote_any = true;
            self.write_indent();
            self.output.push_str("entities {");
            self.write_newline();
            self.indent();
            for (k, v, _) in &orbital.entities_remap {
                self.write_indent();
                self.output.push_str(k);
                self.output.push_str(": ");
                self.output.push_str(v);
                self.write_newline();
            }
            self.dedent();
            self.write_indent();
            self.output.push('}');
            self.write_newline();
        }
        if !orbital.fields_remap.is_empty() {
            if wrote_any {
                self.write_newline();
            }
            wrote_any = true;
            self.write_indent();
            self.output.push_str("fields {");
            self.write_newline();
            self.indent();
            for (k, v, _) in &orbital.fields_remap {
                self.write_indent();
                self.output.push_str(k);
                self.output.push_str(": ");
                self.output.push_str(v);
                self.write_newline();
            }
            self.dedent();
            self.write_indent();
            self.output.push('}');
            self.write_newline();
        }
        // `extend { … }` (Stage C) — printed right after `fields {}`: full
        // entity-field declarations, reusing `fmt_entity_field` (the same
        // shape an `entity { … }` body's own fields print through).
        if !orbital.extend_fields.is_empty() {
            if wrote_any {
                self.write_newline();
            }
            wrote_any = true;
            self.write_indent();
            self.output.push_str("extend {");
            self.write_newline();
            self.indent();
            for field in &orbital.extend_fields {
                self.write_indent();
                self.fmt_entity_field(field);
                self.write_newline();
            }
            self.dedent();
            self.write_indent();
            self.output.push('}');
            self.write_newline();
        }
        if !orbital.retype_fields.is_empty() {
            if wrote_any {
                self.write_newline();
            }
            wrote_any = true;
            self.write_indent();
            self.output.push_str("retype {");
            self.write_newline();
            self.indent();
            for field in &orbital.retype_fields {
                self.write_indent();
                self.fmt_entity_field(field);
                self.write_newline();
            }
            self.dedent();
            self.write_indent();
            self.output.push('}');
            self.write_newline();
        }
        if !orbital.mock_overrides.is_empty() {
            if wrote_any {
                self.write_newline();
            }
            wrote_any = true;
            self.write_indent();
            self.output.push_str("mock {");
            self.write_newline();
            self.indent();
            for (field, samples, _) in &orbital.mock_overrides {
                self.write_indent();
                self.output.push_str(field);
                self.output.push_str(": ");
                self.write_quoted_string(samples);
                self.write_newline();
            }
            self.dedent();
            self.write_indent();
            self.output.push('}');
            self.write_newline();
        }
        if !orbital.import_listens.is_empty() {
            if wrote_any {
                self.write_newline();
            }
            wrote_any = true;
            self.write_indent();
            self.output.push_str("listens {");
            self.write_newline();
            self.indent();
            for listen in &orbital.import_listens {
                self.write_indent();
                self.fmt_listen(listen);
                self.write_newline();
            }
            self.dedent();
            self.write_indent();
            self.output.push('}');
            self.write_newline();
        }
        if !orbital.roles_remap.is_empty() {
            if wrote_any {
                self.write_newline();
            }
            wrote_any = true;
            self.write_indent();
            self.output.push_str("roles {");
            self.write_newline();
            self.indent();
            for (k, targets, _) in &orbital.roles_remap {
                self.write_indent();
                self.output.push_str(k);
                self.output.push_str(": ");
                if targets.len() == 1 {
                    self.output.push_str(&targets[0]);
                } else {
                    self.output.push('[');
                    for (i, t) in targets.iter().enumerate() {
                        if i > 0 {
                            self.output.push_str(", ");
                        }
                        self.output.push_str(t);
                    }
                    self.output.push(']');
                }
                self.write_newline();
            }
            self.dedent();
            self.write_indent();
            self.output.push('}');
            self.write_newline();
        }
        if !orbital.pages_remap.is_empty() {
            if wrote_any {
                self.write_newline();
            }
            wrote_any = true;
            self.write_indent();
            self.output.push_str("pages {");
            self.write_newline();
            self.indent();
            for (k, v, _) in &orbital.pages_remap {
                self.write_indent();
                self.write_quoted_string(k);
                self.output.push_str(": ");
                self.write_quoted_string(v);
                if let Some((_, mount_traits, _)) = orbital.mounts.iter().find(|(up, _, _)| up == k)
                {
                    self.output.push_str(" -> ");
                    for (i, t) in mount_traits.iter().enumerate() {
                        if i > 0 {
                            self.output.push_str(", ");
                        }
                        self.output.push_str(t);
                    }
                }
                if let Some((_, mods)) = orbital.page_modifiers.iter().find(|(up, _)| up == k) {
                    self.output.push_str(" [");
                    self.output
                        .push_str(&head_modifier_sources(mods).join(", "));
                    self.output.push(']');
                }
                self.write_newline();
            }
            self.dedent();
            self.write_indent();
            self.output.push('}');
            self.write_newline();
        }
        if !orbital.omit.is_empty() {
            if wrote_any {
                self.write_newline();
            }
            wrote_any = true;
            self.write_indent();
            self.output.push_str("omit {");
            self.write_newline();
            self.indent();
            for (name, _) in &orbital.omit {
                self.write_indent();
                if name.starts_with('/') {
                    self.output.push_str(&format!("{name:?}"));
                } else {
                    self.output.push_str(name);
                }
                self.write_newline();
            }
            self.dedent();
            self.write_indent();
            self.output.push('}');
            self.write_newline();
        }
        if !orbital.only.is_empty() {
            if wrote_any {
                self.write_newline();
            }
            wrote_any = true;
            self.write_indent();
            self.output.push_str("only {");
            self.write_newline();
            self.indent();
            for name in &orbital.only {
                self.write_indent();
                self.output.push_str(name);
                self.write_newline();
            }
            self.dedent();
            self.write_indent();
            self.output.push('}');
            self.write_newline();
        }
        if !orbital.config_overrides.is_empty() {
            if wrote_any {
                self.write_newline();
            }
            wrote_any = true;
            self.write_indent();
            self.output.push_str("config {");
            self.write_newline();
            self.indent();
            for cf in &orbital.config_overrides {
                self.write_indent();
                self.fmt_config_override_entry(cf);
                self.write_newline();
            }
            self.dedent();
            self.write_indent();
            self.output.push('}');
            self.write_newline();
        }
        if !orbital.events_remap.is_empty() {
            if wrote_any {
                self.write_newline();
            }
            wrote_any = true;
            self.write_indent();
            self.output.push_str("events {");
            self.write_newline();
            self.indent();
            for (k, v) in &orbital.events_remap {
                self.write_indent();
                self.output.push_str(k);
                self.output.push_str(": ");
                self.output.push_str(v);
                self.write_newline();
            }
            self.dedent();
            self.write_indent();
            self.output.push('}');
            self.write_newline();
        }
        if !orbital.trait_overrides.is_empty() {
            if wrote_any {
                self.write_newline();
            }
            wrote_any = true;
            self.write_indent();
            self.output.push_str("traits {");
            self.write_newline();
            self.indent();
            for ov in &orbital.trait_overrides {
                self.write_indent();
                self.output.push_str(&ov.trait_name);
                if let Some(le) = &ov.linked_entity {
                    self.output.push_str(" -> ");
                    self.output.push_str(le);
                }
                self.output.push_str(" {");
                self.write_newline();
                self.indent();
                let ref_data = TraitRefData {
                    reference: ov.trait_name.clone(),
                    name_override: None,
                    events: ov.body.events.clone(),
                    fields: ov.body.fields.clone(),
                    config: ov.body.config.clone(),
                    emits_scope: ov.body.emits_scope.clone(),
                };
                self.fmt_trait_ref_overrides(&ref_data);
                if !ov.body.listens.is_empty() {
                    if !ref_data.events.is_empty()
                        || !ref_data.fields.is_empty()
                        || !ref_data.config.is_empty()
                        || ref_data.emits_scope.is_some()
                    {
                        self.write_newline();
                    }
                    self.write_indent();
                    self.output.push_str("listens {");
                    self.write_newline();
                    self.indent();
                    for listen in &ov.body.listens {
                        self.write_indent();
                        self.fmt_listen(listen);
                        self.write_newline();
                    }
                    self.dedent();
                    self.write_indent();
                    self.output.push('}');
                    self.write_newline();
                }
                self.dedent();
                self.write_indent();
                self.output.push('}');
                self.write_newline();
            }
            self.dedent();
            self.write_indent();
            self.output.push('}');
            self.write_newline();
        }
        // Sibling trait declarations (Stage B) — never prefixed, printed
        // exactly as an inline orbital's own traits are (`fmt_orbital`'s
        // trait loop), skipping any that are inline-synthetic (those
        // reprint as `@(...)` sugar inside their embedder instead).
        for t in &orbital.traits {
            if t.is_inline_synthetic {
                continue;
            }
            if wrote_any {
                self.write_newline();
            }
            wrote_any = true;
            self.emit_comments_before(t.span.line);
            self.write_indent();
            self.fmt_trait(t);
            self.write_newline();
        }
    }

    fn fmt_use_decl(&mut self, u: &UseDecl) {
        self.output.push_str("uses ");
        if u.lazy {
            self.output.push_str("lazy ");
        }
        self.output.push_str(&u.alias);
        self.output.push_str(" from ");
        self.write_quoted_string(&u.path);
        // Optional `{ config { ... } }` override body (orbital-import Stage
        // A) — call-site override of the imported organism's app-level
        // declared `config {}` knobs.
        if !u.config.is_empty() {
            self.output.push_str(" {");
            self.write_newline();
            self.indent();
            self.write_indent();
            self.output.push_str("config {");
            self.write_newline();
            self.indent();
            for cf in &u.config {
                self.write_indent();
                self.fmt_config_override_entry(cf);
                self.write_newline();
            }
            self.dedent();
            self.write_indent();
            self.output.push('}');
            self.write_newline();
            self.dedent();
            self.write_indent();
            self.output.push('}');
        }
    }

    fn fmt_expects_decl(&mut self, e: &ExpectsDecl) {
        self.output.push_str("expects ");
        match &e.kind {
            ExpectsKind::Identity { name, shape } => {
                self.output.push_str("entity ");
                if let Some(name) = name {
                    self.output.push_str(name);
                    self.output.push(' ');
                }
                self.output.push_str("[identity]");
                self.fmt_expects_shape(shape, &ExpectedAccess::default(), &[]);
            }
            ExpectsKind::Entity {
                name,
                shape,
                persistence,
                shared,
                local,
                access,
                invariants,
            } => {
                self.output.push_str("entity ");
                self.output.push_str(name);
                let mut tags: Vec<&str> = Vec::new();
                match persistence {
                    Some(Persistence::Persistent) => tags.push("persistent"),
                    Some(Persistence::Runtime) => tags.push("runtime"),
                    None => {}
                }
                if *shared {
                    tags.push("shared");
                }
                if *local {
                    tags.push("local");
                }
                if !tags.is_empty() {
                    self.output.push_str(" [");
                    self.output.push_str(&tags.join(", "));
                    self.output.push(']');
                }
                self.fmt_expects_shape(shape, access, invariants);
            }
            ExpectsKind::Event {
                trait_name,
                event,
                payload,
            } => {
                self.output.push_str("trait ");
                self.output.push_str(trait_name);
                self.output.push_str(" { emits { ");
                self.output.push_str(event);
                if !payload.is_empty() {
                    self.output.push(' ');
                    self.fmt_event_payload(payload);
                }
                self.output.push_str(" } }");
            }
            ExpectsKind::Page { path, traits } => {
                self.output.push_str("page \"");
                self.output.push_str(path);
                self.output.push('"');
                if !traits.is_empty() {
                    self.output.push_str(" -> ");
                    self.output.push_str(&traits.join(", "));
                }
            }
            ExpectsKind::State { trait_name, state, holds: None } => {
                self.output.push_str(&format!("trait {trait_name} {{ state {state} }}"));
            }
            ExpectsKind::State { trait_name, state, holds: Some(holds) } => {
                self.output.push_str(&format!("trait {trait_name} {{ state {state} @holds "));
                self.fmt_sexpr(holds);
                self.output.push_str(" }");
            }
            ExpectsKind::Transition {
                trait_name,
                from,
                event,
                to,
                ensures,
            } => {
                self.output
                    .push_str(&format!("trait {trait_name} {{ state {from} {{ {event} -> {to}"));
                for p in ensures {
                    self.output.push_str(" @ensures ");
                    self.fmt_sexpr(p);
                }
                self.output.push_str(" } }");
            }
        }
    }

    fn fmt_expects_shape(&mut self, shape: &Option<Vec<EntityField>>, access: &ExpectedAccess, invariants: &[SExpression]) {
        if shape.is_none() && !access.has_any() && invariants.is_empty() {
            return;
        }
        self.output.push_str(" {");
        self.write_newline();
        self.indent();
        self.fmt_access_directives(
            &access.read_policy,
            &access.create_policy,
            &access.update_policy,
            &access.delete_policy,
            &access.access_waivers,
        );
        for field in shape.iter().flatten() {
            self.write_indent();
            self.fmt_entity_field(field);
            self.write_newline();
        }
        for invariant in invariants {
            self.write_indent();
            self.output.push_str("@invariant ");
            self.fmt_sexpr(invariant);
            self.write_newline();
        }
        self.dedent();
        self.write_indent();
        self.output.push('}');
    }

    /// The `@read|@create|@update|@delete` directives of an entity body or an `expects entity` shape: a policy
    /// expression, or the declared omission `@<op> none "<reason>"` in the same slot so a round trip is lossless.
    fn fmt_access_directives(
        &mut self,
        read: &Option<SExpression>,
        create: &Option<SExpression>,
        update: &Option<SExpression>,
        delete: &Option<SExpression>,
        waivers: &[(String, String)],
    ) {
        for (sigil, expr) in [("read", read), ("create", create), ("update", update), ("delete", delete)] {
            if let Some(expr) = expr {
                self.write_indent();
                self.output.push('@');
                self.output.push_str(sigil);
                self.output.push(' ');
                self.fmt_sexpr(expr);
                self.write_newline();
            } else if let Some((_, reason)) = waivers.iter().find(|(op, _)| op == sigil) {
                self.write_indent();
                self.output.push('@');
                self.output.push_str(sigil);
                self.output.push_str(" none ");
                self.output.push('"');
                self.output.push_str(&reason.replace('"', "\\\""));
                self.output.push('"');
                self.write_newline();
            }
        }
    }

    /// `instances [ … ]`, or `instances <lang> [ … ]` for one locale's rows.
    fn fmt_instance_block(
        &mut self,
        locale: Option<&String>,
        rows: &[orbital_core_base::schema::SObject],
    ) {
        self.write_indent();
        match locale {
            Some(l) => self.output.push_str(&format!("instances {l} [")),
            None => self.output.push_str("instances ["),
        }
        self.write_newline();
        self.indent();
        for (i, row) in rows.iter().enumerate() {
            self.write_indent();
            self.fmt_sexpr_object(row);
            if i + 1 < rows.len() {
                self.output.push(',');
            }
            self.write_newline();
        }
        self.dedent();
        self.write_indent();
        self.output.push(']');
        self.write_newline();
    }

    fn fmt_entity(&mut self, entity: &Entity) {
        self.output.push_str("entity ");
        self.output.push_str(&entity.name);
        if let Some(ann) = &entity.type_annotation {
            self.output.push(' ');
            self.fmt_type_annotation(ann);
        }
        self.fmt_persistence_tag(
            entity.persistence.as_ref(),
            entity.collection.as_ref().map(|s| s.as_str()),
            &[
                ("shared", entity.shared),
                ("identity", entity.identity),
                ("local", entity.local),
                ("mock", entity.seed_mock),
            ],
        );
        self.output.push_str(" {");
        let has_access_directives = entity.read_policy.is_some()
            || entity.create_policy.is_some()
            || entity.update_policy.is_some()
            || entity.delete_policy.is_some()
            || !entity.access_waivers.is_empty();
        if !entity.fields.is_empty()
            || has_access_directives
            || !entity.invariants.is_empty()
            || !entity.instances.is_empty()
            || !entity.locale_instances.is_empty()
        {
            self.write_newline();
            self.indent();
            self.fmt_access_directives(
                &entity.read_policy,
                &entity.create_policy,
                &entity.update_policy,
                &entity.delete_policy,
                &entity.access_waivers,
            );
            for field in &entity.fields {
                self.write_indent();
                self.fmt_entity_field(field);
                self.write_newline();
            }
            for invariant in &entity.invariants {
                self.write_indent();
                self.output.push_str("@invariant ");
                self.fmt_sexpr(invariant);
                self.write_newline();
            }
            if !entity.instances.is_empty() {
                self.fmt_instance_block(None, &entity.instances);
            }
            for (locale, rows) in &entity.locale_instances {
                self.fmt_instance_block(Some(locale), rows);
            }
            self.dedent();
            self.write_indent();
        }
        self.output.push('}');
    }

    fn fmt_trait(&mut self, tr: &Trait) {
        self.output.push_str("trait ");
        self.output.push_str(&tr.name);
        if !tr.type_params.is_empty() {
            self.output.push_str(" (");
            for (i, p) in tr.type_params.iter().enumerate() {
                if i > 0 {
                    self.output.push_str(", ");
                }
                self.output.push_str(&p.name);
                self.output.push_str(" : ");
                self.output.push_str(&p.kind);
                if let Some(default) = &p.default {
                    self.output.push_str(" = ");
                    self.output.push_str(default);
                }
            }
            self.output.push(')');
        }
        if let Some(ann) = &tr.type_annotation {
            self.output.push(' ');
            self.fmt_type_annotation(ann);
        }

        if let Some(ref_data) = &tr.reference {
            self.output.push_str(" = ");
            self.output.push_str(&ref_data.reference);
            if let Some(le) = &tr.linked_entity {
                self.output.push_str(" -> ");
                self.output.push_str(le);
            }
            if tr.local {
                self.output.push_str(" [local]");
            }
            self.output.push_str(" {");
            self.write_newline();
            self.indent();
            self.fmt_trait_ref_overrides(ref_data);
            if !tr.listens.is_empty() {
                if !ref_data.events.is_empty()
                    || !ref_data.fields.is_empty()
                    || !ref_data.config.is_empty()
                    || ref_data.emits_scope.is_some()
                {
                    self.write_newline();
                }
                self.write_indent();
                self.output.push_str("listens {");
                self.write_newline();
                self.indent();
                for listen in &tr.listens {
                    self.write_indent();
                    self.fmt_listen(listen);
                    self.write_newline();
                }
                self.dedent();
                self.write_indent();
                self.output.push('}');
                self.write_newline();
            }
            self.dedent();
            self.write_indent();
            self.output.push('}');
            return;
        }

        if let Some(le) = &tr.linked_entity {
            self.output.push_str(" -> ");
            if tr.entity_rebindable {
                self.output.push_str("@rebindable ");
            }
            self.output.push_str(le);
        } else if tr.entity_rebindable {
            self.output.push_str(" -> @rebindable");
        }
        if let Some(desc) = &tr.entity_binding_description {
            self.write_newline();
            self.indent();
            self.write_indent();
            self.output.push_str("@description ");
            self.write_quoted_string(desc);
            self.dedent();
        }
        if let Some(syn) = &tr.entity_binding_synonyms {
            if tr.entity_binding_description.is_none() {
                self.write_newline();
                self.indent();
            } else {
                self.write_newline();
            }
            self.write_indent();
            self.output.push_str("@synonyms ");
            self.write_quoted_string(syn);
            if tr.entity_binding_description.is_none() {
                self.dedent();
            } else {
                self.dedent();
            }
        }

        self.output.push_str(" [");
        let mut parts = Vec::new();
        if let Some(cat) = &tr.category {
            parts.push(cat.clone());
        }
        if let Some(scope) = &tr.scope {
            parts.push(match scope {
                TraitScope::Instance => "instance".to_string(),
                TraitScope::Collection => "collection".to_string(),
            });
        }
        if tr.local {
            parts.push("local".to_string());
        }
        for cap in &tr.capabilities {
            parts.push(cap.clone());
        }
        for (i, p) in parts.iter().enumerate() {
            if i > 0 {
                self.output.push_str(", ");
            }
            self.output.push_str(p);
        }
        self.output.push(']');

        self.output.push_str(" {");
        self.write_newline();
        self.indent();

        if let Some(init) = &tr.initial_state {
            self.write_indent();
            self.output.push_str("initial: ");
            self.output.push_str(init);
            self.write_newline();
        }

        for (i, state) in tr.states.iter().enumerate() {
            if i > 0 || tr.initial_state.is_some() {
                self.write_newline();
            }
            self.emit_comments_before(state.span.line);
            self.write_indent();
            self.fmt_state(state);
            self.write_newline();
        }

        if !tr.emits.is_empty() {
            if tr.initial_state.is_some() || !tr.states.is_empty() {
                self.write_newline();
            }
            self.write_indent();
            self.output.push_str("emits {");
            self.write_newline();
            self.indent();
            for emit in &tr.emits {
                self.write_indent();
                self.fmt_emit(emit);
                self.write_newline();
            }
            self.dedent();
            self.write_indent();
            self.output.push('}');
            self.write_newline();
        }

        if !tr.listens.is_empty() {
            if tr.initial_state.is_some() || !tr.states.is_empty() || !tr.emits.is_empty() {
                self.write_newline();
            }
            self.write_indent();
            self.output.push_str("listens {");
            self.write_newline();
            self.indent();
            for listen in &tr.listens {
                self.write_indent();
                self.fmt_listen(listen);
                self.write_newline();
            }
            self.dedent();
            self.write_indent();
            self.output.push('}');
            self.write_newline();
        }

        if !tr.ticks.is_empty() {
            if tr.initial_state.is_some()
                || !tr.states.is_empty()
                || !tr.emits.is_empty()
                || !tr.listens.is_empty()
            {
                self.write_newline();
            }
            self.write_indent();
            self.output.push_str("ticks {");
            self.write_newline();
            self.indent();
            for tick in &tr.ticks {
                self.write_indent();
                self.fmt_tick(tick);
                self.write_newline();
            }
            self.dedent();
            self.write_indent();
            self.output.push('}');
            self.write_newline();
        }

        if !tr.config.is_empty() {
            if tr.initial_state.is_some()
                || !tr.states.is_empty()
                || !tr.emits.is_empty()
                || !tr.listens.is_empty()
                || !tr.ticks.is_empty()
            {
                self.write_newline();
            }
            self.write_indent();
            self.output.push_str("config {");
            self.write_newline();
            self.indent();
            for cfg in &tr.config {
                self.write_indent();
                self.fmt_config_field(cfg);
                self.write_newline();
            }
            self.dedent();
            self.write_indent();
            self.output.push('}');
            self.write_newline();
        }

        self.dedent();
        self.write_indent();
        self.output.push('}');
    }

    fn fmt_trait_ref_overrides(&mut self, ref_data: &TraitRefData) {
        if let Some(name) = &ref_data.name_override {
            self.write_indent();
            self.output.push_str("name: \"");
            self.output.push_str(name);
            self.output.push('"');
            self.write_newline();
        }
        if !ref_data.events.is_empty() {
            self.write_indent();
            self.output.push_str("events {");
            self.write_newline();
            self.indent();
            for (k, v) in &ref_data.events {
                self.write_indent();
                self.output.push_str(k);
                self.output.push_str(": ");
                self.output.push_str(v);
                self.write_newline();
            }
            self.dedent();
            self.write_indent();
            self.output.push('}');
            self.write_newline();
        }
        if !ref_data.fields.is_empty() {
            if !ref_data.events.is_empty() {
                self.write_newline();
            }
            self.write_indent();
            self.output.push_str("fields {");
            self.write_newline();
            self.indent();
            for (k, v) in &ref_data.fields {
                self.write_indent();
                self.output.push_str(k);
                self.output.push_str(": ");
                self.write_quoted_string(v);
                self.write_newline();
            }
            self.dedent();
            self.write_indent();
            self.output.push('}');
            self.write_newline();
        }
        if !ref_data.config.is_empty() {
            if !ref_data.events.is_empty() || !ref_data.fields.is_empty() {
                self.write_newline();
            }
            self.write_indent();
            self.output.push_str("config {");
            self.write_newline();
            self.indent();
            for cf in &ref_data.config {
                self.write_indent();
                self.fmt_config_override_entry(cf);
                self.write_newline();
            }
            self.dedent();
            self.write_indent();
            self.output.push('}');
            self.write_newline();
        }
        if let Some(scope) = &ref_data.emits_scope {
            if !ref_data.events.is_empty()
                || !ref_data.fields.is_empty()
                || !ref_data.config.is_empty()
            {
                self.write_newline();
            }
            self.write_indent();
            self.output.push_str("emitsScope ");
            self.output.push_str(scope);
            self.write_newline();
        }
    }

    fn fmt_state(&mut self, state: &State) {
        self.output.push_str("state ");
        self.output.push_str(&state.name);
        if let Some(ms) = state.time_bound {
            self.output.push_str(" for ");
            self.output.push_str(&fmt_duration(ms));
        }
        self.output.push_str(" {");
        if !state.transitions.is_empty() || state.holds.is_some() {
            self.write_newline();
            self.indent();
            if let Some(holds) = &state.holds {
                self.write_indent();
                self.output.push_str("@holds ");
                self.fmt_sexpr(holds);
                self.write_newline();
            }
            for tr in &state.transitions {
                self.emit_comments_before(tr.span.line);
                self.write_indent();
                self.fmt_transition(tr);
                self.write_newline();
            }
            self.dedent();
            self.write_indent();
        }
        self.output.push('}');
    }

    fn fmt_transition(&mut self, tr: &Transition) {
        let start = self.output.len();
        self.fmt_transition_body(tr);
        self.transition_ranges.push((start, self.output.len()));
    }

    fn fmt_transition_body(&mut self, tr: &Transition) {
        self.output.push_str(&tr.event);
        self.output.push_str(" -> ");
        self.output.push_str(&tr.target);
        if let Some(guard) = &tr.guard {
            self.output.push_str(" when ");
            self.fmt_sexpr(guard);
        }
        if !tr.ensures.is_empty() {
            self.indent();
            for p in &tr.ensures {
                self.write_newline();
                self.write_indent();
                self.output.push_str("@ensures ");
                self.fmt_sexpr(p);
            }
            if tr.term_match.is_some() {
                self.write_newline();
                self.write_indent();
            }
            self.dedent();
        }
        if let Some(tm) = &tr.term_match {
            if tr.ensures.is_empty() {
                self.output.push(' ');
            }
            self.fmt_term_match(tm);
            return;
        }
        if !tr.effects.is_empty() {
            self.write_newline();
            self.indent();
            for (i, eff) in tr.effects.iter().enumerate() {
                self.write_indent();
                self.fmt_sexpr(eff);
                if i < tr.effects.len() - 1 {
                    self.write_newline();
                }
            }
            self.dedent();
        }
    }

    /// `match @entity.<f> { … }` / `match @payload.<f> { … }` transition
    /// sugar (Phase 2). `{` opens on the `match` line — materializer-safe,
    /// mirroring `fmt_type_expr`'s `TypeExpr::Match` printing.
    fn fmt_term_match(&mut self, tm: &TermMatch) {
        self.output.push_str("match ");
        self.output.push_str(&tm.scrutinee.sigil());
        self.output.push_str(" {");
        self.write_newline();
        self.indent();
        for arm in &tm.arms {
            self.write_indent();
            self.output.push_str(&render_match_pattern(&arm.pattern));
            self.output.push_str(" ->");
            if !arm.effects.is_empty() {
                self.write_newline();
                self.indent();
                for (i, eff) in arm.effects.iter().enumerate() {
                    self.write_indent();
                    self.fmt_sexpr(eff);
                    if i < arm.effects.len() - 1 {
                        self.write_newline();
                    }
                }
                self.dedent();
            }
            self.write_newline();
        }
        self.dedent();
        self.write_indent();
        self.output.push('}');
    }

    fn fmt_emit(&mut self, emit: &Emit) {
        self.output.push_str(&emit.event);
        if let Some(scope) = &emit.scope {
            self.output.push_str(" -> ");
            self.output.push_str(scope);
        }
        if !emit.payload.is_empty() {
            self.output.push(' ');
            self.fmt_event_payload(&emit.payload);
        }
        if let Some(desc) = &emit.description {
            self.fmt_event_annotation("@description", desc);
        }
        if let Some(syn) = &emit.synonyms {
            self.fmt_event_annotation("@synonyms", syn);
        }
        if let Some(tier) = &emit.tier {
            self.fmt_event_annotation("@tier", tier);
        }
    }

    /// Print a trailing `@key "value"` annotation on its own indented line,
    /// the multi-line shape emit/listen declarations use.
    fn fmt_event_annotation(&mut self, key: &str, value: &str) {
        self.write_newline();
        self.indent();
        self.write_indent();
        self.output.push_str(key);
        self.output.push(' ');
        self.write_quoted_string(value);
        self.dedent();
    }

    fn fmt_listen(&mut self, listen: &Listen) {
        self.output.push_str(&listen.event);
        if let Some(triggers) = &listen.triggers {
            self.output.push_str(" -> ");
            if let Some(target) = &listen.target_trait {
                self.output.push_str(target);
                self.output.push('.');
            }
            self.output.push_str(triggers);
        }
        if let Some(guard) = &listen.guard {
            self.output.push_str(" when ");
            self.fmt_sexpr(guard);
        }
        if !listen.payload_mapping.is_empty() {
            self.output.push_str(" with ");
            self.output.push_str("{");
            self.write_newline();
            self.indent();
            for (k, v) in &listen.payload_mapping {
                self.write_indent();
                self.output.push_str(k);
                self.output.push_str(": ");
                self.fmt_sexpr(v);
                self.write_newline();
            }
            self.dedent();
            self.write_indent();
            self.output.push('}');
        }
        if let (Some(inbound), None) = (&listen.inbound, &listen.triggers) {
            self.output.push_str(" -> inbound ");
            self.write_quoted_string(&inbound.name);
            self.output.push_str(" answers ");
            self.output.push_str(&inbound.answers);
            if let Some(v) = &inbound.verifies {
                self.output.push_str(" verifies ");
                self.write_quoted_string(&v.header);
                self.output.push(' ');
                self.write_quoted_string(&v.credential);
            }
            self.output.push(' ');
            self.fmt_event_payload(&listen.payload);
        } else if listen.external && listen.triggers.is_none() {
            self.output.push_str(" -> external ");
            self.fmt_event_payload(&listen.payload);
        } else if !listen.payload.is_empty() && listen.triggers.is_none() {
            self.output.push(' ');
            self.fmt_event_payload(&listen.payload);
        }
        if let Some(desc) = &listen.description {
            self.fmt_event_annotation("@description", desc);
        }
        if let Some(syn) = &listen.synonyms {
            self.fmt_event_annotation("@synonyms", syn);
        }
        if let Some(tier) = &listen.tier {
            self.fmt_event_annotation("@tier", tier);
        }
    }

    fn fmt_tick(&mut self, tick: &Tick) {
        self.output.push_str(&tick.name);
        self.output.push_str(" every ");
        match &tick.interval {
            TickInterval::Millis(ms) => {
                self.output.push_str(&fmt_duration(*ms));
            }
            TickInterval::Cron(cron) => {
                self.write_quoted_string(cron);
            }
        }
        if tick.background {
            self.output.push_str(" [background]");
        }
        if tick.visible {
            self.output.push_str(" [visible]");
        }
        if let Some(guard) = &tick.guard {
            self.output.push_str(" when ");
            self.fmt_sexpr(guard);
        }
        if !tick.effects.is_empty() {
            self.write_newline();
            self.indent();
            for eff in &tick.effects {
                self.write_indent();
                self.fmt_sexpr(eff);
                self.write_newline();
            }
            self.dedent();
            self.write_indent();
        }
    }

    /// One entry of a call-site `config {}` OVERRIDE block — plain wiring
    /// (`key: value`) or a full re-declaration (`key : type = default
    /// @annotations`), same distinction the parser draws
    /// (`has_equals_before_entry_end`). Caller owns indentation/newline.
    /// Shared by every override site: a trait reference's `config {}`, an
    /// orbital reference's `config {}`, and (orbital-import Stage A) a
    /// `uses { config {} }` override.
    fn fmt_config_override_entry(&mut self, cf: &ConfigField) {
        let is_plain_wiring = matches!(&cf.type_expr, TypeExpr::Primitive(s) if s == "unknown")
            && cf.label.is_none()
            && cf.description.is_none()
            && cf.synonyms.is_none()
            && cf.tier.is_none();
        if is_plain_wiring {
            // Compact wiring form: `key: value`
            self.output.push_str(&cf.name);
            self.output.push_str(": ");
            if let Some(v) = &cf.default {
                self.fmt_object_value(&cf.name, v);
            }
        } else {
            // Full declaration form with type and/or annotations.
            self.fmt_config_field(cf);
        }
    }

    fn fmt_config_field(&mut self, cfg: &ConfigField) {
        self.output.push_str(&cfg.name);
        self.output.push_str(" : ");
        self.fmt_type_expr(&cfg.type_expr);
        if cfg.required {
            self.output.push('!');
        }
        if let Some(def) = &cfg.default {
            self.output.push_str(" = ");
            self.fmt_object_value(&cfg.name, def);
        }
        if let Some(label) = &cfg.label {
            self.write_newline();
            self.indent();
            self.write_indent();
            self.output.push_str("@label ");
            self.write_quoted_string(label);
            self.dedent();
        }
        if let Some(desc) = &cfg.description {
            self.write_newline();
            self.indent();
            self.write_indent();
            self.output.push_str("@description ");
            self.write_quoted_string(desc);
            self.dedent();
        }
        if let Some(syn) = &cfg.synonyms {
            self.write_newline();
            self.indent();
            self.write_indent();
            self.output.push_str("@synonyms ");
            self.write_quoted_string(syn);
            self.dedent();
        }
        if let Some(tier) = &cfg.tier {
            self.write_newline();
            self.indent();
            self.write_indent();
            self.output.push_str("@tier ");
            self.write_quoted_string(tier);
            self.dedent();
        }
    }

    fn fmt_event_payload(&mut self, payload: &[EventField]) {
        self.output.push_str("{");
        self.write_newline();
        self.indent();
        for field in payload {
            self.write_indent();
            self.output.push_str(&field.name);
            self.output.push_str(" : ");
            self.fmt_type_expr(&field.type_expr);
            if field.required {
                self.output.push('!');
            }
            self.write_newline();
        }
        self.dedent();
        self.write_indent();
        self.output.push('}');
    }

    fn fmt_page(&mut self, page: &Page) {
        self.output.push_str("page ");
        self.write_quoted_string(&page.path);
        if let Some(ann) = &page.type_annotation {
            self.output.push(' ');
            self.fmt_type_annotation(ann);
        }
        if let Some(name) = &page.name {
            self.output.push_str(" as ");
            self.output.push_str(name);
        }
        if let Some(reference) = &page.reference {
            self.output.push_str(" = ");
            self.output.push_str(reference);
        }
        let visible_traits: Vec<&String> = page
            .traits
            .iter()
            .filter(|t| !self.inline_traits.contains_key(t.as_str()))
            .collect();
        if !visible_traits.is_empty() || page.primary_entity.is_some() {
            self.output.push_str(" -> ");
            for (i, t) in visible_traits.iter().enumerate() {
                if i > 0 {
                    self.output.push_str(", ");
                }
                self.output.push_str(t);
            }
        }
        let mut opts = Vec::new();
        if let Some(view) = &page.view {
            opts.push(format!("view: {}", view));
        }
        if let Some(entity) = &page.primary_entity {
            opts.push(format!("entity: {}", entity));
        }
        if page.is_initial {
            opts.push("initial".to_string());
        }
        opts.extend(head_modifier_sources(
            &orbital_core_base::schema::PageModifiers {
                access: page.access,
                indexing: page.indexing,
                title: page.title.clone(),
                description: page.description.clone(),
                translation_of: page.translation_of.clone(),
                label: None,
                icon: None,
                roles: None,
            },
        ));
        let multiline = page.access.is_some()
            || page.indexing.is_some()
            || page.title.is_some()
            || page.description.is_some()
            || page.translation_of.is_some();
        if multiline {
            self.output.push_str(" [");
            self.write_newline();
            self.indent();
            let last = opts.len() - 1;
            for (i, opt) in opts.iter().enumerate() {
                self.write_indent();
                self.output.push_str(opt);
                if i != last {
                    self.output.push(',');
                }
                self.write_newline();
            }
            self.dedent();
            self.write_indent();
            self.output.push(']');
        } else if !opts.is_empty() {
            self.output.push_str(" [");
            for (i, opt) in opts.iter().enumerate() {
                if i > 0 {
                    self.output.push_str(", ");
                }
                self.output.push_str(opt);
            }
            self.output.push(']');
        }
        if let Some(icon) = &page.icon {
            self.write_newline();
            self.write_indent();
            self.output.push_str("  @icon ");
            self.write_quoted_string(icon);
        }
        if let Some(label) = &page.label {
            self.write_newline();
            self.write_indent();
            self.output.push_str("  @label ");
            self.write_quoted_string(label);
        }
        if let Some(roles) = &page.roles {
            self.write_newline();
            self.write_indent();
            self.output.push_str("  @roles ");
            self.write_quoted_string(&roles.join(", "));
        }
    }

    fn fmt_type_annotation(&mut self, ann: &TypeAnnotation) {
        self.output.push_str(":: ");
        self.output.push_str(&ann.type_name);
        if !ann.type_args.is_empty() {
            self.output.push(' ');
            for (i, arg) in ann.type_args.iter().enumerate() {
                if i > 0 {
                    self.output.push(' ');
                }
                self.output.push_str(arg);
            }
        }
    }

    fn fmt_event_scope(&mut self, scope: &EventScopeAst) {
        match scope {
            EventScopeAst::Internal => self.output.push_str("[internal]"),
            EventScopeAst::External => self.output.push_str("[external]"),
        }
    }

    // ------------------------------------------------------------------
    // S-expressions
    // ------------------------------------------------------------------

    fn fmt_sexpr(&mut self, expr: &SExpression) {
        self.fmt_sexpr_inner(expr, false);
    }

    fn fmt_sexpr_inner(&mut self, expr: &SExpression, force_paren: bool) {
        match expr {
            SExpression::Null => self.output.push_str("null"),
            SExpression::Bool(b) => self.output.push_str(if *b { "true" } else { "false" }),
            SExpression::Number(n) => {
                if n.fract() == 0.0 {
                    self.output.push_str(&format!("{:.0}", n));
                } else {
                    self.output.push_str(&format!("{}", n));
                }
            }
            SExpression::String(s) => {
                self.fmt_sexpr_string(s);
            }
            SExpression::Array(arr) => {
                self.fmt_sexpr_array(arr, force_paren);
            }
            SExpression::Object(map) => {
                self.fmt_sexpr_object(map);
            }
        }
    }

    fn fmt_sexpr_string(&mut self, s: &str) {
        // Reconstruct `@(...)` inline trait references from their synthesized
        // `@trait.InlineXxxN` form. This must run before the generic sigil
        // handling so the formatter preserves the original sugar.
        if let Some(name) = s.strip_prefix("@trait.") {
            if let Some(tr) = self.inline_traits.get(name).cloned() {
                self.fmt_inline_trait_ref(&tr);
                return;
            }
        }
        // Canonical payload sigil form
        if s == "@payload" {
            self.output.push('?');
            return;
        }
        if let Some(rest) = s.strip_prefix("@payload.") {
            self.output.push('?');
            self.output.push_str(rest);
            return;
        }
        // Sigils (clean dotted paths only), bare atoms, and symbolic operator
        // heads (`==`, `+`, `<=`, …) print unquoted. Anything else — including
        // a sigil with an array index like `@config.x[0].y`, which the sigil
        // reader can't consume bare — must be quoted to round-trip.
        if (is_bare_sigil(s) || is_bare_atom(s)) && !self.is_lexically_bound(s) {
            self.output.push_str(s);
            return;
        }
        // Otherwise quote it
        self.write_quoted_string(s);
    }

    fn fmt_inline_trait_ref(&mut self, tr: &Trait) {
        self.output.push_str("@(");
        if let Some(ref_data) = &tr.reference {
            self.output.push_str(&ref_data.reference);
        }
        if let Some(le) = &tr.linked_entity {
            self.output.push_str(" -> ");
            self.output.push_str(le);
        }
        self.output.push(' ');
        self.fmt_inline_trait_ref_body(tr);
        self.output.push(')');
    }

    fn fmt_inline_trait_ref_body(&mut self, tr: &Trait) {
        let ref_data = tr
            .reference
            .as_ref()
            .expect("inline synthetic trait must carry a reference");

        // Complex bodies (listens, emitsScope, or annotated config fields) are
        // emitted multi-line using the same layout as a top-level trait-ref
        // declaration so annotations and nested blocks stay readable.
        let needs_multiline = !tr.listens.is_empty()
            || ref_data.emits_scope.is_some()
            || ref_data.config.iter().any(|cf| {
                !matches!(&cf.type_expr, TypeExpr::Primitive(s) if s == "unknown")
                    || cf.label.is_some()
                    || cf.description.is_some()
                    || cf.synonyms.is_some()
                    || cf.tier.is_some()
            });

        if needs_multiline {
            self.output.push_str("{");
            self.write_newline();
            self.indent();
            self.fmt_trait_ref_overrides(ref_data);
            // `listens` live on the Trait, not on TraitRefData, so the
            // overrides helper does not emit them — append them here.
            if !tr.listens.is_empty() {
                let had_overrides = !ref_data.events.is_empty()
                    || !ref_data.fields.is_empty()
                    || !ref_data.config.is_empty()
                    || ref_data.emits_scope.is_some();
                if had_overrides {
                    self.write_newline();
                }
                self.write_indent();
                self.output.push_str("listens {");
                self.write_newline();
                self.indent();
                for listen in &tr.listens {
                    self.write_indent();
                    self.fmt_listen(listen);
                    self.write_newline();
                }
                self.dedent();
                self.write_indent();
                self.output.push('}');
                self.write_newline();
            }
            self.dedent();
            self.write_indent();
            self.output.push('}');
            return;
        }

        // Compact form for the common config-only case.
        self.output.push_str("{");
        let mut first = true;
        if !ref_data.events.is_empty() {
            first = false;
            self.output.push_str(" events { ");
            for (i, (k, v)) in ref_data.events.iter().enumerate() {
                if i > 0 {
                    self.output.push_str(", ");
                }
                self.output.push_str(k);
                self.output.push_str(": ");
                self.output.push_str(v);
            }
            self.output.push_str(" }");
        }
        if !ref_data.fields.is_empty() {
            if !first {
                self.output.push_str(", ");
            }
            first = false;
            self.output.push_str(" fields { ");
            for (i, (k, v)) in ref_data.fields.iter().enumerate() {
                if i > 0 {
                    self.output.push_str(", ");
                }
                self.output.push_str(k);
                self.output.push_str(": ");
                self.write_quoted_string(v);
            }
            self.output.push_str(" }");
        }
        if !ref_data.config.is_empty() {
            if !first {
                self.output.push_str(", ");
            }
            first = false;
            self.output.push_str(" config { ");
            for (i, cf) in ref_data.config.iter().enumerate() {
                if i > 0 {
                    self.output.push_str(", ");
                }
                self.output.push_str(&cf.name);
                self.output.push_str(": ");
                if let Some(v) = &cf.default {
                    self.fmt_object_value(&cf.name, v);
                }
            }
            self.output.push_str(" }");
        }
        if !tr.listens.is_empty() {
            if !first {
                self.output.push_str(", ");
            }
            first = false;
            self.output.push_str(" listens { ");
            for (i, listen) in tr.listens.iter().enumerate() {
                if i > 0 {
                    self.output.push_str(", ");
                }
                self.fmt_listen(listen);
            }
            self.output.push_str(" }");
        }
        if let Some(scope) = &ref_data.emits_scope {
            if !first {
                self.output.push_str(", ");
            }
            self.output.push_str("emitsScope ");
            self.output.push_str(scope);
        }
        self.output.push_str(" }");
    }

    // ------------------------------------------------------------------
    // JSX render-tree formatting
    // ------------------------------------------------------------------
    //
    // Render element trees (the pattern argument to `render-ui` and any
    // `children` nested within) format as JSX — the canonical authoring
    // surface. Three element kinds, mirroring the parser's desugar:
    //   - Object with a string `type` key        → `<Type …>` pattern element
    //   - `"@trait.<Name>"` (synthetic inline)    → `<Alias.traits.X …>` element
    //   - `"@trait.<Name>"` (named reference)     → `<trait.Name />`
    // Anything else falls through to ordinary s-expression formatting.

    fn fmt_render_pattern(&mut self, expr: &SExpression) {
        // A render tree converts to JSX only if it is *recursively*
        // JSX-renderable: static-typed pattern objects with array (or no)
        // children, and inline trait refs without `listens`. Any subtree that
        // can't be JSX (single-object `children`, dynamic `type`, `listens`, a
        // non-array children config) makes the whole block fall back to
        // object-literal / `@(...)` formatting so the lowered `.orb` is
        // preserved exactly and no bare `{...}` leaks into a JSX child slot.
        if !self.is_jsx_renderable(expr) {
            self.fmt_sexpr(expr);
            return;
        }
        match expr {
            SExpression::Object(map) => {
                self.fmt_jsx_pattern(map);
            }
            SExpression::String(s) if s.starts_with("@trait.") => {
                let name = &s["@trait.".len()..];
                if let Some(tr) = self.inline_traits.get(name).cloned() {
                    self.fmt_jsx_inline_trait(&tr);
                } else {
                    self.output.push_str("<trait.");
                    self.output.push_str(name);
                    self.output.push_str(" />");
                }
            }
            SExpression::Array(arr) => {
                // A bare children-list in render position (rare).
                if arr.is_empty() {
                    self.output.push_str("[]");
                } else {
                    self.output.push('[');
                    self.write_newline();
                    self.indent();
                    for item in arr {
                        self.write_indent();
                        self.fmt_render_pattern(item);
                        self.write_newline();
                    }
                    self.dedent();
                    self.write_indent();
                    self.output.push(']');
                }
            }
            other => self.fmt_sexpr(other),
        }
    }

    /// True when `expr` (and, recursively, every nested child) can be emitted
    /// as JSX without changing the lowered `.orb`. The blockers are:
    ///   - a pattern object whose `type` is not a static lowercase name
    ///     (e.g. `@config.detailPattern`);
    ///   - `children` that is a single object (JSX only has array children);
    ///   - an inline trait ref carrying a `listens` override, or a non-array
    ///     `children` config.
    /// When this returns false the whole subtree stays object-literal / `@(...)`.
    fn is_jsx_renderable(&self, expr: &SExpression) -> bool {
        match expr {
            SExpression::Object(map) => {
                let static_type = matches!(
                    map.get("type"),
                    Some(SExpression::String(s)) if is_static_pattern_type(s)
                );
                if !static_type {
                    return false;
                }
                // `children` may be an array, a single object (normalized to a
                // one-element array by the emitter), or absent.
                match map.get("children") {
                    None => true,
                    Some(SExpression::Array(arr)) => arr.iter().all(|c| self.is_jsx_child(c)),
                    Some(single) => self.is_jsx_child(single),
                }
            }
            SExpression::String(s) if s.starts_with("@trait.") => {
                let name = &s["@trait.".len()..];
                if let Some(tr) = self.inline_traits.get(name) {
                    // An inline trait *reference* only remaps events from the
                    // referenced atom, so its `listens` is inherently Shape A
                    // (`Source.EVENT -> TRIGGER`). It is JSX-renderable iff
                    // every listen carries a trigger; any trigger-less
                    // (Shape B) entry — a payload declaration that a reference
                    // shouldn't own — falls back to `@(...)`.
                    let listens_ok = tr.listens.iter().all(|l| l.triggers.is_some());
                    listens_ok && {
                        let child_cfg = tr
                            .reference
                            .as_ref()
                            .and_then(|r| r.config.iter().find(|c| c.name == "children"))
                            .and_then(|c| c.default.as_ref());
                        match child_cfg {
                            None => true,
                            Some(SExpression::Array(arr)) => {
                                arr.iter().all(|c| self.is_jsx_child(c))
                            }
                            Some(single) => self.is_jsx_child(single),
                        }
                    }
                } else {
                    true
                }
            }
            SExpression::Array(arr) => arr.iter().all(|c| self.is_jsx_renderable(c)),
            _ => true,
        }
    }

    /// A JSX child slot only holds elements: a pattern object or a `@trait.X`
    /// embed. A binding, a call or a nested list there has no JSX spelling.
    fn is_jsx_child(&self, child: &SExpression) -> bool {
        match child {
            SExpression::Object(_) => self.is_jsx_renderable(child),
            SExpression::String(s) if s.starts_with("@trait.") => self.is_jsx_renderable(child),
            _ => false,
        }
    }

    /// Format an anonymous pattern object as `<Type attrs>children</Type>`
    /// (or `<Type attrs />` when childless). The `type` key becomes the tag
    /// name (PascalCased); `children` becomes nested JSX; every other key is
    /// a sorted attribute.
    fn fmt_jsx_pattern(&mut self, map: &orbital_core_base::schema::SObject) {
        let type_str = match map.get("type") {
            Some(SExpression::String(s)) => s.clone(),
            _ => return self.fmt_sexpr_object(map),
        };
        let tag = pascal_case(&type_str);

        // Normalize children to an array: a single-object child
        // (`children: { … }`) is treated as a one-element array so JSX — which
        // only has sequence children — can represent it as `<Tag><Child/></Tag>`.
        // `children_present` tracks whether the KEY exists at all, separate
        // from whether the (possibly empty) array itself is empty: a present
        // `children: []` is a deliberate "render nothing here" the pattern
        // layer distinguishes from no children slot set at all (absent key)
        // — collapsing both to "no key" made the round trip lossy (`{"children":[]}`
        // -> `<Tag />` -> `{}`, dropping the explicit empty).
        let children_present = map.contains_key("children");
        let children_owned: Vec<SExpression> = match map.get("children") {
            Some(SExpression::Array(arr)) => arr.clone(),
            Some(single) => vec![single.clone()],
            None => Vec::new(),
        };

        let mut attr_keys: Vec<&String> = map
            .keys()
            .filter(|k| *k != "type" && *k != "children")
            .collect();
        attr_keys.sort();

        self.output.push('<');
        self.output.push_str(&tag);
        for k in &attr_keys {
            self.fmt_jsx_attr(k, &map[*k]);
        }

        if !children_owned.is_empty() {
            self.output.push('>');
            self.write_newline();
            self.indent();
            for child in &children_owned {
                self.write_indent();
                self.fmt_render_pattern(child);
                self.write_newline();
            }
            self.dedent();
            self.write_indent();
            self.output.push_str("</");
            self.output.push_str(&tag);
            self.output.push('>');
        } else if children_present {
            self.output.push_str("></");
            self.output.push_str(&tag);
            self.output.push('>');
        } else {
            self.output.push_str(" />");
        }
    }

    /// Format a hoisted synthetic inline-trait reference as
    /// `<Alias.traits.X entity={E} attrs>children</Alias.traits.X>`. Reuses the
    /// same attribute/children formatting as pattern elements.
    fn fmt_jsx_inline_trait(&mut self, tr: &Trait) {
        let ref_data = tr
            .reference
            .as_ref()
            .expect("inline synthetic trait must carry a reference");
        let reference = &ref_data.reference;

        let children_config = ref_data.config.iter().find(|c| c.name == "children");
        // Normalize children to an array (single object → one-element array),
        // matching `fmt_jsx_pattern` — including its `children_present` vs
        // "owned array is empty" distinction (see that function's comment).
        let children_present = children_config.is_some();
        let children_owned: Vec<SExpression> = match children_config {
            Some(cf) => match &cf.default {
                Some(SExpression::Array(arr)) => arr.clone(),
                Some(single) => vec![single.clone()],
                None => Vec::new(),
            },
            None => Vec::new(),
        };

        let mut attr_keys: Vec<&str> = ref_data
            .config
            .iter()
            .filter(|c| c.name != "children")
            .map(|c| c.name.as_str())
            .collect();
        attr_keys.sort();
        attr_keys.dedup();

        self.output.push('<');
        self.output.push_str(reference);
        if let Some(le) = &tr.linked_entity {
            self.output.push_str(" linkedEntity={");
            self.output.push_str(le);
            self.output.push('}');
        }
        for key in &attr_keys {
            if let Some(cf) = ref_data.config.iter().find(|c| c.name == *key) {
                if let Some(v) = &cf.default {
                    self.fmt_jsx_attr(key, v);
                }
            }
        }
        if !tr.listens.is_empty() {
            self.output.push_str(" listens={{ ");
            let mut sorted: Vec<&Listen> = tr.listens.iter().collect();
            sorted.sort_by(|a, b| a.event.cmp(&b.event));
            for (i, l) in sorted.iter().enumerate() {
                if i > 0 {
                    self.output.push_str(", ");
                }
                // Quote the key: cross-orbital sources are dotted
                // (`Source.EVENT`) which aren't valid bare object keys.
                self.write_quoted_string(&l.event);
                self.output.push_str(": ");
                self.output.push_str(l.triggers.as_deref().unwrap_or(""));
            }
            self.output.push_str(" }}");
        }

        if !children_owned.is_empty() {
            self.output.push('>');
            self.write_newline();
            self.indent();
            for child in &children_owned {
                self.write_indent();
                self.fmt_render_pattern(child);
                self.write_newline();
            }
            self.dedent();
            self.write_indent();
            self.output.push_str("</");
            self.output.push_str(reference);
            self.output.push('>');
        } else if children_present {
            self.output.push_str("></");
            self.output.push_str(reference);
            self.output.push('>');
        } else {
            self.output.push_str(" />");
        }
    }

    /// Format one JSX attribute: bare `key` for `true`, otherwise
    /// `key={value}`. String values use the same bare/quoted rule as the rest
    /// of the formatter so round-tripping is deterministic.
    fn fmt_jsx_attr(&mut self, key: &str, value: &SExpression) {
        self.output.push(' ');
        self.output.push_str(key);
        if let SExpression::Bool(true) = value {
            return;
        }
        self.output.push('=');
        match value {
            SExpression::String(s) if is_bare_atom(s) || s.starts_with('@') => {
                self.output.push('{');
                self.fmt_sexpr_string(s);
                self.output.push('}');
            }
            SExpression::String(s) => {
                self.output.push('{');
                self.write_quoted_string(s);
                self.output.push('}');
            }
            other => {
                self.output.push('{');
                self.fmt_sexpr_inner(other, false);
                self.output.push('}');
            }
        }
    }

    fn fmt_sexpr_array(&mut self, arr: &[SExpression], force_paren: bool) {
        if arr.is_empty() {
            if force_paren {
                self.output.push_str("()");
            } else {
                self.output.push_str("[]");
            }
            return;
        }

        // `(render-ui slot pattern …)` — the pattern argument (index 2) is the
        // root of a render element tree. Format it (and, recursively, its
        // `children`) as JSX so the canonical authoring surface for render
        // trees is JSX. Any trailing args (resolved props, priority) are
        // compiler-injected and never authored, so they format normally.
        if arr.len() >= 3 && matches!(&arr[0], SExpression::String(s) if s == "render-ui") {
            self.output.push('(');
            self.fmt_sexpr_inner(&arr[0], false);
            self.output.push(' ');
            self.fmt_sexpr_inner(&arr[1], false);
            self.output.push(' ');
            self.fmt_render_pattern(&arr[2]);
            for item in &arr[3..] {
                self.output.push(' ');
                self.fmt_sexpr_inner(item, false);
            }
            self.output.push(')');
            return;
        }

        if let Some(quoted) =
            orbital_core_base::schema::quote::quote_body_of(&SExpression::Array(arr.to_vec()))
                .and_then(orbital_core_base::schema::quote::decode_quote_body)
        {
            self.output.push_str("(quote ");
            self.fmt_sexpr_inner(&quoted, false);
            self.output.push(')');
            return;
        }

        if let Some(template) = Self::quasiquote_template(arr) {
            self.output.push_str("(quasiquote ");
            self.fmt_sexpr_inner(&template, false);
            self.output.push(')');
            return;
        }

        // Detect let form
        let is_let = matches!(&arr[0], SExpression::String(s) if s == "let");
        if is_let && arr.len() == 3 {
            self.output.push('(');
            self.output.push_str("let");
            self.output.push(' ');
            self.fmt_sexpr_inner(&arr[1], true); // binding list forced to paren
            self.output.push(' ');
            // The binding names are in scope for the body only; the list itself
            // is printed first so the names themselves stay unquoted.
            self.binding_scopes.push(Self::let_binding_names(&arr[1]));
            self.fmt_sexpr_inner(&arr[2], false);
            self.binding_scopes.pop();
            self.output.push(')');
            return;
        }

        // Lambda: parameters print bare, the body sees them as bound. Handles
        // both the grouped `["fn", ["a", "b"], body]` and flat
        // `["fn", "a", "b", body]` shapes.
        let is_lambda = matches!(&arr[0], SExpression::String(s) if s == "fn" || s == "lambda")
            && arr.len() >= 3;
        if is_lambda {
            let (params, body) = arr.split_at(arr.len() - 1);
            let names: Vec<String> = match &params[1] {
                SExpression::Array(grouped) if params.len() == 2 => grouped
                    .iter()
                    .filter_map(|p| match p {
                        SExpression::String(n) => Some(n.clone()),
                        _ => None,
                    })
                    .collect(),
                _ => params[1..]
                    .iter()
                    .filter_map(|p| match p {
                        SExpression::String(n) => Some(n.clone()),
                        _ => None,
                    })
                    .collect(),
            };
            self.output.push('(');
            for (i, item) in params.iter().enumerate() {
                if i > 0 {
                    self.output.push(' ');
                }
                self.fmt_sexpr_inner(item, false);
            }
            self.output.push(' ');
            self.binding_scopes.push(names);
            self.fmt_sexpr_inner(&body[0], false);
            self.binding_scopes.pop();
            self.output.push(')');
            return;
        }

        let is_call = force_paren || is_call_array(arr);

        if is_call {
            self.output.push('(');
            for (i, item) in arr.iter().enumerate() {
                if i > 0 {
                    self.output.push(' ');
                }
                match item {
                    SExpression::String(op) if i == 0 && is_operator_symbol(op) => {
                        self.output.push_str(op)
                    }
                    _ => self.fmt_sexpr_inner(item, false),
                }
            }
            self.output.push(')');
        } else {
            self.output.push('[');
            for (i, item) in arr.iter().enumerate() {
                if i > 0 {
                    self.output.push_str(", ");
                }
                self.fmt_sexpr_inner(item, false);
            }
            self.output.push(']');
        }
    }

    /// An object key prints bare when it re-lexes as a key, quoted otherwise
    /// (a route like `"/dash"`), so every object round-trips.
    fn write_object_key(&mut self, k: &str) {
        let bare = k
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            && k.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
        if bare {
            self.output.push_str(k);
        } else {
            self.write_quoted_string(k);
        }
    }

    fn fmt_sexpr_object(&mut self, map: &orbital_core_base::schema::SObject) {
        if map.is_empty() {
            self.output.push_str("{}");
            return;
        }
        // Sort keys for deterministic output
        let mut keys: Vec<&String> = map.keys().collect();
        keys.sort();

        // Check if all values are simple scalars
        let all_simple = keys.iter().all(|k| is_simple_value(&map[*k]));
        let single_line = all_simple && keys.len() <= 2;

        if single_line {
            self.output.push_str("{ ");
            for (i, k) in keys.iter().enumerate() {
                if i > 0 {
                    self.output.push_str(", ");
                }
                self.write_object_key(k);
                self.output.push_str(": ");
                self.fmt_object_value(k, &map[*k]);
            }
            self.output.push_str(" }");
        } else {
            self.output.push_str("{");
            self.write_newline();
            self.indent();
            for k in keys {
                self.write_indent();
                self.write_object_key(k);
                self.output.push_str(": ");
                self.fmt_object_value(k, &map[k]);
                self.write_newline();
            }
            self.dedent();
            self.write_indent();
            self.output.push('}');
        }
    }

    /// Format one object-literal / config-field value. `children` arrays and
    /// any `@trait.*` value are render elements, so they format as JSX — this
    /// covers render trees expressed as config overrides on trait references
    /// (`config { children: […] }`, `contentTrait: @(...)`, `idleContent: @(...)`)
    /// regardless of the knob name, since `@trait.*` is always a render element.
    fn fmt_object_value(&mut self, key: &str, value: &SExpression) {
        if key == "children" {
            if let SExpression::Array(arr) = value {
                if !is_call_array(arr) {
                    self.fmt_render_children_array(arr);
                    return;
                }
            }
        }
        if let SExpression::String(s) = value {
            if s.starts_with("@trait.") {
                self.fmt_render_pattern(value);
                return;
            }
        }
        self.fmt_sexpr_inner(value, false);
    }

    /// Format a `children:` array as a multi-line list of JSX render patterns.
    fn fmt_render_children_array(&mut self, arr: &[SExpression]) {
        if arr.is_empty() {
            self.output.push_str("[]");
            return;
        }
        self.output.push('[');
        self.write_newline();
        self.indent();
        for child in arr {
            self.write_indent();
            self.fmt_render_pattern(child);
            self.write_newline();
        }
        self.dedent();
        self.write_indent();
        self.output.push(']');
    }

    fn write_quoted_string(&mut self, s: &str) {
        self.output.push_str(&quote_lolo(s));
    }
}

// ------------------------------------------------------------------
// Helpers
// ------------------------------------------------------------------

/// Render one `MatchPattern`: `Str` quoted (same escaping as
/// `write_quoted_string`), `Word`/`Bool`/`Int` bare, `Wildcard` as `_`.
/// Shared by `fmt_type_expr`'s `TypeExpr::Match` branch (§25) and
/// `fmt_term_match`'s transition-position `match` (Phase 2) — same arm
/// vocabulary, two syntax positions. A free function (not a formatter
/// method) so `fmt_type_expr` can measure the type-level arm column width
/// before any of it is written.
fn render_match_pattern(pattern: &MatchPattern) -> String {
    match pattern {
        MatchPattern::Str(s) => {
            let mut out = String::with_capacity(s.len() + 2);
            out.push('"');
            for c in s.chars() {
                match c {
                    '\\' => out.push_str("\\\\"),
                    '"' => out.push_str("\\\""),
                    '\n' => out.push_str("\\n"),
                    '\t' => out.push_str("\\t"),
                    '\r' => out.push_str("\\r"),
                    c => push_plain_char(&mut out, c),
                }
            }
            out.push('"');
            out
        }
        MatchPattern::Word(w) => w.clone(),
        MatchPattern::Bool(b) => b.to_string(),
        MatchPattern::Int(n) => n.to_string(),
        MatchPattern::Wildcard => "_".to_string(),
    }
}

/// True if `s` is a sigil that can be printed unquoted: an `@`-prefixed clean
/// dotted path (`@config.foo.bar`, `@now`, `@user.id`). Sigils carrying anything
/// the sigil reader can't consume bare — notably array indices (`@x[0].y`) — are
/// rejected so the formatter quotes them instead.
fn is_bare_sigil(s: &str) -> bool {
    let Some(rest) = s.strip_prefix('@') else {
        return false;
    };
    !rest.is_empty()
        && rest
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
}

fn is_bare_atom(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    // Reserved bare words that the lexer/parser re-interpret rather than treat as
    // plain string atoms — they MUST stay quoted to round-trip as string values:
    //   `true`/`false`/`null` lex as Boolean/Null literals; bare `now`/`state` are
    //   special-cased by the parser into the `@now`/`@state` bindings.
    if matches!(s, "true" | "false" | "null" | "now" | "state") {
        return false;
    }
    let first = s.chars().next().unwrap();
    if !first.is_ascii_alphabetic() && first != '_' {
        return false;
    }
    s.chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '/')
}

fn is_call_array(arr: &[SExpression]) -> bool {
    matches!(&arr.first(), Some(SExpression::String(s)) if is_bare_atom(s) || is_operator_symbol(s))
}

/// True if `s` is a symbolic operator head (the `OP_SYMBOL` set from the
/// grammar). These start with a non-alphabetic char so `is_bare_atom`
/// rejects them; without this, `[- a b]` / `["==", a, b]` would leak as
/// JSON-array literals instead of printing as `(- a b)` / `(== a b)`.
fn is_operator_symbol(s: &str) -> bool {
    matches!(
        s,
        "+" | "-" | "*" | "/" | "%" | "=" | "==" | "!=" | "<" | ">" | "<=" | ">="
    )
}

/// Inverse of `kebab_case`: kebab-case pattern `type` → PascalCase JSX tag.
/// `stack` → `Stack`, `data-grid` → `DataGrid`, `ui-spinner` → `UiSpinner`.
fn pascal_case(kebab: &str) -> String {
    kebab
        .split('-')
        .map(|seg| {
            let mut c = seg.chars();
            match c.next() {
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                None => String::new(),
            }
        })
        .collect()
}

/// True when `s` is a static (literal) pattern name: lowercase kebab-case
/// (`stack`, `data-grid`, `form-section`). Dynamic `type` bindings such as
/// `@config.detailPattern` are NOT static, so their host object falls back to
/// object-literal formatting instead of becoming an invalid JSX tag.
fn is_static_pattern_type(s: &str) -> bool {
    s.chars().next().is_some_and(|c| c.is_ascii_lowercase())
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && crate::parser::jsx::kebab_case(&pascal_case(s)) == s
}

fn is_simple_value(expr: &SExpression) -> bool {
    match expr {
        SExpression::Null | SExpression::Bool(_) | SExpression::Number(_) => true,
        SExpression::String(s) => is_bare_atom(s) || s.starts_with('@'),
        _ => false,
    }
}

fn fmt_duration(ms: u64) -> String {
    if ms % (60 * 60 * 24 * 1000) == 0 && ms > 0 {
        format!("{}d", ms / (60 * 60 * 24 * 1000))
    } else if ms % (60 * 60 * 1000) == 0 && ms > 0 {
        format!("{}h", ms / (60 * 60 * 1000))
    } else if ms % (60 * 1000) == 0 && ms > 0 {
        format!("{}m", ms / (60 * 1000))
    } else if ms % 1000 == 0 && ms > 0 {
        format!("{}s", ms / 1000)
    } else {
        format!("{}ms", ms)
    }
}

/// A page's head modifiers as `key: value` bracket entries, in canonical order.
fn head_modifier_sources(mods: &orbital_core_base::schema::PageModifiers) -> Vec<String> {
    use orbital_core_base::schema::{PageAccess, PageIndexing};
    let mut out = Vec::new();
    if let Some(access) = mods.access {
        out.push(format!(
            "access: {}",
            match access {
                PageAccess::Public => "public",
                PageAccess::Authenticated => "authenticated",
            }
        ));
    }
    if let Some(indexing) = mods.indexing {
        out.push(format!(
            "indexing: {}",
            match indexing {
                PageIndexing::Index => "index",
                PageIndexing::Noindex => "noindex",
            }
        ));
    }
    if let Some(title) = &mods.title {
        out.push(format!("title: {}", page_meta_source(title)));
    }
    if let Some(description) = &mods.description {
        out.push(format!("description: {}", page_meta_source(description)));
    }
    if let Some(group) = &mods.translation_of {
        out.push(format!("translationOf: {}", quote_lolo(group)));
    }
    if let Some(label) = &mods.label {
        out.push(format!("label: {}", quote_lolo(label)));
    }
    if let Some(icon) = &mods.icon {
        out.push(format!("icon: {}", quote_lolo(icon)));
    }
    if let Some(roles) = &mods.roles {
        out.push(format!("roles: {}", quote_lolo(&roles.join(", "))));
    }
    out
}

/// A page metadata value as `.lolo` source.
fn page_meta_source(meta: &orbital_core_base::schema::PageMeta) -> String {
    let quote = quote_lolo;
    match meta {
        orbital_core_base::schema::PageMeta::Literal(s) => quote(s),
        orbital_core_base::schema::PageMeta::Message(orbital_core_base::schema::PageMessage(_, key)) => {
            format!("(i18n/t {})", quote(key))
        }
    }
}

/// A shape field name as written: bare when the lexer reads it back as one identifier, quoted otherwise.
fn shape_field_name(name: &str) -> String {
    use crate::lexer::{tokenize, TokenKind};
    let bare = match tokenize(name) {
        Ok(tokens) => {
            let mut words = tokens
                .iter()
                .filter(|t| !matches!(t.kind, TokenKind::Newline | TokenKind::Eof));
            matches!((words.next(), words.next()), (Some(t), None) if t.kind == TokenKind::Identifier(name.to_string()))
        }
        Err(_) => false,
    };
    if bare {
        name.to_string()
    } else {
        quote_lolo(name)
    }
}

/// A character with no named escape: a control character prints as `\u{H}` (uppercase hex, no padding), the
/// canonical spelling whichever form the source wrote; every other character prints as itself.
fn push_plain_char(out: &mut String, c: char) {
    if c.is_control() {
        out.push_str(&format!("\\u{{{:X}}}", c as u32));
    } else {
        out.push(c);
    }
}

/// A `.lolo` string literal for `s`.
fn quote_lolo(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            c => push_plain_char(&mut out, c),
        }
    }
    out.push('"');
    out
}
