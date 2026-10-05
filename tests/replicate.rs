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
    let found = evidence::lookup(&b, &c.state).unwrap();
    assert!(found[0].1.as_ref().unwrap().passed, "evidence travelled with the graph");
    assert_eq!(accept::status(&b, &c.id).unwrap(), accept::Status::Verified);

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
