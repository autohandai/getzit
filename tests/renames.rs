//! A file git reports as renamed between base and result is a rename in the
//! footprint, not a delete plus an add: readers and writers of the old path
//! see only the units the move changed.

mod common;

use common::{git, write, Cli, Fixture};
use std::fs;
use zit::accept::{self, Invalid, Outcome, Status};
use zit::change::{self, Change, Record};
use zit::footprint::ConflictKind;
use zit::resource::Resource;
use zit::workspace;

const LIB: &str = "pub fn price(x: u32) -> u32 {\n    x\n}\n\npub fn tax(x: u32) -> u32 {\n    x / 10\n}\n";

fn fixture() -> Fixture {
    Fixture::new(&[("src/lib.rs", LIB), ("src/shop.rs", "pub fn buy() {}\n")])
}

/// A change that declared it read `src/lib.rs#price` and edited `src/shop.rs` without naming anything.
fn reader(fx: &Fixture) -> Change {
    let ws = fx.workspace("claude");
    workspace::declare_reads(&fx.repo, &ws.id, &[Resource::parse("src/lib.rs#price")]).unwrap();
    write(ws.path(), &[("src/shop.rs", "pub fn buy() { 1; }\n")]);
    let c = change::record(&fx.repo, &ws.id, &Record::default()).unwrap().unwrap();
    workspace::dispose(&fx.repo, &ws.id).unwrap();
    c
}

/// A change that moved `src/lib.rs` to `src/pricing.rs`, rewriting it when `body` is given.
fn moved(fx: &Fixture, body: Option<&str>) -> Change {
    let ws = fx.workspace("codex");
    fs::rename(ws.path().join("src/lib.rs"), ws.path().join("src/pricing.rs")).unwrap();
    if let Some(body) = body {
        fs::write(ws.path().join("src/pricing.rs"), body).unwrap();
    }
    let c = change::record(&fx.repo, &ws.id, &Record::default()).unwrap().unwrap();
    workspace::dispose(&fx.repo, &ws.id).unwrap();
    c
}

fn stale(status: Status) -> Vec<(String, ConflictKind, Option<zit::Oid>)> {
    match status {
        Status::Invalid(Invalid::Stale(s)) => s.into_iter().map(|s| (s.resource.to_string(), s.kind, s.by)).collect(),
        other => panic!("expected stale, got {other:?}"),
    }
}

#[test]
fn a_declared_reader_survives_a_rename_that_left_its_symbol_alone() {
    let fx = fixture();
    let r = reader(&fx);
    let m = moved(&fx, None);
    assert!(matches!(accept::accept(&fx.repo, &m.id).unwrap(), Outcome::Accepted { .. }));
    assert_eq!(accept::status(&fx.repo, &r.id).unwrap(), Status::Verified);
    let Outcome::Accepted { current, composed, .. } = accept::accept(&fx.repo, &r.id).unwrap() else {
        panic!("the reader was not accepted");
    };
    assert!(composed);
    assert_eq!(git(&fx.root(), &["show", &format!("{current}:src/shop.rs")]), "pub fn buy() { 1; }");
    assert!(git(&fx.root(), &["show", &format!("{current}:src/pricing.rs")]).contains("pub fn price"));
    assert!(!git(&fx.root(), &["ls-tree", "--name-only", "-r", current.as_str()]).contains("src/lib.rs"));
}

#[test]
fn a_declared_reader_is_stale_when_the_rename_changed_its_symbols_interface() {
    let fx = fixture();
    let r = reader(&fx);
    let m = moved(&fx, Some(&LIB.replace("price(x: u32)", "price(x: u32, t: u32)")));
    accept::accept(&fx.repo, &m.id).unwrap();
    assert_eq!(
        stale(accept::status(&fx.repo, &r.id).unwrap()),
        [("src/pricing.rs#price".to_string(), ConflictKind::ReadWrite, Some(m.id.clone()))]
    );
    assert!(matches!(accept::accept(&fx.repo, &r.id).unwrap(), Outcome::Rejected(Invalid::Stale(_))));
}

#[test]
fn a_rename_that_changes_a_symbol_is_stale_against_an_accepted_reader_of_the_old_path() {
    let fx = fixture();
    let r = reader(&fx);
    let m = moved(&fx, Some(&LIB.replace("price(x: u32)", "price(x: u32, t: u32)")));
    accept::accept(&fx.repo, &r.id).unwrap();
    assert_eq!(
        stale(accept::status(&fx.repo, &m.id).unwrap()),
        [("src/pricing.rs#price".to_string(), ConflictKind::WriteRead, Some(r.id.clone()))]
    );
    assert_eq!(accept::status(&fx.repo, &moved(&fx, None).id).unwrap(), Status::Verified, "a pure move is not");
}

#[test]
fn show_lists_a_rename() {
    let dir = tempfile::tempdir().unwrap();
    let cli = Cli::new(dir.path(), &[("src/lib.rs", LIB)]);
    cli.run(&["init"]).ok();
    let ws = cli.run(&["materialise", "--agent", "a", "--intent", "move"]).ok().stdout.trim().to_string();
    fs::rename(format!("{ws}/src/lib.rs"), format!("{ws}/src/pricing.rs")).unwrap();
    let id = cli.run_in(std::path::Path::new(&ws), &["record", "--dispose"]).ok().stdout.trim().to_string();
    let out = cli.run(&["show", &id]).ok();
    assert!(out.stdout.contains("renamed  src/lib.rs -> src/pricing.rs"), "{}", out.stdout);
    assert!(!out.stdout.contains("wrote"), "a pure move writes nothing: {}", out.stdout);
    let shown = cli.run(&["show", &id, "--json"]).ok().json();
    assert_eq!(shown["renames"], serde_json::json!({"src/lib.rs": "src/pricing.rs"}));
    assert_eq!(shown["writes"], serde_json::json!([]));
}
