//! Pretty-printer for the `.lolo` surface language.
//!
//! Takes a parsed `ast::Program` and emits canonical, consistently-indented
//! `.lolo` source. Source comments (`;;`, `#`, `#= … =#`) are preserved: the
//! lexer captures them as trivia and the formatter re-interleaves them by source
//! line before the node that follows them (re-indented to the output).

use orbital_core::schema::SExpression;
use std::collections::HashMap;

use crate::ast::{
    AppHeader, Comment, ConfigField, Emit, Entity, EntityField, EventField, EventScopeAst,
    ExpectsDecl, ExpectsKind, Listen, Orbital, Page, Persistence, Program, State, Tick,
    TickInterval, Trait, TraitRefData, TraitScope, Transition, TypeAlias, TypeAnnotation, TypeBody,
    TypeExpr, UseDecl,
};

/// Format a `Program` AST into canonical `.lolo` source text.
pub fn format_program(program: &Program) -> String {
    let mut f = Formatter::new(program.comments.clone());
    f.fmt_program(program);
    f.finish()
}

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
            binding_scopes: Vec::new(),
        }
    }

    fn is_lexically_bound(&self, name: &str) -> bool {
        self.binding_scopes
            .iter()
            .any(|frame| frame.iter().any(|bound| bound == name))
    }

    /// Binding names declared by a `let` binding list (`((a v) (b w))`).
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
        if !program.type_aliases.is_empty() || !program.orbitals.is_empty() {
            self.write_newline();
        }
        for (i, alias) in program.type_aliases.iter().enumerate() {
            if i > 0 {
                self.write_newline();
            }
            self.emit_comments_before(alias.span.line);
            self.fmt_type_alias(alias);
        }
        if !program.type_aliases.is_empty() && !program.orbitals.is_empty() {
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
        if let Some(v) = &app.version {
            self.output.push(' ');
            self.write_quoted_string(v);
        }
        if let Some(desc) = &app.description {
            self.write_newline();
            self.write_indent();
            self.write_quoted_string(desc);
        }
        if let Some(theme) = &app.theme {
            self.write_newline();
            self.write_indent();
            self.output.push_str("theme ");
            self.write_quoted_string(theme);
        }
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
                    self.output.push_str(name);
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
                    false,
                    false,
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

    fn fmt_type_expr(&mut self, ty: &TypeExpr) {
        match ty {
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
                self.fmt_type_expr(inner);
                self.output.push(']');
            }
            TypeExpr::Map(key, value) => {
                // Surface syntax: `Map K V` (juxtaposition).
                self.output.push_str("Map ");
                self.fmt_type_expr(key);
                self.output.push(' ');
                self.fmt_type_expr(value);
            }
            TypeExpr::Object(fields) => {
                self.output.push_str("{ ");
                for (i, (name, fty, req)) in fields.iter().enumerate() {
                    if i > 0 {
                        self.output.push_str(", ");
                    }
                    self.output.push_str(name);
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
                self.fmt_type_expr(inner);
                self.output.push('?');
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
            TypeExpr::EntityRef => {
                self.output.push_str("entity");
            }
            TypeExpr::EventRef => {
                self.output.push_str("event");
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
        }
    }

    fn fmt_persistence_tag(
        &mut self,
        persistence: Option<&Persistence>,
        collection: Option<&str>,
        shared: bool,
        identity: bool,
    ) {
        let mut tags = Vec::new();
        match (persistence, collection) {
            (Some(Persistence::Persistent), Some(c)) => tags.push(format!("persistent: {c}")),
            (Some(Persistence::Persistent), None) => tags.push("persistent".to_string()),
            (Some(Persistence::Runtime), _) => tags.push("runtime".to_string()),
            (None, _) => {}
        }
        if shared {
            tags.push("shared".to_string());
        }
        if identity {
            tags.push("identity".to_string());
        }
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
        if let Some(ann) = &orbital.type_annotation {
            self.output.push(' ');
            self.fmt_type_annotation(ann);
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
            self.output.push_str("theme ");
            self.write_quoted_string(theme);
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

    fn fmt_use_decl(&mut self, u: &UseDecl) {
        self.output.push_str("uses ");
        self.output.push_str(&u.alias);
        self.output.push_str(" from ");
        self.write_quoted_string(&u.path);
    }

    fn fmt_expects_decl(&mut self, e: &ExpectsDecl) {
        self.output.push_str("expects ");
        match &e.kind {
            ExpectsKind::Identity { name, shape } => {
                self.output.push_str("identity");
                if let Some(name) = name {
                    self.output.push(' ');
                    self.output.push_str(name);
                }
                self.fmt_expects_shape(shape);
            }
            ExpectsKind::Entity { name, shape } => {
                self.output.push_str("entity ");
                self.output.push_str(name);
                self.fmt_expects_shape(shape);
            }
            ExpectsKind::Event { trait_name, event } => {
                self.output.push_str("event ");
                self.output.push_str(trait_name);
                self.output.push('.');
                self.output.push_str(event);
            }
            ExpectsKind::Page { path } => {
                self.output.push_str("page \"");
                self.output.push_str(path);
                self.output.push('"');
            }
        }
    }

    fn fmt_expects_shape(&mut self, shape: &Option<Vec<EntityField>>) {
        let Some(fields) = shape else {
            return;
        };
        self.output.push_str(" {");
        self.write_newline();
        self.indent();
        for field in fields {
            self.write_indent();
            self.fmt_entity_field(field);
            self.write_newline();
        }
        self.dedent();
        self.write_indent();
        self.output.push('}');
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
            entity.shared,
            entity.identity,
        );
        self.output.push_str(" {");
        let has_access_directives = entity.read_policy.is_some()
            || entity.create_policy.is_some()
            || entity.update_policy.is_some()
            || entity.delete_policy.is_some()
            || !entity.access_waivers.is_empty();
        if !entity.fields.is_empty() || has_access_directives {
            self.write_newline();
            self.indent();
            for (sigil, expr) in [
                ("read", &entity.read_policy),
                ("create", &entity.create_policy),
                ("update", &entity.update_policy),
                ("delete", &entity.delete_policy),
            ] {
                if let Some(expr) = expr {
                    self.write_indent();
                    self.output.push('@');
                    self.output.push_str(sigil);
                    self.output.push(' ');
                    self.fmt_sexpr(expr);
                    self.write_newline();
                } else if let Some((_, reason)) =
                    entity.access_waivers.iter().find(|(op, _)| op == sigil)
                {
                    // `@<op> none "<reason>"` — the declared omission, printed
                    // in the same slot as a real directive so a round-trip is
                    // lossless.
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
            for field in &entity.fields {
                self.write_indent();
                self.fmt_entity_field(field);
                self.write_newline();
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
        if !state.transitions.is_empty() {
            self.write_newline();
            self.indent();
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
        self.output.push_str(&tr.event);
        self.output.push_str(" -> ");
        self.output.push_str(&tr.target);
        if let Some(guard) = &tr.guard {
            self.output.push_str(" when ");
            self.fmt_sexpr(guard);
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
        if !listen.payload.is_empty() && listen.triggers.is_none() {
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
        if !opts.is_empty() {
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
        if (is_bare_sigil(s) || is_bare_atom(s) || is_operator_symbol(s))
            && !self.is_lexically_bound(s)
        {
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
                    Some(SExpression::Array(arr)) => arr.iter().all(|c| self.is_jsx_renderable(c)),
                    Some(single) => self.is_jsx_renderable(single),
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
                                arr.iter().all(|c| self.is_jsx_renderable(c))
                            }
                            Some(single) => self.is_jsx_renderable(single),
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

    /// Format an anonymous pattern object as `<Type attrs>children</Type>`
    /// (or `<Type attrs />` when childless). The `type` key becomes the tag
    /// name (PascalCased); `children` becomes nested JSX; every other key is
    /// a sorted attribute.
    fn fmt_jsx_pattern(&mut self, map: &std::collections::HashMap<String, SExpression>) {
        let type_str = match map.get("type") {
            Some(SExpression::String(s)) => s.clone(),
            _ => return self.fmt_sexpr_object(map),
        };
        let tag = pascal_case(&type_str);

        // Normalize children to an array: a single-object child
        // (`children: { … }`) is treated as a one-element array so JSX — which
        // only has sequence children — can represent it as `<Tag><Child/></Tag>`.
        let children_owned: Vec<SExpression> = match map.get("children") {
            Some(SExpression::Array(arr)) => arr.clone(),
            Some(single) => vec![single.clone()],
            None => Vec::new(),
        };
        let children = if children_owned.is_empty() {
            None
        } else {
            Some(children_owned)
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

        match children {
            Some(arr) if !arr.is_empty() => {
                self.output.push('>');
                self.write_newline();
                self.indent();
                for child in &arr {
                    self.write_indent();
                    self.fmt_render_pattern(child);
                    self.write_newline();
                }
                self.dedent();
                self.write_indent();
                self.output.push_str("</");
                self.output.push_str(&tag);
                self.output.push('>');
            }
            _ => self.output.push_str(" />"),
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
        // matching `fmt_jsx_pattern`.
        let children_owned: Vec<SExpression> = match children_config {
            Some(cf) => match &cf.default {
                Some(SExpression::Array(arr)) => arr.clone(),
                Some(single) => vec![single.clone()],
                None => Vec::new(),
            },
            None => Vec::new(),
        };
        let children = if children_owned.is_empty() {
            None
        } else {
            Some(children_owned)
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

        match children {
            Some(arr) if !arr.is_empty() => {
                self.output.push('>');
                self.write_newline();
                self.indent();
                for child in &arr {
                    self.write_indent();
                    self.fmt_render_pattern(child);
                    self.write_newline();
                }
                self.dedent();
                self.write_indent();
                self.output.push_str("</");
                self.output.push_str(reference);
                self.output.push('>');
            }
            _ => self.output.push_str(" />"),
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
        let is_lambda =
            matches!(&arr[0], SExpression::String(s) if s == "fn" || s == "lambda") && arr.len() >= 3;
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
                self.fmt_sexpr_inner(item, false);
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

    fn fmt_sexpr_object(&mut self, map: &std::collections::HashMap<String, SExpression>) {
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
                self.output.push_str(k);
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
                self.output.push_str(k);
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
                self.fmt_render_children_array(arr);
                return;
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
        self.output.push('"');
        for c in s.chars() {
            match c {
                '\\' => self.output.push_str("\\\\"),
                '"' => self.output.push_str("\\\""),
                '\n' => self.output.push_str("\\n"),
                '\t' => self.output.push_str("\\t"),
                '\r' => self.output.push_str("\\r"),
                c => self.output.push(c),
            }
        }
        self.output.push('"');
    }
}

// ------------------------------------------------------------------
// Helpers
// ------------------------------------------------------------------

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
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '/' || c == '=')
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
