mod common;

use common::Fixture;
use std::fs;
use zit::accept;
use zit::change::{self, Record};

/// Installs "dependencies" (a stamped directory) the way `npm ci` would.
const PREPARE: &str = "[prepare]\nrun = \"mkdir -p deps && od -An -N8 -tx8 /dev/urandom > deps/stamp && cp lock.txt deps/\"\ninputs = [\"lock.txt\"]\n";

fn fixture() -> Fixture {
    Fixture::new(&[("zit.toml", PREPARE), (".gitignore", "deps/\n"), ("lock.txt", "v1\n"), ("app.txt", "a\n")])
}

fn stamp(ws: &zit::workspace::Workspace) -> String {
    fs::read_to_string(ws.path().join("deps/stamp")).unwrap()
}

#[test]
fn workspaces_start_with_dependencies_installed_once() {
    let fx = fixture();
    let (a, b) = (fx.workspace("claude"), fx.workspace("codex"));
    assert_eq!(fs::read_to_string(a.path().join("deps/lock.txt")).unwrap(), "v1\n");
    assert_eq!(stamp(&a), stamp(&b), "the second workspace cloned the first's install");
    assert!(change::record(&fx.repo, &a.id, &Record::default()).unwrap().is_none(), "installed files are not a change");
}

#[test]
fn dependencies_are_reinstalled_only_when_their_inputs_change() {
    let fx = fixture();
    let first = stamp(&fx.workspace("a"));
    let docs = fx.change("a", &[("app.txt", "b\n")]);
    accept::accept(&fx.repo, &docs.id).unwrap();
    assert_eq!(stamp(&fx.workspace("b")), first, "an unrelated change keeps the install");

    let bump = fx.change("a", &[("lock.txt", "v2\n")]);
    accept::accept(&fx.repo, &bump.id).unwrap();
    let ws = fx.workspace("c");
    assert_ne!(stamp(&ws), first, "a lockfile change reinstalls");
    assert_eq!(fs::read_to_string(ws.path().join("deps/lock.txt")).unwrap(), "v2\n");
}

#[test]
fn a_prepare_step_must_only_create_ignored_files() {
    let fx =
        Fixture::new(&[("zit.toml", "[prepare]\nrun = \"echo generated > not-ignored.txt\"\n"), ("app.txt", "a\n")]);
    let new = zit::workspace::NewWorkspace { from: None, intent: "", agent: "claude", session: None };
    let err = zit::workspace::materialise(&fx.repo, &new).unwrap_err().to_string();
    assert!(err.contains("not-ignored.txt") && err.contains("ignore"), "{err}");
    assert!(zit::workspace::list(&fx.repo).unwrap().is_empty());
}

#[test]
fn a_failing_prepare_step_is_reported() {
    let fx = Fixture::new(&[("zit.toml", "[prepare]\nrun = \"echo broken lockfile; exit 3\"\n"), ("app.txt", "a\n")]);
    let new = zit::workspace::NewWorkspace { from: None, intent: "", agent: "claude", session: None };
    let err = zit::workspace::materialise(&fx.repo, &new).unwrap_err().to_string();
    assert!(err.contains("prepare") && err.contains("broken lockfile"), "{err}");
}

/// A change to the lockfile is checked against the dependencies it declares, not the last ones installed.
#[test]
fn a_verification_view_reinstalls_when_the_dependencies_change() {
    let config = "ignore = [\"deps.txt\"]\n\n[prepare]\nrun = \"cp lock deps.txt\"\ninputs = [\"lock\"]\n\n\
                  [[check]]\nname = \"deps\"\nrun = \"cmp lock deps.txt\"\n";
    let fx = common::Fixture::new(&[("zit.toml", config), ("lock", "v1\n")]);
    let current = fx.repo.current().unwrap();
    assert!(zit::evidence::verify(&fx.repo, &current, false).unwrap()[0].evidence.passed);
    let bumped = fx.change("agent", &[("lock", "v2\n")]);
    let verdict = &zit::evidence::verify(&fx.repo, &bumped.id, false).unwrap()[0];
    assert!(verdict.evidence.passed, "checked against stale dependencies: {}", verdict.evidence.output);
}
