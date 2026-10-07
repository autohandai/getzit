mod common;

use common::Fixture;
use zit::change::{self, Record};
use zit::footprint::{self, ConflictKind, Footprint};
use zit::resource::Resource;
use zit::workspace;

const LIB: &str = "use std::fmt;\n\npub fn price(x: u32) -> u32 { x }\n\npub fn tax(x: u32) -> u32 { x / 10 }\n";

fn fixture() -> Fixture {
    Fixture::new(&[("src/lib.rs", LIB), ("src/shop.rs", "pub fn buy() {}\n"), ("data.csv", "a,b\n")])
}

fn res(items: &[&str]) -> Vec<Resource> {
    items.iter().map(|s| Resource::parse(s)).collect()
}

fn writes(fp: &Footprint) -> Vec<Resource> {
    fp.writes.iter().cloned().collect()
}

fn footprint_of(fx: &Fixture, files: &[(&str, &str)]) -> Footprint {
    let base = fx.repo.current().unwrap();
    let c = fx.change("agent", files);
    footprint::between(&fx.repo, &base, &c.id).unwrap()
}

#[test]
fn writes_are_symbol_granular() {
    let fx = fixture();
    let fp = footprint_of(&fx, &[("src/lib.rs", &LIB.replace("x / 10", "x / 5"))]);
    assert_eq!(writes(&fp), res(&["src/lib.rs#tax"]));
}

#[test]
fn editing_imports_writes_the_imports_unit() {
    let fx = fixture();
    let fp = footprint_of(&fx, &[("src/lib.rs", &LIB.replace("std::fmt", "std::io"))]);
    assert_eq!(writes(&fp), res(&["src/lib.rs#(imports)"]));
}

#[test]
fn editing_module_level_code_writes_top() {
    let fx = fixture();
    let fp = footprint_of(&fx, &[("src/lib.rs", &format!("{LIB}\nregister!(price);\n"))]);
    assert_eq!(writes(&fp), res(&["src/lib.rs#"]));
}

#[test]
fn a_new_file_writes_each_of_its_symbols() {
    let fx = fixture();
    let fp = footprint_of(&fx, &[("src/new.rs", "fn a() {}\nfn b() {}\n")]);
    assert_eq!(writes(&fp), res(&["src/new.rs#a", "src/new.rs#b"]));
}

#[test]
fn a_deleted_file_writes_each_of_its_symbols() {
    let fx = fixture();
    let base = fx.repo.current().unwrap();
    let ws = fx.workspace("agent");
    std::fs::remove_file(ws.path().join("src/shop.rs")).unwrap();
    let c = change::record(&fx.repo, &ws.id, &Record::default()).unwrap().unwrap();
    let fp = footprint::between(&fx.repo, &base, &c.id).unwrap();
    assert_eq!(writes(&fp), res(&["src/shop.rs#buy"]));
}

#[test]
fn files_without_a_grammar_are_written_whole() {
    let fx = fixture();
    let fp = footprint_of(&fx, &[("data.csv", "a,b\n1,2\n")]);
    assert_eq!(writes(&fp), res(&["data.csv"]));
}

const CARGO: &str = "[package]\nname = \"a\"\nversion = \"0.1.0\"\n\n[dependencies]\nserde = \"1\"\n";
const PACKAGE: &str =
    "{\n  \"name\": \"a\",\n  \"scripts\": { \"test\": \"jest\" },\n  \"dependencies\": { \"react\": \"19\" }\n}\n";

/// A manifest's top-level tables (TOML) or keys (JSON) are units: only the
/// section whose content changed is written, so edits to different sections compose.
#[test]
fn manifest_sections_are_units() {
    let fx = Fixture::new(&[("Cargo.toml", CARGO), ("package.json", PACKAGE)]);
    let deps =
        footprint_of(&fx, &[("Cargo.toml", &CARGO.replace("serde = \"1\"\n", "serde = \"1\"\nanyhow = \"1\"\n"))]);
    assert_eq!(writes(&deps), res(&["Cargo.toml#dependencies"]));
    let version = footprint_of(&fx, &[("Cargo.toml", &CARGO.replace("0.1.0", "0.2.0"))]);
    assert_eq!(writes(&version), res(&["Cargo.toml#package"]));
    assert_eq!(kinds(&deps, &version), []);
    let other_dep =
        footprint_of(&fx, &[("Cargo.toml", &CARGO.replace("serde = \"1\"\n", "serde = \"1\"\ntoml = \"1\"\n"))]);
    assert_eq!(kinds(&deps, &other_dep), [("Cargo.toml#dependencies".to_string(), ConflictKind::WriteWrite)]);
    assert!(deps.refs.is_empty() && version.refs.is_empty(), "no reads are inferred from a manifest");

    let scripts = footprint_of(&fx, &[("package.json", &PACKAGE.replace("jest", "vitest"))]);
    assert_eq!(writes(&scripts), res(&["package.json#scripts"]));
    let reformatted = footprint_of(
        &fx,
        &[("package.json", "{\"name\":\"a\",\"scripts\":{\"test\":\"jest\"},\"dependencies\":{\"react\":\"19\"}}")],
    );
    assert_eq!(writes(&reformatted), [], "formatting is not content");
    let new_key =
        footprint_of(&fx, &[("package.json", &PACKAGE.replace("\"name\"", "\"private\": true,\n  \"name\""))]);
    assert_eq!(writes(&new_key), res(&["package.json#private"]));
    let broken = footprint_of(&fx, &[("package.json", "{\n")]);
    assert_eq!(writes(&broken), res(&["package.json"]), "a manifest that does not parse is one resource");
}

#[test]
fn refs_come_only_from_what_the_change_touched() {
    let fx = fixture();
    let fp = footprint_of(&fx, &[("src/shop.rs", "pub fn buy() { lib::price(3); }\n")]);
    assert!(fp.refs.contains("price"));
    assert!(!fp.refs.contains("tax"));
}

#[test]
fn declared_reads_of_every_change_in_range_are_included() {
    let fx = fixture();
    let base = fx.repo.current().unwrap();
    let ws = fx.workspace("agent");
    workspace::declare_reads(&fx.repo, &ws.id, &res(&["data.csv"])).unwrap();
    common::write(ws.path(), &[("src/shop.rs", "pub fn buy() { 1; }\n")]);
    change::record(&fx.repo, &ws.id, &Record::default()).unwrap().unwrap();
    common::write(ws.path(), &[("src/shop.rs", "pub fn buy() { 2; }\n")]);
    let c2 = change::record(&fx.repo, &ws.id, &Record::default()).unwrap().unwrap();
    let fp = footprint::between(&fx.repo, &base, &c2.id).unwrap();
    assert_eq!(fp.reads.iter().cloned().collect::<Vec<_>>(), res(&["data.csv"]));
}

fn kinds(mine: &Footprint, theirs: &Footprint) -> Vec<(String, ConflictKind)> {
    mine.conflicts(theirs).into_iter().map(|c| (c.resource.to_string(), c.kind)).collect()
}

#[test]
fn different_symbols_of_one_file_do_not_conflict() {
    let fx = fixture();
    let a = footprint_of(&fx, &[("src/lib.rs", &LIB.replace("x / 10", "x / 5"))]);
    let b = footprint_of(&fx, &[("src/lib.rs", &LIB.replace("{ x }", "{ x + 1 }"))]);
    assert_eq!(kinds(&a, &b), []);
}

#[test]
fn writing_the_same_symbol_conflicts() {
    let fx = fixture();
    let a = footprint_of(&fx, &[("src/lib.rs", &LIB.replace("x / 10", "x / 5"))]);
    let b = footprint_of(&fx, &[("src/lib.rs", &LIB.replace("x / 10", "x / 4"))]);
    assert_eq!(kinds(&a, &b), [("src/lib.rs#tax".to_string(), ConflictKind::WriteWrite)]);
}

#[test]
fn referencing_a_symbol_someone_else_rewrote_conflicts_both_ways() {
    let fx = fixture();
    let caller = footprint_of(&fx, &[("src/shop.rs", "pub fn buy() { lib::price(3); }\n")]);
    let callee = footprint_of(&fx, &[("src/lib.rs", &LIB.replace("price(x: u32)", "price(x: u32, t: u32)"))]);
    assert_eq!(kinds(&caller, &callee), [("src/lib.rs#price".to_string(), ConflictKind::ReadWrite)]);
    assert_eq!(kinds(&callee, &caller), [("src/lib.rs#price".to_string(), ConflictKind::WriteRead)]);
}

#[test]
fn a_declared_file_read_conflicts_with_any_write_inside_it() {
    let fx = fixture();
    let mut reader = footprint_of(&fx, &[("src/shop.rs", "pub fn buy() { 1; }\n")]);
    reader.reads.insert(Resource::parse("src/lib.rs"));
    let writer = footprint_of(&fx, &[("src/lib.rs", &LIB.replace("x / 10", "x / 5"))]);
    assert_eq!(kinds(&reader, &writer), [("src/lib.rs#tax".to_string(), ConflictKind::ReadWrite)]);
}

/// An attribute or doc comment belongs to the item it annotates, not to module-level code.
#[test]
fn attributes_and_doc_comments_belong_to_their_item() {
    let before = zit::symbols::index("a.rs", b"/// A.\nstruct A;\n\nfn b() {}\n").unwrap();
    let after = zit::symbols::index("a.rs", b"/// A, documented.\n#[derive(Debug)]\nstruct A;\n\nfn b() {}\n").unwrap();
    assert_eq!(before.top, after.top, "module-level code did not change");
    assert_ne!(before.symbols["A"], after.symbols["A"], "A did");
    assert_eq!(before.symbols["b"], after.symbols["b"]);
}
