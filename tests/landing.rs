//! What an accept leaves behind whether or not the change lands: the
//! evidence its checks produced is kept, so nothing is run twice.

mod common;

use common::Fixture;
use zit::accept::{self, Invalid, Outcome};
use zit::evidence;

const GATE: &str = "[[check]]\nname = \"ok\"\nrun = \"true\"\n";

fn passed(fx: &Fixture, state: &zit::Oid) -> bool {
    let found = evidence::lookup(&fx.repo, state).unwrap();
    !found.is_empty() && found.iter().all(|(_, e)| e.as_ref().is_some_and(|e| e.passed))
}

#[test]
fn evidence_produced_by_an_accept_that_could_not_move_current_is_kept() {
    let fx = Fixture::new(&[("zit.toml", GATE), ("a.txt", "a\n")]);
    let c = fx.change("a", &[("a.txt", "b\n")]);
    std::fs::write(fx.repo.git_dir().join("refs/zit/current.lock"), "").unwrap();
    assert!(accept::accept(&fx.repo, &c.id).is_err(), "current is locked");
    assert!(passed(&fx, &c.state), "the check ran; its evidence stays although current did not move");
}

#[test]
fn evidence_of_a_rejected_change_is_kept() {
    let gate = "[[check]]\nname = \"never\"\nrun = \"false\"\n";
    let fx = Fixture::new(&[("zit.toml", gate), ("a.txt", "a\n")]);
    let c = fx.change("a", &[("a.txt", "b\n")]);
    assert!(matches!(accept::accept(&fx.repo, &c.id).unwrap(), Outcome::Rejected(Invalid::Failed(_))));
    let found = evidence::lookup(&fx.repo, &c.state).unwrap();
    assert!(found.iter().all(|(_, e)| e.as_ref().is_some_and(|e| !e.passed)), "the failure is evidence");
}

#[test]
fn an_accept_that_lands_stores_its_evidence_with_the_move() {
    let fx = Fixture::new(&[("zit.toml", GATE), ("a.txt", "a\n")]);
    let c = fx.change("a", &[("a.txt", "b\n")]);
    assert!(matches!(accept::accept(&fx.repo, &c.id).unwrap(), Outcome::Accepted { .. }));
    assert_eq!(fx.repo.current().unwrap(), c.id);
    assert!(passed(&fx, &c.state));
}
