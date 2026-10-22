//! Generates the language reference's construct table from `src/syntax/ast.rs`'s own
//! declarations and doc comments.

use quote::ToTokens;
use std::fmt::Write;

const SOURCE: &str = "src/syntax/ast.rs";

fn doc_of(attrs: &[syn::Attribute]) -> String {
    attrs
        .iter()
        .filter(|a| a.path().is_ident("doc"))
        .filter_map(|a| match &a.meta {
            syn::Meta::NameValue(nv) => match &nv.value {
                syn::Expr::Lit(syn::ExprLit {
                    lit: syn::Lit::Str(s),
                    ..
                }) => Some(s.value().trim().to_string()),
                _ => None,
            },
            _ => None,
        })
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .to_string()
}

fn type_text(ty: &syn::Type) -> String {
    ty.to_token_stream()
        .to_string()
        .replace(" < ", "<")
        .replace("< ", "<")
        .replace(" >", ">")
        .replace(" ,", ",")
        .replace(" :: ", "::")
}

fn field(out: &mut String, name: &str, ty: &str, doc: &str) {
    write!(
        out,
        "ConstructField {{ name: {name:?}, ty: {ty:?}, doc: {doc:?} }}, "
    )
    .unwrap();
}

fn main() {
    println!("cargo:rerun-if-changed={SOURCE}");
    let source = std::fs::read_to_string(SOURCE).expect("read ast.rs");
    let file = syn::parse_file(&source).expect("parse ast.rs");
    let mut out = String::from("static CONSTRUCTS: &[Construct] = &[\n");
    for item in &file.items {
        let (name, kind, doc, fields) = match item {
            syn::Item::Struct(s) if matches!(s.vis, syn::Visibility::Public(_)) => {
                let mut fields = String::new();
                for f in &s.fields {
                    if let Some(ident) = &f.ident {
                        field(
                            &mut fields,
                            &ident.to_string(),
                            &type_text(&f.ty),
                            &doc_of(&f.attrs),
                        );
                    }
                }
                (s.ident.to_string(), "struct", doc_of(&s.attrs), fields)
            }
            syn::Item::Enum(e) if matches!(e.vis, syn::Visibility::Public(_)) => {
                let mut fields = String::new();
                for v in &e.variants {
                    let ty = v
                        .fields
                        .iter()
                        .map(|f| type_text(&f.ty))
                        .collect::<Vec<_>>()
                        .join(", ");
                    field(&mut fields, &v.ident.to_string(), &ty, &doc_of(&v.attrs));
                }
                (e.ident.to_string(), "enum", doc_of(&e.attrs), fields)
            }
            _ => continue,
        };
        writeln!(
            out,
            "    Construct {{ name: {name:?}, kind: {kind:?}, doc: {doc:?}, fields: &[{fields}] }},"
        )
        .unwrap();
    }
    out.push_str("];\n");
    let dest = std::path::Path::new(&std::env::var("OUT_DIR").unwrap()).join("ast_reference.rs");
    std::fs::write(dest, out).expect("write ast_reference.rs");
}
