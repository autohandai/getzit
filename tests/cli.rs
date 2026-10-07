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
    // Accepting the fix retired the original too: it is no longer speculative.
    let gone = cli.run(&["discard", &caller]);
    assert_eq!(gone.code, 2, "{}", gone.stderr);
    assert!(gone.stderr.contains("not a speculative change"), "{}", gone.stderr);
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

fn fake_program(bin: &Path, name: &str, script: &str) {
    std::fs::create_dir_all(bin).unwrap();
    let path = bin.join(name);
    std::fs::write(&path, format!("#!/bin/sh\n{script}\n")).unwrap();
    std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
}

/// A PATH holding `bin`, git, and the usual system directories only.
fn narrow_path(bin: &Path) -> String {
    let path = std::env::var("PATH").unwrap();
    let git_dir = std::env::split_paths(&path).find(|p| p.join("git").is_file()).expect("git on PATH");
    let dirs = [bin.to_path_buf(), git_dir, "/usr/bin".into(), "/bin".into()];
    std::env::join_paths(dirs).unwrap().into_string().unwrap()
}

fn doctor(cli: &Cli, cwd: &Path, path: &str, json: bool) -> common::Ran {
    let mut args = vec!["doctor"];
    if json {
        args.push("--json");
    }
    let out = cli.command(cwd).env("PATH", path).args(args).output().unwrap();
    common::Ran {
        code: out.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&out.stdout).to_string(),
        stderr: String::from_utf8_lossy(&out.stderr).to_string(),
    }
}

/// `zit doctor`: one line per check, pass/warn/fail, exit 1 only when something fails.
#[test]
fn doctor_checks_the_machine_the_repository_and_the_agents() {
    let dir = tempfile::tempdir().unwrap();
    let cli = Cli::new(dir.path(), &[("zit.toml", "[[check]]\nname = \"t\"\nrun = \"test -f ok\"\n"), ("ok", "\n")]);
    let bin = dir.path().join("bin");
    fake_program(&bin, "claude", "echo '2.1.288 (Claude Code)'");
    fake_program(&bin, "gh", "echo 'gh version 2.80.0 (2025-10-01)'");
    let path = narrow_path(&bin);

    // Before `zit init`: the repository check fails, and so does the command.
    let before = doctor(&cli, &cli.root, &path, true);
    assert_eq!(before.code, 1, "{}", before.stdout);
    let report = before.json();
    assert_eq!(report["schema"], 1);
    let checks = report["checks"].as_array().unwrap().clone();
    let find = |name: &str| checks.iter().find(|c| c["name"] == name).unwrap_or_else(|| panic!("no check {name}"));
    assert_eq!(find("repository")["status"], "fail");
    assert!(find("repository")["detail"].as_str().unwrap().contains("zit init"));

    cli.run(&["init"]).ok();
    let after = doctor(&cli, &cli.root, &path, true);
    assert_eq!(after.code, 0, "{}\n{}", after.stdout, after.stderr);
    let checks = after.json()["checks"].as_array().unwrap().clone();
    let find = |name: &str| checks.iter().find(|c| c["name"] == name).unwrap_or_else(|| panic!("no check {name}"));
    let status = |name: &str| find(name)["status"].as_str().unwrap().to_string();
    let detail = |name: &str| find(name)["detail"].as_str().unwrap().to_string();
    assert_eq!(status("git"), "pass");
    assert!(detail("git").contains("git version 2."), "{}", detail("git"));
    assert_eq!(status("home"), "pass");
    assert!(detail("home").contains(cli.home.to_str().unwrap()), "{}", detail("home"));
    assert!(["pass", "warn"].contains(&status("copy-on-write").as_str()));
    assert!(!detail("copy-on-write").is_empty(), "names the file system");
    assert!(["pass", "warn"].contains(&status("free space").as_str()));
    assert!(detail("free space").contains("GB"), "{}", detail("free space"));
    assert_eq!(status("repository"), "pass");
    assert!(detail("repository").contains(&git(&cli.root, &["rev-parse", "--short=10", "HEAD"])));
    assert_eq!(status("zit.toml"), "pass");
    assert!(detail("zit.toml").contains("1 check"), "{}", detail("zit.toml"));
    assert_eq!(status("agent claude"), "pass");
    assert_eq!(detail("agent claude"), "2.1.288 (Claude Code)");
    for missing in ["agent autohand", "agent codex", "agent pi"] {
        assert_eq!(status(missing), "warn", "{missing}");
        assert!(detail(missing).contains("not on PATH"), "{}", detail(missing));
    }
    assert_eq!(status("gh"), "pass");
    assert!(detail("gh").starts_with("gh version 2.80.0"));
    assert_eq!(status("languages"), "pass");
    for lang in ["c#", "go", "java", "javascript", "python", "ruby", "rust", "typescript"] {
        assert!(detail("languages").contains(lang), "{}", detail("languages"));
    }

    let text = doctor(&cli, &cli.root, &path, false);
    assert_eq!(text.code, 0);
    assert!(text.stdout.lines().any(|l| l.starts_with("pass  git ")), "{}", text.stdout);
    assert!(text.stdout.lines().any(|l| l.starts_with("warn  agent codex")), "{}", text.stdout);
    assert!(text.stdout.lines().all(|l| l.starts_with("pass  ") || l.starts_with("warn  ")), "{}", text.stdout);

    // Outside any repository the machine checks still run; the repository check fails.
    let outside = doctor(&cli, dir.path(), &path, true);
    assert_eq!(outside.code, 1);
    let checks = outside.json()["checks"].as_array().unwrap().clone();
    let find = |name: &str| checks.iter().find(|c| c["name"] == name).unwrap();
    assert_eq!(find("git")["status"], "pass");
    assert_eq!(find("repository")["status"], "fail");
    assert!(find("repository")["detail"].as_str().unwrap().contains("not a git repository"));
}

/// A check whose program is missing, or a zit.toml that does not parse, fails the doctor:
/// every accept would be rejected.
#[test]
fn doctor_fails_on_a_missing_check_program_or_a_broken_zit_toml() {
    let dir = tempfile::tempdir().unwrap();
    let cli = Cli::new(dir.path(), &[("zit.toml", "[[check]]\nname = \"t\"\nrun = \"FOO=1 no-such-tool-zz --all\"\n")]);
    cli.run(&["init"]).ok();
    let path = narrow_path(&dir.path().join("bin"));
    let ran = doctor(&cli, &cli.root, &path, false);
    assert_eq!(ran.code, 1, "{}", ran.stdout);
    let line = ran.stdout.lines().find(|l| l.starts_with("fail  zit.toml")).unwrap_or_else(|| panic!("{}", ran.stdout));
    assert!(line.contains("no-such-tool-zz") && line.contains("check t"), "{line}");

    let broken = tempfile::tempdir().unwrap();
    let cli = Cli::new(broken.path(), &[("zit.toml", "[[check]\n")]);
    cli.run(&["init"]).ok();
    let ran = doctor(&cli, &cli.root, &path, true);
    assert_eq!(ran.code, 1);
    let checks = ran.json()["checks"].as_array().unwrap().clone();
    let toml = checks.iter().find(|c| c["name"] == "zit.toml").unwrap();
    assert_eq!(toml["status"], "fail");
    assert!(toml["detail"].as_str().unwrap().contains("zit.toml"), "{toml}");
}

/// The GitHub Actions integrator's shell logic (integrations/github/integrate.sh), run here
/// against a local bare "origin": fetch refs/zit/*, accept what can land in recorded order,
/// export to the branch, push the branch and the graph back.
#[test]
fn the_github_integrator_script_accepts_fetched_changes_and_publishes_them() {
    let dir = tempfile::tempdir().unwrap();
    // The developers' side: changes recorded and pushed with the graph.
    let dev = Cli::new(
        dir.path(),
        &[
            ("src/lib.rs", LIB),
            ("src/shop.rs", "pub fn buy() {}\n"),
            ("zit.toml", "[[check]]\nname = \"t\"\nrun = \"test -f ok\"\n"),
            ("ok", "\n"),
        ],
    );
    dev.run(&["init"]).ok();
    let origin = dir.path().join("origin.git");
    git(dir.path(), &["init", "-q", "--bare", origin.to_str().unwrap()]);
    git(&dev.root, &["remote", "add", "origin", origin.to_str().unwrap()]);
    git(&dev.root, &["push", "-q", "origin", "main"]);
    let caller = change(&dev, "codex", &[("src/shop.rs", "pub fn buy() { lib::price(3); }\n")]);
    let good = change(&dev, "pi", &[("src/lib.rs", &LIB.replace("x / 10", "x / 5"))]);
    let callee = change(&dev, "claude", &[("src/lib.rs", &LIB.replace("price(x: u32)", "price(x: u32, t: u32)"))]);
    dev.run(&["accept", &callee]).ok();
    git(&dev.root, &["push", "-q", "origin", "refs/zit/*:refs/zit/*"]);

    // The integrator's side: a fresh clone, as actions/checkout leaves one.
    let ci = dir.path().join("ci");
    git(dir.path(), &["clone", "-q", origin.to_str().unwrap(), ci.to_str().unwrap()]);
    git(&ci, &["config", "user.name", "integrator"]);
    git(&ci, &["config", "user.email", "ci@example.com"]);
    let outputs = dir.path().join("outputs.txt");
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("integrations/github/integrate.sh");
    let ran = std::process::Command::new("bash")
        .arg(&script)
        .current_dir(&ci)
        .env("ZIT", env!("CARGO_BIN_EXE_zit"))
        .env("ZIT_HOME", dir.path().join("ci-home"))
        .env("ZIT_TRUST_EVIDENCE", "false")
        .env("GITHUB_OUTPUT", &outputs)
        .output()
        .unwrap();
    let (out, err) = (String::from_utf8_lossy(&ran.stdout), String::from_utf8_lossy(&ran.stderr));
    assert!(ran.status.success(), "stdout:\n{out}\nstderr:\n{err}");
    let outputs = std::fs::read_to_string(&outputs).unwrap();
    let output = |key: &str| {
        outputs.lines().find_map(|l| l.strip_prefix(&format!("{key}="))).unwrap_or_else(|| panic!("{key} in {outputs}"))
    };
    assert_eq!(output("accepted"), good, "the compatible change landed");
    assert_eq!(output("rejected"), format!("{caller}:stale"), "the stale one was left in the graph, with why");
    let current = git(&ci, &["rev-parse", "refs/zit/current"]);
    assert_eq!(output("current"), current);
    assert_ne!(current, callee, "current moved past the pushed one");

    let at_origin =
        |rev: &str| git(dir.path(), &["--git-dir", origin.to_str().unwrap(), "rev-parse", "--verify", "--quiet", rev]);
    assert_eq!(at_origin("refs/zit/current"), current, "the graph was pushed back");
    assert_eq!(at_origin("refs/heads/main"), current, "and main exported");
    assert_eq!(at_origin(&format!("refs/zit/changes/{caller}")), caller, "the rejected change stays for its author");
    let gone = std::process::Command::new("git")
        .args([
            "--git-dir",
            origin.to_str().unwrap(),
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/zit/changes/{good}"),
        ])
        .output()
        .unwrap();
    assert!(!gone.status.success(), "the accepted change's ref is removed from origin");
    assert!(std::fs::read_to_string(ci.join("src/lib.rs")).unwrap().contains("x / 5"), "the checkout follows main");
    assert!(out.contains(&good[..10]) && out.contains("accepted"), "{out}");
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

/// With --json, an error is still a JSON document on stdout (and the message on stderr).
#[test]
fn an_error_under_json_is_json() {
    let dir = tempfile::tempdir().unwrap();
    let cli = Cli::new(dir.path(), &[("a.txt", "a\n")]);
    let ran = cli.run(&["status", "--json"]);
    assert_eq!(ran.code, 2);
    let error = ran.json()["error"].as_str().unwrap().to_string();
    assert!(error.contains("zit init"), "{error}");
    assert!(ran.stderr.contains("zit init"), "{}", ran.stderr);
}

/// A wrong `$ZIT_GIT` is named, so it is not mistaken for a missing git.
#[test]
fn a_wrong_zit_git_is_named() {
    let dir = tempfile::tempdir().unwrap();
    let cli = Cli::new(dir.path(), &[("a.txt", "a\n")]);
    let out = cli.command(&cli.root).env("ZIT_GIT", "/no/such/git").args(["status"]).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("/no/such/git") && err.contains("ZIT_GIT"), "{err}");
}

/// A repository with no commits cannot be initialised; say so instead of "unknown revision: HEAD".
#[test]
fn init_before_the_first_commit_says_what_is_missing() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("repo");
    std::fs::create_dir(&root).unwrap();
    git(&root, &["init", "-q", "-b", "main"]);
    let cli = Cli { root, home: dir.path().join("home") };
    let ran = cli.run(&["init"]);
    assert_eq!(ran.code, 2);
    assert!(ran.stderr.contains("no commits"), "{}", ran.stderr);
}

/// A git error names the command and git's reason, not the store's plumbing arguments.
#[test]
fn a_git_error_does_not_print_the_git_directory() {
    let dir = tempfile::tempdir().unwrap();
    let cli = cli(dir.path());
    let ran = cli.run(&["export", "--branch", "bad name"]);
    assert_eq!(ran.code, 2);
    assert!(ran.stderr.contains("update-ref") && ran.stderr.contains("bad name"), "{}", ran.stderr);
    assert!(!ran.stderr.contains("--git-dir"), "{}", ran.stderr);
}

/// An unwritable `$ZIT_HOME` is named in the error, not just "Permission denied".
#[test]
fn an_unwritable_zit_home_is_named() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let cli = Cli::new(dir.path(), &[("a.txt", "a\n")]);
    cli.run(&["init"]).ok();
    let sealed = dir.path().join("sealed");
    std::fs::create_dir(&sealed).unwrap();
    std::fs::set_permissions(&sealed, PermissionsExt::from_mode(0o500)).unwrap();
    let home = sealed.join("home");
    let out = cli.command(&cli.root).env("ZIT_HOME", &home).args(["materialise"]).output().unwrap();
    std::fs::set_permissions(&sealed, PermissionsExt::from_mode(0o700)).unwrap();
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains(&home.display().to_string()) && err.contains("ermission denied"), "{err}");
}

/// `$ZIT_HOME` may be relative to where zit is run (and contain spaces): git is pointed at
/// files under it by path, from the repository's git directory and from workspaces.
#[test]
fn a_relative_zit_home_is_resolved_against_the_working_directory() {
    let dir = tempfile::tempdir().unwrap();
    let cli = Cli::new(dir.path(), &[("a.txt", "a\n")]);
    cli.run(&["init"]).ok();
    let run = |args: &[&str]| {
        let out = cli.command(&cli.root).env("ZIT_HOME", "rel home").args(args).output().unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        serde_json::from_slice::<serde_json::Value>(&out.stdout).unwrap()
    };
    let ws = run(&["materialise", "--json"]);
    let path = Path::new(ws["path"].as_str().unwrap()).to_path_buf();
    assert!(path.starts_with(cli.root.canonicalize().unwrap().join("rel home")), "{}", path.display());
    write(&path, &[("b.txt", "b\n")]);
    run(&["record", "--json", "--workspace", ws["id"].as_str().unwrap()]);
    assert_eq!(run(&["status", "--json"])["changes"].as_array().unwrap().len(), 1);
}

/// Code in a language Zit cannot parse is one resource with no inferred reads; recording says so.
#[test]
fn recording_code_zit_cannot_parse_says_so() {
    let dir = tempfile::tempdir().unwrap();
    let cli = Cli::new(dir.path(), &[("Shop.kt", "class Shop\n"), ("Shop.java", "class Shop {}\n")]);
    cli.run(&["init"]).ok();
    let ws = cli.run(&["materialise", "--agent", "a", "--intent", "x"]).ok().stdout.trim().to_string();
    std::fs::write(format!("{ws}/Shop.kt"), "class Shop { fun price() = 1 }\n").unwrap();
    std::fs::write(format!("{ws}/Shop.java"), "class Shop { int price() { return 1; } }\n").unwrap();
    let out = cli.run_in(std::path::Path::new(&ws), &["record", "--dispose"]).ok();
    assert!(out.stderr.contains("Shop.kt is not parsed"), "{}", out.stderr);
    assert!(!out.stderr.contains("Shop.java is not parsed"), "{}", out.stderr);
    assert!(out.stderr.contains("wrote Shop.java#Shop::price"), "{}", out.stderr);
}
