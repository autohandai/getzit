mod common;

use common::{write, Cli};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Stdio};

struct Server {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
}

impl Server {
    fn start(cli: &Cli) -> Server {
        Server::start_with(cli, &[])
    }

    fn start_with(cli: &Cli, flags: &[&str]) -> Server {
        let mut child =
            cli.command(&cli.root).arg("mcp").args(flags).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        let mut server = Server { child, stdin, stdout, next_id: 0 };
        let init = server.request(
            "initialize",
            json!({"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "test-agent", "version": "1"}}),
        );
        assert_eq!(init["result"]["serverInfo"]["name"], "zit");
        assert_eq!(init["result"]["protocolVersion"], "2025-06-18");
        assert!(init["result"]["capabilities"]["tools"].is_object());
        server.send(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
        server
    }

    fn send(&mut self, msg: Value) {
        writeln!(self.stdin, "{msg}").unwrap();
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        self.next_id += 1;
        self.send(json!({"jsonrpc": "2.0", "id": self.next_id, "method": method, "params": params}));
        let mut line = String::new();
        self.stdout.read_line(&mut line).unwrap();
        let reply: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(reply["id"], self.next_id);
        reply
    }

    /// Call a tool; returns (is_error, structured payload).
    fn call(&mut self, tool: &str, args: Value) -> (bool, Value) {
        let reply = self.request("tools/call", json!({"name": tool, "arguments": args}));
        let result = &reply["result"];
        let text = result["content"][0]["text"].as_str().unwrap_or_else(|| panic!("no content: {reply}"));
        (result["isError"] == true, serde_json::from_str(text).unwrap_or(Value::String(text.to_string())))
    }
}

impl Drop for Server {
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

#[test]
fn lists_its_tools_with_schemas() {
    let dir = tempfile::tempdir().unwrap();
    let mut server = Server::start(&cli(dir.path()));
    let tools = server.request("tools/list", json!({}));
    let names: Vec<&str> =
        tools["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(
        names,
        [
            "zit_materialise",
            "zit_claim",
            "zit_read",
            "zit_record",
            "zit_status",
            "zit_show",
            "zit_log",
            "zit_diff",
            "zit_check",
            "zit_retry",
            "zit_dispose"
        ]
    );
    for tool in tools["result"]["tools"].as_array().unwrap() {
        assert_eq!(tool["inputSchema"]["type"], "object", "{tool}");
        assert!(tool["description"].as_str().unwrap().len() > 20);
    }
}

/// Agents get no tool that moves current or removes another agent's work.
#[test]
fn accept_and_discard_are_for_the_integrator_only() {
    let dir = tempfile::tempdir().unwrap();
    let cli = cli(dir.path());
    let mut agent = Server::start(&cli);
    assert_eq!(
        agent.request("tools/call", json!({"name": "zit_accept", "arguments": {"change": "current"}}))["error"]["code"],
        -32602
    );
    assert_eq!(
        agent.request("tools/call", json!({"name": "zit_discard", "arguments": {"change": "current"}}))["error"]
            ["code"],
        -32602
    );
    let mut integrator = Server::start_with(&cli, &["--integrator"]);
    let tools = integrator.request("tools/list", json!({}));
    let names: Vec<&str> =
        tools["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"zit_accept") && names.contains(&"zit_discard"), "{names:?}");
}

#[test]
fn an_agent_can_do_the_whole_loop_over_mcp() {
    let dir = tempfile::tempdir().unwrap();
    let mut server = Server::start_with(&cli(dir.path()), &["--integrator"]);

    let (err, ws) = server.call("zit_materialise", json!({"intent": "Raise price"}));
    assert!(!err, "{ws}");
    assert_eq!(ws["agent"], "test-agent", "agent identity defaults to the MCP client name");
    let path = Path::new(ws["path"].as_str().unwrap()).to_path_buf();
    write(&path, &[("src/lib.rs", "pub fn price(x: u32) -> u32 { x + 1 }\n")]);

    let (err, _) = server.call("zit_read", json!({"workspace": ws["id"], "resources": ["a.txt"]}));
    assert!(!err);
    let (err, rec) = server.call("zit_record", json!({"workspace": ws["id"], "dispose": true}));
    assert!(!err, "{rec}");
    assert_eq!(rec["writes"], json!(["src/lib.rs#price"]));
    assert_eq!(rec["change"]["reads"], json!(["a.txt"]));
    let id = rec["change"]["id"].clone();

    let (_, status) = server.call("zit_status", json!({}));
    assert_eq!(status["changes"][0]["id"], id);
    assert_eq!(status["workspaces"], json!([]));

    let (_, shown) = server.call("zit_show", json!({"change": id}));
    assert_eq!(shown["status"], "verified");
    let (_, checked) = server.call("zit_check", json!({"change": id}));
    assert_eq!(checked, json!([]));

    let (err, accepted) = server.call("zit_accept", json!({"change": id}));
    assert!(!err);
    assert_eq!(accepted["outcome"], "accepted");
    let (_, status) = server.call("zit_status", json!({}));
    assert_eq!(status["current"]["id"], id);
}

/// `zit_log` and `zit_diff` mirror the CLI commands, for agents that review rather than write.
#[test]
fn an_agent_can_read_the_log_and_a_changes_diff() {
    let dir = tempfile::tempdir().unwrap();
    let cli = cli(dir.path());
    let mut server = Server::start_with(&cli, &["--integrator"]);
    let (_, ws) = server.call("zit_materialise", json!({"intent": "Raise price"}));
    write(Path::new(ws["path"].as_str().unwrap()), &[("src/lib.rs", "pub fn price(x: u32) -> u32 { x + 1 }\n")]);
    let (_, rec) = server.call("zit_record", json!({"workspace": ws["id"], "dispose": true}));
    let id = rec["change"]["id"].clone();

    let (err, diff) = server.call("zit_diff", json!({"change": id}));
    assert!(!err, "{diff}");
    assert!(diff["diff"].as_str().unwrap().contains("+pub fn price(x: u32) -> u32 { x + 1 }"), "{diff}");
    let (_, stat) = server.call("zit_diff", json!({"change": id, "against": "current", "stat": true}));
    assert!(stat["diff"].as_str().unwrap().contains("1 file changed"), "{stat}");

    server.call("zit_accept", json!({"change": id}));
    let (err, log) = server.call("zit_log", json!({"limit": 1}));
    assert!(!err, "{log}");
    assert_eq!(log["changes"][0]["id"], id);
    assert_eq!(log["changes"].as_array().unwrap().len(), 1);
    assert_eq!(log["totals"]["changes"], 1);
    let (_, all) = server.call("zit_log", json!({}));
    assert_eq!(all["changes"].as_array().unwrap().len(), 2, "genesis too");
}

#[test]
fn tool_failures_are_reported_as_tool_errors_not_protocol_errors() {
    let dir = tempfile::tempdir().unwrap();
    let mut server = Server::start(&cli(dir.path()));
    let (err, msg) = server.call("zit_show", json!({"change": "nope"}));
    assert!(err);
    assert!(msg.as_str().unwrap().contains("unknown revision"), "{msg}");
    let (err, msg) = server.call("zit_show", json!({}));
    assert!(err);
    assert!(msg.as_str().unwrap().contains("change"), "{msg}");
}

#[test]
fn unknown_methods_get_a_json_rpc_error_and_ping_works() {
    let dir = tempfile::tempdir().unwrap();
    let mut server = Server::start(&cli(dir.path()));
    assert_eq!(server.request("nope/nope", json!({}))["error"]["code"], -32601);
    assert_eq!(server.request("ping", json!({}))["result"], json!({}));
    let unknown_tool = server.request("tools/call", json!({"name": "nope", "arguments": {}}));
    assert_eq!(unknown_tool["error"]["code"], -32602);
}

#[test]
fn an_agent_can_claim_work_and_learn_who_holds_the_rest() {
    let dir = tempfile::tempdir().unwrap();
    let mut server = Server::start(&cli(dir.path()));
    let (_, a) = server.call("zit_materialise", json!({"intent": "prices", "agent": "claude"}));
    let (_, b) = server.call("zit_materialise", json!({"intent": "prices too", "agent": "codex"}));
    let (err, granted) = server.call("zit_claim", json!({"workspace": a["id"], "resources": ["src/lib.rs"]}));
    assert!(!err);
    assert_eq!(granted["claim"], "granted");
    let (err, refused) = server.call("zit_claim", json!({"workspace": b["id"], "resources": ["src/lib.rs#price"]}));
    assert!(!err, "a refusal is an answer, not a tool failure");
    assert_eq!(refused["claim"], "refused");
    assert_eq!(refused["held"][0]["by"]["agent"], "claude");
    let (_, status) = server.call("zit_status", json!({}));
    let holder = status["workspaces"].as_array().unwrap().iter().find(|w| w["id"] == a["id"]).unwrap();
    assert_eq!(holder["claims"], json!(["src/lib.rs"]));
}
