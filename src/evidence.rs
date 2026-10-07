//! Evidence: content-addressed results of checks run against a state.
//! A result is keyed by the check and the content of its declared inputs,
//! so it is reused by every state that shares those inputs.

use crate::git::{Oid, Repo, EVIDENCE};
use crate::workspace::{self, NewWorkspace, Workspace};
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::process::{Command, Stdio};
use std::time::Instant;

/// Declared in `zit.toml` at the root of the state being checked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Check {
    pub name: String,
    /// Shell command, run at the root of a materialised view of the state.
    pub run: String,
    /// Paths the result depends on. Empty means the whole state.
    #[serde(default)]
    pub inputs: Vec<String>,
    /// Seconds before the check is stopped and counted as failed. Default 1800.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<u64>,
}

/// How long a check may run when `zit.toml` does not say.
pub const DEFAULT_CHECK_TIMEOUT: u64 = 1800;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Evidence {
    pub check: String,
    pub key: String,
    /// The state the check actually ran against.
    pub state: Oid,
    pub passed: bool,
    pub exit_code: i32,
    pub duration_ms: u64,
    /// Tail of the combined output.
    pub output: String,
    pub at: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Verdict {
    pub evidence: Evidence,
    /// Reused from an earlier run with identical inputs.
    pub cached: bool,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub(crate) struct Config {
    #[serde(default)]
    check: Vec<Check>,
    #[serde(default)]
    derive: Vec<Derive>,
    /// Extra gitignore patterns for files agents and tools leave behind.
    #[serde(default)]
    ignore: Vec<String>,
    #[serde(default)]
    prepare: Option<Prepare>,
    #[serde(default)]
    accept: AcceptRules,
}

/// `[accept]` in zit.toml: rules for how changes land.
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct AcceptRules {
    /// Compose onto current as one commit, never a merge commit.
    #[serde(default)]
    linear: bool,
}

/// Whether the state at `rev` asks for linear history.
pub fn linear(repo: &Repo, rev: &Oid) -> Result<bool> {
    Ok(config(repo, rev)?.accept.linear)
}

/// How to install dependencies into a fresh checkout (`npm ci`, `uv sync`,
/// `cargo fetch`, …). Run once per distinct `inputs`, in the cached
/// checkout every workspace is cloned from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Prepare {
    pub run: String,
    /// Paths the installed result depends on, usually the manifest and lockfile.
    #[serde(default)]
    pub inputs: Vec<String>,
}

/// A generated file, declared in `zit.toml` with the command that produces
/// it. Its content is a function of other files, so concurrent writes to it
/// are not conflicts: it is regenerated on the composed state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Derive {
    pub path: String,
    pub run: String,
}

const OUTPUT_TAIL: usize = 4000;

fn parse_config(text: &[u8]) -> Result<Config> {
    let text = std::str::from_utf8(text).map_err(|e| Error::msg(format!("zit.toml: {e}")))?;
    toml::from_str(text).map_err(|e| Error::msg(format!("zit.toml: {e}")))
}

fn config(repo: &Repo, rev: &Oid) -> Result<Config> {
    match repo.objects()?.read(&format!("{rev}:zit.toml"))? {
        Some(text) => parse_config(&text),
        None => Ok(Config::default()),
    }
}

/// The checks a state (or change) asks for.
pub fn checks(repo: &Repo, state: &Oid) -> Result<Vec<Check>> {
    Ok(config(repo, state)?.check)
}

/// The dependency install step a state declares, if any.
pub fn prepare(repo: &Repo, rev: &Oid) -> Result<Option<Prepare>> {
    Ok(config(repo, rev)?.prepare)
}

/// Identifies an install: the command and the content of its inputs.
pub fn prepare_key(repo: &Repo, rev: &Oid, prepare: &Prepare) -> Result<String> {
    let as_check =
        Check { name: "prepare".into(), run: prepare.run.clone(), inputs: prepare.inputs.clone(), timeout: None };
    Ok(keys(repo, rev, std::slice::from_ref(&as_check))?.remove(0))
}

/// The extra ignore patterns a state (or change) declares.
pub fn ignored(repo: &Repo, rev: &Oid) -> Result<Vec<String>> {
    Ok(config(repo, rev)?.ignore)
}

/// The generated files a state (or change) declares.
pub fn derived(repo: &Repo, rev: &Oid) -> Result<Vec<Derive>> {
    Ok(config(repo, rev)?.derive)
}

/// Regenerate `rules` in a view of `change` and return the resulting state.
/// Only the declared paths are taken from the view.
pub(crate) fn regenerate(
    repo: &Repo,
    change: &Oid,
    state: &Oid,
    rules: &[Derive],
) -> Result<std::result::Result<Oid, String>> {
    let view = View::acquire(repo, change, state)?;
    let workspace = &view.workspace;
    for rule in rules {
        let out = Command::new("sh")
            .arg("-c")
            .arg(format!("exec 2>&1\n{}", rule.run))
            .current_dir(workspace.path())
            .env("ZIT_CACHE_DIR", repo.cache_dir()?)
            .env("TMPDIR", workspace.temp_dir()?)
            .stdin(Stdio::null())
            .output()?;
        if !out.status.success() {
            return Ok(Err(format!("derive {}", rule.path)));
        }
    }
    let paths: Vec<&str> = rules.iter().map(|r| r.path.as_str()).collect();
    crate::git::run(workspace.git().args(["add", "-A", "--"]).args(&paths))?;
    Ok(Ok(Oid::new(crate::git::run(workspace.git().args(["write-tree"]))?)))
}

/// Cache keys: each check plus the content address of each of its inputs.
fn keys(repo: &Repo, state: &Oid, checks: &[Check]) -> Result<Vec<String>> {
    let normalised: Vec<Vec<String>> = checks
        .iter()
        .map(|check| {
            let mut inputs = check.inputs.iter().map(|i| normalise(i)).collect::<Result<Vec<_>>>()?;
            inputs.sort_unstable();
            Ok(inputs)
        })
        .collect::<Result<_>>()?;

    let paths: std::collections::BTreeSet<&String> = normalised.iter().flatten().filter(|p| !p.is_empty()).collect();
    let mut address = std::collections::BTreeMap::new();
    if !paths.is_empty() {
        let mut args = vec!["ls-tree".to_string(), "-z".to_string(), state.to_string(), "--".to_string()];
        args.extend(paths.into_iter().cloned());
        for entry in repo.git(&args)?.split('\0') {
            if let Some((meta, path)) = entry.split_once('\t') {
                address.insert(path.to_string(), meta.to_string());
            }
        }
    }
    Ok(checks
        .iter()
        .zip(normalised)
        .map(|(check, inputs)| {
            let mut material = format!("{}\0{}\0", check.name, check.run);
            if inputs.is_empty() {
                material.push_str(state.as_str());
            }
            for input in inputs {
                // The root is the state itself; an absent input is itself distinguishing.
                let meta =
                    if input.is_empty() { state.as_str() } else { address.get(&input).map_or("-", String::as_str) };
                material.push_str(&format!("{input}\0{meta}\0"));
            }
            crate::hash(material.as_bytes())
        })
        .collect())
}

/// An input path exactly as git prints it: `./ui/` -> `ui`. The root is "".
fn normalise(input: &str) -> Result<String> {
    let parts: Vec<&str> = input.split('/').filter(|p| !p.is_empty() && *p != ".").collect();
    if parts.contains(&"..") {
        return Err(Error::msg(format!("zit.toml: input `{input}` is outside the state")));
    }
    // `ls-tree` takes paths, not patterns: a glob would address nothing and
    // the evidence would never expire.
    if input.contains(['*', '?']) {
        return Err(Error::msg(format!("zit.toml: input `{input}` is a pattern; inputs are paths")));
    }
    Ok(parts.join("/"))
}

/// Checks of a state with their keys and any existing evidence, in two git calls.
/// `governing` is a trusted revision (current, at accept) whose checks apply too,
/// so a change cannot remove or weaken the gate it is judged by.
fn lookup_keyed(repo: &Repo, state: &Oid, governing: Option<&Oid>) -> Result<Vec<(Check, String, Option<Evidence>)>> {
    let mut objects = repo.objects()?;
    let mut checks = Vec::new();
    for rev in governing.into_iter().chain([state]) {
        if let Some(text) = objects.read(&format!("{rev}:zit.toml"))? {
            for check in parse_config(&text)?.check {
                if !checks.contains(&check) {
                    checks.push(check);
                }
            }
        }
    }
    if checks.is_empty() {
        return Ok(vec![]);
    }
    let keys = keys(repo, state, &checks)?;
    let trust_fetched = repo.git(&["config", "--bool", "zit.trustFetchedEvidence"]).is_ok_and(|v| v.trim() == "true");
    checks
        .into_iter()
        .zip(keys)
        .map(|(check, key)| {
            // Evidence from elsewhere is trusted only when this machine is told to.
            let trusted = trust_fetched || produced_here(repo, &key);
            let found = match trusted {
                true => {
                    objects.read(&format!("{EVIDENCE}/{key}"))?.and_then(|bytes| serde_json::from_slice(&bytes).ok())
                }
                false => None,
            };
            Ok((check, key, found))
        })
        .collect()
}

/// Existing evidence for each check of a state. Runs nothing.
pub fn lookup(repo: &Repo, state: &Oid) -> Result<Vec<(Check, Option<Evidence>)>> {
    Ok(lookup_keyed(repo, state, None)?.into_iter().map(|(check, _, found)| (check, found)).collect())
}

/// This clone's record of the evidence it produced. Evidence refs travel
/// with the graph and anyone who can push refs can write one; the ledger is
/// what says "I ran this".
fn ledger(repo: &Repo) -> std::path::PathBuf {
    // In the clone's own git directory: never pushed, and kept by `zit clean`.
    repo.git_dir().join("zit/evidence-produced")
}

fn produced_here(repo: &Repo, key: &str) -> bool {
    ledger(repo).join(key).exists()
}

fn store(repo: &Repo, fresh: &[&Evidence]) -> Result<()> {
    let dir = ledger(repo);
    std::fs::create_dir_all(&dir)?;
    for evidence in fresh {
        std::fs::write(dir.join(&evidence.key), evidence.state.as_str())?;
    }
    let mut updates = Vec::new();
    let mut objects = repo.objects()?;
    for evidence in fresh {
        // Already recorded (fetched) with the same verdict: keep that object, so
        // machines that agree never fight over the ref.
        let existing: Option<Evidence> =
            objects.read(&format!("{EVIDENCE}/{}", evidence.key))?.and_then(|b| serde_json::from_slice(&b).ok());
        if existing.is_some_and(|e| e.passed == evidence.passed && e.exit_code == evidence.exit_code) {
            continue;
        }
        let blob = repo.git_stdin(&["hash-object", "-w", "--stdin"], &[], &serde_json::to_string(evidence)?)?;
        updates.push(format!("update {EVIDENCE}/{} {blob}", evidence.key));
    }
    if !updates.is_empty() && !repo.transaction(&updates)? {
        return Err(Error::msg("could not store evidence"));
    }
    Ok(())
}

fn execute(repo: &Repo, check: &Check, key: String, state: &Oid, change: &Oid, view: &Workspace) -> Result<Evidence> {
    use std::os::unix::process::CommandExt;
    let started = Instant::now();
    let limit = std::time::Duration::from_secs(check.timeout.unwrap_or(DEFAULT_CHECK_TIMEOUT));
    let mut child = Command::new("sh")
        .arg("-c")
        .arg(format!("exec 2>&1\n{}", check.run))
        .current_dir(view.path())
        .env("ZIT_CACHE_DIR", repo.cache_dir()?)
        .env("TMPDIR", view.temp_dir()?)
        .env("ZIT_CHANGE", change.as_str())
        .env("ZIT_STATE", state.as_str())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .process_group(0)
        .spawn()?;
    // Keep only the end of the output, however much there is.
    let mut out = child.stdout.take().expect("piped");
    let reader = std::thread::spawn(move || {
        use std::io::Read;
        let (mut kept, mut buf) = (Vec::new(), [0u8; 8192]);
        while let Ok(n @ 1..) = out.read(&mut buf) {
            kept.extend_from_slice(&buf[..n]);
            let excess = kept.len().saturating_sub(2 * OUTPUT_TAIL);
            kept.drain(..excess);
        }
        kept
    });
    let group = -(child.id() as i32);
    let mut timed_out = false;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if started.elapsed() >= limit {
            timed_out = true;
            // SAFETY: signalling the process group this check leads.
            unsafe { libc::kill(group, libc::SIGKILL) };
            break child.wait()?;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    // Whatever the check started goes with it.
    unsafe { libc::kill(group, libc::SIGKILL) };
    let mut text = String::from_utf8_lossy(&reader.join().unwrap_or_default()).into_owned();
    if timed_out {
        text.push_str(&format!("\nzit: timed out after {}s\n", limit.as_secs()));
    }
    let tail = text.len().saturating_sub(OUTPUT_TAIL);
    let tail = (tail..text.len()).find(|&i| text.is_char_boundary(i)).unwrap_or(text.len());
    Ok(Evidence {
        check: check.name.clone(),
        key,
        state: state.clone(),
        passed: status.success() && !timed_out,
        exit_code: if timed_out { -1 } else { status.code().unwrap_or(-1) },
        duration_ms: started.elapsed().as_millis() as u64,
        output: text[tail..].to_string(),
        at: workspace::now(),
    })
}

/// Produce evidence for every check of a change's state, running only
/// those with no evidence yet (or all of them when `rerun`).
pub fn verify(repo: &Repo, change: &Oid, rerun: bool) -> Result<Vec<Verdict>> {
    verify_state(repo, change, &repo.tree_of(change)?, None, rerun)
}

pub(crate) fn verify_state(
    repo: &Repo,
    change: &Oid,
    state: &Oid,
    governing: Option<&Oid>,
    rerun: bool,
) -> Result<Vec<Verdict>> {
    let mut view: Option<View> = None;
    let mut verdicts = Vec::new();
    let mut run_all = || -> Result<()> {
        for (check, key, found) in lookup_keyed(repo, state, governing)? {
            if let (Some(evidence), false) = (found, rerun) {
                verdicts.push(Verdict { evidence, cached: true });
                continue;
            }
            match &view {
                // A check may have edited the view; the next one sees the state.
                Some(view) => view.restore()?,
                None => view = Some(View::acquire(repo, change, state)?),
            }
            let workspace = &view.as_ref().expect("acquired above").workspace;
            verdicts.push(Verdict { evidence: execute(repo, &check, key, state, change, workspace)?, cached: false });
        }
        Ok(())
    };
    let outcome = run_all();
    // Keep what was learned before anything else can fail. A check that timed out or was
    // killed (exit code -1) says nothing lasting about the state, so it is not kept.
    let fresh: Vec<_> =
        verdicts.iter().filter(|v| !v.cached && v.evidence.exit_code != -1).map(|v| &v.evidence).collect();
    let stored = store(repo, &fresh);
    outcome?;
    stored?;
    Ok(verdicts)
}

/// How many states can be verified at the same time; further ones wait.
const MAX_VIEWS: usize = 8;

/// A verification view: a workspace at a stable path, moved from state to
/// state in place. Only files that differ are rewritten and ignored files
/// are kept, so build tools that key their caches on paths and mtimes stay
/// warm. Untracked files and edits left by an earlier check are removed.
struct View {
    workspace: Workspace,
    /// Held for as long as the view is in use.
    _lock: std::fs::File,
}

impl View {
    fn acquire(repo: &Repo, change: &Oid, state: &Oid) -> Result<View> {
        use std::os::fd::AsRawFd;
        let root = repo.home().join("verify");
        std::fs::create_dir_all(&root)?;
        for slot in 0..MAX_VIEWS {
            let lock = std::fs::OpenOptions::new()
                .create(true)
                .truncate(false)
                .write(true)
                .open(root.join(format!("{slot}.lock")))?;
            // Take the first idle view; when all are busy, wait for the last.
            let wait = if slot + 1 == MAX_VIEWS { 0 } else { libc::LOCK_NB };
            // SAFETY: `lock` is an open descriptor we own for the duration of the call.
            if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | wait) } != 0 {
                continue;
            }
            let dir = root.join(slot.to_string());
            let workspace = match Self::reuse(repo, &dir, change, state) {
                Some(workspace) => workspace,
                None => {
                    let _ = std::fs::remove_dir_all(&dir);
                    let new = NewWorkspace { from: Some(change), intent: "verify", agent: "zit-check", session: None };
                    workspace::materialise_in(
                        repo,
                        &dir,
                        format!("verify{slot}"),
                        change.clone(),
                        state.clone(),
                        &new,
                        false,
                    )?
                }
            };
            // Temp files of an earlier verification are not part of this one.
            let _ = std::fs::remove_dir_all(workspace.temp_dir()?);
            return Ok(View { workspace, _lock: lock });
        }
        unreachable!("the last view is waited for")
    }

    /// Put the view back at its state: tracked edits and untracked files go, ignored files stay.
    fn restore(&self) -> Result<()> {
        crate::git::run(self.workspace.git().args(["reset", "--hard", "--quiet"]))?;
        crate::git::run(self.workspace.git().args(["clean", "-fd", "--quiet"]))?;
        Ok(())
    }

    /// Move an existing view to `change` in place; `None` if there is none or it is damaged.
    /// Also `None` when the state declares different dependencies: a fresh view installs them.
    fn reuse(repo: &Repo, dir: &std::path::Path, change: &Oid, state: &Oid) -> Option<Workspace> {
        let mut workspace = workspace::open(dir).ok()?;
        let installed = |tree: &Oid| -> Option<Option<String>> {
            prepare(repo, tree)
                .ok()?
                .map(|step| prepare_key(repo, tree, &step).ok())
                .map_or(Some(None), |k| k.map(Some))
        };
        if installed(&workspace.base_state)? != installed(state)? {
            return None;
        }
        crate::git::run(workspace.git().args(["reset", "--hard", "--quiet", change.as_str()])).ok()?;
        crate::git::run(workspace.git().args(["clean", "-fd", "--quiet"])).ok()?;
        workspace.base = change.clone();
        workspace.base_state = state.clone();
        workspace.save().ok()?;
        Some(workspace)
    }
}
