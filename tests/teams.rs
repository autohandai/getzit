//! Fitting team rules: who commits, signed commits, linear history, pull requests.
mod common;

use common::{git, Fixture};
use std::fs;
use zit::accept::{self, Outcome, Policy};

const LIB: &str = "pub fn price(x: u32) -> u32 {\n    x\n}\n\npub fn tax(x: u32) -> u32 {\n    x / 10\n}\n";

fn two_concurrent(fx: &Fixture) -> (zit::change::Change, zit::change::Change) {
    let a = fx.change("claude", &[("src/lib.rs", &LIB.replace("x / 10", "x / 5"))]);
    let b = fx.change("codex", &[("src/shop.rs", "pub fn buy() {}\n")]);
    (a, b)
}

/// The agent is the author; the person whose git identity runs Zit is the committer.
#[test]
fn the_committer_is_the_person_running_zit() {
    let fx = Fixture::new(&[("src/lib.rs", LIB)]);
    let (a, b) = two_concurrent(&fx);
    accept::accept(&fx.repo, &a.id).unwrap();
    accept::accept(&fx.repo, &b.id).unwrap();
    let current = fx.repo.current().unwrap();
    let who = git(&fx.root(), &["log", "-1", "--format=%an|%cn <%ce>", current.as_str()]);
    assert_eq!(who, "codex|human <human@example.com>");
}

/// With `commit.gpgsign`, every commit Zit writes is signed (here with an SSH key).
#[test]
fn commits_are_signed_when_git_is_configured_to_sign() {
    let fx = Fixture::new(&[("src/lib.rs", LIB)]);
    let key = fx.dir.path().join("key");
    let made = std::process::Command::new("ssh-keygen")
        .args(["-q", "-t", "ed25519", "-N", "", "-f"])
        .arg(&key)
        .status()
        .unwrap();
    assert!(made.success());
    git(&fx.root(), &["config", "gpg.format", "ssh"]);
    git(&fx.root(), &["config", "user.signingkey", &format!("{}.pub", key.display())]);
    // git accepts any boolean spelling; "yes" must sign as "true" does.
    git(&fx.root(), &["config", "commit.gpgsign", "yes"]);
    let (a, b) = two_concurrent(&fx);
    accept::accept(&fx.repo, &a.id).unwrap();
    accept::accept(&fx.repo, &b.id).unwrap();
    for id in [a.id.as_str().to_string(), fx.repo.current().unwrap().as_str().to_string()] {
        let raw = git(&fx.root(), &["cat-file", "-p", &id]);
        assert!(raw.contains("gpgsig"), "{id} is not signed:\n{raw}");
    }
}

/// Linear history: a composed change is one commit on top of current, not a merge.
#[test]
fn linear_history_composes_without_merge_commits_and_accepts_once() {
    let fx = Fixture::new(&[("src/lib.rs", LIB)]);
    let (a, b) = two_concurrent(&fx);
    let linear = Policy { linear: true, ..Default::default() };
    accept::accept_with(&fx.repo, &a.id, &linear).unwrap();
    let Outcome::Accepted { current, composed: true, .. } = accept::accept_with(&fx.repo, &b.id, &linear).unwrap()
    else {
        panic!("expected a composed accept");
    };
    let parents = git(&fx.root(), &["log", "-1", "--format=%P", current.as_str()]);
    assert_eq!(parents.split_whitespace().count(), 1, "one parent: {parents}");
    let merges = git(&fx.root(), &["rev-list", "--merges", "--count", current.as_str()]);
    assert_eq!(merges, "0");
    assert!(git(&fx.root(), &["log", "-1", "--format=%B", current.as_str()]).contains(&format!("Zit-Change: {}", b.id)));
    let landed = zit::change::load(&fx.repo, &current).unwrap();
    assert!(
        landed.summary.as_deref().is_none_or(|s| !s.contains("Zit-Change")),
        "a trailer leaked into the account: {:?}",
        landed.summary
    );
    // The same change again is recognised as landed, not applied twice.
    assert!(matches!(accept::accept_with(&fx.repo, &b.id, &linear).unwrap(), Outcome::AlreadyAccepted));
}

/// `[accept] linear = true` in current's zit.toml makes it the repository's rule.
#[test]
fn the_repository_can_require_linear_history() {
    let fx = Fixture::new(&[("src/lib.rs", LIB), ("zit.toml", "[accept]\nlinear = true\n")]);
    let (a, b) = two_concurrent(&fx);
    accept::accept(&fx.repo, &a.id).unwrap();
    accept::accept(&fx.repo, &b.id).unwrap();
    let current = fx.repo.current().unwrap();
    assert_eq!(git(&fx.root(), &["rev-list", "--merges", "--count", current.as_str()]), "0");
}

/// `zit export --pr` publishes a branch and opens a pull request whose body carries every change's reason.
#[test]
fn export_opens_a_pull_request_with_each_changes_reason() {
    let dir = tempfile::tempdir().unwrap();
    let cli = common::Cli::new(dir.path(), &[("src/lib.rs", LIB)]);
    cli.run(&["init"]).ok();
    let remote = dir.path().join("remote.git");
    git(dir.path(), &["init", "-q", "--bare", remote.to_str().unwrap()]);
    git(&cli.root, &["remote", "add", "origin", remote.to_str().unwrap()]);
    let ws = cli.run(&["materialise", "--agent", "claude", "--intent", "Halve the tax"]).ok().stdout.trim().to_string();
    fs::write(format!("{ws}/src/lib.rs"), LIB.replace("x / 10", "x / 20")).unwrap();
    let id =
        cli.run_in(std::path::Path::new(&ws), &["record", "--summary", "Finance lowered the rate.", "--dispose"]).ok();
    let id = id.stdout.lines().next().unwrap().to_string();
    cli.run(&["accept", &id]).ok();

    let log = dir.path().join("gh.log");
    let gh = dir.path().join("gh");
    fs::write(&gh, format!("#!/bin/sh\nprintf '%s\\n' \"$@\" > {}\necho https://example.com/pr/1\n", log.display()))
        .unwrap();
    std::os::unix::fs::PermissionsExt::set_mode(&mut fs::metadata(&gh).unwrap().permissions(), 0o755);
    fs::set_permissions(&gh, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let out = cli
        .command(&cli.root)
        .env("ZIT_GH", &gh)
        .args(["export", "--branch", "zit/ready", "--pr", "--base", "main"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(String::from_utf8_lossy(&out.stdout).contains("https://example.com/pr/1"));
    let pushed = git(dir.path(), &["--git-dir", remote.to_str().unwrap(), "rev-parse", "zit/ready"]);
    assert_eq!(pushed, git(&cli.root, &["rev-parse", "refs/zit/current"]));
    let args = fs::read_to_string(&log).unwrap();
    for expected in
        ["pr", "create", "--base", "main", "--head", "zit/ready", "Halve the tax", "Finance lowered the rate."]
    {
        assert!(args.contains(expected), "missing {expected:?} in gh args:\n{args}");
    }
}
