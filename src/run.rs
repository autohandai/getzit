//! CLI-to-CLI: run any agent command inside a disposable workspace. However
//! the agent ends — exit, crash, SIGINT, SIGTERM — its work is recorded.

use crate::accept::{self, Outcome};
use crate::change::{self, Change, Record};
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
    pub command: Vec<String>,
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
        "claude" => s(&["claude", "-p", prompt, "--permission-mode", "acceptEdits"]),
        "codex" => {
            // Codex writes its final message where zit picks it up as the change's account.
            let mut argv = s(&["codex", "exec", "--sandbox", "workspace-write", "--output-last-message", SUMMARY_FILE]);
            for dir in writable {
                argv.extend(["--add-dir".to_string(), dir.display().to_string()]);
            }
            argv.push(prompt.to_string());
            argv
        }
        "autohand" => s(&["autohand", "-p", prompt, "--yes"]),
        "pi" => s(&["pi", "-p", prompt]),
        _ => return None,
    })
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

    // Pass the agent's output through, keeping the end of it.
    let tail = Arc::new(std::sync::Mutex::new(Vec::<u8>::new()));
    let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
    if let Some(mut out) = child.stdout.take() {
        let tail = tail.clone();
        let quiet = opts.quiet_stdout;
        std::thread::spawn(move || {
            use std::io::{Read, Write};
            let mut buf = [0u8; 8192];
            while let Ok(n) = out.read(&mut buf) {
                if n == 0 {
                    break;
                }
                let _ =
                    if quiet { std::io::stderr().write_all(&buf[..n]) } else { std::io::stdout().write_all(&buf[..n]) };
                let mut kept = tail.lock().expect("tail lock");
                kept.extend_from_slice(&buf[..n]);
                let excess = kept.len().saturating_sub(4 * MAX_SUMMARY);
                kept.drain(..excess);
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
            signal(libc::SIGKILL);
        }
        std::thread::sleep(POLL);
    };

    if own_group {
        stop_group(pid);
    }
    // The agent and everything it started are gone: a lock it left behind is stale.
    let _ = std::fs::remove_file(ws.dir().join("git/index.lock"));
    // Children the agent left running may hold the pipe open; do not wait for them.
    let _ = done_rx.recv_timeout(Duration::from_secs(2));
    let reported = std::fs::read_to_string(&summary_file).ok().filter(|s| !s.trim().is_empty());
    let summary =
        account(&reported.unwrap_or_else(|| String::from_utf8_lossy(&tail.lock().expect("tail lock")).into_owned()));
    let change = change::record(repo, &ws.id, &Record { summary, ..Default::default() })?;
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
