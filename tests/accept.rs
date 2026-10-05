mod common;

use common::{git, write, Fixture};
use std::fs;
use zit::accept::{self, Invalid, Outcome, Status};
use zit::change::{self, Record};
use zit::footprint::ConflictKind;
use zit::workspace;

const LIB: &str = "pub fn price(x: u32) -> u32 {\n    x\n}\n\npub fn tax(x: u32) -> u32 {\n    x / 10\n}\n";

fn fixture() -> Fixture {
    Fixture::new(&[("src/lib.rs", LIB), ("src/shop.rs", "pub fn buy() {}\n"), ("notes.txt", "n\n")])
}

fn show(fx: &Fixture, rev: &str, path: &str) -> String {
    git(&fx.root(), &["show", &format!("{rev}:{path}")])
}

fn accepted(outcome: Outcome) -> (zit::Oid, bool) {
    match outcome {
        Outcome::Accepted { current, composed, .. } => (current, composed),
        other => panic!("expected acceptance, got {other:?}"),
    }
}

#[test]
fn a_change_based_on_current_becomes_current() {
    let fx = fixture();
    let c = fx.change("claude", &[("notes.txt", "edited\n")]);
    assert_eq!(accept::status(&fx.repo, &c.id).unwrap(), Status::Verified);

    let (current, composed) = accepted(accept::accept(&fx.repo, &c.id).unwrap());
    assert_eq!((current.clone(), composed), (c.id.clone(), false));
    assert_eq!(fx.repo.current().unwrap(), c.id);
    assert_eq!(accept::status(&fx.repo, &c.id).unwrap(), Status::Current);
    assert!(change::speculative(&fx.repo).unwrap().is_empty());
}

#[test]
fn accepting_twice_is_a_no_op() {
    let fx = fixture();
    let c = fx.change("claude", &[("notes.txt", "edited\n")]);
    accept::accept(&fx.repo, &c.id).unwrap();
    assert!(matches!(accept::accept(&fx.repo, &c.id).unwrap(), Outcome::AlreadyAccepted));
}

#[test]
fn concurrent_changes_to_different_symbols_of_one_file_compose() {
    let fx = fixture();
    let a = fx.change("claude", &[("src/lib.rs", &LIB.replace("x / 10", "x / 5"))]);
    let b = fx.change("codex", &[("src/lib.rs", &LIB.replace("    x\n", "    x + 1\n"))]);

    accept::accept(&fx.repo, &a.id).unwrap();
    let (current, composed) = accepted(accept::accept(&fx.repo, &b.id).unwrap());

    assert!(composed);
    let merged = show(&fx, current.as_str(), "src/lib.rs");
    assert!(merged.contains("x / 5") && merged.contains("x + 1"), "{merged}");
    assert_eq!(change::load(&fx.repo, &current).unwrap().parents, vec![a.id.clone(), b.id.clone()]);
    assert_eq!(accept::status(&fx.repo, &b.id).unwrap(), Status::Accepted);
    assert!(change::speculative(&fx.repo).unwrap().is_empty());
}

#[test]
fn a_change_that_read_what_current_rewrote_is_stale_even_if_git_would_merge_it() {
    let fx = fixture();
    let callee = fx.change("claude", &[("src/lib.rs", &LIB.replace("price(x: u32)", "price(x: u32, t: u32)"))]);
    let caller = fx.change("codex", &[("src/shop.rs", "pub fn buy() { lib::price(3); }\n")]);
    accept::accept(&fx.repo, &callee.id).unwrap();

    let Outcome::Rejected(Invalid::Stale(why)) = accept::accept(&fx.repo, &caller.id).unwrap() else {
        panic!("expected stale");
    };
    assert_eq!(why.len(), 1);
    assert_eq!(why[0].resource.to_string(), "src/lib.rs#price");
    assert_eq!(why[0].kind, ConflictKind::ReadWrite);
    assert_eq!(why[0].by.as_ref(), Some(&callee.id));

    // Nothing was destroyed: current is untouched, the attempt is preserved.
    assert_eq!(fx.repo.current().unwrap(), callee.id);
    assert_eq!(change::speculative(&fx.repo).unwrap(), vec![caller.clone()]);
    assert!(matches!(accept::status(&fx.repo, &caller.id).unwrap(), Status::Invalid(Invalid::Stale(_))));
}

#[test]
fn a_textual_conflict_is_reported_as_such() {
    // Distinct symbols on adjacent lines: no semantic overlap, yet git cannot merge the text.
    let fx = Fixture::new(&[("m.py", "A = 1\nB = 2\n")]);
    let a = fx.change("a", &[("m.py", "A = 10\nB = 2\n")]);
    let b = fx.change("b", &[("m.py", "A = 1\nB = 20\n")]);
    accept::accept(&fx.repo, &a.id).unwrap();
    let Outcome::Rejected(Invalid::Conflict(paths)) = accept::accept(&fx.repo, &b.id).unwrap() else {
        panic!("expected textual conflict");
    };
    assert_eq!(paths, ["m.py"]);
    assert_eq!(fx.repo.current().unwrap(), a.id);
}

const GATE: &str = "[[check]]\nname = \"gate\"\nrun = \"! (grep -q A a.txt && grep -q B b.txt)\"\n";

#[test]
fn the_composed_state_must_pass_its_checks() {
    let fx = Fixture::new(&[("zit.toml", GATE), ("a.txt", "-\n"), ("b.txt", "-\n")]);
    let a = fx.change("a", &[("a.txt", "A\n")]);
    let b = fx.change("b", &[("b.txt", "B\n")]);
    accepted(accept::accept(&fx.repo, &a.id).unwrap());

    let Outcome::Rejected(Invalid::Failed(checks)) = accept::accept(&fx.repo, &b.id).unwrap() else {
        panic!("expected failed check");
    };
    assert_eq!(checks, ["gate"]);
    assert_eq!(fx.repo.current().unwrap(), a.id);
}

/// The gate is not the change's to remove: current's checks apply to every change it accepts.
#[test]
fn a_change_cannot_weaken_the_checks_it_is_judged_by() {
    let gate = "[[check]]\nname = \"gate\"\nrun = \"test ! -f bad\"\n";
    let fx = Fixture::new(&[("zit.toml", gate), ("a.txt", "a\n")]);
    let sneaky = fx.change("agent", &[("zit.toml", "[[check]]\nname = \"gate\"\nrun = \"true\"\n"), ("bad", "\n")]);

    let Outcome::Rejected(Invalid::Failed(checks)) = accept::accept(&fx.repo, &sneaky.id).unwrap() else {
        panic!("a change that rewrote its own gate was accepted");
    };
    assert_eq!(checks, ["gate"]);
}

/// A change may still add checks; they apply alongside current's.
#[test]
fn a_change_can_add_a_check() {
    let fx = Fixture::new(&[("a.txt", "a\n")]);
    let added = fx.change("agent", &[("zit.toml", "[[check]]\nname = \"new\"\nrun = \"false\"\n")]);
    let Outcome::Rejected(Invalid::Failed(checks)) = accept::accept(&fx.repo, &added.id).unwrap() else {
        panic!("the change's own new check did not run");
    };
    assert_eq!(checks, ["new"]);
}

#[test]
fn status_follows_the_evidence() {
    let fx = Fixture::new(&[("zit.toml", "[[check]]\nname = \"t\"\nrun = \"test -f ok\"\n"), ("ok", "\n")]);
    let good = fx.change("a", &[("x.txt", "x\n")]);
    let ws = fx.workspace("b");
    fs::remove_file(ws.path().join("ok")).unwrap();
    let bad = change::record(&fx.repo, &ws.id, &Record::default()).unwrap().unwrap();

    assert_eq!(accept::status(&fx.repo, &good.id).unwrap(), Status::Speculative);
    zit::evidence::verify(&fx.repo, &good.id, false).unwrap();
    zit::evidence::verify(&fx.repo, &bad.id, false).unwrap();
    assert_eq!(accept::status(&fx.repo, &good.id).unwrap(), Status::Verified);
    assert_eq!(accept::status(&fx.repo, &bad.id).unwrap(), Status::Invalid(Invalid::Failed(vec!["t".into()])));
}

#[test]
fn accepting_the_tip_of_a_chain_accepts_the_chain() {
    let fx = fixture();
    let c1 = fx.change("a", &[("notes.txt", "1\n")]);
    let c2 = fx.change_from(Some(&c1.id), "a", &[("notes.txt", "2\n")]);
    accepted(accept::accept(&fx.repo, &c2.id).unwrap());
    assert_eq!(accept::status(&fx.repo, &c1.id).unwrap(), Status::Accepted);
    assert!(change::speculative(&fx.repo).unwrap().is_empty());
}

#[test]
fn a_plain_git_commit_is_a_change() {
    let fx = fixture();
    write(&fx.root(), &[("notes.txt", "by hand\n")]);
    git(&fx.root(), &["commit", "-qam", "Human edit"]);
    let head = fx.repo.resolve("HEAD").unwrap();
    accepted(accept::accept(&fx.repo, &head).unwrap());
    assert_eq!(fx.repo.current().unwrap(), head);
    let c = change::load(&fx.repo, &head).unwrap();
    assert_eq!((c.intent.as_str(), c.agent.as_str()), ("Human edit", "human"));
}

#[test]
fn retry_rebuilds_a_stale_change_on_current_for_the_agent_to_reconsider() {
    let fx = fixture();
    let callee = fx.change("claude", &[("src/lib.rs", &LIB.replace("price(x: u32)", "price(x: u32, t: u32)"))]);
    let caller = fx.change("codex", &[("src/shop.rs", "pub fn buy() { lib::price(3); }\n")]);
    accept::accept(&fx.repo, &callee.id).unwrap();

    let ws = accept::retry(&fx.repo, &caller.id).unwrap();
    assert_eq!(ws.base, callee.id);
    assert_eq!((ws.agent.as_str(), ws.intent.as_str()), ("codex", "work"));
    assert!(fs::read_to_string(ws.path().join("src/lib.rs")).unwrap().contains("t: u32"));
    assert!(fs::read_to_string(ws.path().join("src/shop.rs")).unwrap().contains("price(3)"));

    write(ws.path(), &[("src/shop.rs", "pub fn buy() { lib::price(3, 1); }\n")]);
    let fixed = change::record(&fx.repo, &ws.id, &Record::default()).unwrap().unwrap();
    assert_eq!(fixed.parents, vec![callee.id.clone(), caller.id.clone()]);
    let (current, composed) = accepted(accept::accept(&fx.repo, &fixed.id).unwrap());
    assert_eq!((current, composed), (fixed.id, false));
}

#[test]
fn export_fast_forwards_a_checked_out_branch_and_its_files() {
    let fx = fixture();
    let c = fx.change("claude", &[("notes.txt", "exported\n")]);
    accept::accept(&fx.repo, &c.id).unwrap();

    assert_eq!(accept::export(&fx.repo, "main").unwrap(), c.id);
    assert_eq!(git(&fx.root(), &["rev-parse", "main"]), c.id.as_str());
    assert_eq!(fs::read_to_string(fx.root().join("notes.txt")).unwrap(), "exported\n");
    assert_eq!(git(&fx.root(), &["status", "--porcelain"]), "");

    assert_eq!(accept::export(&fx.repo, "release").unwrap(), c.id);
    assert_eq!(git(&fx.root(), &["rev-parse", "release"]), c.id.as_str());
}

#[test]
fn export_refuses_a_branch_that_diverged_from_current() {
    let fx = fixture();
    write(&fx.root(), &[("notes.txt", "by hand\n")]);
    git(&fx.root(), &["commit", "-qam", "Human edit"]);
    let c = fx.change("claude", &[("src/shop.rs", "pub fn buy() { 1; }\n")]);
    accept::accept(&fx.repo, &c.id).unwrap();
    assert!(accept::export(&fx.repo, "main").is_err());

    // The way out is the same primitive: accept the branch, then export.
    accept::accept(&fx.repo, &fx.repo.resolve("main").unwrap()).unwrap();
    accept::export(&fx.repo, "main").unwrap();
    assert_eq!(fs::read_to_string(fx.root().join("src/shop.rs")).unwrap(), "pub fn buy() { 1; }\n");
    assert_eq!(fs::read_to_string(fx.root().join("notes.txt")).unwrap(), "by hand\n");
}

#[test]
fn racing_accepts_all_land_exactly_once() {
    let fx = Fixture::new(&[("f0.txt", "-\n")]);
    let changes: Vec<_> = (0..8).map(|i| fx.change("agent", &[(&*format!("f{i}.txt"), "x\n")])).collect();
    std::thread::scope(|s| {
        for c in &changes {
            let repo = fx.repo.clone();
            s.spawn(move || accepted(accept::accept(&repo, &c.id).unwrap()));
        }
    });
    let files = git(&fx.root(), &["ls-tree", "--name-only", "refs/zit/current"]);
    assert_eq!(files.lines().count(), 8);
    assert!(change::speculative(&fx.repo).unwrap().is_empty());
    assert!(workspace::list(&fx.repo).unwrap().is_empty());
}

const CALLER: &str = "pub fn buy() { lib::price(3); }\n";

#[test]
fn the_acceptance_authority_may_let_evidence_decide_a_stale_change() {
    let fx = fixture();
    let callee = fx.change("claude", &[("src/lib.rs", &LIB.replace("    x\n", "    x + 1\n"))]);
    let caller = fx.change("codex", &[("src/shop.rs", CALLER)]);
    accept::accept(&fx.repo, &callee.id).unwrap();
    assert!(matches!(accept::accept(&fx.repo, &caller.id).unwrap(), Outcome::Rejected(Invalid::Stale(_))));

    let policy = accept::Policy { allow_stale: true, ..Default::default() };
    let (current, composed) = accepted(accept::accept_with(&fx.repo, &caller.id, &policy).unwrap());
    assert!(composed);
    assert!(show(&fx, current.as_str(), "src/lib.rs").contains("x + 1"));
    assert_eq!(show(&fx, current.as_str(), "src/shop.rs"), CALLER.trim());
}

#[test]
fn allowing_stale_still_requires_the_composed_state_to_pass_and_the_text_to_merge() {
    let gate =
        "[[check]]\nname = \"gate\"\nrun = \"! (grep -q 't: u32' src/lib.rs && grep -q 'price(3)' src/shop.rs)\"\n";
    let fx = Fixture::new(&[("zit.toml", gate), ("src/lib.rs", LIB), ("src/shop.rs", "pub fn buy() {}\n")]);
    let callee = fx.change("claude", &[("src/lib.rs", &LIB.replace("price(x: u32)", "price(x: u32, t: u32)"))]);
    let caller = fx.change("codex", &[("src/shop.rs", CALLER)]);
    let rival = fx.change("other", &[("src/lib.rs", &LIB.replace("price(x: u32)", "price(y: u32)"))]);
    accept::accept(&fx.repo, &callee.id).unwrap();

    let policy = accept::Policy { allow_stale: true, ..Default::default() };
    let failed = accept::accept_with(&fx.repo, &caller.id, &policy).unwrap();
    assert!(matches!(failed, Outcome::Rejected(Invalid::Failed(_))), "{failed:?}");
    let conflict = accept::accept_with(&fx.repo, &rival.id, &policy).unwrap();
    assert!(matches!(conflict, Outcome::Rejected(Invalid::Conflict(_))), "{conflict:?}");
    assert_eq!(fx.repo.current().unwrap(), callee.id);
}

#[test]
fn one_unevaluable_change_does_not_break_status_for_the_rest() {
    let fx = fixture();
    let good = fx.change("a", &[("notes.txt", "fine\n")]);
    let broken = fx.change("b", &[("zit.toml", "this is not toml [[[\n")]);
    let overview = zit::api::overview(&fx.repo).unwrap();
    let status = |id: &zit::Oid| overview.changes.iter().find(|r| r.change.id == *id).unwrap().status.clone();
    assert_eq!(status(&good.id), Status::Verified);
    assert!(matches!(status(&broken.id), Status::Invalid(Invalid::Error(_))), "{:?}", status(&broken.id));
}

#[test]
fn accept_gives_up_instead_of_spinning_when_current_cannot_be_moved() {
    let fx = fixture();
    let c = fx.change("a", &[("notes.txt", "x\n")]);
    std::fs::write(fx.repo.git_dir().join("refs/zit/current.lock"), "").unwrap();
    let started = std::time::Instant::now();
    let err = accept::accept(&fx.repo, &c.id).unwrap_err();
    assert!(started.elapsed() < std::time::Duration::from_secs(30));
    assert!(err.to_string().contains("refs/zit/current"), "{err}");
    assert_eq!(change::speculative(&fx.repo).unwrap(), vec![c], "the change is still there");
}

#[test]
fn a_cached_failure_can_be_rerun_at_accept_time() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("fixed");
    let config = format!("[[check]]\nname = \"flaky\"\nrun = \"test -f {}\"\n", marker.display());
    let fx = Fixture::new(&[("zit.toml", &config), ("a.txt", "a\n")]);
    let c = fx.change("a", &[("a.txt", "b\n")]);
    assert!(matches!(accept::accept(&fx.repo, &c.id).unwrap(), Outcome::Rejected(Invalid::Failed(_))));
    std::fs::write(&marker, "").unwrap();
    assert!(
        matches!(accept::accept(&fx.repo, &c.id).unwrap(), Outcome::Rejected(Invalid::Failed(_))),
        "failure is evidence"
    );
    let policy = accept::Policy { rerun: true, ..Default::default() };
    accepted(accept::accept_with(&fx.repo, &c.id, &policy).unwrap());
}

const DERIVE: &str = "[[derive]]\npath = \"registry.txt\"\nrun = \"ls items | sort > registry.txt\"\n\n[[check]]\nname = \"fresh\"\nrun = \"ls items | sort | diff - registry.txt\"\n";

/// A generated file is a function of other files: it is regenerated on the
/// composed state, never reported as a conflict.
#[test]
fn generated_files_are_regenerated_on_compose_not_conflicted() {
    let fx = Fixture::new(&[("zit.toml", DERIVE), ("items/a", "a\n"), ("registry.txt", "a\n")]);
    let b = fx.change("dev-1", &[("items/b", "b\n"), ("registry.txt", "a\nb\n")]);
    let c = fx.change("dev-2", &[("items/c", "c\n"), ("registry.txt", "a\nc\n")]);
    accepted(accept::accept(&fx.repo, &b.id).unwrap());
    let (current, composed) = accepted(accept::accept(&fx.repo, &c.id).unwrap());
    assert!(composed);
    assert_eq!(show(&fx, current.as_str(), "registry.txt"), "a\nb\nc");
    assert_eq!(change::load(&fx.repo, &current).unwrap().parents, vec![b.id, c.id]);
}

#[test]
fn a_generated_file_is_not_a_reason_to_be_stale() {
    let fx = Fixture::new(&[("zit.toml", DERIVE), ("items/a", "a\n"), ("registry.txt", "a\n")]);
    let b = fx.change("dev-1", &[("items/b", "b\n"), ("registry.txt", "a\nb\n")]);
    let c = fx.change("dev-2", &[("items/c", "c\n"), ("registry.txt", "a\nc\n")]);
    accept::accept(&fx.repo, &b.id).unwrap();
    assert_eq!(accept::status(&fx.repo, &c.id).unwrap(), Status::Speculative, "not stale: its checks have not run yet");
}

/// Prose has no read semantics: two edits to one Markdown section compose
/// when the text merges, and are a conflict when it does not.
#[test]
fn edits_to_one_markdown_section_compose_when_the_text_merges() {
    let list = "# Agents\n\n- alpha\n- delta\n- kilo\n- zulu\n";
    let fx = Fixture::new(&[("README.md", list)]);
    let b = fx.change("dev-1", &[("README.md", &list.replace("- delta\n", "- bravo\n- delta\n"))]);
    let c = fx.change("dev-2", &[("README.md", &list.replace("- zulu\n", "- yankee\n- zulu\n"))]);
    let d = fx.change("dev-3", &[("README.md", &list.replace("- delta\n", "- charlie\n- delta\n"))]);
    accepted(accept::accept(&fx.repo, &b.id).unwrap());
    let (current, _) = accepted(accept::accept(&fx.repo, &c.id).unwrap());
    assert_eq!(
        show(&fx, current.as_str(), "README.md"),
        "# Agents\n\n- alpha\n- bravo\n- delta\n- kilo\n- yankee\n- zulu"
    );
    assert!(matches!(accept::accept(&fx.repo, &d.id).unwrap(), Outcome::Rejected(Invalid::Conflict(_))));
}
