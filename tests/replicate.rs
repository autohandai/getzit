mod common;

use common::{git, Fixture};
use zit::{accept, change, evidence, Repo};

const SPEC: &str = "refs/zit/*:refs/zit/*";

/// The graph is git refs: fetch them and another machine has the same
/// changes and evidence, can materialise them, and can push its decision back.
#[test]
fn the_graph_replicates_with_plain_git() {
    let a = Fixture::new(&[("zit.toml", "[[check]]\nname = \"t\"\nrun = \"test -f a.txt\"\n"), ("a.txt", "a\n")]);
    let c = a.change("claude", &[("a.txt", "edited on machine A\n")]);
    evidence::verify(&a.repo, &c.id, false).unwrap();

    let b_root = a.dir.path().join("machine-b");
    git(a.dir.path(), &["clone", "-q", a.root().to_str().unwrap(), b_root.to_str().unwrap()]);
    git(&b_root, &["fetch", "-q", "origin", SPEC]);
    let b = Repo::open(&b_root, &a.dir.path().join("home-b")).unwrap();

    assert_eq!(change::speculative(&b).unwrap(), vec![c.clone()]);
    assert_eq!(b.current().unwrap(), a.repo.current().unwrap());
    // Evidence travels with the graph, but a result this machine did not produce
    // is not trusted: anyone who can push refs could have written it.
    assert!(evidence::lookup(&b, &c.state).unwrap()[0].1.is_none(), "fetched evidence is not trusted");
    assert_eq!(accept::status(&b, &c.id).unwrap(), accept::Status::Speculative);
    git(&b_root, &["config", "zit.trustFetchedEvidence", "true"]);
    assert!(evidence::lookup(&b, &c.state).unwrap()[0].1.as_ref().unwrap().passed, "trusted once configured");
    assert_eq!(accept::status(&b, &c.id).unwrap(), accept::Status::Verified);
    git(&b_root, &["config", "--unset", "zit.trustFetchedEvidence"]);

    let ws = zit::workspace::materialise(
        &b,
        &zit::workspace::NewWorkspace { from: Some(&c.id), intent: "", agent: "codex", session: None },
    )
    .unwrap();
    assert_eq!(std::fs::read_to_string(ws.path().join("a.txt")).unwrap(), "edited on machine A\n");

    assert!(matches!(accept::accept(&b, &c.id).unwrap(), accept::Outcome::Accepted { .. }));
    git(&b_root, &["push", "-q", "--prune", "origin", SPEC]);
    assert_eq!(a.repo.current().unwrap(), c.id);
    assert!(change::speculative(&a.repo).unwrap().is_empty());
}

/// A forged passing result pushed by someone else is run again, not believed.
#[test]
fn a_planted_passing_result_does_not_get_a_failing_change_accepted() {
    let a = Fixture::new(&[("zit.toml", "[[check]]\nname = \"t\"\nrun = \"test ! -f bad\"\n"), ("a.txt", "a\n")]);
    let bad = a.change("claude", &[("bad", "x\n")]);
    // Machine A runs the check (it fails), then someone rewrites that evidence to "passed".
    assert!(!evidence::verify(&a.repo, &bad.id, false).unwrap()[0].evidence.passed);
    let refname = git(&a.root(), &["for-each-ref", "--format=%(refname)", "refs/zit/evidence"]);
    let forged = git(&a.root(), &["cat-file", "-p", refname.lines().next().unwrap()])
        .replace("\"passed\":false", "\"passed\":true");
    let blob = std::process::Command::new("git")
        .current_dir(a.root())
        .args(["hash-object", "-w", "--stdin"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            child.stdin.take().unwrap().write_all(forged.as_bytes())?;
            child.wait_with_output()
        })
        .unwrap();
    let blob = String::from_utf8(blob.stdout).unwrap();
    git(&a.root(), &["update-ref", refname.lines().next().unwrap(), blob.trim()]);

    let b_root = a.dir.path().join("machine-b");
    git(a.dir.path(), &["clone", "-q", a.root().to_str().unwrap(), b_root.to_str().unwrap()]);
    git(&b_root, &["fetch", "-q", "origin", SPEC]);
    let b = Repo::open(&b_root, &a.dir.path().join("home-b")).unwrap();
    assert!(matches!(accept::accept(&b, &bad.id).unwrap(), accept::Outcome::Rejected(accept::Invalid::Failed(_))));
}
