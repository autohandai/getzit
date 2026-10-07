//! Adversarial probes of the interfaces: error paths, malformed input,
//! partial failures.

mod common;

use common::{write, Cli};
use std::path::Path;
use std::process::Stdio;

fn cli(dir: &Path) -> Cli {
    let cli = Cli::new(dir, &[("src/lib.rs", "pub fn price(x: u32) -> u32 { x }\n"), ("a.txt", "a\n")]);
    cli.run(&["init"]).ok();
    cli
}

fn change(cli: &Cli, agent: &str, files: &[(&str, &str)]) -> String {
    let ws = cli.run(&["materialise", "--agent", agent, "--intent", "edit", "--json"]).ok().json();
    let path = Path::new(ws["path"].as_str().unwrap()).to_path_buf();
    write(&path, files);
    let rec = cli.run_in(&path, &["record", "--dispose", "--json"]).ok().json();
    rec["change"]["id"].as_str().unwrap().to_string()
}

/// `zit status | head` must not end in a panic about a broken pipe.
#[test]
fn a_closed_stdout_does_not_panic() {
    let dir = tempfile::tempdir().unwrap();
    let cli = cli(dir.path());
    let mut child = cli
        .command(&cli.root)
        .args(["status", "--json"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    drop(child.stdout.take());
    let out = child.wait_with_output().unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(!err.contains("panicked"), "{err}");
    assert_ne!(out.status.code(), Some(101), "{err}");
}

/// With `--json`, a failure is still JSON on stdout, with one shape, and a non-zero exit.
#[test]
fn errors_are_json_when_json_was_asked_for() {
    let dir = tempfile::tempdir().unwrap();
    let cli = Cli::new(dir.path(), &[("a.txt", "a\n")]);
    let ran = cli.run(&["status", "--json"]);
    assert_eq!(ran.code, 2);
    assert!(ran.json()["error"].as_str().unwrap().contains("zit init"), "{}", ran.stdout);
    cli.run(&["init"]).ok();
    for args in
        [&["show", "nope", "--json"][..], &["accept", "nope", "--json"], &["materialise", "--from", "nope", "--json"]]
    {
        let ran = cli.run(args);
        assert_eq!(ran.code, 2, "{args:?}");
        assert_eq!(ran.json()["error"], "unknown revision: nope", "{args:?}: {}", ran.stdout);
    }
    let ran = cli.run(&["record", "--workspace", "nope", "--json"]);
    assert_eq!(ran.code, 2);
    assert!(ran.json()["error"].as_str().unwrap().contains("unknown workspace"), "{}", ran.stdout);
}

/// `zit discard A nope` must not discard A: all names are checked before anything is removed.
#[test]
fn discard_is_all_or_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let cli = cli(dir.path());
    let a = change(&cli, "a", &[("a.txt", "1\n")]);
    let ran = cli.run(&["discard", &a, "nope"]);
    assert_eq!(ran.code, 2);
    let status = cli.run(&["status", "--json"]).ok().json();
    assert_eq!(status["changes"][0]["id"], a.as_str(), "{status}");
}

/// `zit dispose` with nothing to dispose is a usage error, not a silent success.
#[test]
fn dispose_needs_ids_or_all() {
    let dir = tempfile::tempdir().unwrap();
    let cli = cli(dir.path());
    let ran = cli.run(&["dispose"]);
    assert_ne!(ran.code, 0);
    assert!(ran.stderr.contains("<IDS>") && ran.stderr.contains("Usage"), "{}", ran.stderr);
}

/// Only speculative changes can be discarded; current, accepted history and an already
/// discarded change are errors, not silent successes.
#[test]
fn discarding_what_is_not_speculative_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let cli = cli(dir.path());
    let ran = cli.run(&["discard", "current"]);
    assert_eq!(ran.code, 2, "{}", ran.stdout);
    assert!(ran.stderr.contains("not a speculative change"), "{}", ran.stderr);
    let a = change(&cli, "a", &[("a.txt", "1\n")]);
    cli.run(&["discard", &a]).ok();
    let ran = cli.run(&["discard", &a]);
    assert_eq!(ran.code, 2, "{}", ran.stdout);
}
