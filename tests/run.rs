mod common;

use common::Cli;
use std::path::Path;
use std::process::Stdio;
use std::time::{Duration, Instant};

fn cli(dir: &Path) -> Cli {
    let cli = Cli::new(dir, &[("a.txt", "a\n")]);
    cli.run(&["init"]).ok();
    cli
}

fn status(cli: &Cli) -> serde_json::Value {
    cli.run(&["status", "--json"]).ok().json()
}

#[test]
fn run_records_what_the_agent_did_and_leaves_no_workspace() {
    let dir = tempfile::tempdir().unwrap();
    let cli = cli(dir.path());
    let ran = cli
        .run(&[
            "run",
            "--agent",
            "bot",
            "--intent",
            "Add b",
            "--json",
            "--",
            "sh",
            "-c",
            "echo b > b.txt; pwd > /dev/null; test -n \"$ZIT_WORKSPACE\"",
        ])
        .ok()
        .json();
    assert_eq!(ran["exit_code"], 0);
    assert_eq!(ran["change"]["intent"], "Add b");
    assert_eq!(ran["change"]["agent"], "bot");
    let s = status(&cli);
    assert_eq!(s["changes"].as_array().unwrap().len(), 1);
    assert_eq!(s["workspaces"], serde_json::json!([]));
    assert!(!cli.root.join("b.txt").exists(), "the agent never touched the user's checkout");
}

#[test]
fn work_survives_a_failing_agent() {
    let dir = tempfile::tempdir().unwrap();
    let cli = cli(dir.path());
    let ran = cli.run(&["run", "--json", "--", "sh", "-c", "echo b > b.txt; exit 7"]);
    assert_eq!(ran.code, 7);
    assert!(ran.json()["change"]["id"].is_string());
}

#[test]
fn an_agent_that_changes_nothing_records_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let cli = cli(dir.path());
    let ran = cli.run(&["run", "--json", "--", "true"]).ok().json();
    assert!(ran["change"].is_null());
    assert_eq!(status(&cli)["changes"], serde_json::json!([]));
}

#[test]
fn accept_flag_lands_the_change() {
    let dir = tempfile::tempdir().unwrap();
    let cli = cli(dir.path());
    let ran = cli.run(&["run", "--accept", "--json", "--", "sh", "-c", "echo b > b.txt"]).ok().json();
    assert_eq!(ran["outcome"]["outcome"], "accepted");
    assert_eq!(status(&cli)["current"]["id"], ran["change"]["id"]);
}

#[test]
fn keep_flag_leaves_the_workspace_for_inspection() {
    let dir = tempfile::tempdir().unwrap();
    let cli = cli(dir.path());
    cli.run(&["run", "--keep", "--", "sh", "-c", "echo b > b.txt"]).ok();
    assert_eq!(status(&cli)["workspaces"].as_array().unwrap().len(), 1);
}

/// SIGINT/SIGTERM out: the agent is stopped, its partial work is recorded.
fn interrupted_by(signal: i32) {
    let dir = tempfile::tempdir().unwrap();
    let cli = cli(dir.path());
    let marker = dir.path().join("started");
    let script = format!("echo partial > b.txt; touch {}; exec sleep 60", marker.display());
    let child = cli
        .command(&cli.root)
        .args(["run", "--agent", "bot", "--json", "--", "sh", "-c", &script])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    let deadline = Instant::now() + Duration::from_secs(20);
    while !marker.exists() {
        assert!(Instant::now() < deadline, "agent never started");
        std::thread::sleep(Duration::from_millis(20));
    }
    let sent = Instant::now();
    assert_eq!(unsafe { libc::kill(child.id() as i32, signal) }, 0);
    let out = child.wait_with_output().unwrap();

    assert!(sent.elapsed() < Duration::from_secs(10), "zit did not stop promptly");
    assert_eq!(out.status.code(), Some(128 + signal));
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["interrupted"], true);
    assert!(report["change"]["id"].is_string(), "partial work was not recorded: {report}");
    let s = status(&cli);
    assert_eq!(s["changes"].as_array().unwrap().len(), 1);
    assert_eq!(s["workspaces"], serde_json::json!([]));
}

#[test]
fn sigint_stops_the_agent_and_records_its_partial_work() {
    interrupted_by(libc::SIGINT);
}

#[test]
fn sigterm_stops_the_agent_and_records_its_partial_work() {
    interrupted_by(libc::SIGTERM);
}

#[test]
fn a_known_agent_needs_no_command() {
    // Only Zit's own directory: never the repository's git directory (hooks and config run code).
    let writable = [Path::new("/zit-home").to_path_buf()];
    assert_eq!(
        zit::run::preset("claude", "Fix it", &writable).unwrap(),
        ["claude", "-p", "Fix it", "--permission-mode", "acceptEdits", "--output-format", "json"]
    );
    // Codex's sandbox must be allowed to write where zit claim and zit status write.
    assert_eq!(
        zit::run::preset("codex", "Fix it", &writable).unwrap(),
        [
            "codex",
            "exec",
            "--json",
            "--sandbox",
            "workspace-write",
            "--output-last-message",
            "{ZIT_SUMMARY_FILE}",
            "--add-dir",
            "/zit-home",
            "Fix it"
        ]
    );
    assert_eq!(
        zit::run::preset("autohand", "Fix it", &writable).unwrap(),
        ["autohand", "-p", "Fix it", "--yes", "--output-format", "stream-json"]
    );
    assert_eq!(zit::run::preset("pi", "Fix it", &writable).unwrap(), ["pi", "-p", "Fix it"]);
    assert!(zit::run::preset("unknown", "x", &writable).is_none());
}

/// While the agent runs, its workspace is alive to `status` and protected from `clean`.
#[test]
fn a_running_agents_workspace_is_alive_and_not_cleaned() {
    let dir = tempfile::tempdir().unwrap();
    let cli = cli(dir.path());
    let marker = dir.path().join("started");
    let script = format!("touch {}; exec sleep 60", marker.display());
    let mut child = cli
        .command(&cli.root)
        .args(["run", "--json", "--", "sh", "-c", &script])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    while !marker.exists() {
        std::thread::sleep(Duration::from_millis(20));
    }
    let s = status(&cli);
    assert_eq!(s["workspaces"][0]["alive"], true, "{s}");
    let refused = cli.run(&["clean"]);
    assert_eq!(refused.code, 2);
    assert!(refused.stderr.contains("running"), "{}", refused.stderr);
    unsafe { libc::kill(child.id() as i32, libc::SIGTERM) };
    child.wait().unwrap();
    assert_eq!(status(&cli)["workspaces"], serde_json::json!([]));
}

/// An agent that ignores the signal cannot hold its work hostage.
#[test]
fn an_agent_that_ignores_the_signal_is_killed_and_its_work_recorded() {
    let dir = tempfile::tempdir().unwrap();
    let cli = cli(dir.path());
    let marker = dir.path().join("started");
    let script = format!("trap '' INT; echo partial > b.txt; touch {}; while :; do sleep 1; done", marker.display());
    let child = cli
        .command(&cli.root)
        .args(["run", "--json", "--", "sh", "-c", &script])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    while !marker.exists() {
        std::thread::sleep(Duration::from_millis(20));
    }
    let sent = Instant::now();
    assert_eq!(unsafe { libc::kill(child.id() as i32, libc::SIGINT) }, 0);
    let out = child.wait_with_output().unwrap();

    assert!(sent.elapsed() >= Duration::from_secs(9), "it was given its grace period");
    assert!(sent.elapsed() < Duration::from_secs(45), "it ends (generous: the machine may be loaded)");
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(report["change"]["id"].is_string());
    assert_eq!(out.status.code(), Some(130));
}

/// Once the agent's work is recorded, a signal stops zit itself (here: a hung check under --accept).
#[test]
fn after_recording_a_signal_stops_zit() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("check-started");
    let config = format!("[[check]]\nname = \"slow\"\nrun = \"touch {}; sleep 60\"\n", marker.display());
    let cli = Cli::new(dir.path(), &[("zit.toml", &config), ("a.txt", "a\n")]);
    cli.run(&["init"]).ok();
    let child = cli
        .command(&cli.root)
        .args(["run", "--accept", "--", "sh", "-c", "echo b > b.txt"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    while !marker.exists() {
        std::thread::sleep(Duration::from_millis(20));
    }
    let sent = Instant::now();
    assert_eq!(unsafe { libc::kill(child.id() as i32, libc::SIGINT) }, 0);
    let out = child.wait_with_output().unwrap();
    assert!(sent.elapsed() < Duration::from_secs(10), "zit kept running");
    assert_eq!(out.status.code(), Some(130));
    assert_eq!(status(&cli)["changes"].as_array().unwrap().len(), 1, "the work was recorded before the signal");
}

/// A deadline bounds a runaway agent; what it did so far is still recorded.
#[test]
fn a_timeout_stops_the_agent_and_records_its_partial_work() {
    let dir = tempfile::tempdir().unwrap();
    let cli = cli(dir.path());
    let started = Instant::now();
    let ran = cli.run(&["run", "--timeout", "1", "--json", "--", "sh", "-c", "echo partial > b.txt; exec sleep 60"]);
    assert!(started.elapsed() < Duration::from_secs(10));
    assert_eq!(ran.code, 124, "the conventional timeout exit code");
    let report = ran.json();
    assert_eq!(report["timed_out"], true);
    assert!(report["change"]["id"].is_string());
    assert_eq!(status(&cli)["workspaces"], serde_json::json!([]));
}

/// Agents on one machine share /tmp unless told otherwise; each run gets its own.
#[test]
fn each_run_has_a_private_temp_directory_that_goes_with_the_workspace() {
    let dir = tempfile::tempdir().unwrap();
    let cli = cli(dir.path());
    let seen = dir.path().join("seen");
    let script = format!("test -d \"$TMPDIR\" && touch \"$TMPDIR/scratch\" && echo \"$TMPDIR\" > {}", seen.display());
    cli.run(&["run", "--", "sh", "-c", &script]).ok();
    let tmp = std::fs::read_to_string(&seen).unwrap();
    assert!(tmp.contains("/ws/"), "inside the workspace's directory: {tmp}");
    assert!(!Path::new(tmp.trim()).exists(), "and gone with it");
    assert_eq!(status(&cli)["changes"], serde_json::json!([]), "temp files are not part of the change");
}

/// Processes the agent left behind (a dev server, a watcher) must not keep writing while its work is recorded.
#[test]
fn processes_the_agent_started_are_stopped_with_it() {
    let dir = tempfile::tempdir().unwrap();
    let cli = cli(dir.path());
    let pidfile = dir.path().join("bg.pid");
    let script = format!(
        "(trap '' HUP; while :; do date >> bg.txt; sleep 0.1; done) </dev/null >/dev/null 2>&1 & echo $! > {}; echo b > b.txt",
        pidfile.display()
    );
    cli.run(&["run", "--", "sh", "-c", &script]).ok();
    let pid: i32 = std::fs::read_to_string(&pidfile).unwrap().trim().parse().unwrap();
    std::thread::sleep(Duration::from_millis(300));
    let alive = unsafe { libc::kill(pid, 0) } == 0;
    if alive {
        unsafe { libc::kill(pid, libc::SIGKILL) };
    }
    assert!(!alive, "the agent's background process outlived it");
}

/// An agent killed in the middle of a git command leaves `index.lock` behind; its work is still recorded.
#[test]
fn a_stale_index_lock_does_not_lose_the_work() {
    let dir = tempfile::tempdir().unwrap();
    let cli = cli(dir.path());
    let ran = cli
        .run(&[
            "run",
            "--json",
            "--",
            "sh",
            "-c",
            "echo b > b.txt; touch \"$(git rev-parse --git-dir)/index.lock\"; exit 9",
        ])
        .json();
    assert!(ran["change"]["id"].is_string(), "{ran}");
}
