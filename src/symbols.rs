//! Semantic index of one source file: top-level symbols, their content
//! hashes and the identifiers they reference.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Unit {
    pub hash: String,
    pub refs: BTreeSet<String>,
}

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
        _ => return None,
    })
}

/// Node kinds that annotate the next item: outer attributes and comments.
fn annotates(kind: &str) -> bool {
    matches!(kind, "attribute_item" | "line_comment" | "block_comment" | "comment")
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

    let mut texts: BTreeMap<Option<String>, (String, BTreeSet<String>)> = BTreeMap::new();
    let root = tree.root_node();
    let mut cursor = root.walk();
    // Attributes and comments wait for the next node: they belong to the item they annotate.
    let mut leading: Vec<tree_sitter::Node> = Vec::new();
    for node in root.children(&mut cursor) {
        if annotates(node.kind()) {
            leading.push(node);
            continue;
        }
        let defs = definitions(lang, node, text);
        // The identifier that declares a symbol is not a reference to it.
        let declared: Vec<usize> = defs.iter().filter_map(|d| d.declared_by).collect();
        let keys: Vec<Option<String>> = match defs.is_empty() {
            true => vec![None],
            false => defs.into_iter().map(|d| Some(d.name)).collect(),
        };
        for key in keys {
            let (body, refs) = texts.entry(key).or_default();
            for part in leading.iter().chain([&node]) {
                body.push_str(&text[part.byte_range()]);
                body.push('\n');
                collect_identifiers(*part, text, &declared, refs);
            }
        }
        leading.clear();
    }
    // Trailing attributes or comments with no item after them.
    for part in leading {
        let (body, refs) = texts.entry(None).or_default();
        body.push_str(&text[part.byte_range()]);
        body.push('\n');
        collect_identifiers(part, text, &[], refs);
    }

    let mut ix = FileIndex::default();
    for (key, (body, refs)) in texts {
        let unit = Unit { hash: crate::hash(body.as_bytes()), refs };
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
        let unit = Unit { hash: crate::hash(body.as_bytes()), refs: BTreeSet::new() };
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
            .map(|r| {
                let inner = r.trim_matches(['(', ')']);
                Def::using(type_name(inner.rsplit([' ', '*']).next().unwrap_or(inner)))
            })
            .into_iter()
            .collect(),
        (Lang::Go, "function_declaration") => named(node),
        (Lang::Go, "type_declaration") => specs(node, &["type_spec", "type_alias"]),
        (Lang::Go, "const_declaration") => specs(node, &["const_spec"]),
        (Lang::Go, "var_declaration") => specs(node, &["var_spec"]),
        _ => vec![],
    }
}

fn collect_identifiers(node: tree_sitter::Node, text: &str, skip: &[usize], out: &mut BTreeSet<String>) {
    let mut cursor = node.walk();
    loop {
        let n = cursor.node();
        if n.child_count() == 0 && n.kind().ends_with("identifier") && !skip.contains(&n.id()) {
            out.insert(text[n.byte_range()].to_string());
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn names(ix: &FileIndex) -> Vec<&str> {
        ix.symbols.keys().map(String::as_str).collect()
    }

    #[test]
    fn rust_methods_belong_to_their_type() {
        let ix = index(
            "a.rs",
            b"use std::fmt;\nstruct Foo;\nimpl Foo { fn new() -> Foo { Foo } }\nimpl fmt::Debug for Foo<'_> {}\nfn free() {}\nconst MAX: u8 = 1;\ntrait T {}\nenum E { A }\n",
        )
        .unwrap();
        assert_eq!(names(&ix), ["E", "Foo", "MAX", "T", "free"]);
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
    fn code_outside_symbols_is_top() {
        let a = index("a.rs", b"use x::y;\nfn foo() {}\n").unwrap();
        let b = index("a.rs", b"use x::z;\nfn foo() {}\n").unwrap();
        assert_ne!(a.top.hash, b.top.hash);
        assert_eq!(a.symbols, b.symbols);
    }

    #[test]
    fn refs_are_the_identifiers_a_symbol_mentions() {
        let ix = index("a.rs", b"fn foo(c: Client) { stripe::charge(c.id) }\n").unwrap();
        let refs = &ix.symbols["foo"].refs;
        for id in ["Client", "stripe", "charge", "id"] {
            assert!(refs.contains(id), "missing {id} in {refs:?}");
        }
    }

    #[test]
    fn defining_a_name_is_not_referencing_it() {
        let ix = index("a.py", b"def run():\n    return lib.f(1)\n").unwrap();
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
            b"import os\nLIMIT = 3\n@cache\ndef price(x):\n    return lib.tax(x)\nclass Shop:\n    def buy(self): pass\nprint(1)\n",
        )
        .unwrap();
        assert_eq!(names(&ix), ["LIMIT", "Shop", "price"]);
        assert!(ix.symbols["price"].refs.contains("tax"));
        assert!(ix.top.refs.contains("os"));
    }

    #[test]
    fn typescript_symbols() {
        let ix = index(
            "a.ts",
            b"import { x } from './x';\nexport function f(a: A): B { return lib.g(a) }\nexport class C {}\ninterface I {}\ntype T = string;\nexport const k = 1, m = 2;\nenum E { A }\nexport default f;\n",
        )
        .unwrap();
        assert_eq!(names(&ix), ["C", "E", "I", "T", "f", "k", "m"]);
        assert!(ix.symbols["f"].refs.contains("g"));
    }

    #[test]
    fn tsx_and_javascript_symbols() {
        let ix = index("a.tsx", b"export function App() { return <Btn/> }\n").unwrap();
        assert_eq!(names(&ix), ["App"]);
        let ix = index("a.js", b"function f() {}\nclass C {}\nvar v = 1;\n").unwrap();
        assert_eq!(names(&ix), ["C", "f", "v"]);
    }

    #[test]
    fn go_methods_belong_to_their_receiver() {
        let ix = index(
            "a.go",
            b"package a\nimport \"fmt\"\ntype S struct{}\nfunc (s *S) M() { fmt.Println() }\nfunc F() {}\nconst K = 1\nvar V = 2\n",
        )
        .unwrap();
        assert_eq!(names(&ix), ["F", "K", "S", "V"]);
        assert!(ix.symbols["S"].refs.contains("Println"));
    }

    #[test]
    fn go_methods_on_generic_types_belong_to_the_type() {
        let ix = index("a.go", b"package a\ntype Stack[T any] struct{}\nfunc (s *Stack[T]) Push(v T) {}\n").unwrap();
        assert_eq!(names(&ix), ["Stack"]);
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
        assert!(index("a.rs", &[0xff, 0xfe, 0x00]).is_none());
    }
}
