//! Adversarial probes of the interfaces: error paths, malformed input,
//! partial failures.

mod common;

use common::{write, Cli};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Stdio};

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
