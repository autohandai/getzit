//! Semantic index of one source file: top-level symbols, their content
//! hashes and the identifiers they reference.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Unit {
    pub hash: String,
    /// Hash of the unit without function bodies (and comments): its interface.
    /// Code that only mentions a symbol by name depends on this, not on `hash`.
    #[serde(default)]
    pub sig: String,
    pub refs: BTreeSet<String>,
}

/// The unit holding a file's imports. Concurrent additions to it are decided
/// by whether the text merges, like prose.
pub const IMPORTS: &str = "(imports)";

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileIndex {
    /// Top-level definitions by name. Methods belong to their type.
    pub symbols: BTreeMap<String, Unit>,
    /// Module-level code outside any symbol (imports, statements).
    pub top: Unit,
}

#[derive(Clone, Copy, PartialEq)]
enum Lang {
    Rust,
    Python,
    Js,
    Go,
    Java,
    Ruby,
    CSharp,
}

fn language(path: &str) -> Option<(Lang, tree_sitter::Language)> {
    let ext = path.rsplit_once('.')?.1;
    Some(match ext {
        "rs" => (Lang::Rust, tree_sitter_rust::LANGUAGE.into()),
        "py" => (Lang::Python, tree_sitter_python::LANGUAGE.into()),
        "js" | "mjs" | "cjs" | "jsx" => (Lang::Js, tree_sitter_javascript::LANGUAGE.into()),
        "ts" | "mts" | "cts" => (Lang::Js, tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()),
        "tsx" => (Lang::Js, tree_sitter_typescript::LANGUAGE_TSX.into()),
        "go" => (Lang::Go, tree_sitter_go::LANGUAGE.into()),
        "java" => (Lang::Java, tree_sitter_java::LANGUAGE.into()),
        "rb" => (Lang::Ruby, tree_sitter_ruby::LANGUAGE.into()),
        "cs" => (Lang::CSharp, tree_sitter_c_sharp::LANGUAGE.into()),
        _ => return None,
    })
}

/// Node kinds that annotate the next item: outer attributes and comments.
fn annotates(kind: &str) -> bool {
    matches!(kind, "attribute_item" | "line_comment" | "block_comment" | "comment")
}

fn is_comment(kind: &str) -> bool {
    matches!(kind, "line_comment" | "block_comment" | "comment")
}

fn is_import(lang: Lang, node: tree_sitter::Node, text: &str) -> bool {
    let kind = node.kind();
    match lang {
        Lang::Rust => matches!(kind, "use_declaration" | "extern_crate_declaration"),
        Lang::Python => matches!(kind, "import_statement" | "import_from_statement" | "future_import_statement"),
        Lang::Js => kind == "import_statement",
        Lang::Go => kind == "import_declaration",
        Lang::Java => kind == "import_declaration",
        // `require 'x'` is a call like any other to the grammar.
        Lang::CSharp => kind == "using_directive",
        Lang::Ruby => {
            kind == "call"
                && node.child_by_field_name("receiver").is_none()
                && node
                    .child_by_field_name("method")
                    .is_some_and(|m| matches!(&text[m.byte_range()], "require" | "require_relative" | "load"))
        }
    }
}

/// The nodes that make up the file's top level. C# code lives inside
/// namespace blocks, which are not units themselves: their declarations are
/// the top level and the namespace's name is module-level code.
fn top_level<'t>(lang: Lang, parent: tree_sitter::Node<'t>) -> Vec<tree_sitter::Node<'t>> {
    let mut out = Vec::new();
    let mut cursor = parent.walk();
    for node in parent.children(&mut cursor) {
        match (lang, node.kind()) {
            (Lang::CSharp, "namespace_declaration") => {
                out.extend(node.child_by_field_name("name"));
                if let Some(body) = node.child_by_field_name("body") {
                    out.extend(top_level(lang, body).into_iter().filter(|n| n.is_named()));
                }
            }
            _ => out.push(node),
        }
    }
    out
}

/// Visit `node` and every node below it.
fn walk(node: tree_sitter::Node, mut visit: impl FnMut(tree_sitter::Node)) {
    let mut cursor = node.walk();
    loop {
        visit(cursor.node());
        if cursor.goto_first_child() {
            continue;
        }
        loop {
            if cursor.node() == node {
                return;
            }
            if cursor.goto_next_sibling() {
                break;
            }
            cursor.goto_parent();
        }
    }
}

/// Names an import binds in this file: the receivers of module-qualified uses
/// such as `lib.price()` (Python, JavaScript) or `fmt.Println()` (Go).
fn imported_names(node: tree_sitter::Node, text: &str, out: &mut BTreeSet<String>) {
    walk(node, |n| {
        if n.child_count() == 0 && n.kind().ends_with("identifier") {
            out.insert(text[n.byte_range()].to_string());
        } else if n.kind() == "interpreted_string_literal" {
            // Go: `import "net/http"` binds `http`.
            let path = text[n.byte_range()].trim_matches('"');
            out.insert(path.rsplit('/').next().unwrap_or(path).to_string());
        }
    });
}

/// `node`'s text without the byte ranges in `cut` (which lie inside it).
fn without(node: tree_sitter::Node, text: &str, cut: &[std::ops::Range<usize>]) -> String {
    let (mut out, mut at) = (String::new(), node.start_byte());
    let mut cut: Vec<_> = cut.iter().filter(|c| c.start >= node.start_byte() && c.end <= node.end_byte()).collect();
    cut.sort_by_key(|c| c.start);
    for c in cut {
        if c.start >= at {
            out.push_str(&text[at..c.start]);
            at = c.end;
        }
    }
    out.push_str(&text[at..node.end_byte()]);
    out
}

/// The methods a type-defining node contains, as `(Type::method, node, name node id)`:
/// Rust `impl` blocks, Python classes, JavaScript and TypeScript classes.
fn methods<'t>(lang: Lang, node: tree_sitter::Node<'t>, text: &str) -> Vec<(String, tree_sitter::Node<'t>, usize)> {
    let name_of =
        |n: tree_sitter::Node| n.child_by_field_name("name").map(|c| (text[c.byte_range()].to_string(), c.id()));
    let (owner, body, kinds): (Option<String>, Option<tree_sitter::Node>, &[&str]) = match (lang, node.kind()) {
        (Lang::Rust, "impl_item") => (
            node.child_by_field_name("type").map(|t| type_name(&text[t.byte_range()])),
            node.child_by_field_name("body"),
            &["function_item"],
        ),
        (Lang::Python, "class_definition") => (
            name_of(node).map(|(n, _)| n),
            node.child_by_field_name("body"),
            &["function_definition", "decorated_definition"],
        ),
        (Lang::Python, "decorated_definition") => {
            return node.child_by_field_name("definition").map(|d| methods(lang, d, text)).unwrap_or_default()
        }
        (Lang::Js, "export_statement") => {
            return node.child_by_field_name("declaration").map(|d| methods(lang, d, text)).unwrap_or_default()
        }
        (Lang::Js, "class_declaration" | "abstract_class_declaration") => {
            // TypeScript's abstract methods and overload signatures are methods without a body.
            (
                name_of(node).map(|(n, _)| n),
                node.child_by_field_name("body"),
                &["method_definition", "abstract_method_signature", "method_signature"],
            )
        }
        (Lang::Java, "class_declaration" | "interface_declaration" | "enum_declaration" | "record_declaration") => {
            // An enum's methods sit after its constants, in `enum_body_declarations`.
            let body = node.child_by_field_name("body").map(|b| {
                let mut c = b.walk();
                let decls = b.named_children(&mut c).find(|n| n.kind() == "enum_body_declarations");
                decls.unwrap_or(b)
            });
            (name_of(node).map(|(n, _)| n), body, &["method_declaration", "constructor_declaration"])
        }
        (Lang::Ruby, "class" | "module") => {
            (name_of(node).map(|(n, _)| n), node.child_by_field_name("body"), &["method", "singleton_method"])
        }
        (Lang::CSharp, "class_declaration" | "interface_declaration" | "struct_declaration" | "record_declaration") => {
            (
                name_of(node).map(|(n, _)| n),
                node.child_by_field_name("body"),
                &["method_declaration", "constructor_declaration"],
            )
        }
        _ => return vec![],
    };
    let (Some(owner), Some(body)) = (owner, body) else { return vec![] };
    let mut cursor = body.walk();
    body.named_children(&mut cursor)
        .filter(|m| kinds.contains(&m.kind()))
        .filter_map(|m| {
            let named = match m.kind() {
                "decorated_definition" => m.child_by_field_name("definition").and_then(name_of),
                _ => name_of(m),
            };
            named.map(|(name, id)| (format!("{owner}::{name}"), m, id))
        })
        .collect()
}

/// The unit's interface: its text without function bodies or comments, nor
/// the ranges in `cut` (the methods that are units of their own).
fn signature(node: tree_sitter::Node, text: &str, cut: &[std::ops::Range<usize>]) -> String {
    if is_comment(node.kind()) {
        return String::new();
    }
    let mut cuts: Vec<std::ops::Range<usize>> =
        cut.iter().filter(|c| c.start >= node.start_byte() && c.end <= node.end_byte()).cloned().collect();
    walk(node, |n| {
        let k = n.kind();
        if is_comment(k) {
            cuts.push(n.byte_range());
        } else if k.contains("function") || k.contains("method") || k.contains("constructor") || k == "func_literal" {
            if let Some(body) = n.child_by_field_name("body") {
                cuts.push(body.byte_range());
            }
        }
    });
    cuts.sort_by_key(|r| r.start);
    let (mut out, mut at) = (String::new(), node.start_byte());
    for cut in cuts {
        if cut.start >= at {
            out.push_str(&text[at..cut.start]);
            at = cut.end;
        }
    }
    out.push_str(&text[at.max(node.start_byte())..node.end_byte().max(at)]);
    out
}

/// Source code Zit cannot parse into symbols: such a file is one resource
/// and no reads are inferred from it, so semantic staleness is not detected.
pub fn unparsed_code(path: &str) -> bool {
    const CODE: &[&str] = &[
        "kt", "kts", "scala", "fs", "c", "h", "cc", "cpp", "cxx", "hpp", "hh", "m", "mm", "swift", "php", "pl", "lua",
        "dart", "ex", "exs", "erl", "hs", "ml", "clj", "zig", "nim", "jl", "r", "sql", "sh", "bash", "vue", "svelte",
    ];
    let ext = path.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase());
    language(path).is_none() && ext.is_some_and(|e| CODE.contains(&e.as_str()))
}

/// Index `src`. `None` when the language is unsupported or the file is not
/// UTF-8; callers then treat the file as one indivisible resource.
pub fn index(path: &str, src: &[u8]) -> Option<FileIndex> {
    if matches!(path.rsplit_once('.').map(|(_, ext)| ext), Some("md" | "mdx" | "markdown")) {
        return std::str::from_utf8(src).ok().map(index_markdown);
    }
    let (lang, grammar) = language(path)?;
    let text = std::str::from_utf8(src).ok()?;
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&grammar).ok()?;
    let tree = parser.parse(text, None)?;

    let mut texts: BTreeMap<Option<String>, (String, String, BTreeSet<String>)> = BTreeMap::new();
    let root = tree.root_node();
    let mut imports = BTreeSet::new();
    let nodes = top_level(lang, root);
    for node in &nodes {
        if is_import(lang, *node, text) {
            imported_names(*node, text, &mut imports);
        }
    }
    // Attributes and comments wait for the next node: they belong to the item they annotate.
    let mut leading: Vec<tree_sitter::Node> = Vec::new();
    for node in nodes {
        if annotates(node.kind()) {
            leading.push(node);
            continue;
        }
        let import = is_import(lang, node, text);
        let defs = if import { vec![] } else { definitions(lang, node, text) };
        // The identifier that declares a symbol is not a reference to it.
        let declared: Vec<usize> = defs.iter().filter_map(|d| d.declared_by).collect();
        let keys: Vec<Option<String>> = match (import, defs.is_empty()) {
            (true, _) => vec![Some(IMPORTS.to_string())],
            (false, true) => vec![None],
            (false, false) => defs.into_iter().map(|d| Some(d.name)).collect(),
        };
        // Methods are units of their own, `Type::method`; the rest of the type stays `Type`.
        let methods = methods(lang, node, text);
        let cut: Vec<std::ops::Range<usize>> = methods.iter().map(|(_, m, _)| m.byte_range()).collect();
        for (name, method, declared_by) in &methods {
            let (body, sig, refs) = texts.entry(Some(name.clone())).or_default();
            body.push_str(&text[method.byte_range()]);
            sig.push_str(&signature(*method, text, &[]));
            collect_identifiers(lang, *method, text, &[*declared_by], &imports, refs);
        }
        for key in keys {
            let (body, sig, refs) = texts.entry(key).or_default();
            for part in leading.iter().chain([&node]) {
                let rest = without(*part, text, &cut);
                body.push_str(&rest);
                body.push('\n');
                sig.push_str(&signature(*part, text, &cut));
                sig.push('\n');
                let mut found = BTreeSet::new();
                collect_identifiers(lang, *part, text, &declared, &imports, &mut found);
                if !cut.is_empty() {
                    // Identifiers inside methods belong to the methods.
                    let mut inside = BTreeSet::new();
                    for (_, method, _) in &methods {
                        collect_identifiers(lang, *method, text, &[], &imports, &mut inside);
                    }
                    let mut outside = BTreeSet::new();
                    walk(*part, |n| {
                        if n.child_count() == 0
                            && !declared.contains(&n.id())
                            && !cut.iter().any(|c| c.contains(&n.start_byte()))
                        {
                            outside.insert(text[n.byte_range()].to_string());
                        }
                    });
                    found.retain(|id| !inside.contains(id) || outside.contains(id));
                }
                refs.extend(found);
            }
        }
        leading.clear();
    }
    // Trailing attributes or comments with no item after them.
    for part in leading {
        let (body, sig, refs) = texts.entry(None).or_default();
        body.push_str(&text[part.byte_range()]);
        body.push('\n');
        sig.push_str(&signature(part, text, &[]));
        collect_identifiers(lang, part, text, &[], &imports, refs);
    }

    let mut ix = FileIndex::default();
    for (key, (body, sig, refs)) in texts {
        let unit = Unit { hash: crate::hash(body.as_bytes()), sig: crate::hash(sig.as_bytes()), refs };
        match key {
            Some(name) => {
                ix.symbols.insert(name, unit);
            }
            None => ix.top = unit,
        }
    }
    Some(ix)
}

/// Markdown: each heading starts a section named after it; text before
/// the first heading is module-level. Prose carries no code references.
fn index_markdown(text: &str) -> FileIndex {
    let mut bodies: BTreeMap<Option<String>, String> = BTreeMap::new();
    let (mut section, mut fenced) = (None, false);
    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            fenced = !fenced;
        }
        let hashes = trimmed.bytes().take_while(|b| *b == b'#').count();
        if !fenced && (1..=6).contains(&hashes) && trimmed[hashes..].starts_with(' ') {
            section = Some(trimmed[hashes..].trim().trim_end_matches('#').trim().to_string());
        }
        let body = bodies.entry(section.clone()).or_default();
        body.push_str(line);
        body.push('\n');
    }
    let mut ix = FileIndex::default();
    for (name, body) in bodies {
        let hash = crate::hash(body.as_bytes());
        let unit = Unit { sig: hash.clone(), hash, refs: BTreeSet::new() };
        match name {
            Some(name) if !name.is_empty() => {
                ix.symbols.insert(name, unit);
            }
            _ => ix.top = unit,
        }
    }
    ix
}

/// A top-level symbol a node defines.
struct Def {
    name: String,
    /// Id of the identifier node that declares it; `None` when the name is
    /// itself a use (an `impl` target, a method receiver).
    declared_by: Option<usize>,
}

impl Def {
    fn at(node: tree_sitter::Node, text: &str) -> Def {
        Def { name: text[node.byte_range()].to_string(), declared_by: Some(node.id()) }
    }
    fn using(name: String) -> Def {
        Def { name, declared_by: None }
    }
}

/// Bare type name: `&mut a::Foo<T>` -> `Foo`, `Stack[T]` -> `Stack`.
fn type_name(raw: &str) -> String {
    let base = raw.split(['<', '[']).find(|s| !s.is_empty()).unwrap_or(raw);
    let base = base.rsplit("::").next().unwrap_or(base);
    base.trim_start_matches(['&', '*', ' ']).trim().to_string()
}

/// The top-level symbols `node` defines; empty for module-level code.
fn definitions(lang: Lang, node: tree_sitter::Node, text: &str) -> Vec<Def> {
    let field = |n: tree_sitter::Node, f: &str| n.child_by_field_name(f).map(|c| text[c.byte_range()].to_string());
    let named = |n: tree_sitter::Node| n.child_by_field_name("name").map(|c| Def::at(c, text)).into_iter().collect();
    let specs = |n: tree_sitter::Node, kinds: &[&str]| {
        let mut c = n.walk();
        n.named_children(&mut c)
            .filter(|s| kinds.contains(&s.kind()))
            .flat_map(|s| {
                let mut c = s.walk();
                s.children_by_field_name("name", &mut c)
                    .filter(|n| n.kind().ends_with("identifier"))
                    .map(|n| Def::at(n, text))
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>()
    };
    match (lang, node.kind()) {
        (Lang::Rust, "impl_item") => field(node, "type").map(|t| Def::using(type_name(&t))).into_iter().collect(),
        (Lang::Rust, k) if k.ends_with("_item") || k == "macro_definition" => named(node),
        (Lang::Python, "function_definition" | "class_definition") => named(node),
        (Lang::Python, "decorated_definition") => node.child_by_field_name("definition").map(named).unwrap_or_default(),
        (Lang::Python, "expression_statement") => node
            .named_child(0)
            .filter(|n| n.kind() == "assignment")
            .and_then(|n| n.child_by_field_name("left"))
            .filter(|n| n.kind() == "identifier")
            .map(|n| vec![Def::at(n, text)])
            .unwrap_or_default(),
        (Lang::Js, "export_statement") => {
            node.child_by_field_name("declaration").map(|d| definitions(lang, d, text)).unwrap_or_default()
        }
        (Lang::Js, "lexical_declaration" | "variable_declaration") => specs(node, &["variable_declarator"]),
        (Lang::Js, k) if k.ends_with("_declaration") || k == "internal_module" => named(node),
        (Lang::Go, "method_declaration") => field(node, "receiver")
            .zip(node.child_by_field_name("name"))
            .map(|(r, name)| {
                let inner = r.trim_matches(['(', ')']);
                let owner = type_name(inner.rsplit([' ', '*']).next().unwrap_or(inner));
                Def { name: format!("{owner}::{}", &text[name.byte_range()]), declared_by: Some(name.id()) }
            })
            .into_iter()
            .collect(),
        (Lang::Go, "function_declaration") => named(node),
        (Lang::Go, "type_declaration") => specs(node, &["type_spec", "type_alias"]),
        (Lang::Go, "const_declaration") => specs(node, &["const_spec"]),
        (Lang::Go, "var_declaration") => specs(node, &["var_spec"]),
        (
            Lang::Java,
            "class_declaration"
            | "interface_declaration"
            | "enum_declaration"
            | "record_declaration"
            | "annotation_type_declaration",
        ) => named(node),
        (Lang::Ruby, "class" | "module" | "method" | "singleton_method") => named(node),
        (Lang::Ruby, "assignment") => node
            .child_by_field_name("left")
            .filter(|n| n.kind() == "constant")
            .map(|n| vec![Def::at(n, text)])
            .unwrap_or_default(),
        (Lang::CSharp, "file_scoped_namespace_declaration" | "using_directive") => vec![],
        (Lang::CSharp, k) if k.ends_with("_declaration") => named(node),
        _ => vec![],
    }
}

fn collect_identifiers(
    lang: Lang,
    node: tree_sitter::Node,
    text: &str,
    skip: &[usize],
    imports: &BTreeSet<String>,
    out: &mut BTreeSet<String>,
) {
    walk(node, |n| {
        if n.child_count() == 0
            && is_identifier(lang, n.kind())
            && !skip.contains(&n.id())
            && !member_of_a_value(lang, n, text, imports)
        {
            out.insert(text[n.byte_range()].to_string());
        }
    });
}

/// Ruby names classes, modules and constants with `constant`, not an identifier kind.
fn is_identifier(lang: Lang, kind: &str) -> bool {
    kind.ends_with("identifier") || (lang == Lang::Ruby && kind == "constant")
}

/// `cache.get` names a member of a value, not the top-level `get`. A member
/// of an imported module (`lib.price`, `fmt.Println`) is still a reference.
fn member_of_a_value(lang: Lang, n: tree_sitter::Node, text: &str, imports: &BTreeSet<String>) -> bool {
    let Some(parent) = n.parent() else { return false };
    let is_field = |f: &str| parent.child_by_field_name(f).map(|a| a.id()) == Some(n.id());
    let receiver = match (lang, n.kind(), parent.kind()) {
        (Lang::Rust, "field_identifier" | "shorthand_field_identifier", _) => return true,
        (Lang::Go, "field_identifier", "selector_expression") => parent.child_by_field_name("operand"),
        (Lang::Js, "property_identifier", "member_expression") => parent.child_by_field_name("object"),
        (Lang::Python, "identifier", "attribute") if is_field("attribute") => parent.child_by_field_name("object"),
        (Lang::Java, "identifier", "field_access") if is_field("field") => parent.child_by_field_name("object"),
        (Lang::Java, "identifier", "method_invocation") if is_field("name") => parent.child_by_field_name("object"),
        (Lang::Ruby, "identifier", "call") if is_field("method") => parent.child_by_field_name("receiver"),
        (Lang::CSharp, "identifier", "member_access_expression") if is_field("name") => {
            parent.child_by_field_name("expression")
        }
        _ => return false,
    };
    // An unqualified call (`price()`) names the top-level symbol.
    let Some(r) = receiver else { return false };
    // A Ruby constant receiver (`Lib.price`) is a class or module, not a value.
    let module = matches!(r.kind(), "constant" | "scope_resolution")
        || (r.kind().ends_with("identifier") && imports.contains(&text[r.byte_range()]));
    !module
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(ix: &FileIndex) -> Vec<&str> {
        ix.symbols.keys().map(String::as_str).collect()
    }

    #[test]
    fn java_types_methods_and_imports_are_units() {
        let ix = index(
            "Shop.java",
            b"package shop;\nimport java.util.List;\nimport shop.Util;\n/** Doc. */\n@Entity\npublic class Shop extends Base {\n  private int n = 1;\n  public Shop() { n = 2; }\n  public int price(Item it) { return lib.tax(it.cost) + Util.max(1, 2) + n; }\n  static class Inner { void f() {} }\n}\ninterface I { int price(Item it); }\nenum E { A; int f() { return 1; } }\nrecord R(int a) { int g() { return a; } }\n@interface Ann {}\n",
        )
        .unwrap();
        assert_eq!(
            names(&ix),
            [IMPORTS, "Ann", "E", "E::f", "I", "I::price", "R", "R::g", "Shop", "Shop::Shop", "Shop::price"]
        );
        let refs = &ix.symbols["Shop::price"].refs;
        assert!(refs.contains("Util") && refs.contains("max"), "a member of an imported class: {refs:?}");
        assert!(refs.contains("Item") && !refs.contains("cost") && !refs.contains("tax"), "{refs:?}");
        assert!(ix.symbols["Shop"].refs.contains("Base") && ix.symbols["Shop"].refs.contains("Entity"));
        assert!(!ix.symbols["Shop"].refs.contains("Shop"), "declaring is not referencing");
        assert!(ix.symbols[IMPORTS].refs.contains("List"));
        assert!(ix.top.hash != ix.symbols["Shop"].hash, "the package line is module-level code");
    }

    #[test]
    fn ruby_classes_modules_methods_and_requires_are_units() {
        let ix = index(
            "shop.rb",
            b"require 'json'\nrequire_relative 'lib'\n# The shop.\nclass Shop < Base\n  include Comparable\n  attr_reader :n\n  def initialize(n)\n    @n = n\n  end\n  def price(item)\n    Lib.tax(item.cost) + helper(n)\n  end\n  def self.build\n    new(1)\n  end\nend\nmodule Util\n  LIMIT = 3\n  def self.max(a, b); a; end\nend\ndef free(x)\n  x\nend\nLIMIT = 2\nputs 1\n",
        )
        .unwrap();
        assert_eq!(
            names(&ix),
            [IMPORTS, "LIMIT", "Shop", "Shop::build", "Shop::initialize", "Shop::price", "Util", "Util::max", "free"]
        );
        let refs = &ix.symbols["Shop::price"].refs;
        assert!(refs.contains("Lib") && refs.contains("tax"), "a module's method is a reference: {refs:?}");
        assert!(refs.contains("helper") && !refs.contains("cost"), "a value's member is not: {refs:?}");
        let refs = &ix.symbols["Shop"].refs;
        assert!(refs.contains("Base") && refs.contains("Comparable") && !refs.contains("Shop"), "{refs:?}");
        assert!(ix.top.refs.contains("puts"));
        let a = index("shop.rb", b"require 'json'\ndef f\n  1\nend\n").unwrap();
        let b = index("shop.rb", b"require 'yaml'\ndef f\n  1\nend\n").unwrap();
        assert_ne!(a.symbols[IMPORTS], b.symbols[IMPORTS]);
        assert_eq!(a.symbols["f"], b.symbols["f"]);
    }

    #[test]
    fn ruby_interface_is_the_text_without_bodies_and_comments() {
        let src = "# Doc.\nclass Shop\n  attr_reader :n\n  # Price.\n  def price(item)\n    n\n  end\nend\n";
        let a = index("shop.rb", src.as_bytes()).unwrap();
        let body = index("shop.rb", src.replace("    n\n", "    n + 1\n").as_bytes()).unwrap();
        let docs = index("shop.rb", src.replace("Doc.", "Docs.").replace("Price.", "The price.").as_bytes()).unwrap();
        let param = index("shop.rb", src.replace("(item)", "(item, q)").as_bytes()).unwrap();
        let attr = index("shop.rb", src.replace(":n", ":m").as_bytes()).unwrap();
        assert_ne!(a.symbols["Shop::price"].hash, body.symbols["Shop::price"].hash);
        assert_eq!(a.symbols["Shop::price"].sig, body.symbols["Shop::price"].sig);
        assert_eq!(a.symbols["Shop"], body.symbols["Shop"]);
        assert_eq!(a.symbols["Shop"].sig, docs.symbols["Shop"].sig);
        assert_eq!(a.symbols["Shop::price"].sig, docs.symbols["Shop::price"].sig);
        assert_ne!(a.symbols["Shop::price"].sig, param.symbols["Shop::price"].sig);
        assert_ne!(a.symbols["Shop"].sig, attr.symbols["Shop"].sig);
    }

    #[test]
    fn csharp_types_methods_and_usings_are_units() {
        let ix = index(
            "Shop.cs",
            b"using System;\nusing Shop.Util;\nnamespace Shop.Core\n{\n  /// <summary>Doc.</summary>\n  [Serializable]\n  public class Shop : Base, I\n  {\n    private int n = 1;\n    public int Count { get; set; }\n    public Shop() { n = 2; }\n    public int Price(Item it) => lib.Tax(it.Cost) + Util.Max(1, 2) + n;\n    public void Run() { Console.WriteLine(n); }\n  }\n  public interface I { int Price(Item it); }\n  public enum E { A, B }\n  public struct S { public int X; public int Y() => X; }\n  public record R(int A);\n  public delegate int D(int x);\n}\n",
        )
        .unwrap();
        assert_eq!(
            names(&ix),
            [IMPORTS, "D", "E", "I", "I::Price", "R", "S", "S::Y", "Shop", "Shop::Price", "Shop::Run", "Shop::Shop"]
        );
        let refs = &ix.symbols["Shop::Price"].refs;
        assert!(refs.contains("Util") && refs.contains("Max"), "a member of a used namespace: {refs:?}");
        assert!(refs.contains("Item") && !refs.contains("Cost") && !refs.contains("Tax"), "{refs:?}");
        assert!(!ix.symbols["Shop::Run"].refs.contains("WriteLine"));
        let refs = &ix.symbols["Shop"].refs;
        assert!(refs.contains("Base") && refs.contains("Serializable") && !refs.contains("Shop"), "{refs:?}");
        let file_scoped =
            index("Shop.cs", b"namespace Shop.Core;\npublic class Shop { public int Price() => 1; }\n").unwrap();
        assert_eq!(names(&file_scoped), ["Shop", "Shop::Price"]);
        let renamed =
            index("Shop.cs", b"namespace Shop.Next;\npublic class Shop { public int Price() => 1; }\n").unwrap();
        assert_ne!(file_scoped.top.hash, renamed.top.hash, "the namespace is module-level code");
        assert_eq!(file_scoped.symbols, renamed.symbols);
    }

    #[test]
    fn csharp_interface_is_the_text_without_bodies_and_comments() {
        let src = "namespace N {\n  /// Doc.\n  class Shop {\n    int n = 1;\n    Shop() { n = 2; }\n    /// Price.\n    int Price(Item it) { return n; }\n    int Twice(int x) => x * 2;\n  }\n}\n";
        let a = index("Shop.cs", src.as_bytes()).unwrap();
        let body = index("Shop.cs", src.replace("return n;", "return n + 1;").as_bytes()).unwrap();
        let arrow = index("Shop.cs", src.replace("x * 2", "x + x").as_bytes()).unwrap();
        let ctor = index("Shop.cs", src.replace("n = 2;", "n = 3;").as_bytes()).unwrap();
        let docs = index("Shop.cs", src.replace("Doc.", "Docs.").replace("Price.", "The price.").as_bytes()).unwrap();
        let param = index("Shop.cs", src.replace("Item it", "Item it, int q").as_bytes()).unwrap();
        let field = index("Shop.cs", src.replace("int n = 1;", "long n = 1;").as_bytes()).unwrap();
        assert_ne!(a.symbols["Shop::Price"].hash, body.symbols["Shop::Price"].hash);
        assert_eq!(a.symbols["Shop::Price"].sig, body.symbols["Shop::Price"].sig);
        assert_eq!(a.symbols["Shop"], body.symbols["Shop"]);
        assert_ne!(a.symbols["Shop::Twice"].hash, arrow.symbols["Shop::Twice"].hash);
        assert_eq!(a.symbols["Shop::Twice"].sig, arrow.symbols["Shop::Twice"].sig, "an expression body is a body");
        assert_ne!(a.symbols["Shop::Shop"].hash, ctor.symbols["Shop::Shop"].hash);
        assert_eq!(a.symbols["Shop::Shop"].sig, ctor.symbols["Shop::Shop"].sig);
        assert_eq!(a.symbols["Shop"].sig, docs.symbols["Shop"].sig);
        assert_eq!(a.symbols["Shop::Price"].sig, docs.symbols["Shop::Price"].sig);
        assert_ne!(a.symbols["Shop::Price"].sig, param.symbols["Shop::Price"].sig);
        assert_ne!(a.symbols["Shop"].sig, field.symbols["Shop"].sig);
    }

    #[test]
    fn java_interface_is_the_text_without_bodies_and_comments() {
        let src = "/** Doc. */\nclass Shop {\n  int n = 1;\n  Shop() { n = 2; }\n  /** Price. */\n  int price(Item it) { return n; }\n}\n";
        let a = index("Shop.java", src.as_bytes()).unwrap();
        let body = index("Shop.java", src.replace("return n;", "return n + 1;").as_bytes()).unwrap();
        let ctor = index("Shop.java", src.replace("n = 2;", "n = 3;").as_bytes()).unwrap();
        let docs = index("Shop.java", src.replace("Doc.", "Docs.").replace("Price.", "The price.").as_bytes()).unwrap();
        let param = index("Shop.java", src.replace("Item it", "Item it, int q").as_bytes()).unwrap();
        let field = index("Shop.java", src.replace("int n = 1;", "long n = 1;").as_bytes()).unwrap();
        assert_ne!(a.symbols["Shop::price"].hash, body.symbols["Shop::price"].hash);
        assert_eq!(a.symbols["Shop::price"].sig, body.symbols["Shop::price"].sig);
        assert_eq!(a.symbols["Shop"], body.symbols["Shop"], "a method body is not part of its type");
        assert_ne!(a.symbols["Shop::Shop"].hash, ctor.symbols["Shop::Shop"].hash);
        assert_eq!(a.symbols["Shop::Shop"].sig, ctor.symbols["Shop::Shop"].sig);
        assert_eq!(a.symbols["Shop"].sig, docs.symbols["Shop"].sig, "comments are not the interface");
        assert_eq!(a.symbols["Shop::price"].sig, docs.symbols["Shop::price"].sig);
        assert_ne!(a.symbols["Shop::price"].sig, param.symbols["Shop::price"].sig);
        assert_ne!(a.symbols["Shop"].sig, field.symbols["Shop"].sig, "a field's type is interface");
        assert_eq!(a.symbols["Shop::price"], field.symbols["Shop::price"]);
    }

    #[test]
    fn rust_methods_are_units_of_their_type() {
        let ix = index(
            "a.rs",
            b"use std::fmt;\nstruct Foo;\nimpl Foo { fn new() -> Foo { Foo } }\nimpl fmt::Debug for Foo<'_> {}\nfn free() {}\nconst MAX: u8 = 1;\ntrait T {}\nenum E { A }\n",
        )
        .unwrap();
        assert_eq!(names(&ix), [IMPORTS, "E", "Foo", "Foo::new", "MAX", "T", "free"]);
    }

    #[test]
    fn editing_one_symbol_changes_only_its_hash() {
        let a = index("a.rs", b"fn foo() { 1 }\nfn bar() { 2 }\n").unwrap();
        let b = index("a.rs", b"fn foo() { 1 }\nfn bar() { 3 }\n").unwrap();
        assert_eq!(a.symbols["foo"], b.symbols["foo"]);
        assert_ne!(a.symbols["bar"].hash, b.symbols["bar"].hash);
        assert_eq!(a.top, b.top);
    }

    #[test]
    fn imports_are_their_own_unit() {
        let a = index("a.rs", b"use x::y;\nfn foo() {}\n").unwrap();
        let b = index("a.rs", b"use x::z;\nfn foo() {}\n").unwrap();
        assert_ne!(a.symbols[IMPORTS], b.symbols[IMPORTS]);
        assert_eq!(a.symbols["foo"], b.symbols["foo"]);
        assert_eq!(a.top, b.top);
    }

    #[test]
    fn code_outside_symbols_is_top() {
        let a = index("a.rs", b"fn foo() {}\nregister!(a);\n").unwrap();
        let b = index("a.rs", b"fn foo() {}\nregister!(b);\n").unwrap();
        assert_ne!(a.top.hash, b.top.hash);
        assert_eq!(a.symbols, b.symbols);
    }

    #[test]
    fn a_body_change_keeps_the_interface() {
        let a = index("a.rs", b"/// Price.\npub fn price(x: u32) -> u32 { x }\n").unwrap();
        let body = index("a.rs", b"/// Price.\npub fn price(x: u32) -> u32 { x + 1 }\n").unwrap();
        let doc = index("a.rs", b"/// The price.\npub fn price(x: u32) -> u32 { x }\n").unwrap();
        let sig = index("a.rs", b"/// Price.\npub fn price(x: u64) -> u32 { x }\n").unwrap();
        assert_ne!(a.symbols["price"].hash, body.symbols["price"].hash);
        assert_eq!(a.symbols["price"].sig, body.symbols["price"].sig);
        assert_eq!(a.symbols["price"].sig, doc.symbols["price"].sig, "comments are not the interface");
        assert_ne!(a.symbols["price"].sig, sig.symbols["price"].sig);
        for (path, before, after) in [
            ("a.py", "def f(x):\n    return x\n", "def f(x):\n    return x + 1\n"),
            ("a.ts", "export function f(x: number) { return x }\n", "export function f(x: number) { return x + 1 }\n"),
            ("a.go", "package a\nfunc F(x int) int { return x }\n", "package a\nfunc F(x int) int { return x + 1 }\n"),
        ] {
            let (a, b) = (index(path, before.as_bytes()).unwrap(), index(path, after.as_bytes()).unwrap());
            let name = a.symbols.keys().find(|k| *k != IMPORTS).unwrap();
            assert_eq!(a.symbols[name].sig, b.symbols[name].sig, "{path}");
            assert_ne!(a.symbols[name].hash, b.symbols[name].hash, "{path}");
        }
    }

    #[test]
    fn members_of_values_are_not_references_but_members_of_modules_are() {
        let ix = index("a.py", b"import lib\ndef run(cache):\n    return cache.get(1) + lib.price(2)\n").unwrap();
        let refs = &ix.symbols["run"].refs;
        assert!(refs.contains("price") && !refs.contains("get"), "{refs:?}");
        let ix = index(
            "a.ts",
            b"import * as lib from './lib';\nexport function run(c: C) { return c.get(1) + lib.price(2) }\n",
        )
        .unwrap();
        let refs = &ix.symbols["run"].refs;
        assert!(refs.contains("price") && !refs.contains("get"), "{refs:?}");
        let ix = index(
            "a.go",
            b"package a\nimport \"example.com/lib\"\nfunc Run(c C) int { return c.Get(1) + lib.Price(2) }\n",
        )
        .unwrap();
        let refs = &ix.symbols["Run"].refs;
        assert!(refs.contains("Price") && !refs.contains("Get"), "{refs:?}");
        let ix = index("a.rs", b"fn run(c: &Cache) -> u32 { c.get(1) + lib::price(2) }\n").unwrap();
        let refs = &ix.symbols["run"].refs;
        assert!(refs.contains("price") && !refs.contains("get"), "{refs:?}");
    }

    #[test]
    fn refs_are_the_identifiers_a_symbol_mentions() {
        let ix = index("a.rs", b"fn foo(c: Client) { stripe::charge(c.id) }\n").unwrap();
        let refs = &ix.symbols["foo"].refs;
        for id in ["Client", "stripe", "charge", "c"] {
            assert!(refs.contains(id), "missing {id} in {refs:?}");
        }
        assert!(!refs.contains("id"), "a field of a value is not a reference: {refs:?}");
    }

    #[test]
    fn defining_a_name_is_not_referencing_it() {
        let ix = index("a.py", b"import lib\ndef run():\n    return lib.f(1)\n").unwrap();
        assert_eq!(ix.symbols["run"].refs.iter().collect::<Vec<_>>(), ["f", "lib"]);
        let ix = index("a.rs", b"fn run() { f() }\nconst K: u8 = 1;\n").unwrap();
        assert!(!ix.symbols["run"].refs.contains("run") && !ix.symbols["K"].refs.contains("K"));
        let ix = index("a.ts", b"export function run() { f() }\nconst k = g();\n").unwrap();
        assert!(!ix.symbols["run"].refs.contains("run") && !ix.symbols["k"].refs.contains("k"));
        let ix = index("a.go", b"package a\nfunc Run() { f() }\n").unwrap();
        assert!(!ix.symbols["Run"].refs.contains("Run"));
    }

    #[test]
    fn recursion_and_use_as_a_type_are_still_references() {
        let ix = index("a.py", b"def fact(n):\n    return fact(n - 1)\n").unwrap();
        assert!(ix.symbols["fact"].refs.contains("fact"));
        let ix = index("a.rs", b"struct Foo;\nimpl Foo { fn new() -> Foo { Foo } }\n").unwrap();
        assert!(ix.symbols["Foo"].refs.contains("Foo"));
    }

    #[test]
    fn python_symbols() {
        let ix = index(
            "a.py",
            b"import os, lib\nLIMIT = 3\n@cache\ndef price(x):\n    return lib.tax(x)\nclass Shop:\n    def buy(self): pass\nprint(1)\n",
        )
        .unwrap();
        assert_eq!(names(&ix), [IMPORTS, "LIMIT", "Shop", "Shop::buy", "price"]);
        assert!(ix.symbols["price"].refs.contains("tax"));
        assert!(ix.symbols[IMPORTS].refs.contains("os"));
        assert!(ix.top.refs.contains("print"));
    }

    #[test]
    fn typescript_symbols() {
        let ix = index(
            "a.ts",
            b"import * as lib from './lib';\nexport function f(a: A): B { return lib.g(a) }\nexport class C {}\ninterface I {}\ntype T = string;\nexport const k = 1, m = 2;\nenum E { A }\nexport default f;\n",
        )
        .unwrap();
        assert_eq!(names(&ix), [IMPORTS, "C", "E", "I", "T", "f", "k", "m"]);
        assert!(ix.symbols["f"].refs.contains("g"));
    }

    /// Every language with classes names their methods `Class::method`.
    #[test]
    fn class_methods_are_units_of_their_class_in_every_language() {
        let ix = index(
            "a.ts",
            b"export class Shop {\n  n = 1;\n  constructor(n: number) { this.n = n }\n  price(it: Item): number { return lib.tax(it.cost) }\n  static build() { return new Shop(1) }\n  get count() { return this.n }\n  async run() {}\n}\nexport abstract class Base { abstract run(): void; go() {} }\nexport default class Def { m() {} }\n",
        )
        .unwrap();
        assert_eq!(
            names(&ix),
            [
                "Base",
                "Base::go",
                "Base::run",
                "Def",
                "Def::m",
                "Shop",
                "Shop::build",
                "Shop::constructor",
                "Shop::count",
                "Shop::price",
                "Shop::run",
            ]
        );
        assert!(ix.symbols["Shop::price"].refs.contains("Item"));
        assert!(!ix.symbols["Shop"].refs.contains("Item"), "a method's references are the method's");
        let ix = index(
            "a.js",
            b"class Shop {\n  price(it) { return it.cost }\n  static build() {}\n}\nmodule.exports = Shop;\n",
        )
        .unwrap();
        assert_eq!(names(&ix), ["Shop", "Shop::build", "Shop::price"]);
        let ix = index(
            "a.py",
            b"class Shop(Base):\n    n = 1\n    def __init__(self, n):\n        self.n = n\n    @staticmethod\n    def build():\n        return Shop(1)\n    async def run(self):\n        pass\n",
        )
        .unwrap();
        assert_eq!(names(&ix), ["Shop", "Shop::__init__", "Shop::build", "Shop::run"]);
        let ix = index("a.go", b"package a\ntype Shop struct{ n int }\nfunc (s Shop) Price() int { return s.n }\nfunc (s *Shop) Run() {}\n")
            .unwrap();
        assert_eq!(names(&ix), ["Shop", "Shop::Price", "Shop::Run"]);
    }

    #[test]
    fn tsx_and_javascript_symbols() {
        let ix = index("a.tsx", b"export function App() { return <Btn/> }\n").unwrap();
        assert_eq!(names(&ix), ["App"]);
        let ix = index("a.js", b"function f() {}\nclass C {}\nvar v = 1;\n").unwrap();
        assert_eq!(names(&ix), ["C", "f", "v"]);
    }

    #[test]
    fn go_methods_are_units_of_their_receiver() {
        let ix = index(
            "a.go",
            b"package a\nimport \"fmt\"\ntype S struct{}\nfunc (s *S) M() { fmt.Println() }\nfunc F() {}\nconst K = 1\nvar V = 2\n",
        )
        .unwrap();
        assert_eq!(names(&ix), [IMPORTS, "F", "K", "S", "S::M", "V"]);
        assert!(ix.symbols["S::M"].refs.contains("Println"));
    }

    #[test]
    fn go_methods_on_generic_types_are_named_after_the_type() {
        let ix = index("a.go", b"package a\ntype Stack[T any] struct{}\nfunc (s *Stack[T]) Push(v T) {}\n").unwrap();
        assert_eq!(names(&ix), ["Stack", "Stack::Push"]);
    }

    #[test]
    fn markdown_sections_are_symbols() {
        let doc = b"intro\n\n# Install\nrun it\n\n## Linux\napt\n\n```sh\n# not a heading\n```\n\n# Usage\nuse it\n";
        let ix = index("README.md", doc).unwrap();
        assert_eq!(names(&ix), ["Install", "Linux", "Usage"]);
        let edited = index("README.md", &String::from_utf8_lossy(doc).replace("apt", "dnf").into_bytes()).unwrap();
        assert_eq!(ix.symbols["Install"], edited.symbols["Install"]);
        assert_ne!(ix.symbols["Linux"], edited.symbols["Linux"]);
        assert_eq!(ix.top, edited.top);
        assert!(ix.symbols["Usage"].refs.is_empty(), "prose mentions are not code references");
        assert!(index("notes.mdx", b"# A\n").is_some());
    }

    #[test]
    fn unsupported_or_binary_files_have_no_index() {
        assert!(index("a.json", b"{}").is_none());
        assert!(index("Makefile", b"all:").is_none());
        assert!(unparsed_code("Shop.kt") && !unparsed_code("Shop.java") && !unparsed_code("a.rs"));
        assert!(!unparsed_code("shop.rb") && !unparsed_code("Shop.cs"));
        assert!(index("a.rs", &[0xff, 0xfe, 0x00]).is_none());
    }
}
