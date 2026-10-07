mod common;

use common::{git, write, Cli, Fixture};
use std::path::Path;
use zit::change::{self, Record, Usage};
use zit::workspace::{self, NewWorkspace};
use zit::{accept, Oid};

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

/// A recorded change with an account and a reported cost, as `zit run` leaves one.
fn costed(fx: &Fixture, agent: &str, intent: &str, summary: &str, usage: Option<Usage>, file: (&str, &str)) -> Oid {
    let new = NewWorkspace { from: None, intent, agent, session: None };
    let ws = workspace::materialise(&fx.repo, &new).unwrap();
    write(ws.path(), &[file]);
    let opts = Record { summary: Some(summary.to_string()), usage, ..Default::default() };
    let change = change::record(&fx.repo, &ws.id, &opts).unwrap().unwrap();
    workspace::dispose(&fx.repo, &ws.id).unwrap();
    change.id
}

/// `zit log`: the accepted history, newest first, with what each change cost and the totals.
#[test]
fn log_lists_accepted_changes_with_their_cost_and_totals() {
    let fx = Fixture::new(&[("a.txt", "a\n")]);
    let cost = |i, o, c| Some(Usage { input_tokens: i, output_tokens: o, cost_usd: c });
    let first = costed(
        &fx,
        "claude",
        "Raise the price",
        "Finance asked.\n\nMore detail.",
        cost(1000, 50, Some(0.25)),
        ("a.txt", "1\n"),
    );
    accept::accept(&fx.repo, &first).unwrap();
    let second = costed(&fx, "codex", "Add tax", "Tax is law.", cost(300, 10, None), ("b.txt", "2\n"));
    accept::accept(&fx.repo, &second).unwrap();
    let unaccepted = costed(&fx, "pi", "Not yet", "Pending.", None, ("c.txt", "3\n"));
    let cli = Cli { root: fx.root(), home: fx.dir.path().join("home") };

    let log = cli.run(&["log", "--json"]).ok().json();
    assert_eq!(log["schema"], 1);
    let changes = log["changes"].as_array().unwrap();
    assert_eq!(changes.len(), 3, "second, first, genesis: {changes:?}");
    assert_eq!(changes[0]["id"], second.as_str());
    assert_eq!(changes[0]["usage"]["input_tokens"], 300);
    assert_eq!(changes[1]["id"], first.as_str());
    assert_eq!(changes[1]["usage"]["cost_usd"], 0.25);
    assert!(changes[2]["usage"].is_null(), "a plain git commit reports no cost");
    assert!(!changes.iter().any(|c| c["id"] == unaccepted.as_str()), "only accepted history is listed");
    assert_eq!(
        log["totals"],
        serde_json::json!({"changes": 3, "input_tokens": 1300, "output_tokens": 60, "cost_usd": 0.25})
    );

    let text = cli.run(&["log"]).ok().stdout;
    assert!(text.contains(second.short()) && text.contains("codex") && text.contains("Add tax"), "{text}");
    assert!(text.contains("Finance asked.") && !text.contains("More detail."), "first line of the account: {text}");
    assert!(text.contains("1000 in, 50 out, $0.2500") && text.contains("300 in, 10 out"), "{text}");
    assert!(text.contains("3 changes, 1300 tokens in, 60 out, $0.2500"), "{text}");

    let limited = cli.run(&["log", "-n", "1", "--json"]).ok().json();
    assert_eq!(limited["changes"].as_array().unwrap().len(), 1);
    assert_eq!(limited["totals"]["input_tokens"], 300, "totals cover what is listed");
}

/// `zit diff`: what a change did, against its base by default or against current.
#[test]
fn diff_shows_a_change_against_its_base_or_against_current() {
    let dir = tempfile::tempdir().unwrap();
    let cli = cli(dir.path());
    let landed = change(&cli, "claude", &[("src/shop.rs", "pub fn buy() { 1; }\n")]);
    let b = change(&cli, "codex", &[("src/lib.rs", &LIB.replace("x / 10", "x / 5"))]);
    cli.run(&["accept", &landed]).ok();

    let base = cli.run(&["diff", &b]).ok().stdout;
    assert!(base.contains("+    x / 5") && base.contains("-    x / 10"), "{base}");
    assert!(!base.contains("shop.rs"), "against its own base, shop.rs is untouched: {base}");

    let current = cli.run(&["diff", &b, "--against", "current"]).ok().stdout;
    assert!(current.contains("-pub fn buy() { 1; }"), "current has shop.rs edits the change lacks: {current}");
    assert!(current.contains("+    x / 5"), "{current}");

    let stat = cli.run(&["diff", &b, "--stat"]).ok().stdout;
    assert!(stat.contains("src/lib.rs") && stat.contains("1 file changed"), "{stat}");
    assert!(!stat.contains("+    x / 5"), "a summary, not the patch: {stat}");

    let json = cli.run(&["diff", &b, "--json"]).ok().json();
    assert_eq!(json["schema"], 1);
    assert_eq!(json["change"], b.as_str());
    assert_eq!(json["against"], git(&cli.root, &["rev-parse", &format!("{b}^")]));
    assert!(json["diff"].as_str().unwrap().contains("+    x / 5"));

    assert_eq!(cli.run(&["diff", "nope"]).code, 2);
}

/// `zit completions SHELL` prints a completion script for bash, zsh or fish; no repository needed.
#[test]
fn completions_are_generated_for_each_shell() {
    let dir = tempfile::tempdir().unwrap();
    let cli = Cli::new(dir.path(), &[("a.txt", "a\n")]);
    for (shell, marker) in [("bash", "_zit()"), ("zsh", "#compdef zit"), ("fish", "complete -c zit")] {
        let out = cli.run_in(dir.path(), &["completions", shell]).ok().stdout;
        assert!(out.contains(marker), "{shell}: {}", &out[..out.len().min(200)]);
        assert!(out.contains("materialise") && out.contains("log"), "{shell} knows every subcommand");
    }
    assert_eq!(cli.run_in(dir.path(), &["completions", "powershell"]).code, 2, "only shells it is tested for");
}

/// Integrations detect a format change by the `schema` field every `--json` report carries.
#[test]
fn json_reports_carry_a_schema_version() {
    let dir = tempfile::tempdir().unwrap();
    let cli = cli(dir.path());
    let id = change(&cli, "claude", &[("src/shop.rs", "pub fn buy() { 1; }\n")]);
    assert_eq!(cli.run(&["status", "--json"]).ok().json()["schema"], 1);
    let shown = cli.run(&["show", &id, "--json"]).ok().json();
    assert_eq!(shown["schema"], 1);
    assert_eq!(shown["id"], id.as_str(), "the rest of the report is unchanged");
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

/// Zit needs git 2.38 (`merge-tree --write-tree`); an older git is named, not failed on later.
#[test]
fn an_old_git_is_refused_with_the_version_needed() {
    let dir = tempfile::tempdir().unwrap();
    let cli = Cli::new(dir.path(), &[("a.txt", "a\n")]);
    let fake = dir.path().join("old-git");
    std::fs::write(&fake, "#!/bin/sh\necho 'git version 2.30.1'\n").unwrap();
    std::fs::set_permissions(&fake, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let out = cli.command(&cli.root).env("ZIT_GIT", &fake).args(["status"]).output().unwrap();
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("git 2.38 or newer") && err.contains("2.30.1"), "{err}");
}

/// Code in a language Zit cannot parse is one resource with no inferred reads; recording says so.
#[test]
fn recording_code_zit_cannot_parse_says_so() {
    let dir = tempfile::tempdir().unwrap();
    let cli = Cli::new(dir.path(), &[("Shop.java", "class Shop {}\n"), ("a.rs", "fn a() {}\n")]);
    cli.run(&["init"]).ok();
    let ws = cli.run(&["materialise", "--agent", "a", "--intent", "x"]).ok().stdout.trim().to_string();
    std::fs::write(format!("{ws}/Shop.java"), "class Shop { int price() { return 1; } }\n").unwrap();
    std::fs::write(format!("{ws}/a.rs"), "fn a() { 1; }\n").unwrap();
    let out = cli.run_in(std::path::Path::new(&ws), &["record", "--dispose"]).ok();
    assert!(out.stderr.contains("Shop.java is not parsed"), "{}", out.stderr);
    assert!(!out.stderr.contains("a.rs is not parsed"), "{}", out.stderr);
}
