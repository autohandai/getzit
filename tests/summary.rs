mod common;

use common::{git, write, Cli, Fixture};
use std::path::Path;
use zit::change::{self, Record};

/// A change keeps the intent it was given and the account its author gave
/// afterwards: what was done and why.
#[test]
fn a_change_stores_what_was_done_and_why() {
    let fx = Fixture::new(&[("a.txt", "a\n")]);
    let ws = fx.workspace("claude");
    write(ws.path(), &[("a.txt", "b\n")]);
    let summary = "Changed a.txt to b.\n\nWhy: the spec says b, and the test expects it.";
    let opts = Record { summary: Some(summary.into()), ..Default::default() };
    let c = change::record(&fx.repo, &ws.id, &opts).unwrap().unwrap();
    assert_eq!(c.summary.as_deref(), Some(summary));
    assert_eq!(c.intent, "work");
    let loaded = change::load(&fx.repo, &c.id).unwrap();
    assert_eq!((loaded.intent.as_str(), loaded.summary.as_deref()), ("work", Some(summary)));
}

#[test]
fn a_multi_paragraph_intent_stays_the_intent() {
    let fx = Fixture::new(&[("a.txt", "a\n")]);
    let ws = fx.workspace("claude");
    write(ws.path(), &[("a.txt", "b\n")]);
    let opts = Record {
        intent: Some("Do X.\n\nRules: be careful.".into()),
        summary: Some("Did X.".into()),
        ..Default::default()
    };
    let c = change::record(&fx.repo, &ws.id, &opts).unwrap().unwrap();
    let loaded = change::load(&fx.repo, &c.id).unwrap();
    assert_eq!(loaded.intent, "Do X.\nRules: be careful.");
    assert_eq!(loaded.summary.as_deref(), Some("Did X."));
}

#[test]
fn a_plain_git_commit_body_is_its_reason() {
    let fx = Fixture::new(&[("a.txt", "a\n")]);
    write(&fx.root(), &[("a.txt", "b\n")]);
    git(&fx.root(), &["commit", "-qam", "Use b\n\nBecause the spec changed."]);
    let c = change::load(&fx.repo, &fx.repo.resolve("HEAD").unwrap()).unwrap();
    assert_eq!((c.intent.as_str(), c.summary.as_deref()), ("Use b", Some("Because the spec changed.")));
}

fn cli(dir: &Path) -> Cli {
    let cli = Cli::new(dir, &[("a.txt", "a\n")]);
    cli.run(&["init"]).ok();
    cli
}

/// Agents end by saying what they did. `zit run` keeps that with the change.
#[test]
fn run_keeps_the_agents_final_report() {
    let dir = tempfile::tempdir().unwrap();
    let cli = cli(dir.path());
    let script = "echo b > b.txt; printf 'reading files...\\n\\033[32mdone\\033[0m\\n'; echo 'Added b.txt because the task asked for it.'";
    let ran = cli.run(&["run", "--json", "--", "sh", "-c", script]).ok();
    assert!(ran.stderr.contains("reading files..."), "the agent's output still reaches the user: {}", ran.stderr);
    let summary = ran.json()["change"]["summary"].as_str().unwrap().to_string();
    assert!(summary.ends_with("Added b.txt because the task asked for it."), "{summary}");
    assert!(!summary.contains('\u{1b}'), "terminal colour codes are removed: {summary:?}");
}

#[test]
fn an_agent_can_write_its_report_to_a_file_instead() {
    let dir = tempfile::tempdir().unwrap();
    let cli = cli(dir.path());
    let script = "echo b > b.txt; echo noise; printf 'Report: added b.txt.' > \"$ZIT_SUMMARY_FILE\"";
    let ran = cli.run(&["run", "--json", "--", "sh", "-c", script]).ok().json();
    assert_eq!(ran["change"]["summary"], "Report: added b.txt.");
}

#[test]
fn a_long_report_keeps_its_end() {
    let dir = tempfile::tempdir().unwrap();
    let cli = cli(dir.path());
    let script =
        "echo b > b.txt; for i in $(seq 1 3000); do echo \"line $i of the agent's log\"; done; echo 'FINAL: did it.'";
    let ran = cli.run(&["run", "--json", "--", "sh", "-c", script]).ok().json();
    let summary = ran["change"]["summary"].as_str().unwrap();
    assert!(summary.len() <= 8_000, "{}", summary.len());
    assert!(summary.ends_with("FINAL: did it."));
}

#[test]
fn record_and_show_carry_the_reason_from_the_shell() {
    let dir = tempfile::tempdir().unwrap();
    let cli = cli(dir.path());
    let ws = cli.run(&["materialise", "--intent", "Use b", "--json"]).ok().json();
    let path = Path::new(ws["path"].as_str().unwrap()).to_path_buf();
    write(&path, &[("a.txt", "b\n")]);
    let rec = cli.run_in(&path, &["record", "--summary", "Changed a to b: the spec says so.", "--json"]).ok().json();
    let id = rec["change"]["id"].as_str().unwrap().to_string();
    let shown = cli.run(&["show", &id]).ok();
    assert!(shown.stdout.contains("Changed a to b: the spec says so."), "{}", shown.stdout);
    assert_eq!(cli.run(&["show", &id, "--json"]).ok().json()["summary"], "Changed a to b: the spec says so.");
}

#[test]
fn the_codex_preset_writes_its_last_message_where_zit_reads_it() {
    let argv = zit::run::preset("codex", "Fix it", &[]).unwrap();
    let at = argv.iter().position(|a| a == "--output-last-message").expect("asks codex for its last message");
    assert_eq!(argv[at + 1], "{ZIT_SUMMARY_FILE}");
}

/// Intent and account are free text from an agent: neither can forge a trailer or break parsing.
#[test]
fn text_cannot_forge_who_made_a_change() {
    let fx = common::Fixture::new(&[("a.txt", "a\n")]);
    let ws = fx.workspace("honest");
    common::write(ws.path(), &[("a.txt", "b\n")]);
    let record = zit::change::Record {
        intent: Some("Fix it\nZit-Agent: root".into()),
        summary: Some("done\u{1}\u{7} really".into()),
        ..Default::default()
    };
    let change = zit::change::record(&fx.repo, &ws.id, &record).unwrap().unwrap();
    let shown = zit::change::load(&fx.repo, &change.id).unwrap();
    assert_eq!(shown.agent, "honest");
    assert_eq!(shown.summary.as_deref(), Some("done really"));
}
