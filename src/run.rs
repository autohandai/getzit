//! CLI-to-CLI: run any agent command inside a disposable workspace. However
//! the agent ends — exit, crash, SIGINT, SIGTERM — its work is recorded.

use crate::accept::{self, Outcome};
use crate::change::{self, Change, Record, Usage};
use crate::git::{Oid, Repo};
use crate::workspace::{self, NewWorkspace};
use crate::{Error, Result};
use serde::Serialize;
use signal_hook::consts::{SIGINT, SIGTERM};
use std::os::unix::process::ExitStatusExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub struct Run {
    pub agent: String,
    pub intent: String,
    pub session: Option<String>,
    pub from: Option<Oid>,
    /// Leave the workspace in place afterwards.
    pub keep: bool,
    /// Try to accept the recorded change.
    pub accept: bool,
    /// Send the agent's stdout to our stderr, keeping our stdout machine-readable.
    pub quiet_stdout: bool,
    /// Stop the agent after this long; its partial work is still recorded.
    pub timeout: Option<Duration>,
    /// The agent prints JSON events (a preset in its JSON mode): read its final
    /// message and usage from them, and show the final message instead of the raw stream.
    pub structured: bool,
    /// Keep the end of the agent's combined output with the recorded change
    /// (`refs/zit/logs/<change>`), for `zit show --log`.
    pub log: bool,
    pub command: Vec<String>,
}

/// Where `--log` keeps an agent's output: a blob per change.
pub const LOGS: &str = "refs/zit/logs";

/// How much of the agent's output `--log` keeps: the last 64 KB.
pub const LOG_BYTES: usize = 64 * 1024;

/// The output `zit run --log` kept for a change, if any.
pub fn log_of(repo: &Repo, change: &Oid) -> Result<Option<String>> {
    Ok(repo.objects()?.read(&format!("{LOGS}/{change}"))?.map(|b| String::from_utf8_lossy(&b).into_owned()))
}

fn store_log(repo: &Repo, change: &Oid, text: &str) -> Result<()> {
    let blob = repo.git_stdin(&["hash-object", "-w", "--stdin"], &[], text)?;
    repo.git(&["update-ref", &format!("{LOGS}/{change}"), &blob])?;
    Ok(())
}

/// Append to a buffer that keeps only its last `keep` bytes.
fn keep_tail(kept: &mut Vec<u8>, bytes: &[u8], keep: usize) {
    kept.extend_from_slice(bytes);
    let excess = kept.len().saturating_sub(keep);
    kept.drain(..excess);
}

#[derive(Debug, Serialize)]
pub struct Report {
    pub workspace: String,
    pub path: PathBuf,
    pub exit_code: i32,
    pub interrupted: bool,
    /// The agent was stopped by `--timeout`.
    pub timed_out: bool,
    pub change: Option<Change>,
    pub outcome: Option<Outcome>,
}

/// In a command, replaced by the path of the file whose content becomes the
/// change's account. Also exported to the agent as `$ZIT_SUMMARY_FILE`.
pub const SUMMARY_FILE: &str = "{ZIT_SUMMARY_FILE}";

/// The longest account kept; agents put their conclusion last, so the end is kept.
const MAX_SUMMARY: usize = 8_000;

/// Headless invocation of a known agent CLI for `prompt`. `writable` are
/// directories outside the workspace that zit commands run by the agent
/// write to (zit's home);
/// sandboxed agents are granted them.
pub fn preset(agent: &str, prompt: &str, writable: &[PathBuf]) -> Option<Vec<String>> {
    let s = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    Some(match agent {
        "claude" => s(&["claude", "-p", prompt, "--permission-mode", "acceptEdits", "--output-format", "json"]),
        "codex" => {
            // Codex writes its final message where zit picks it up as the change's account.
            let mut argv =
                s(&["codex", "exec", "--json", "--sandbox", "workspace-write", "--output-last-message", SUMMARY_FILE]);
            for dir in writable {
                argv.extend(["--add-dir".to_string(), dir.display().to_string()]);
            }
            argv.push(prompt.to_string());
            argv
        }
        "autohand" => s(&["autohand", "-p", prompt, "--yes", "--output-format", "stream-json"]),
        "pi" => s(&["pi", "-p", prompt]),
        _ => return None,
    })
}

/// Whether `argv` runs a known agent in the JSON mode `zit run` reads:
/// `claude … --output-format json|stream-json`, `codex exec … --json`,
/// `autohand … --output-format stream-json`.
pub fn prints_json_events(argv: &[String]) -> bool {
    let program = argv.first().and_then(|p| std::path::Path::new(p).file_name()).and_then(|n| n.to_str());
    let has = |flag: &str, values: &[&str]| {
        argv.windows(2).any(|w| w[0] == flag && values.contains(&w[1].as_str()))
            || argv.iter().any(|a| values.iter().any(|v| *a == format!("{flag}={v}")))
    };
    match program {
        Some("claude") => has("--output-format", &["json", "stream-json"]),
        Some("autohand") => has("--output-format", &["stream-json"]),
        Some("codex") => argv.iter().any(|a| a == "--json"),
        _ => false,
    }
}

/// A terminal Ctrl-C already reaches the agent (same process group); a
/// signal sent to us alone does not and must be forwarded.
fn terminal_delivers_signals() -> bool {
    // SAFETY: plain queries on our own stdin and process group.
    unsafe { libc::isatty(0) == 1 && libc::tcgetpgrp(0) == libc::getpgrp() }
}

const POLL: Duration = Duration::from_millis(20);
const TERMINAL_GRACE: Duration = Duration::from_secs(1);
const KILL_AFTER: Duration = Duration::from_secs(10);

pub fn run(repo: &Repo, opts: &Run) -> Result<Report> {
    let program = opts.command.first().ok_or_else(|| Error::msg("no command to run"))?;
    let interrupt = Arc::new(AtomicBool::new(false));
    let terminate = Arc::new(AtomicBool::new(false));
    // While the agent runs, signals are forwarded to it. Once its work is
    // recorded (`recorded` set), a signal stops zit itself.
    let recorded = Arc::new(AtomicBool::new(false));
    for signal in [SIGINT, SIGTERM] {
        signal_hook::flag::register_conditional_shutdown(signal, 128 + signal, recorded.clone())?;
    }
    signal_hook::flag::register(SIGINT, interrupt.clone())?;
    signal_hook::flag::register(SIGTERM, terminate.clone())?;

    let new = NewWorkspace {
        from: opts.from.as_ref(),
        intent: &opts.intent,
        agent: &opts.agent,
        session: opts.session.as_deref(),
    };
    let mut ws = workspace::materialise(repo, &new)?;
    ws.pid = Some(std::process::id());
    ws.pid_started = workspace::process_start(std::process::id());
    ws.save()?;

    let summary_file = ws.summary_file();
    let args = opts.command[1..].iter().map(|a| a.replace(SUMMARY_FILE, &summary_file.display().to_string()));
    let mut cmd = Command::new(program);
    cmd.args(args)
        .current_dir(ws.path())
        .env("ZIT_WORKSPACE", &ws.id)
        .env("ZIT_SUMMARY_FILE", &summary_file)
        .env("ZIT_CACHE_DIR", repo.cache_dir()?)
        .env("TMPDIR", ws.temp_dir()?)
        .stdout(Stdio::piped());
    if let Some(port) = ws.port {
        cmd.env("ZIT_PORT", port.to_string());
    }
    if opts.log {
        cmd.stderr(Stdio::piped());
    }
    // Headless, the agent leads its own process group, so whatever it starts is stopped with it.
    // Interactive, it must stay in the terminal's foreground group to read the terminal.
    let own_group = !interactive();
    if own_group {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) => {
            workspace::dispose(repo, &ws.id)?;
            return Err(Error::msg(format!("cannot run {program}: {e}")));
        }
    };

    // Pass the agent's output through, keeping the end of it: stdout for its
    // account, and with `--log` both streams for the log.
    let tail = Arc::new(std::sync::Mutex::new(Vec::<u8>::new()));
    let log = Arc::new(std::sync::Mutex::new(Vec::<u8>::new()));
    let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
    let mut readers = 0;
    if let Some(mut err) = child.stderr.take() {
        let (log, done_tx) = (log.clone(), done_tx.clone());
        readers += 1;
        std::thread::spawn(move || {
            use std::io::{Read, Write};
            let mut buf = [0u8; 8192];
            while let Ok(n @ 1..) = err.read(&mut buf) {
                let _ = std::io::stderr().write_all(&buf[..n]);
                keep_tail(&mut log.lock().expect("log lock"), &buf[..n], LOG_BYTES);
            }
            let _ = done_tx.send(());
        });
    }
    if let Some(mut out) = child.stdout.take() {
        let (tail, log) = (tail.clone(), log.clone());
        readers += 1;
        let quiet = opts.quiet_stdout;
        let (echo, keep) = match opts.structured {
            true => (false, 64 * MAX_SUMMARY),
            false => (true, 4 * MAX_SUMMARY),
        };
        if opts.structured {
            eprintln!(
                "zit: {} is working in {}; its final message follows when it is done",
                opts.agent,
                ws.path().display()
            );
        }
        std::thread::spawn(move || {
            use std::io::{Read, Write};
            let mut buf = [0u8; 8192];
            while let Ok(n) = out.read(&mut buf) {
                if n == 0 {
                    break;
                }
                if echo {
                    let _ = if quiet {
                        std::io::stderr().write_all(&buf[..n])
                    } else {
                        std::io::stdout().write_all(&buf[..n])
                    };
                }
                keep_tail(&mut tail.lock().expect("tail lock"), &buf[..n], keep);
                keep_tail(&mut log.lock().expect("log lock"), &buf[..n], LOG_BYTES);
            }
            let _ = done_tx.send(());
        });
    }
    let pid = child.id() as i32;
    let target = if own_group { -pid } else { pid };
    let signal = |sig| {
        // SAFETY: `target` is our own child, or the process group it leads.
        unsafe { libc::kill(target, sig) };
    };
    let (mut stopped_by, mut forward_at, mut kill_at): (Option<i32>, Option<Instant>, Option<Instant>) =
        (None, None, None);
    let deadline = opts.timeout.map(|limit| Instant::now() + limit);
    let mut timed_out = false;
    let mut killed = false;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        let now = Instant::now();
        if !timed_out && stopped_by.is_none() && deadline.is_some_and(|at| now >= at) {
            timed_out = true;
            signal(SIGTERM);
            kill_at.get_or_insert(now + KILL_AFTER);
        }
        if terminate.swap(false, Ordering::Relaxed) {
            stopped_by.get_or_insert(SIGTERM);
            signal(SIGTERM);
            kill_at.get_or_insert(now + KILL_AFTER);
        }
        if interrupt.swap(false, Ordering::Relaxed) {
            stopped_by.get_or_insert(SIGINT);
            let grace = if terminal_delivers_signals() { TERMINAL_GRACE } else { Duration::ZERO };
            forward_at.get_or_insert(now + grace);
            kill_at.get_or_insert(now + KILL_AFTER);
        }
        if forward_at.is_some_and(|at| now >= at) {
            forward_at = None;
            signal(SIGINT);
        }
        if kill_at.is_some_and(|at| now >= at) {
            kill_at = None;
            killed = true;
            signal(libc::SIGKILL);
        }
        std::thread::sleep(POLL);
    };

    // Already SIGKILLed as a group: nothing is left to ask nicely.
    if own_group && !killed {
        stop_group(pid);
    }
    // The agent and everything it started are gone: a lock it left behind is stale.
    let _ = std::fs::remove_file(ws.dir().join("git/index.lock"));
    // Children the agent left running may hold the pipes open; do not wait for them.
    let until = Instant::now() + Duration::from_secs(2);
    for _ in 0..readers {
        let _ = done_rx.recv_timeout(until.saturating_duration_since(Instant::now()));
    }
    let output = String::from_utf8_lossy(&tail.lock().expect("tail lock")).into_owned();
    let (said, usage) = if opts.structured { read_events(&output) } else { (None, None) };
    let reported = std::fs::read_to_string(&summary_file).ok().filter(|s| !s.trim().is_empty());
    let summary = account(&reported.or(said).unwrap_or(output));
    if opts.structured {
        if let Some(text) = &summary {
            match opts.quiet_stdout {
                true => eprintln!("{text}"),
                false => println!("{text}"),
            }
        }
    }
    let change = change::record(repo, &ws.id, &Record { summary, usage, ..Default::default() })?;
    if let (true, Some(change)) = (opts.log, &change) {
        let text = String::from_utf8_lossy(&log.lock().expect("log lock")).into_owned();
        store_log(repo, &change.id, &text)?;
    }
    if !opts.keep {
        workspace::dispose(repo, &ws.id)?;
    }
    recorded.store(true, Ordering::SeqCst);
    let outcome = match (&change, opts.accept && stopped_by.is_none() && !timed_out) {
        (Some(change), true) => Some(accept::accept(repo, &change.id)?),
        _ => None,
    };
    let exit_code = match stopped_by {
        Some(sig) => 128 + sig,
        None if timed_out => 124,
        None => status.code().unwrap_or_else(|| 128 + status.signal().unwrap_or(0)),
    };
    Ok(Report {
        workspace: ws.id.clone(),
        path: ws.path().to_path_buf(),
        exit_code,
        interrupted: stopped_by.is_some(),
        timed_out,
        change,
        outcome,
    })
}

/// The agent's account: colour codes removed, trimmed, at most
/// `MAX_SUMMARY` bytes from the end.
/// The final message and usage in an agent's JSON output: Claude Code's
/// result object, Codex's JSONL events, Autohand's stream-json.
fn read_events(output: &str) -> (Option<String>, Option<Usage>) {
    let (mut said, mut usage): (Option<String>, Option<Usage>) = (None, None);
    let n = |v: &serde_json::Value, k: &str| v[k].as_u64().unwrap_or(0);
    for line in output.lines() {
        let Ok(event) = serde_json::from_str::<serde_json::Value>(line.trim()) else { continue };
        match event["type"].as_str() {
            Some("result") => {
                if let Some(text) = event["result"].as_str().or(event["content"].as_str()) {
                    said = Some(text.to_string());
                }
                let u = &event["usage"];
                if u.is_object() || event["total_cost_usd"].is_number() {
                    let input =
                        n(u, "input_tokens") + n(u, "cache_creation_input_tokens") + n(u, "cache_read_input_tokens");
                    usage = Some(Usage {
                        input_tokens: input,
                        output_tokens: n(u, "output_tokens"),
                        cost_usd: event["total_cost_usd"].as_f64(),
                    });
                }
            }
            Some("item.completed") if event["item"]["type"] == "agent_message" => {
                said = event["item"]["text"].as_str().map(str::to_string).or(said);
            }
            Some("turn.completed") => {
                let u = usage.get_or_insert_with(Usage::default);
                u.input_tokens += n(&event["usage"], "input_tokens");
                u.output_tokens += n(&event["usage"], "output_tokens");
            }
            _ => {}
        }
    }
    (said, usage)
}

fn account(raw: &str) -> Option<String> {
    let mut clean = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            // CSI: ESC [ parameters final-byte. Other escapes: ESC and one character.
            if chars.next_if_eq(&'[').is_some() {
                for c in chars.by_ref() {
                    if ('@'..='~').contains(&c) {
                        break;
                    }
                }
            } else {
                chars.next();
            }
        } else if c != '\r' {
            clean.push(c);
        }
    }
    let clean = clean.trim();
    if clean.is_empty() {
        return None;
    }
    let start = clean.len().saturating_sub(MAX_SUMMARY);
    let start = (start..clean.len()).find(|&i| clean.is_char_boundary(i)).unwrap_or(0);
    Some(clean[start..].trim_start().to_string())
}

fn interactive() -> bool {
    // SAFETY: isatty only inspects the descriptor.
    unsafe { libc::isatty(libc::STDIN_FILENO) == 1 }
}

/// Stop what is left of the agent's process group before its work is recorded:
/// SIGTERM, a short grace period, then SIGKILL.
fn stop_group(leader: i32) {
    // SAFETY: signalling a process group we created; signal 0 only probes.
    let alive = || unsafe { libc::kill(-leader, 0) } == 0;
    if !alive() {
        return;
    }
    unsafe { libc::kill(-leader, SIGTERM) };
    let until = Instant::now() + Duration::from_secs(2);
    while alive() && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(20));
    }
    if alive() {
        unsafe { libc::kill(-leader, libc::SIGKILL) };
    }
}
