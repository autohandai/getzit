//! Adversarial probes of the interfaces: error paths, malformed input,
//! partial failures.

mod common;

use common::{write, Cli};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Stdio};

/// A `zit mcp` process on pipes.
struct Mcp {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl Mcp {
    fn start(cli: &Cli, flags: &[&str]) -> Mcp {
        let mut child =
            cli.command(&cli.root).arg("mcp").args(flags).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        Mcp { child, stdin, stdout }
    }

    fn send(&mut self, raw: &[u8]) {
        self.stdin.write_all(raw).unwrap();
        self.stdin.write_all(b"\n").unwrap();
        self.stdin.flush().unwrap();
    }

    fn read(&mut self) -> Value {
        let mut line = String::new();
        self.stdout.read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap_or_else(|e| panic!("not json ({e}): {line}"))
    }

    fn request(&mut self, id: u64, method: &str, params: Value) -> Value {
        self.send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}).to_string().as_bytes());
        let reply = self.read();
        assert_eq!(reply["id"], id, "{reply}");
        reply
    }

    /// Call a tool; returns (is_error, payload).
    fn call(&mut self, tool: &str, args: Value) -> (bool, Value) {
        let reply = self.request(99, "tools/call", json!({"name": tool, "arguments": args}));
        let result = &reply["result"];
        let text = result["content"][0]["text"].as_str().unwrap_or_else(|| panic!("no content: {reply}"));
        (result["isError"] == true, serde_json::from_str(text).unwrap_or(Value::String(text.to_string())))
    }
}

impl Drop for Mcp {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

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

/// `resources: "a.txt"` (a string, not a list) must be refused, not silently treated as an
/// empty list that is then "granted".
#[test]
fn mcp_rejects_resources_that_are_not_a_list_of_strings() {
    let dir = tempfile::tempdir().unwrap();
    let cli = cli(dir.path());
    let mut mcp = Mcp::start(&cli, &[]);
    let (err, ws) = mcp.call("zit_materialise", json!({"intent": "x"}));
    assert!(!err, "{ws}");
    for bad in [json!("a.txt"), json!(["a.txt", 7]), json!({"path": "a.txt"})] {
        let (err, msg) = mcp.call("zit_claim", json!({"workspace": ws["id"], "resources": bad}));
        assert!(err, "{msg}");
        assert!(msg.as_str().unwrap().contains("resources"), "{msg}");
        let (err, msg) = mcp.call("zit_read", json!({"workspace": ws["id"], "resources": bad}));
        assert!(err, "{msg}");
        let (err, msg) = mcp.call("zit_record", json!({"workspace": ws["id"], "reads": bad}));
        assert!(err, "{msg}");
    }
    let (_, status) = mcp.call("zit_status", json!({}));
    assert_eq!(status["workspaces"][0]["claims"], json!([]));
}

/// JSON that is not a request object is answered with -32600 (Invalid Request), id null;
/// an object without a method is -32600 too, not "method not found".
#[test]
fn mcp_answers_invalid_requests_with_32600() {
    let dir = tempfile::tempdir().unwrap();
    let cli = cli(dir.path());
    let mut mcp = Mcp::start(&cli, &[]);
    for raw in ["42", "\"ping\"", "null"] {
        mcp.send(raw.as_bytes());
        let reply = mcp.read();
        assert_eq!(reply["error"]["code"], -32600, "{raw}: {reply}");
        assert_eq!(reply["id"], Value::Null, "{raw}: {reply}");
    }
    mcp.send(br#"{"jsonrpc": "2.0", "id": 7}"#);
    let reply = mcp.read();
    assert_eq!(reply["error"]["code"], -32600, "{reply}");
    assert_eq!(reply["id"], 7);
    mcp.send(br#"{"jsonrpc": "2.0", "id": 8, "method": 5}"#);
    assert_eq!(mcp.read()["error"]["code"], -32600);
    assert_eq!(mcp.request(9, "ping", json!({}))["result"], json!({}), "still serving");
}

/// A JSON-RPC batch (allowed by protocol 2025-03-26) is answered with a batch; its
/// notifications get no entry, and an empty batch is an invalid request.
#[test]
fn mcp_answers_batches_with_batches() {
    let dir = tempfile::tempdir().unwrap();
    let cli = cli(dir.path());
    let mut mcp = Mcp::start(&cli, &[]);
    mcp.send(
        json!([
            {"jsonrpc": "2.0", "id": 1, "method": "ping"},
            {"jsonrpc": "2.0", "method": "notifications/initialized"},
            {"jsonrpc": "2.0", "id": 2, "method": "nope"},
            {"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "zit_show", "arguments": {"change": "current"}}}
        ])
        .to_string()
        .as_bytes(),
    );
    let replies = mcp.read();
    let replies = replies.as_array().unwrap_or_else(|| panic!("not a batch: {replies}"));
    assert_eq!(replies.len(), 3, "{replies:?}");
    assert_eq!(replies[0]["id"], 1);
    assert_eq!(replies[0]["result"], json!({}));
    assert_eq!(replies[1]["error"]["code"], -32601);
    assert_eq!(replies[2]["result"]["isError"], false, "{}", replies[2]);
    mcp.send(b"[]");
    assert_eq!(mcp.read()["error"]["code"], -32600);
    mcp.send(br#"[{"jsonrpc": "2.0", "method": "notifications/initialized"}]"#);
    assert_eq!(mcp.request(9, "ping", json!({}))["result"], json!({}), "a batch of notifications gets no reply");
}
