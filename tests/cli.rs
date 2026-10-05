mod common;

use common::{git, write, Cli};
use std::path::Path;

const LIB: &str = "pub fn price(x: u32) -> u32 {\n    x\n}\n\npub fn tax(x: u32) -> u32 {\n    x / 10\n}\n";

fn cli(dir: &Path) -> Cli {
    let cli = Cli::new(dir, &[("src/lib.rs", LIB), ("src/shop.rs", "pub fn buy() {}\n")]);
    cli.run(&["init"]).ok();
    cli
}

/// materialise -> edit -> record, as an agent would from the shell.
fn change(cli: &Cli, agent: &str, files: &[(&str, &str)]) -> String {
    let ws = cli.run(&["materialise", "--agent", agent, "--intent", "edit", "--json"]).ok().json();
    let path = Path::new(ws["path"].as_str().unwrap()).to_path_buf();
    write(&path, files);
    let rec = cli.run_in(&path, &["record", "--dispose", "--json"]).ok().json();
    rec["change"]["id"].as_str().unwrap().to_string()
}

#[test]
fn commands_fail_clearly_before_init() {
    let dir = tempfile::tempdir().unwrap();
    let cli = Cli::new(dir.path(), &[("a.txt", "a\n")]);
    let ran = cli.run(&["status"]);
    assert_eq!(ran.code, 2);
    assert!(ran.stderr.contains("zit init"), "{}", ran.stderr);
}

#[test]
fn the_whole_loop_from_the_shell() {
    let dir = tempfile::tempdir().unwrap();
    let cli = cli(dir.path());

    let ws = cli.run(&["materialise", "--agent", "claude", "--intent", "Change tax", "--json"]).ok().json();
    let path = Path::new(ws["path"].as_str().unwrap()).to_path_buf();
    write(&path, &[("src/lib.rs", &LIB.replace("x / 10", "x / 5"))]);

    // Inside a workspace, record needs no arguments.
    cli.run_in(&path, &["read", "src/shop.rs"]).ok();
    let rec = cli.run_in(&path, &["record", "--json"]).ok().json();
    let id = rec["change"]["id"].as_str().unwrap().to_string();
    assert_eq!(rec["change"]["intent"], "Change tax");
    assert_eq!(rec["writes"], serde_json::json!(["src/lib.rs#tax"]));
    assert_eq!(rec["change"]["reads"], serde_json::json!(["src/shop.rs"]));

    let status = cli.run(&["status", "--json"]).ok().json();
    assert_eq!(status["changes"][0]["id"], id.as_str());
    assert_eq!(status["changes"][0]["status"], "verified");
    assert_eq!(status["workspaces"][0]["id"], ws["id"]);
    assert_eq!(status["workspaces"][0]["dirty"], false);

    let accepted = cli.run(&["accept", &id[..10], "--json"]).ok().json();
    assert_eq!(accepted["outcome"], "accepted");
    cli.run(&["dispose", ws["id"].as_str().unwrap()]).ok();
    let status = cli.run(&["status", "--json"]).ok().json();
    assert_eq!(status["current"]["id"], id.as_str());
    assert_eq!(status["changes"], serde_json::json!([]));
    assert_eq!(status["workspaces"], serde_json::json!([]));

    cli.run(&["export", "--branch", "main"]).ok();
    assert_eq!(git(&cli.root, &["rev-parse", "main"]), id);
    assert!(std::fs::read_to_string(cli.root.join("src/lib.rs")).unwrap().contains("x / 5"));
}

#[test]
fn a_rejected_accept_exits_1_and_says_why() {
    let dir = tempfile::tempdir().unwrap();
    let cli = cli(dir.path());
    let callee = change(&cli, "claude", &[("src/lib.rs", &LIB.replace("price(x: u32)", "price(x: u32, t: u32)"))]);
    let caller = change(&cli, "codex", &[("src/shop.rs", "pub fn buy() { lib::price(3); }\n")]);
    cli.run(&["accept", &callee]).ok();

    let ran = cli.run(&["accept", &caller]);
    assert_eq!(ran.code, 1);
    assert!(ran.stdout.contains("stale") && ran.stdout.contains("src/lib.rs#price"), "{}", ran.stdout);
    assert!(ran.stdout.contains(&callee[..10]), "{}", ran.stdout);

    let status = cli.run(&["status"]).ok();
    assert!(status.stdout.contains("invalid: stale"), "{}", status.stdout);

    let shown = cli.run(&["show", &caller, "--json"]).ok().json();
    assert_eq!(shown["status"], "invalid");
    assert_eq!(shown["reason"], "stale");
    assert_eq!(shown["writes"], serde_json::json!(["src/shop.rs#buy"]));

    // retry -> fix -> record -> accept
    let ws = cli.run(&["retry", &caller, "--json"]).ok().json();
    let path = Path::new(ws["path"].as_str().unwrap()).to_path_buf();
    write(&path, &[("src/shop.rs", "pub fn buy() { lib::price(3, 1); }\n")]);
    let fixed = cli.run_in(&path, &["record", "--dispose", "--json"]).ok().json();
    cli.run(&["accept", fixed["change"]["id"].as_str().unwrap()]).ok();
    cli.run(&["discard", &caller]).ok();
    assert_eq!(cli.run(&["status", "--json"]).ok().json()["changes"], serde_json::json!([]));
}

#[test]
fn check_reports_evidence_and_exits_1_on_failure() {
    let dir = tempfile::tempdir().unwrap();
    let cli = Cli::new(dir.path(), &[("zit.toml", "[[check]]\nname = \"t\"\nrun = \"test -f ok\"\n"), ("ok", "\n")]);
    cli.run(&["init"]).ok();
    let good = change(&cli, "a", &[("x.txt", "x\n")]);
    let checked = cli.run(&["check", &good, "--json"]).ok().json();
    assert_eq!(checked[0]["evidence"]["passed"], true);
    assert_eq!(checked[0]["cached"], false);

    let ws = cli.run(&["materialise", "--json"]).ok().json();
    let path = Path::new(ws["path"].as_str().unwrap()).to_path_buf();
    std::fs::remove_file(path.join("ok")).unwrap();
    let bad = cli.run_in(&path, &["record", "--json"]).ok().json();
    assert_eq!(cli.run(&["check", bad["change"]["id"].as_str().unwrap()]).code, 1);
}

#[test]
fn sync_imports_a_branch_and_publishes_current_to_it() {
    let dir = tempfile::tempdir().unwrap();
    let cli = cli(dir.path());
    let c = change(&cli, "claude", &[("src/shop.rs", "pub fn buy() { 1; }\n")]);
    cli.run(&["accept", &c]).ok();
    write(&cli.root, &[("notes.txt", "by hand\n")]);
    git(&cli.root, &["add", "-A"]);
    git(&cli.root, &["commit", "-qm", "Human edit"]);

    cli.run(&["sync", "--branch", "main"]).ok();
    assert_eq!(git(&cli.root, &["rev-parse", "main"]), git(&cli.root, &["rev-parse", "refs/zit/current"]));
    assert!(cli.root.join("notes.txt").exists());
    assert_eq!(std::fs::read_to_string(cli.root.join("src/shop.rs")).unwrap(), "pub fn buy() { 1; }\n");
}

#[test]
fn dispose_all_removes_every_workspace() {
    let dir = tempfile::tempdir().unwrap();
    let cli = cli(dir.path());
    cli.run(&["materialise"]).ok();
    cli.run(&["materialise"]).ok();
    cli.run(&["dispose", "--all"]).ok();
    assert_eq!(cli.run(&["status", "--json"]).ok().json()["workspaces"], serde_json::json!([]));
}

#[test]
fn ui_without_a_terminal_is_an_error_not_a_crash() {
    let dir = tempfile::tempdir().unwrap();
    let cli = cli(dir.path());
    let ran = cli.run(&["ui"]);
    assert_eq!(ran.code, 2);
    assert!(ran.stderr.contains("terminal"), "{}", ran.stderr);
}

#[test]
fn claim_exits_1_and_names_the_holder_when_refused() {
    let dir = tempfile::tempdir().unwrap();
    let cli = cli(dir.path());
    let a = cli.run(&["materialise", "--agent", "claude", "--json"]).ok().json();
    let b = cli.run(&["materialise", "--agent", "codex", "--json"]).ok().json();
    let in_a = Path::new(a["path"].as_str().unwrap()).to_path_buf();
    cli.run_in(&in_a, &["claim", "src/lib.rs#price", "src/shop.rs"]).ok();

    let refused = cli.run(&["claim", "--workspace", b["id"].as_str().unwrap(), "src/lib.rs"]);
    assert_eq!(refused.code, 1);
    assert!(refused.stdout.contains("src/lib.rs#price") && refused.stdout.contains("claude"), "{}", refused.stdout);

    let status = cli.run(&["status"]).ok();
    assert!(status.stdout.contains("claims src/lib.rs#price, src/shop.rs"), "{}", status.stdout);
}

/// Installed next to `zit`, `git-zit` makes every command available as `git zit …`.
#[test]
fn zit_is_a_git_extension() {
    let dir = tempfile::tempdir().unwrap();
    let cli = Cli::new(dir.path(), &[("a.txt", "a\n")]);
    let bin_dir = Path::new(env!("CARGO_BIN_EXE_git-zit")).parent().unwrap().to_path_buf();
    let path = format!("{}:{}", bin_dir.display(), std::env::var("PATH").unwrap());
    let git_zit = |args: &[&str]| {
        let out = std::process::Command::new("git")
            .current_dir(&cli.root)
            .env("PATH", &path)
            .env("ZIT_HOME", &cli.home)
            .arg("zit")
            .args(args)
            .output()
            .unwrap();
        (
            out.status.code(),
            String::from_utf8_lossy(&out.stdout).to_string(),
            String::from_utf8_lossy(&out.stderr).to_string(),
        )
    };
    let (code, out, err) = git_zit(&["init"]);
    assert_eq!(code, Some(0), "{out}{err}");
    assert!(out.starts_with("current is "), "{out}");
    let (code, out, _) = git_zit(&["status", "--json"]);
    assert_eq!(code, Some(0));
    assert!(out.contains("\"current\""));
    // git turns `git zit --help` into a man-page lookup; `-h` and `help` reach zit.
    for help in [&["-h"][..], &["help"][..]] {
        let (_, out, _) = git_zit(help);
        assert!(out.contains("Usage: git zit"), "help names the command the user typed: {out}");
    }
}
