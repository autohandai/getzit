//! What an agent turn cost: tokens and money, read from the agent's own output.
mod common;

use common::Cli;
use std::fs;
use std::path::Path;

fn fake(bin: &Path, name: &str, script: &str) {
    fs::create_dir_all(bin).unwrap();
    let path = bin.join(name);
    fs::write(&path, format!("#!/bin/sh\n{script}\n")).unwrap();
    fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
}

fn run_preset(cli: &Cli, bin: &Path, agent: &str) -> serde_json::Value {
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
    let out = cli
        .command(&cli.root)
        .env("PATH", path)
        .args(["run", "--json", "--agent", agent, "--intent", "Do the thing"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    serde_json::from_slice(&out.stdout).unwrap()
}

#[test]
fn a_claude_turn_records_its_cost_tokens_and_final_message() {
    let dir = tempfile::tempdir().unwrap();
    let cli = Cli::new(dir.path(), &[("a.txt", "a\n")]);
    cli.run(&["init"]).ok();
    let bin = dir.path().join("bin");
    fake(
        &bin,
        "claude",
        r#"echo edited > b.txt
printf '%s\n' '{"type":"result","result":"Added b.txt because the task asked for it.","total_cost_usd":0.25,"usage":{"input_tokens":10,"cache_creation_input_tokens":100,"cache_read_input_tokens":1000,"output_tokens":50}}'"#,
    );
    let report = run_preset(&cli, &bin, "claude");
    let change = &report["change"];
    assert_eq!(change["summary"], "Added b.txt because the task asked for it.");
    assert_eq!(change["usage"]["input_tokens"], 1110);
    assert_eq!(change["usage"]["output_tokens"], 50);
    assert_eq!(change["usage"]["cost_usd"], 0.25);
    let shown = cli.run(&["show", change["id"].as_str().unwrap()]).ok().stdout;
    assert!(shown.contains("usage    1110 tokens in, 50 out, $0.2500"), "{shown}");
}

#[test]
fn a_codex_turn_records_the_tokens_of_every_turn() {
    let dir = tempfile::tempdir().unwrap();
    let cli = Cli::new(dir.path(), &[("a.txt", "a\n")]);
    cli.run(&["init"]).ok();
    let bin = dir.path().join("bin");
    // The preset passes --output-last-message <file>; the fake writes its final message there.
    fake(
        &bin,
        "codex",
        r#"while [ $# -gt 0 ]; do [ "$1" = "--output-last-message" ] && out=$2; shift; done
echo edited > b.txt
echo "Wrote b.txt; nothing else needed." > "$out"
echo '{"type":"turn.completed","usage":{"input_tokens":100,"cached_input_tokens":40,"output_tokens":7}}'
echo '{"type":"item.completed","item":{"type":"agent_message","text":"Wrote b.txt; nothing else needed."}}'
echo '{"type":"turn.completed","usage":{"input_tokens":200,"cached_input_tokens":0,"output_tokens":3}}'"#,
    );
    let report = run_preset(&cli, &bin, "codex");
    let change = &report["change"];
    assert_eq!(change["summary"], "Wrote b.txt; nothing else needed.");
    assert_eq!(change["usage"]["input_tokens"], 300);
    assert_eq!(change["usage"]["output_tokens"], 10);
    assert!(change["usage"]["cost_usd"].is_null(), "codex reports no price");
}

/// A fake agent that writes `b.txt` and replays a captured output file.
fn replay(bin: &Path, name: &str, fixture: &str) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(fixture);
    fake(bin, name, &format!("echo edited > b.txt\ncat '{}'", path.display()));
}

/// Autohand Code 0.9.9's stream-json carries its final message and nothing about cost.
#[test]
fn an_autohand_turn_keeps_its_final_message() {
    let dir = tempfile::tempdir().unwrap();
    let cli = Cli::new(dir.path(), &[("a.txt", "a\n")]);
    cli.run(&["init"]).ok();
    let bin = dir.path().join("bin");
    replay(&bin, "autohand", "autohand-stream-json.jsonl");
    let report = run_preset(&cli, &bin, "autohand");
    assert_eq!(report["change"]["summary"], "ok");
    assert!(report["change"]["usage"].is_null(), "autohand reports no usage");
}

/// Pi's `--mode json` reports every assistant message's tokens and its own
/// price for them; the last assistant message is the account.
#[test]
fn a_pi_turn_records_its_tokens_price_and_final_message() {
    let dir = tempfile::tempdir().unwrap();
    let cli = Cli::new(dir.path(), &[("a.txt", "a\n")]);
    cli.run(&["init"]).ok();
    let bin = dir.path().join("bin");
    replay(&bin, "pi", "pi-mode-json.jsonl");
    let report = run_preset(&cli, &bin, "pi");
    let change = &report["change"];
    assert_eq!(change["summary"], "Hey! How can I help you today?");
    assert_eq!(change["usage"]["input_tokens"], 6321);
    assert_eq!(change["usage"]["output_tokens"], 67);
    assert_eq!(change["usage"]["cost_usd"], 0.0, "a free model: Pi prices it at zero");
    let shown = cli.run(&["show", change["id"].as_str().unwrap()]).ok().stdout;
    assert!(shown.contains("usage    6321 tokens in, 67 out, $0.0000"), "{shown}");
}

/// Tokens and cost are summed over Pi's assistant messages (one per model call); cached
/// input counts as input, and the events that repeat a message are not counted again.
#[test]
fn pi_usage_is_summed_over_its_messages() {
    let events = r#"{"type":"message_end","message":{"role":"user","content":[{"type":"text","text":"hi"}]}}
{"type":"message_end","message":{"role":"assistant","content":[{"type":"toolCall","id":"1","name":"read"}],"usage":{"input":100,"output":10,"cacheRead":50,"cacheWrite":25,"totalTokens":185,"cost":{"input":0.001,"output":0.002,"cacheRead":0,"cacheWrite":0,"total":0.003}},"stopReason":"toolUse"}}
{"type":"message_end","message":{"role":"assistant","content":[{"type":"text","text":"Done."}],"usage":{"input":200,"output":20,"cacheRead":0,"cacheWrite":0,"totalTokens":220,"cost":{"input":0.002,"output":0.004,"cacheRead":0,"cacheWrite":0,"total":0.006}},"stopReason":"stop"}}
{"type":"turn_end","message":{"role":"assistant","content":[{"type":"text","text":"Done."}],"usage":{"input":200,"output":20,"cacheRead":0,"cacheWrite":0,"totalTokens":220,"cost":{"total":0.006}}}}
"#;
    let (said, usage) = zit::run::read_events(events);
    assert_eq!(said.as_deref(), Some("Done."));
    let usage = usage.unwrap();
    assert_eq!((usage.input_tokens, usage.output_tokens), (375, 30));
    assert!((usage.cost_usd.unwrap() - 0.009).abs() < 1e-9, "{usage:?}");
}

/// An explicit command already in the agent's JSON mode is read the same way as a preset.
#[test]
fn an_explicit_json_command_is_read_like_a_preset() {
    let dir = tempfile::tempdir().unwrap();
    let cli = Cli::new(dir.path(), &[("a.txt", "a\n")]);
    cli.run(&["init"]).ok();
    let bin = dir.path().join("bin");
    fake(
        &bin,
        "claude",
        r#"echo edited > b.txt
printf '%s\n' '{"type":"result","result":"Done.","total_cost_usd":0.5,"usage":{"input_tokens":7,"output_tokens":3}}'"#,
    );
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
    let out = cli
        .command(&cli.root)
        .env("PATH", path)
        .args([
            "run",
            "--json",
            "--agent",
            "claude",
            "--intent",
            "x",
            "--",
            "claude",
            "-p",
            "x",
            "--output-format",
            "json",
        ])
        .output()
        .unwrap();
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["change"]["summary"], "Done.");
    assert_eq!(report["change"]["usage"]["cost_usd"], 0.5);
}
