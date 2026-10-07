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
    let callee = fx.change("claude", &[("src/lib.rs", &LIB.replace("price(x: u32)", "price(x: u32, t: u32)"))]);
    let caller = fx.change("codex", &[("src/shop.rs", CALLER)]);
    accept::accept(&fx.repo, &callee.id).unwrap();
    assert!(matches!(accept::accept(&fx.repo, &caller.id).unwrap(), Outcome::Rejected(Invalid::Stale(_))));

    let policy = accept::Policy { allow_stale: true, ..Default::default() };
    let (current, composed) = accepted(accept::accept_with(&fx.repo, &caller.id, &policy).unwrap());
    assert!(composed);
    assert!(show(&fx, current.as_str(), "src/lib.rs").contains("t: u32"));
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

/// A caller depends on the callee's signature, not its body: a body-only fix does not stale it.
#[test]
fn a_callee_body_change_does_not_make_its_callers_stale() {
    let fx = fixture();
    let fix = fx.change("claude", &[("src/lib.rs", &LIB.replace("    x\n}", "    x + 0\n}"))]);
    let caller = fx.change("codex", &[("src/shop.rs", "pub fn buy() { lib::price(3); }\n")]);
    accepted(accept::accept(&fx.repo, &fix.id).unwrap());
    let (_, composed) = accepted(accept::accept(&fx.repo, &caller.id).unwrap());
    assert!(composed);
}

/// Declared reads still see any change, body included.
#[test]
fn a_declared_read_sees_a_body_change() {
    let fx = fixture();
    let fix = fx.change("claude", &[("src/lib.rs", &LIB.replace("    x\n}", "    x + 0\n}"))]);
    let ws = fx.workspace("codex");
    common::write(ws.path(), &[("src/shop.rs", "pub fn buy() { lib::price(3); }\n")]);
    let reads = [zit::resource::Resource::parse("src/lib.rs#price")];
    let caller =
        change::record(&fx.repo, &ws.id, &Record { reads: reads.to_vec(), ..Default::default() }).unwrap().unwrap();
    accepted(accept::accept(&fx.repo, &fix.id).unwrap());
    assert!(matches!(accept::accept(&fx.repo, &caller.id).unwrap(), Outcome::Rejected(Invalid::Stale(_))));
}

/// `cache.get()` is a method call, not a read of every top-level `get` in the repository.
#[test]
fn a_method_call_does_not_read_a_top_level_function_of_the_same_name() {
    let fx = Fixture::new(&[
        ("src/store.rs", "pub fn get(k: u32) -> u32 {\n    k\n}\n"),
        ("src/app.rs", "pub fn run() {}\n"),
    ]);
    let sig = fx.change("a", &[("src/store.rs", "pub fn get(k: u64) -> u64 {\n    k\n}\n")]);
    let user = fx.change("b", &[("src/app.rs", "pub fn run(cache: &Cache) { cache.get(1); }\n")]);
    accepted(accept::accept(&fx.repo, &sig.id).unwrap());
    accepted(accept::accept(&fx.repo, &user.id).unwrap());
}

/// Two changes that each add an import to one file compose when the text merges.
#[test]
fn concurrent_imports_to_one_file_compose() {
    let src = "use a::A;\n\nuse z::Z;\n\npub fn f() {}\n\npub fn g() {}\n";
    let fx = Fixture::new(&[("src/m.rs", src)]);
    let one = fx.change(
        "a",
        &[(
            "src/m.rs",
            &src.replace("use a::A;\n", "use a::A;\nuse b::B;\n").replace("pub fn f() {}", "pub fn f() { B; }"),
        )],
    );
    let two = fx.change(
        "b",
        &[(
            "src/m.rs",
            &src.replace("use z::Z;\n", "use y::Y;\nuse z::Z;\n").replace("pub fn g() {}", "pub fn g() { Y; }"),
        )],
    );
    accepted(accept::accept(&fx.repo, &one.id).unwrap());
    let (_, composed) = accepted(accept::accept(&fx.repo, &two.id).unwrap());
    assert!(composed);
}

/// Two agents editing different methods of one type compose; the type is not one indivisible unit.
#[test]
fn edits_to_different_methods_of_one_type_compose() {
    for (path, src, (a_from, a_to), (b_from, b_to)) in [
        (
            "src/shape.rs",
            "pub struct Shape { w: u32, h: u32 }\n\nimpl Shape {\n    pub fn area(&self) -> u32 {\n        self.w * self.h\n    }\n\n    pub fn perimeter(&self) -> u32 {\n        2 * (self.w + self.h)\n    }\n}\n",
            ("self.w * self.h", "self.h * self.w"),
            ("2 * (self.w + self.h)", "(self.w + self.h) * 2"),
        ),
        (
            "shape.py",
            "class Shape:\n    def area(self):\n        return self.w * self.h\n\n    def perimeter(self):\n        return 2 * (self.w + self.h)\n",
            ("self.w * self.h", "self.h * self.w"),
            ("2 * (self.w + self.h)", "(self.w + self.h) * 2"),
        ),
    ] {
        let fx = Fixture::new(&[(path, src)]);
        let one = fx.change("a", &[(path, &src.replace(a_from, a_to))]);
        let two = fx.change("b", &[(path, &src.replace(b_from, b_to))]);
        accepted(accept::accept(&fx.repo, &one.id).unwrap());
        let (_, composed) = accepted(accept::accept(&fx.repo, &two.id).unwrap());
        assert!(composed, "{path}");
    }
}

/// A check that leaves a mark each time it runs.
fn counting_check(scratch: &std::path::Path) -> (String, std::path::PathBuf) {
    let log = scratch.join("runs.log");
    (format!("[[check]]\nname = \"t\"\nrun = \"echo run >> {}\"\n", log.display()), log)
}

fn run_count(log: &std::path::Path) -> usize {
    fs::read_to_string(log).unwrap_or_default().lines().count()
}

fn change_with_intent(fx: &Fixture, agent: &str, intent: &str, files: &[(&str, &str)]) -> change::Change {
    let ws = fx.workspace(agent);
    write(ws.path(), files);
    let record = Record { intent: Some(intent.into()), ..Default::default() };
    let c = change::record(&fx.repo, &ws.id, &record).unwrap().unwrap();
    workspace::dispose(&fx.repo, &ws.id).unwrap();
    c
}

#[test]
fn a_batch_composes_every_change_checks_once_and_moves_current_once() {
    let scratch = tempfile::tempdir().unwrap();
    let (config, log) = counting_check(scratch.path());
    let fx = Fixture::new(&[("zit.toml", &config), ("a.txt", "-\n"), ("b.txt", "-\n"), ("c.txt", "-\n")]);
    let a = change_with_intent(&fx, "a", "Change a", &[("a.txt", "A\n")]);
    let b = change_with_intent(&fx, "b", "Change b", &[("b.txt", "B\n")]);
    let c = change_with_intent(&fx, "c", "Change c", &[("c.txt", "C\n")]);

    let ids = [a.id.clone(), b.id.clone(), c.id.clone()];
    let outcome = accept::accept_batch(&fx.repo, &ids, &accept::Policy::default()).unwrap();
    let accept::BatchOutcome::Accepted { current, landed, skipped, verdicts } = outcome else {
        panic!("expected the batch to land, got {outcome:?}");
    };
    assert_eq!(landed, ids);
    assert!(skipped.is_empty());
    assert_eq!(fx.repo.current().unwrap(), current);
    assert_eq!(run_count(&log), 1, "the checks ran once, on the combined state");
    assert_eq!(verdicts.len(), 1);
    for file in ["a.txt", "b.txt", "c.txt"] {
        assert_eq!(show(&fx, current.as_str(), file), file[..1].to_uppercase());
    }
    assert!(change::speculative(&fx.repo).unwrap().is_empty());

    // Each change is its own step of history, with its reason.
    let history = change::accepted(&fx.repo, 10).unwrap();
    let intents: Vec<&str> = history.iter().map(|c| c.intent.as_str()).collect();
    assert_eq!(intents.len(), 4, "{intents:?}");
    assert!(intents[0].ends_with(": Change c") && intents[1].ends_with(": Change b"), "{intents:?}");
    assert_eq!(&intents[2..], ["Change a", "genesis"]);
    assert_eq!(history[0].parents[1], c.id, "the compose of c has c as its second parent");
    assert_eq!(history[1].parents[1], b.id);
    assert_eq!(history[2].id, a.id, "the first change was built on current: a fast-forward");
}

#[test]
fn a_batch_skips_what_is_stale_or_conflicts_and_says_why() {
    let fx = fixture();
    let callee = fx.change("claude", &[("src/lib.rs", &LIB.replace("price(x: u32)", "price(x: u32, t: u32)"))]);
    let caller = fx.change("codex", &[("src/shop.rs", CALLER)]);
    let note = fx.change("gemini", &[("notes.txt", "edited\n")]);

    let ids = [callee.id.clone(), caller.id.clone(), note.id.clone()];
    let outcome = accept::accept_batch(&fx.repo, &ids, &accept::Policy::default()).unwrap();
    let accept::BatchOutcome::Accepted { landed, skipped, .. } = outcome else { panic!("{outcome:?}") };
    assert_eq!(landed, [callee.id.clone(), note.id.clone()]);
    assert_eq!(skipped.len(), 1);
    assert_eq!(skipped[0].change, caller.id);
    let Invalid::Stale(why) = &skipped[0].reason else { panic!("{:?}", skipped[0].reason) };
    assert_eq!(why[0].resource.to_string(), "src/lib.rs#price");
    assert_eq!(why[0].by.as_ref(), Some(&callee.id), "stale against what landed earlier in the batch");
    assert_eq!(change::speculative(&fx.repo).unwrap(), vec![caller], "a skipped change stays in the graph");
}

#[test]
fn a_batch_lands_nothing_when_the_combined_state_fails_a_check() {
    let fx = Fixture::new(&[("zit.toml", GATE), ("a.txt", "-\n"), ("b.txt", "-\n")]);
    let a = fx.change("a", &[("a.txt", "A\n")]);
    let b = fx.change("b", &[("b.txt", "B\n")]);
    let before = fx.repo.current().unwrap();

    let ids = [a.id.clone(), b.id.clone()];
    let outcome = accept::accept_batch(&fx.repo, &ids, &accept::Policy::default()).unwrap();
    let accept::BatchOutcome::Rejected { failed, tried, .. } = outcome else { panic!("{outcome:?}") };
    assert_eq!(failed, ["gate"]);
    assert_eq!(tried, ids);
    assert_eq!(fx.repo.current().unwrap(), before);
    assert_eq!(change::speculative(&fx.repo).unwrap().len(), 2);
    // One at a time finds the culprit: a lands, b is what fails.
    accepted(accept::accept(&fx.repo, &a.id).unwrap());
    assert!(matches!(accept::accept(&fx.repo, &b.id).unwrap(), Outcome::Rejected(Invalid::Failed(_))));
}

/// `--batch` with no ids takes every verified change; one without evidence is not verified.
#[test]
fn the_default_batch_is_every_verified_change() {
    let fx = Fixture::new(&[("zit.toml", "[[check]]\nname = \"t\"\nrun = \"true\"\n"), ("a.txt", "a\n")]);
    let checked = fx.change("a", &[("a.txt", "A\n")]);
    let unchecked = fx.change("b", &[("b.txt", "B\n")]);
    zit::evidence::verify(&fx.repo, &checked.id, false).unwrap();
    assert_eq!(accept::verified(&fx.repo).unwrap(), vec![checked.id.clone()]);
    let fx2 = fixture();
    let x = fx2.change("a", &[("notes.txt", "x\n")]);
    let y = fx2.change("b", &[("src/shop.rs", "pub fn buy() { 1; }\n")]);
    assert_eq!(
        accept::verified(&fx2.repo).unwrap(),
        vec![x.id, y.id],
        "no checks: every composable change is verified"
    );
    drop(unchecked);
}

#[test]
fn batch_from_the_shell_lists_what_landed_and_what_was_skipped() {
    let dir = tempfile::tempdir().unwrap();
    let cli =
        common::Cli::new(dir.path(), &[("src/lib.rs", LIB), ("src/shop.rs", "pub fn buy() {}\n"), ("n.txt", "n\n")]);
    cli.run(&["init"]).ok();
    let make = |agent: &str, files: &[(&str, &str)]| {
        let ws = cli.run(&["materialise", "--agent", agent, "--intent", "edit", "--json"]).ok().json();
        let path = std::path::PathBuf::from(ws["path"].as_str().unwrap());
        write(&path, files);
        cli.run_in(&path, &["record", "--dispose", "--json"]).ok().json()["change"]["id"].as_str().unwrap().to_string()
    };
    let callee = make("claude", &[("src/lib.rs", &LIB.replace("price(x: u32)", "price(x: u32, t: u32)"))]);
    let caller = make("codex", &[("src/shop.rs", CALLER)]);
    let note = make("gemini", &[("n.txt", "edited\n")]);

    // Several ids: composed in the order given.
    let ran = cli.run(&["accept", &callee, &caller, &note]).ok();
    assert!(ran.stdout.contains(&format!("landed   {}", &callee[..10])), "{}", ran.stdout);
    assert!(ran.stdout.contains(&format!("landed   {}", &note[..10])), "{}", ran.stdout);
    assert!(ran.stdout.contains(&format!("skipped  {}  invalid: stale", &caller[..10])), "{}", ran.stdout);
    assert!(ran.stdout.contains("src/lib.rs#price"), "{}", ran.stdout);
    assert!(ran.stdout.contains("2 landed, 1 skipped"), "{}", ran.stdout);

    // --batch takes what is verified; the stale change is not even tried.
    let fresh = make("pi", &[("n.txt", "again\n")]);
    let out = cli.run(&["accept", "--batch", "--json"]).ok().json();
    assert_eq!(out["outcome"], "accepted");
    assert_eq!(out["landed"], serde_json::json!([fresh]));
    assert_eq!(out["skipped"], serde_json::json!([]));

    // Named explicitly, a skipped change is reported with its reason.
    let again = make("pi", &[("n.txt", "and again\n")]);
    let out = cli.run(&["accept", &caller, &again, "--json"]).ok().json();
    assert_eq!(out["landed"], serde_json::json!([again]));
    assert_eq!(out["skipped"][0]["change"], caller.as_str());
    assert_eq!(out["skipped"][0]["reason"], "stale");
    assert_eq!(
        cli.run(&["accept", &caller, &again]).code,
        0,
        "already accepted and stale: nothing to do, not an error"
    );
}

#[test]
fn a_dry_run_reports_the_outcome_and_the_checks_but_moves_and_runs_nothing() {
    let scratch = tempfile::tempdir().unwrap();
    let (config, log) = counting_check(scratch.path());
    let config = format!("{config}\n[[check]]\nname = \"scoped\"\nrun = \"true\"\ninputs = [\"b.txt\"]\n");
    let fx = Fixture::new(&[("zit.toml", &config), ("a.txt", "-\n"), ("b.txt", "-\n")]);
    let a = fx.change("a", &[("a.txt", "A\n")]);
    let before = fx.repo.current().unwrap();
    zit::evidence::verify(&fx.repo, &before, false).unwrap();
    assert_eq!(run_count(&log), 1);

    let policy = accept::Policy::default();
    let dry = accept::dry_run(&fx.repo, &a.id, &policy).unwrap();
    let accept::DryRun::WouldAccept { composed, checks } = dry else { panic!("{dry:?}") };
    assert!(!composed);
    let planned: Vec<(&str, bool)> = checks.iter().map(|c| (c.check.as_str(), c.run)).collect();
    assert_eq!(planned, [("t", true), ("scoped", false)], "scoped's input is unchanged, so it would be reused");
    assert_eq!(run_count(&log), 1, "nothing ran");
    assert_eq!(fx.repo.current().unwrap(), before, "nothing moved");
    assert_eq!(change::speculative(&fx.repo).unwrap(), vec![a.clone()]);

    // --rerun would run everything again.
    let rerun = accept::Policy { rerun: true, ..Default::default() };
    let accept::DryRun::WouldAccept { checks, .. } = accept::dry_run(&fx.repo, &a.id, &rerun).unwrap() else {
        panic!()
    };
    assert!(checks.iter().all(|c| c.run));

    // Once current moves, the dry run composes and validates against it.
    let other = fx.change("b", &[("b.txt", "B\n")]);
    accepted(accept::accept(&fx.repo, &other.id).unwrap());
    let accept::DryRun::WouldAccept { composed, .. } = accept::dry_run(&fx.repo, &a.id, &policy).unwrap() else {
        panic!()
    };
    assert!(composed);
    assert!(matches!(accept::dry_run(&fx.repo, &other.id, &policy).unwrap(), accept::DryRun::AlreadyAccepted));
}

#[test]
fn a_dry_run_rejects_what_accept_would_reject() {
    let fx = fixture();
    let callee = fx.change("claude", &[("src/lib.rs", &LIB.replace("price(x: u32)", "price(x: u32, t: u32)"))]);
    let caller = fx.change("codex", &[("src/shop.rs", CALLER)]);
    accepted(accept::accept(&fx.repo, &callee.id).unwrap());
    let dry = accept::dry_run(&fx.repo, &caller.id, &accept::Policy::default()).unwrap();
    assert!(matches!(dry, accept::DryRun::Rejected(Invalid::Stale(_))), "{dry:?}");

    let fx = Fixture::new(&[("m.go", GO)]);
    let (wrap, split) = wrap_and_split(&fx);
    accepted(accept::accept(&fx.repo, &wrap.id).unwrap());
    let policy = accept::Policy { allow_stale: true, ..Default::default() };
    let dry = accept::dry_run(&fx.repo, &split.id, &policy).unwrap();
    assert!(matches!(dry, accept::DryRun::Rejected(Invalid::Error(_))), "{dry:?}");
}

#[test]
fn dry_run_from_the_shell() {
    let dir = tempfile::tempdir().unwrap();
    let cli =
        common::Cli::new(dir.path(), &[("zit.toml", "[[check]]\nname = \"t\"\nrun = \"true\"\n"), ("a.txt", "a\n")]);
    cli.run(&["init"]).ok();
    let ws = cli.run(&["materialise", "--agent", "a", "--intent", "Edit a", "--json"]).ok().json();
    let path = std::path::PathBuf::from(ws["path"].as_str().unwrap());
    write(&path, &[("a.txt", "A\n")]);
    let id =
        cli.run_in(&path, &["record", "--dispose", "--json"]).ok().json()["change"]["id"].as_str().unwrap().to_string();

    let ran = cli.run(&["accept", "--dry-run", &id]).ok();
    assert!(ran.stdout.contains("would accept") && ran.stdout.contains("fast-forward"), "{}", ran.stdout);
    assert!(ran.stdout.contains("t (run)"), "{}", ran.stdout);
    let out = cli.run(&["accept", "--dry-run", &id, "--json"]).ok().json();
    assert_eq!(out["outcome"], "would-accept");
    assert_eq!(out["checks"][0]["check"], "t");
    assert_eq!(out["checks"][0]["run"], true);
    let status = cli.run(&["status", "--json"]).ok().json();
    assert_eq!(status["changes"][0]["id"], id.as_str(), "still speculative");
    assert_eq!(status["changes"][0]["status"], "speculative", "no evidence was produced");
}

const GO: &str = "package m\n\nfunc f() {\n\ta()\n\tb()\n\tc()\n\td()\n\te()\n}\n";

/// One side wraps the body in a block, the other splits the function: each
/// parses, git merges the text, and the result is a function inside a function.
fn wrap_and_split(fx: &Fixture) -> (zit::change::Change, zit::change::Change) {
    let wrap =
        fx.change("a", &[("m.go", &GO.replace("\ta()\n", "\tif x {\n\t\ta()\n").replace("\te()\n", "\te()\n\t}\n"))]);
    let split = fx.change("b", &[("m.go", &GO.replace("\tc()\n", "\tc()\n}\n\nfunc g() {\n"))]);
    (wrap, split)
}

#[test]
fn a_composed_state_that_does_not_parse_is_rejected_even_when_stale_is_allowed() {
    let fx = Fixture::new(&[("m.go", GO)]);
    let (wrap, split) = wrap_and_split(&fx);
    accepted(accept::accept(&fx.repo, &wrap.id).unwrap());
    let policy = accept::Policy { allow_stale: true, ..Default::default() };
    let Outcome::Rejected(Invalid::Error(why)) = accept::accept_with(&fx.repo, &split.id, &policy).unwrap() else {
        panic!("expected a parse rejection");
    };
    assert_eq!(why, "does not parse after composing: m.go");
    assert_eq!(fx.repo.current().unwrap(), wrap.id);
    assert_eq!(change::speculative(&fx.repo).unwrap(), vec![split]);
}

/// Imports are decided by text alone, so two edits there compose without being
/// stale; the composed file must still parse.
const COMMENTED_IMPORT: &str = "use a::A;\n/* off:\n// slow\nuse b::B;\n// end\n*/\nuse c::C;\n\npub fn f() {}\n";

fn uncomment_and_annotate(fx: &Fixture) -> (zit::change::Change, zit::change::Change) {
    let uncomment = fx.change("a", &[("m.rs", &COMMENTED_IMPORT.replace("/* off:\n", "").replace("*/\n", ""))]);
    let annotate = fx.change("b", &[("m.rs", &COMMENTED_IMPORT.replace("use b::B;", "use b::B (see #12)"))]);
    (uncomment, annotate)
}

#[test]
fn the_parse_check_is_on_by_default_for_text_decided_merges() {
    let fx = Fixture::new(&[("m.rs", COMMENTED_IMPORT)]);
    let (uncomment, annotate) = uncomment_and_annotate(&fx);
    accepted(accept::accept(&fx.repo, &uncomment.id).unwrap());
    let outcome = accept::accept(&fx.repo, &annotate.id).unwrap();
    let Outcome::Rejected(Invalid::Error(why)) = outcome else { panic!("expected a parse rejection, got {outcome:?}") };
    assert_eq!(why, "does not parse after composing: m.rs");
}

#[test]
fn the_parse_check_can_be_turned_off_in_zit_toml() {
    let fx = Fixture::new(&[("m.rs", COMMENTED_IMPORT), ("zit.toml", "[accept]\nparse_check = false\n")]);
    let (uncomment, annotate) = uncomment_and_annotate(&fx);
    accepted(accept::accept(&fx.repo, &uncomment.id).unwrap());
    let (_, composed) = accepted(accept::accept(&fx.repo, &annotate.id).unwrap());
    assert!(composed);
}

/// A file that was already broken on one side is left to the checks: the
/// compose did not introduce the error.
#[test]
fn a_file_one_side_could_not_parse_is_not_a_parse_rejection() {
    let fx = Fixture::new(&[("m.go", GO)]);
    let broken = fx.change("a", &[("m.go", &GO.replace("\ta()\n", "\ta(\n"))]);
    let other = fx.change("b", &[("m.go", &GO.replace("\te()\n", "\te()\n\tee()\n"))]);
    accepted(accept::accept(&fx.repo, &broken.id).unwrap());
    let policy = accept::Policy { allow_stale: true, ..Default::default() };
    accepted(accept::accept_with(&fx.repo, &other.id, &policy).unwrap());
}

/// Changing a method's signature still stales code that uses the type.
#[test]
fn a_method_signature_change_stales_users_of_the_type() {
    let src = "pub struct Shape;\n\nimpl Shape {\n    pub fn area(&self) -> u32 {\n        1\n    }\n}\n";
    let fx = Fixture::new(&[("src/shape.rs", src), ("src/use.rs", "pub fn f() {}\n")]);
    let sig = fx.change("a", &[("src/shape.rs", &src.replace("area(&self) -> u32", "area(&self, k: u32) -> u32"))]);
    let user = fx.change("b", &[("src/use.rs", "pub fn f(s: &Shape) -> u32 { s.area() }\n")]);
    accepted(accept::accept(&fx.repo, &sig.id).unwrap());
    let Outcome::Rejected(Invalid::Stale(why)) = accept::accept(&fx.repo, &user.id).unwrap() else {
        panic!("expected stale");
    };
    assert_eq!(why[0].resource.to_string(), "src/shape.rs#Shape::area");
}
