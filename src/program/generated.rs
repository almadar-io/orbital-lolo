//! Selecting, rendering and anchoring a shell's generated files (`orb emit ts|py`).

use crate::orb_json::NodeKind;
use orbital_core_base::generate::GeneratedFile;
use orbital_core_base::str_utils::{kebab_to_pascal, to_kebab_case_simple, to_snake_case};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodeTarget {
    Ts,
    Py,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Format {
    #[default]
    Stream,
    Markdown,
    Json,
    Raw,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Section {
    All,
    AllEntities,
    Entity(String),
    AllTraits,
    Trait(String),
    AllPages,
    Page(String),
    AllServices,
    Service(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Only {
    Entities,
    Traits,
    Pages,
    Hooks,
    Components,
    Routes,
    Services,
    Tests,
}

fn is_name(s: &str) -> bool {
    let mut cs = s.chars();
    matches!(cs.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
        && cs.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

impl Section {
    /// `entities[.NAME] | traits[.NAME] | pages[.NAME] | services[.NAME]`, optionally behind `orbitals[i].`.
    pub fn parse(input: &str) -> Result<Section, String> {
        let input = input.trim();
        if input.is_empty() {
            return Ok(Section::All);
        }
        let mut segs: Vec<&str> = input.split('.').collect();
        if segs[0].starts_with("orbitals[") && segs[0].ends_with(']') {
            segs.remove(0);
        }
        let name = segs.get(1).copied();
        Ok(match (segs.first().copied().unwrap_or_default(), name) {
            ("entities" | "entity", None) => Section::AllEntities,
            ("entities", Some(n)) if is_name(n) => Section::Entity(n.into()),
            ("traits", None) => Section::AllTraits,
            ("traits", Some(n)) if is_name(n) => Section::Trait(n.into()),
            ("pages", None) => Section::AllPages,
            ("pages", Some(n)) if is_name(n) => Section::Page(n.into()),
            ("services", None) => Section::AllServices,
            ("services", Some(n)) if is_name(n) => Section::Service(n.into()),
            _ => {
                return Err(format!(
                    "unknown section selector {input:?} — supported forms: entities[.NAME] | traits[.NAME] | pages[.NAME] | services[.NAME] | orbitals[i].<section>"
                ))
            }
        })
    }

    fn matches(&self, file: &GeneratedFile, target: CodeTarget) -> bool {
        let (path, content) = (file.path.as_str(), file.content.as_str());
        match (self, target) {
            (Section::All, _) => true,
            (Section::AllEntities, CodeTarget::Ts) => path.starts_with("packages/shared/"),
            (Section::AllEntities, CodeTarget::Py) => path.starts_with("entities/"),
            (Section::Entity(n), CodeTarget::Ts) => {
                path.starts_with("packages/shared/")
                    && (path.contains(n.as_str())
                        || content.contains(&format!("interface {n} "))
                        || content.contains(&format!("interface {n}{{")))
            }
            (Section::Entity(n), CodeTarget::Py) => {
                path.starts_with("entities/")
                    && (path.contains(n.as_str()) || content.contains(&format!("class {n}")))
            }
            (Section::AllTraits, CodeTarget::Ts) => {
                path.contains("/traits/") || path.starts_with("packages/server/src/routes/")
            }
            (Section::AllTraits, CodeTarget::Py) => {
                path.starts_with("traits/") || path.starts_with("routes/")
            }
            (Section::Trait(n), CodeTarget::Ts) => {
                path.contains(&format!("/traits/use{n}"))
                    || path.contains(&format!("/traits/{n}"))
                    || path.contains(&format!("/routes/{}", to_kebab_case_simple(n)))
            }
            (Section::Trait(n), CodeTarget::Py) => {
                let snake = to_snake_case(n);
                path == format!("traits/{snake}.py") || path == format!("routes/{snake}.py")
            }
            (Section::AllPages, CodeTarget::Ts) => {
                path.starts_with("packages/client/src/features/")
            }
            (Section::Page(n), CodeTarget::Ts) => {
                path.starts_with(&format!("packages/client/src/features/{n}"))
            }
            (Section::AllPages | Section::Page(_), CodeTarget::Py) => false,
            (Section::AllServices, CodeTarget::Ts) => path.contains("/services/"),
            (Section::AllServices, CodeTarget::Py) => {
                path.starts_with("services/") || path.contains("service")
            }
            (Section::Service(n), CodeTarget::Ts) => {
                path.contains("/services/") && path.contains(n.as_str())
            }
            (Section::Service(n), CodeTarget::Py) => {
                path.contains("service")
                    && (path.contains(n.as_str()) || content.contains(n.as_str()))
            }
        }
    }
}

impl Only {
    /// A comma list of kinds (`entities,traits,...`).
    pub fn parse_list(input: &str) -> Result<Vec<Only>, String> {
        input
            .split(',')
            .map(|tok| {
                Ok(match tok.trim().to_ascii_lowercase().as_str() {
                    "entities" | "entity" => Only::Entities,
                    "traits" | "trait" => Only::Traits,
                    "pages" | "page" => Only::Pages,
                    "hooks" | "hook" => Only::Hooks,
                    "components" | "component" => Only::Components,
                    "routes" | "route" => Only::Routes,
                    "services" | "service" => Only::Services,
                    "tests" | "test" => Only::Tests,
                    _ => {
                        return Err(format!(
                            "unknown --only kind {tok:?} — supported: entities, traits, pages, hooks, components, routes, services, tests"
                        ))
                    }
                })
            })
            .collect()
    }

    fn matches(self, file: &GeneratedFile, target: CodeTarget) -> bool {
        let p = file.path.as_str();
        match (self, target) {
            (Only::Entities, CodeTarget::Ts) => p.starts_with("packages/shared/"),
            (Only::Entities, CodeTarget::Py) => p.starts_with("entities/"),
            (Only::Traits, CodeTarget::Ts) => {
                p.contains("/traits/") || p.starts_with("packages/server/src/routes/")
            }
            (Only::Traits, CodeTarget::Py) => p.starts_with("traits/") || p.starts_with("routes/"),
            (Only::Pages, CodeTarget::Ts) => p.starts_with("packages/client/src/features/"),
            (Only::Hooks, CodeTarget::Ts) => p.contains("/hooks/"),
            (Only::Components, CodeTarget::Ts) => p.contains("/components/"),
            (Only::Routes, CodeTarget::Ts) => p.starts_with("packages/server/src/routes/"),
            (Only::Routes, CodeTarget::Py) => p.starts_with("routes/"),
            (Only::Services, _) => p.contains("service"),
            (Only::Tests, CodeTarget::Ts) => {
                p.contains("__tests__") || p.ends_with(".test.tsx") || p.ends_with(".test.ts")
            }
            (Only::Tests, CodeTarget::Py) => p.starts_with("tests/") || p.starts_with("test_"),
            (Only::Pages | Only::Hooks | Only::Components, CodeTarget::Py) => false,
        }
    }
}

/// The files `section` selects, narrowed to the kinds in `only` when given.
pub fn select<'a>(
    files: &'a [GeneratedFile],
    section: &Section,
    only: Option<&[Only]>,
    target: CodeTarget,
) -> Vec<&'a GeneratedFile> {
    files
        .iter()
        .filter(|f| section.matches(f, target))
        .filter(|f| only.is_none_or(|kinds| kinds.iter().any(|k| k.matches(f, target))))
        .collect()
}

fn with_newline(out: &mut String, content: &str) {
    out.push_str(content);
    if !content.ends_with('\n') {
        out.push('\n');
    }
}

/// `files` as text in `format`.
pub fn render(
    files: &[&GeneratedFile],
    format: Format,
    target: CodeTarget,
) -> Result<String, serde_json::Error> {
    let mut out = String::new();
    match format {
        Format::Stream => {
            for (i, f) in files.iter().enumerate() {
                if i > 0 {
                    out.push('\n');
                }
                out.push_str(&format!("// ───── {} ─────\n", f.path));
                with_newline(&mut out, &f.content);
            }
        }
        Format::Markdown => {
            let lang = if target == CodeTarget::Ts {
                "typescript"
            } else {
                "python"
            };
            for f in files {
                out.push_str(&format!("### `{}`\n\n```{lang}\n", f.path));
                with_newline(&mut out, &f.content);
                out.push_str("```\n\n");
            }
        }
        Format::Json => {
            let map: serde_json::Map<String, serde_json::Value> = files
                .iter()
                .map(|f| (f.path.clone(), serde_json::Value::String(f.content.clone())))
                .collect();
            out = serde_json::to_string_pretty(&serde_json::Value::Object(map))?;
        }
        Format::Raw => files
            .iter()
            .for_each(|f| with_newline(&mut out, &f.content)),
    }
    Ok(out)
}

fn between(path: &str, prefix: &str, suffixes: &[&str]) -> Option<String> {
    let rest = path.split(prefix).nth(1)?;
    let name = suffixes.iter().find_map(|s| rest.strip_suffix(s))?;
    (!name.is_empty() && !name.contains('/')).then(|| name.to_string())
}

fn first_ident_after(content: &str, needle: &str) -> Option<String> {
    let rest = &content[content.find(needle)? + needle.len()..];
    let end = rest
        .find(|c: char| !c.is_ascii_alphanumeric() && c != '_')
        .unwrap_or(rest.len());
    (end > 0).then(|| rest[..end].to_string())
}

fn snake_to_pascal(s: &str) -> String {
    s.split('_')
        .filter(|p| !p.is_empty())
        .map(|p| p[..1].to_uppercase() + &p[1..])
        .collect()
}

/// The program node a generated file stands for, by the shell's path layout; `None` for shared scaffolding.
pub fn anchor(file: &GeneratedFile, target: CodeTarget) -> Option<(NodeKind, String)> {
    let (path, content) = (file.path.as_str(), file.content.as_str());
    match target {
        CodeTarget::Ts => {
            if let Some(n) = between(path, "/hooks/traits/use", &["Logic.tsx", "Logic.ts"]) {
                return Some((NodeKind::Trait, n));
            }
            if let Some(n) = between(path, "/hooks/traits/use", &[".tsx", ".ts"]) {
                if !n.starts_with("Orbital") && n != "index" {
                    return Some((NodeKind::Trait, n));
                }
            }
            if let Some(n) = between(path, "/components/traits/", &["View.tsx", "View.ts"]) {
                return Some((NodeKind::Trait, n));
            }
            if let Some(stem) = path
                .strip_prefix("packages/server/src/routes/")
                .and_then(|p| p.strip_suffix(".ts"))
            {
                if stem != "index" && !stem.is_empty() {
                    return Some((NodeKind::Trait, kebab_to_pascal(stem)));
                }
            }
            if let Some((name, _)) = path
                .strip_prefix("packages/client/src/features/")
                .and_then(|r| r.split_once('/'))
            {
                return Some((NodeKind::Page, format!("/{}", name.replace('_', "/"))));
            }
            if path.starts_with("packages/shared/") {
                return first_ident_after(content, "export interface ")
                    .map(|n| (NodeKind::Entity, n));
            }
            None
        }
        CodeTarget::Py => {
            for dir in ["traits/", "routes/"] {
                if let Some(stem) = path.strip_prefix(dir).and_then(|p| p.strip_suffix(".py")) {
                    if stem != "__init__" && !stem.is_empty() {
                        return Some((NodeKind::Trait, snake_to_pascal(stem)));
                    }
                }
            }
            if path == "entities/__init__.py" {
                let mut pos = 0;
                while let Some(idx) = content[pos..].find("class ") {
                    let at = pos + idx;
                    if let Some(name) = first_ident_after(&content[at..], "class ") {
                        if !name.ends_with("State")
                            && !name.ends_with("Event")
                            && content[at..].contains("BaseModel")
                        {
                            return Some((NodeKind::Entity, name));
                        }
                    }
                    pos = at + 6;
                }
            }
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files() -> Vec<GeneratedFile> {
        vec![
            GeneratedFile::new(
                "packages/shared/src/types/entities.ts",
                "export interface ModalRecord { id: string; }\n",
            ),
            GeneratedFile::new("packages/client/src/hooks/traits/useFooLogic.tsx", "x"),
            GeneratedFile::new("packages/server/src/routes/modal-record-modal.ts", "y"),
        ]
    }

    #[test]
    fn section_and_only_select_by_the_shell_layout_and_a_control_selects_nothing() {
        let f = files();
        let names = |s: Section, o: Option<&[Only]>| {
            select(&f, &s, o, CodeTarget::Ts)
                .iter()
                .map(|x| x.path.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            names(Section::parse("entities.ModalRecord").unwrap(), None),
            [f[0].path.clone()]
        );
        assert_eq!(
            names(Section::parse("traits.ModalRecordModal").unwrap(), None),
            [f[2].path.clone()]
        );
        assert_eq!(
            names(Section::All, Some(&Only::parse_list("hooks").unwrap())),
            [f[1].path.clone()]
        );
        assert!(names(Section::parse("pages").unwrap(), None).is_empty());
        assert!(
            Section::parse("xyz").is_err()
                && Section::parse("entities.2Bad").is_err()
                && Only::parse_list("garbage").is_err()
        );
        assert_eq!(
            Section::parse("orbitals[0].traits.Foo").unwrap(),
            Section::Trait("Foo".into())
        );
    }

    #[test]
    fn anchors_name_the_node_a_file_stands_for() {
        let f = files();
        assert_eq!(
            anchor(&f[0], CodeTarget::Ts),
            Some((NodeKind::Entity, "ModalRecord".into()))
        );
        assert_eq!(
            anchor(&f[1], CodeTarget::Ts),
            Some((NodeKind::Trait, "Foo".into()))
        );
        assert_eq!(
            anchor(&f[2], CodeTarget::Ts),
            Some((NodeKind::Trait, "ModalRecordModal".into()))
        );
        let py = GeneratedFile::new("traits/modal_record_modal.py", "");
        assert_eq!(
            anchor(&py, CodeTarget::Py),
            Some((NodeKind::Trait, "ModalRecordModal".into()))
        );
        assert_eq!(
            anchor(&GeneratedFile::new("index.ts", ""), CodeTarget::Ts),
            None
        );
    }

    #[test]
    fn formats_render_differently() {
        let f = files();
        let refs: Vec<&GeneratedFile> = f.iter().collect();
        let outs: Vec<String> = [Format::Stream, Format::Markdown, Format::Json, Format::Raw]
            .map(|x| render(&refs, x, CodeTarget::Ts).unwrap())
            .into();
        assert!(
            outs[0].contains("// ───── packages/shared")
                && outs[1].contains("```typescript")
                && outs[2].starts_with('{')
        );
        assert_eq!(
            outs[3],
            "export interface ModalRecord { id: string; }\nx\ny\n"
        );
    }
}
