//! Model Context Protocol server over stdio (newline-delimited JSON-RPC
//! 2.0). Exposes the graph to any MCP-capable agent as tools.

use crate::accept;
use crate::api;
use crate::change::{self, Record};
use crate::evidence;
use crate::git::Repo;
use crate::resource::Resource;
use crate::workspace::{self, NewWorkspace};
use crate::{Error, Result};
use serde_json::{json, Value};
use std::io::{BufRead, Write};
use std::path::Path;

const PROTOCOL_VERSIONS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];

fn tool(name: &str, description: &str, properties: Value, required: &[&str]) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": {"type": "object", "properties": properties, "required": required},
    })
}

/// Tools that move current or remove work: only for an integrator (`zit mcp --integrator`).
const INTEGRATOR_ONLY: [&str; 2] = ["zit_accept", "zit_discard"];

fn tools(integrator: bool) -> Value {
    let all = all_tools();
    json!(all
        .as_array()
        .expect("a list")
        .iter()
        .filter(|t| integrator || !INTEGRATOR_ONLY.iter().any(|n| t["name"] == *n))
        .collect::<Vec<_>>())
}

fn all_tools() -> Value {
    let s = |d: &str| json!({"type": "string", "description": d});
    let list = |d: &str| json!({"type": "array", "items": {"type": "string"}, "description": d});
    let flag = |d: &str| json!({"type": "boolean", "description": d});
    let change = || s("Change id (any unique prefix), git revision, or `current`.");
    let resource = "Resource: `path` (file), `path#Symbol` (top-level symbol) or `path#` (module-level code).";
    json!([
        tool(
            "zit_materialise",
            "Start work: get a disposable directory holding a program state (default: the current accepted state). Edit files under the returned `path`, then call zit_record. No branch or worktree is created.",
            json!({"intent": s("What this change is meant to achieve."), "agent": s("Your name; defaults to the MCP client name."), "session": s("Conversation or session id, for provenance."), "from": change()}),
            &["intent"],
        ),
        tool(
            "zit_claim",
            "Claim the resources you intend to write, before writing them. Refused (with who holds what) if a live workspace claimed them or an unaccepted change already wrote them: pick other work instead of duplicating it. All or nothing. A claim lapses when its workspace is disposed.",
            json!({"workspace": s("Workspace id."), "resources": list(resource)}),
            &["workspace", "resources"],
        ),
        tool(
            "zit_read",
            "Declare what you observed and relied on. If current later changes any of it, your change is reported stale instead of being silently merged.",
            json!({"workspace": s("Workspace id."), "resources": list(resource)}),
            &["workspace", "resources"],
        ),
        tool(
            "zit_record",
            "Snapshot the workspace into an immutable change in the graph. The change survives the workspace, the process and the machine. Returns the change and the symbols it wrote.",
            json!({"workspace": s("Workspace id."), "intent": s("Override the workspace intent."), "summary": s("What you did and why, in a few sentences. Stored with the change for whoever reviews or redoes it."), "reads": list(resource), "dispose": flag("Delete the workspace afterwards.")}),
            &["workspace"],
        ),
        tool(
            "zit_status",
            "Everything that exists: the current state, every speculative change with its derived status (speculative, verified, invalid + reason), every live workspace.",
            json!({}),
            &[],
        ),
        tool(
            "zit_show",
            "One change in full: intent, agent, parents, what it wrote, its status and reasons, and the evidence for its state.",
            json!({"change": change()}),
            &["change"],
        ),
        tool(
            "zit_check",
            "Run the checks declared in the state's zit.toml against a change. Results are content-addressed evidence; checks whose inputs are unchanged are reused, not re-run.",
            json!({"change": change(), "rerun": flag("Ignore existing evidence.")}),
            &["change"],
        ),
        tool(
            "zit_accept",
            "Make a change part of the current state. If current has moved, the change is composed onto it when their read/write sets do not conflict, then re-verified. A stale, conflicting or failing change is rejected with the exact reason and left intact.",
            json!({"change": change(), "allow_stale": flag("Compose a stale change anyway and let the checks on the composed state decide. Only when the acceptance authority asked for it."), "rerun": flag("Run every check again instead of trusting existing evidence.")}),
            &["change"],
        ),
        tool(
            "zit_retry",
            "Reconsider a stale or conflicting change: returns a new workspace on the current state with the change's edits applied (conflict markers where text collides). Fix it, then zit_record.",
            json!({"change": change()}),
            &["change"],
        ),
        tool(
            "zit_discard",
            "Remove a speculative change from the graph listing. Accepted history is unaffected.",
            json!({"change": change()}),
            &["change"],
        ),
        tool(
            "zit_dispose",
            "Delete a workspace directory. Recorded changes are unaffected; unrecorded edits are lost.",
            json!({"workspace": s("Workspace id.")}),
            &["workspace"],
        ),
    ])
}

fn text<'a>(args: &'a Value, name: &str) -> Result<&'a str> {
    args[name].as_str().ok_or_else(|| Error::msg(format!("missing required argument: {name}")))
}

fn resources(args: &Value, name: &str) -> Vec<Resource> {
    args[name].as_array().into_iter().flatten().filter_map(Value::as_str).map(Resource::parse).collect()
}

/// `None` when the tool does not exist.
fn call(cwd: &Path, client: &str, integrator: bool, name: &str, args: &Value) -> Option<Result<Value>> {
    let run = || -> Result<Value> {
        let repo = Repo::discover(cwd)?;
        let change = || repo.resolve(text(args, "change")?);
        Ok(match name {
            "zit_materialise" => {
                let from = args["from"].as_str().map(|rev| repo.resolve(rev)).transpose()?;
                let new = NewWorkspace {
                    from: from.as_ref(),
                    intent: text(args, "intent")?,
                    agent: args["agent"].as_str().unwrap_or(client),
                    session: args["session"].as_str(),
                };
                json!(workspace::materialise(&repo, &new)?)
            }
            "zit_claim" => json!(crate::claim::claim(&repo, text(args, "workspace")?, &resources(args, "resources"))?),
            "zit_read" => {
                let reads = resources(args, "resources");
                workspace::declare_reads(&repo, text(args, "workspace")?, &reads)?;
                json!({"declared": reads})
            }
            "zit_record" => {
                let opts = Record {
                    intent: args["intent"].as_str().map(str::to_string),
                    summary: args["summary"].as_str().map(str::to_string),
                    reads: resources(args, "reads"),
                };
                let dispose = args["dispose"].as_bool().unwrap_or(false);
                json!(api::record(&repo, text(args, "workspace")?, &opts, dispose)?)
            }
            "zit_status" => json!(api::overview(&repo)?),
            "zit_show" => json!(api::detail(&repo, &change()?)?),
            "zit_check" => json!(evidence::verify(&repo, &change()?, args["rerun"].as_bool().unwrap_or(false))?),
            "zit_accept" => {
                let flag = |name: &str| args[name].as_bool().unwrap_or(false);
                let policy = accept::Policy { allow_stale: flag("allow_stale"), rerun: flag("rerun") };
                json!(accept::accept_with(&repo, &change()?, &policy)?)
            }
            "zit_retry" => json!(accept::retry(&repo, &change()?)?),
            "zit_discard" => {
                let id = change()?;
                change::discard(&repo, &id)?;
                json!({"discarded": id})
            }
            "zit_dispose" => {
                let id = text(args, "workspace")?;
                workspace::dispose(&repo, id)?;
                json!({"disposed": id})
            }
            _ => unreachable!("guarded by the tool list"),
        })
    };
    let known = tools(integrator).as_array().is_some_and(|all| all.iter().any(|t| t["name"] == name));
    known.then(run)
}

/// Serve until `input` closes. Tools act on the repository containing `cwd`.
/// Without `integrator`, the agent cannot accept or discard changes.
pub fn serve(cwd: &Path, integrator: bool, input: impl BufRead, mut output: impl Write) -> Result<()> {
    let mut client = "agent".to_string();
    for line in input.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Value>(&line) {
            Err(e) => Some(json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32700, "message": e.to_string()}})),
            Ok(msg) => {
                let id = msg.get("id").cloned();
                let params = &msg["params"];
                let result = match msg["method"].as_str().unwrap_or_default() {
                    "initialize" => {
                        if let Some(name) = params["clientInfo"]["name"].as_str() {
                            client = name.to_string();
                        }
                        let asked = params["protocolVersion"].as_str().unwrap_or_default();
                        let version = PROTOCOL_VERSIONS.iter().find(|v| **v == asked).unwrap_or(&PROTOCOL_VERSIONS[0]);
                        Ok(json!({
                            "protocolVersion": version,
                            "capabilities": {"tools": {}},
                            "serverInfo": {"name": "zit", "version": env!("CARGO_PKG_VERSION")},
                            "instructions": "Zit: work in a workspace from zit_materialise, claim what you will change with zit_claim, record it with zit_record and a summary of what you did and why. Recorded changes survive your process; an integrator accepts them.",
                        }))
                    }
                    "ping" => Ok(json!({})),
                    "tools/list" => Ok(json!({"tools": tools(integrator)})),
                    "tools/call" => {
                        let name = params["name"].as_str().unwrap_or_default();
                        match call(cwd, &client, integrator, name, &params["arguments"]) {
                            None => Err((-32602, format!("unknown tool: {name}"))),
                            Some(Ok(value)) => Ok(json!({
                                "content": [{"type": "text", "text": serde_json::to_string_pretty(&value)?}],
                                "isError": false,
                            })),
                            Some(Err(e)) => Ok(json!({
                                "content": [{"type": "text", "text": e.to_string()}],
                                "isError": true,
                            })),
                        }
                    }
                    method => Err((-32601, format!("method not found: {method}"))),
                };
                // Notifications carry no id and get no reply.
                id.map(|id| match result {
                    Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
                    Err((code, message)) => {
                        json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
                    }
                })
            }
        };
        if let Some(reply) = reply {
            writeln!(output, "{reply}")?;
            output.flush()?;
        }
    }
    Ok(())
}
