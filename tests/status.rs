//! `zit status` memoises each change's status; the memo is a pure function
//! of the graph and the evidence, so it must vanish the moment either moves.

mod common;

use common::Fixture;
use zit::accept::{Invalid, Status};
use zit::api::overview;
use zit::Oid;

const LIB: &str = "pub fn price(x: u32) -> u32 {\n    x\n}\n";
const GATE: &str = "[[check]]\nname = \"ok\"\nrun = \"true\"\n";

fn status_of(fx: &Fixture, id: &Oid) -> Status {
    let overview = overview(&fx.repo).unwrap();
    overview.changes.iter().find(|r| r.change.id == *id).map(|r| r.status.clone()).expect("listed")
}

fn memo_dir(fx: &Fixture) -> std::path::PathBuf {
    fx.repo.home().join("cache/status")
}

#[test]
fn a_repeated_status_reuses_its_answer_until_current_moves() {
    let fx = Fixture::new(&[("src/lib.rs", LIB), ("src/shop.rs", "pub fn buy() {}\n")]);
    let sig = fx.change("a", &[("src/lib.rs", "pub fn price(x: u32, tax: u32) -> u32 {\n    x + tax\n}\n")]);
    let caller = fx.change("b", &[("src/shop.rs", "pub fn buy() { lib::price(3); }\n")]);

    // No checks: nothing to wait for.
    assert_eq!(status_of(&fx, &caller.id), Status::Verified);
    assert!(memo_dir(&fx).join(caller.id.as_str()).is_file(), "the status was memoised");
    assert_eq!(status_of(&fx, &caller.id), Status::Verified, "the memo answers the same");

    zit::accept::accept(&fx.repo, &sig.id).unwrap();
    let stale = status_of(&fx, &caller.id);
    assert!(matches!(stale, Status::Invalid(Invalid::Stale(_))), "current moved: the old memo is not reused");
    assert_eq!(status_of(&fx, &caller.id), stale, "the reasons survive the round trip through the memo");
}

#[test]
fn a_memoised_status_follows_evidence_that_arrives_later() {
    let fx = Fixture::new(&[("zit.toml", GATE), ("src/lib.rs", LIB)]);
    let change = fx.change("a", &[("src/lib.rs", "pub fn price(x: u32) -> u32 {\n    x + 1\n}\n")]);
    assert_eq!(status_of(&fx, &change.id), Status::Speculative);
    assert_eq!(status_of(&fx, &change.id), Status::Speculative);

    zit::evidence::verify(&fx.repo, &change.id, false).unwrap();
    assert_eq!(status_of(&fx, &change.id), Status::Verified, "new evidence is seen");

    // Evidence this clone did not produce is not trusted; forgetting the ledger
    // changes the answer without touching a single ref.
    std::fs::remove_dir_all(fx.repo.git_dir().join("zit/evidence-produced")).unwrap();
    assert_eq!(status_of(&fx, &change.id), Status::Speculative, "the ledger is part of the key");
    zit::evidence::verify(&fx.repo, &change.id, false).unwrap();
    assert_eq!(status_of(&fx, &change.id), Status::Verified, "produced here again: trusted again");
}

#[test]
fn a_status_that_could_not_be_evaluated_is_not_memoised() {
    let fx = Fixture::new(&[("src/lib.rs", LIB)]);
    let broken = fx.change("a", &[("zit.toml", "[[check]\nname = oops\n")]);
    assert!(matches!(status_of(&fx, &broken.id), Status::Invalid(Invalid::Error(_))));
    assert!(!memo_dir(&fx).join(broken.id.as_str()).exists(), "an error may be transient; ask again");
}
