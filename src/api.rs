//! The operations every interface (CLI, MCP, TUI) exposes, with one
//! serialisable result shape each.

use crate::accept::{self, Status};
use crate::change::{self, Change, Record};
use crate::evidence::{self, Evidence};
use crate::footprint;
use crate::git::{Oid, Repo};
use crate::resource::Resource;
use crate::workspace::{self, Workspace};
use crate::Result;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Serialize)]
pub struct ChangeRow {
    #[serde(flatten)]
    pub change: Change,
    #[serde(flatten)]
    pub status: Status,
}

#[derive(Debug, Serialize)]
pub struct WorkspaceRow {
    #[serde(flatten)]
    pub workspace: Workspace,
    /// Holds edits not yet recorded.
    pub dirty: bool,
    /// Whether the owning `zit run` process still exists, when there is one.
    pub alive: Option<bool>,
    /// What it has written so far, derived live from its files.
    pub writes: Vec<Resource>,
    /// What it has claimed for writing.
    pub claims: Vec<Resource>,
    /// Where its writes or claims collide with other unaccepted work.
    pub overlaps: Vec<Overlap>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Overlap {
    pub resource: Resource,
    /// The agent on the other side.
    pub with: String,
    /// Its workspace id or change id.
    pub holder: String,
}

/// Run `f` over `items` on a few threads, keeping order.
pub(crate) fn parallel<T: Sync, R: Send>(items: &[T], f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    let workers = std::thread::available_parallelism().map_or(4, |n| n.get()).min(items.len().max(1));
    let chunk = items.len().div_ceil(workers).max(1);
    std::thread::scope(|scope| {
        let handles: Vec<_> =
            items.chunks(chunk).map(|part| scope.spawn(|| part.iter().map(&f).collect::<Vec<_>>())).collect();
        handles.into_iter().flat_map(|h| h.join().expect("worker panicked")).collect()
    })
}

/// Everything that exists: the current state, every speculative change,
/// every live workspace.
#[derive(Debug, Serialize)]
pub struct Overview {
    pub current: Change,
    pub changes: Vec<ChangeRow>,
    pub workspaces: Vec<WorkspaceRow>,
}

/// A status derived earlier, with everything it was derived from. Status is
/// a pure function of (change, current, evidence), so under the same key the
/// answer is the same; a different key is simply not an answer.
#[derive(Serialize, Deserialize)]
struct Memo {
    current: Oid,
    evidence: String,
    status: Status,
}

/// The status of `change` against `current`, reusing the memo of an earlier
/// listing when it was derived from the same current and evidence.
fn memoised_status(repo: &Repo, change: &Change, current: &Oid, evidence: &str) -> Status {
    let path = repo.home().join("cache/status").join(change.id.as_str());
    if let Ok(memo) = std::fs::read(&path).and_then(|bytes| Ok(serde_json::from_slice::<Memo>(&bytes)?)) {
        if memo.current == *current && memo.evidence == evidence {
            return memo.status;
        }
    }
    let status = accept::evaluate(repo, change, current);
    // A failure to evaluate may be transient (a lock, git dying); ask again next time.
    if !matches!(status, Status::Invalid(accept::Invalid::Error(_))) {
        let memo = Memo { current: current.clone(), evidence: evidence.to_string(), status: status.clone() };
        if let (Some(dir), Ok(bytes)) = (path.parent(), serde_json::to_vec(&memo)) {
            // Written whole, then renamed: a concurrent listing never reads half a memo.
            let tmp = dir.join(format!("{}.{}", change.id, workspace::fresh_id()));
            if std::fs::create_dir_all(dir).is_ok() && std::fs::write(&tmp, bytes).is_ok() {
                let _ = std::fs::rename(&tmp, &path);
            }
        }
    }
    status
}

pub fn overview(repo: &Repo) -> Result<Overview> {
    // One git call: the ref resolves and loads at once, or is not there.
    let current = change::load_revs(repo, &[crate::git::CURRENT])
        .ok()
        .and_then(|mut found| found.pop())
        .ok_or(crate::Error::NotInitialised)?;
    accept::prune_accepted(repo)?;
    let speculative = change::speculative(repo)?;
    let evidence = match speculative.is_empty() {
        true => String::new(),
        false => evidence::fingerprint(repo)?,
    };
    let statuses = parallel(&speculative, |c| memoised_status(repo, c, &current.id, &evidence));
    let changes: Vec<ChangeRow> =
        speculative.into_iter().zip(statuses).map(|(change, status)| ChangeRow { change, status }).collect();

    let spaces = workspace::list(repo)?;
    let window = workspace::AWARENESS_WINDOW;
    let writing = parallel(&spaces, |ws| workspace::in_flight_within(repo, ws, window).unwrap_or_default());

    // Everything unaccepted that touches a resource: (resource, agent, holder id).
    let mut touching: Vec<(Resource, String, String)> = Vec::new();
    for (ws, writes) in spaces.iter().zip(&writing) {
        let mut all: Vec<Resource> = writes.iter().cloned().chain(ws.claims()).collect();
        all.sort();
        all.dedup();
        touching.extend(all.into_iter().map(|r| (r, ws.agent.clone(), ws.id.clone())));
    }
    for row in &changes {
        if let Some(parent) = row.change.parents.first() {
            let writes = footprint::of_change(repo, parent, &row.change.id, &row.change.reads)?.writes;
            touching.extend(writes.into_iter().map(|r| (r, row.change.agent.clone(), row.change.id.to_string())));
        }
    }

    let workspaces = spaces
        .into_iter()
        .zip(writing)
        .map(|(workspace, writes)| {
            let alive = workspace.pid.map(|_| workspace::alive(&workspace));
            let claims = workspace.claims();
            let mut overlaps: Vec<Overlap> = touching
                .iter()
                .filter(|(_, _, holder)| *holder != workspace.id)
                .filter(|(theirs, _, _)| writes.iter().chain(&claims).any(|mine| mine.held_with(theirs)))
                .map(|(resource, with, holder)| Overlap {
                    resource: resource.clone(),
                    with: with.clone(),
                    holder: holder.clone(),
                })
                .collect();
            overlaps.sort_by(|a, b| (&a.resource, &a.holder).cmp(&(&b.resource, &b.holder)));
            overlaps.dedup();
            let writes: Vec<Resource> = writes.into_iter().collect();
            WorkspaceRow { dirty: !writes.is_empty(), workspace, alive, writes, claims, overlaps }
        })
        .collect();
    Ok(Overview { current, changes, workspaces })
}

#[derive(Debug, Serialize)]
pub struct CheckResult {
    pub check: String,
    pub evidence: Option<Evidence>,
}

#[derive(Debug, Serialize)]
pub struct Detail {
    #[serde(flatten)]
    pub change: Change,
    #[serde(flatten)]
    pub status: Status,
    /// What the change wrote, relative to its first parent.
    pub writes: Vec<Resource>,
    /// Files the change moved, old path to new path.
    pub renames: BTreeMap<String, String>,
    pub evidence: Vec<CheckResult>,
}

/// What changed, why, what it depends on, and the evidence for it.
pub fn detail(repo: &Repo, id: &Oid) -> Result<Detail> {
    let change = change::load(repo, id)?;
    let (writes, renames) = match change.parents.first() {
        Some(parent) => {
            let fp = footprint::between(repo, parent, id)?;
            (fp.writes.into_iter().collect(), fp.renames)
        }
        None => (vec![], BTreeMap::new()),
    };
    let evidence = evidence::lookup(repo, &change.state)?
        .into_iter()
        .map(|(check, evidence)| CheckResult { check: check.name, evidence })
        .collect();
    Ok(Detail { status: accept::status(repo, id)?, change, writes, renames, evidence })
}

/// What a run of changes cost altogether, from their `Zit-Tokens` and
/// `Zit-Cost-USD` trailers; changes without them count as zero.
#[derive(Debug, Default, PartialEq, Serialize)]
pub struct Totals {
    pub changes: usize,
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// `null` when no change reported a price.
    pub cost_usd: Option<f64>,
}

impl Totals {
    pub fn of<'a>(changes: impl IntoIterator<Item = &'a Change>) -> Totals {
        changes.into_iter().fold(Totals::default(), |mut t, c| {
            t.changes += 1;
            if let Some(u) = &c.usage {
                t.input_tokens += u.input_tokens;
                t.output_tokens += u.output_tokens;
                if let Some(cost) = u.cost_usd {
                    t.cost_usd = Some(t.cost_usd.unwrap_or(0.0) + cost);
                }
            }
            t
        })
    }
}

/// Accepted history from current backwards, with what it cost.
#[derive(Debug, Serialize)]
pub struct Log {
    /// Newest first.
    pub changes: Vec<Change>,
    /// Over the changes listed.
    pub totals: Totals,
}

/// The accepted line, `limit` changes at most (all of it when `None`).
pub fn log(repo: &Repo, limit: Option<usize>) -> Result<Log> {
    // git reads -n as an int; larger values are "not an integer".
    let changes = change::accepted(repo, limit.unwrap_or(i32::MAX as usize))?;
    Ok(Log { totals: Totals::of(&changes), changes })
}

/// A change's diff, as `git diff` prints it.
#[derive(Debug, Serialize)]
pub struct Diff {
    pub change: Oid,
    /// What it is compared with: its base, or the change asked for. `null`
    /// for a change with no parent, which is shown whole.
    pub against: Option<Oid>,
    pub diff: String,
}

/// The diff of `id` against `against`, or its first parent. `stat` asks for
/// the summary of changed files instead of the patch.
pub fn diff(repo: &Repo, id: &Oid, against: Option<&Oid>, stat: bool) -> Result<Diff> {
    let against = match against {
        Some(other) => Some(other.clone()),
        None => change::load(repo, id)?.parents.first().cloned(),
    };
    let summary: &[&str] = if stat { &["--stat"] } else { &[] };
    let diff = match &against {
        Some(from) => repo.git(&[&["diff", "--no-color"], summary, &[from.as_str(), id.as_str()]].concat())?,
        None => repo.git(&[&["show", "--no-color", "--format="], summary, &[id.as_str()]].concat())?,
    };
    Ok(Diff { change: id.clone(), against, diff })
}

/// One line of `zit doctor`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Probe {
    pub name: String,
    pub status: Health,
    pub detail: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Health {
    Pass,
    /// Works, with less than it could: no copy-on-write, an agent not installed.
    Warn,
    /// Zit cannot do its job here until this is fixed.
    Fail,
}

#[derive(Debug, Serialize)]
pub struct Doctor {
    pub checks: Vec<Probe>,
}

impl Doctor {
    pub fn failed(&self) -> bool {
        self.checks.iter().any(|c| c.status == Health::Fail)
    }
}

/// Can Zit work here? Git, its home, the file system, the repository, the
/// checks it declares, the agents on PATH. Runs outside a repository too.
pub fn doctor(cwd: &std::path::Path) -> Doctor {
    use std::path::PathBuf;
    let mut checks = Vec::new();
    let mut probe = |name: &str, status: Health, detail: String| {
        checks.push(Probe { name: name.into(), status, detail });
    };

    let version = crate::git::git_command().arg("--version").output().ok();
    let version = version.map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default();
    match crate::git::require_git() {
        Ok(()) => probe("git", Health::Pass, version),
        Err(e) => probe("git", Health::Fail, e.to_string()),
    }

    let repo = Repo::discover(cwd);
    let home = match &repo {
        Ok(repo) => repo.home().to_path_buf(),
        Err(_) => std::env::var_os("ZIT_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".zit")))
            .unwrap_or_else(|| PathBuf::from(".zit")),
    };
    let writable = std::fs::create_dir_all(&home).and_then(|()| {
        let probe = home.join(format!(".zit-doctor-{}", std::process::id()));
        std::fs::write(&probe, b"zit")?;
        std::fs::remove_file(probe)
    });
    match writable {
        Ok(()) => probe("home", Health::Pass, format!("{} (writable)", home.display())),
        Err(e) => probe("home", Health::Fail, format!("{}: {e}", home.display())),
    }
    let (fs_name, free) = file_system(&home);
    let cow = home.is_dir() && workspace::copy_on_write(&home);
    probe(
        "copy-on-write",
        if cow { Health::Pass } else { Health::Warn },
        match cow {
            true => format!("{fs_name}: workspaces are clones"),
            false => format!("{fs_name}: no clonefile/FICLONE, workspaces are plain checkouts"),
        },
    );
    const GB: u64 = 1_000_000_000;
    probe(
        "free space",
        if free >= GB { Health::Pass } else { Health::Warn },
        format!("{:.1} GB free on the volume holding {}", free as f64 / GB as f64, home.display()),
    );

    let current = match &repo {
        Err(e) => Err(e.to_string()),
        Ok(repo) => match repo.current() {
            Err(e) => Err(e.to_string()),
            // By name, so git verifies the object is there.
            Ok(id) => {
                repo.resolve("current").map_err(|_| format!("{} points at a missing commit {id}", crate::git::CURRENT))
            }
        },
    };
    match (&repo, &current) {
        (Ok(_), Ok(id)) => probe("repository", Health::Pass, format!("current {}", id.short())),
        (_, Err(why)) => probe("repository", Health::Fail, why.clone()),
        (Err(e), _) => probe("repository", Health::Fail, e.to_string()),
    }
    if let (Ok(repo), Ok(current)) = (&repo, &current) {
        let (status, detail) = match config_programs(repo, current) {
            Err(e) => (Health::Fail, e.to_string()),
            Ok(None) => (Health::Pass, "none".into()),
            Ok(Some((summary, missing))) if missing.is_empty() => (Health::Pass, summary),
            Ok(Some((_, missing))) => (Health::Fail, missing.join("; ")),
        };
        probe("zit.toml", status, detail);
    }

    for agent in ["autohand", "claude", "codex", "pi"] {
        let (status, detail) = match on_path(agent).and_then(|p| version_of(&p)) {
            Some(version) => (Health::Pass, version),
            None => (Health::Warn, format!("not on PATH; `zit run --agent {agent}` needs it")),
        };
        probe(&format!("agent {agent}"), status, detail);
    }
    let gh = std::env::var_os("ZIT_GH").map(PathBuf::from).or_else(|| on_path("gh"));
    match gh.and_then(|p| version_of(&p)) {
        Some(version) => probe("gh", Health::Pass, version),
        None => probe("gh", Health::Warn, "not on PATH; `zit export --pr` needs it".into()),
    }

    // Every grammar built in: an empty file parses when its language loads.
    let languages = [
        ("cs", "c#"),
        ("go", "go"),
        ("java", "java"),
        ("js", "javascript"),
        ("py", "python"),
        ("rb", "ruby"),
        ("rs", "rust"),
        ("ts", "typescript"),
    ];
    let built: Vec<&str> = languages
        .iter()
        .filter(|(ext, _)| crate::symbols::index(&format!("probe.{ext}"), b"").is_some())
        .map(|(_, name)| *name)
        .collect();
    let all = built.len() == languages.len();
    probe("languages", if all { Health::Pass } else { Health::Fail }, built.join(", "));

    Doctor { checks }
}

/// Programs declared by the state's zit.toml that are missing from PATH.
/// `None` when the state has no zit.toml.
fn config_programs(repo: &Repo, state: &Oid) -> Result<Option<(String, Vec<String>)>> {
    if repo.objects()?.read(&format!("{state}:zit.toml"))?.is_none() {
        return Ok(None);
    }
    let checks = evidence::checks(repo, state)?;
    let derived = evidence::derived(repo, state)?;
    let prepare = evidence::prepare(repo, state)?;
    let mut commands: Vec<(String, &str)> =
        checks.iter().map(|c| (format!("check {}", c.name), c.run.as_str())).collect();
    commands.extend(derived.iter().map(|d| (format!("derive {}", d.path), d.run.as_str())));
    commands.extend(prepare.iter().map(|p| ("prepare".to_string(), p.run.as_str())));
    let missing = commands
        .iter()
        .filter_map(|(what, run)| {
            let program = program_of(run)?;
            (!on_path_or_builtin(&program)).then(|| format!("`{program}` ({what}) is not on PATH"))
        })
        .collect();
    let summary = format!(
        "{} check{}, {} derived, prepare: {}",
        checks.len(),
        if checks.len() == 1 { "" } else { "s" },
        derived.len(),
        if prepare.is_some() { "yes" } else { "no" }
    );
    Ok(Some((summary, missing)))
}

/// The program a shell command line starts with: past `VAR=value`
/// assignments; `None` for an empty line or one that starts with a keyword.
fn program_of(run: &str) -> Option<String> {
    let first = run.lines().find(|l| !l.trim().is_empty())?;
    first
        .split_whitespace()
        .find(|word| !word.split_once('=').is_some_and(|(k, _)| k.chars().all(|c| c.is_alphanumeric() || c == '_')))
        .map(str::to_string)
}

/// Shell builtins and keywords `sh -c` runs without a file on PATH.
fn on_path_or_builtin(program: &str) -> bool {
    const BUILTIN: &[&str] = &[
        "test", "[", "true", "false", "cd", "echo", "exit", ":", ".", "source", "set", "export", "if", "for", "while",
        "case", "!", "{", "(", "eval", "exec", "command", "printf", "read", "trap", "wait", "umask", "ulimit",
    ];
    // A path into the checked-out state cannot be verified from here.
    BUILTIN.contains(&program) || program.contains('/') || on_path(program).is_some()
}

fn on_path(program: &str) -> Option<std::path::PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|dir| dir.join(program))
        .find(|p| p.metadata().is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0))
}

/// The first line `program --version` prints, within five seconds.
fn version_of(program: &std::path::Path) -> Option<String> {
    use std::process::{Command, Stdio};
    let mut child = Command::new(program)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;
    let started = std::time::Instant::now();
    while child.try_wait().ok()?.is_none() {
        if started.elapsed() > std::time::Duration::from_secs(5) {
            let _ = child.kill();
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let out = child.wait_with_output().ok()?;
    let text = [out.stdout, out.stderr].concat();
    String::from_utf8_lossy(&text).lines().map(str::trim).find(|l| !l.is_empty()).map(str::to_string)
}

fn wide<T: TryInto<u64>>(n: T) -> u64 {
    n.try_into().unwrap_or(0)
}

/// The file system type holding `dir`, and the bytes free on it.
#[cfg(target_os = "macos")]
fn file_system(dir: &std::path::Path) -> (String, u64) {
    use std::os::unix::ffi::OsStrExt;
    let Ok(path) = std::ffi::CString::new(dir.as_os_str().as_bytes()) else { return ("unknown".into(), 0) };
    let mut fs: libc::statfs = unsafe { std::mem::zeroed() };
    // SAFETY: `path` is NUL-terminated and `fs` is a zeroed statfs for the call to fill.
    if unsafe { libc::statfs(path.as_ptr(), &mut fs) } != 0 {
        return ("unknown".into(), 0);
    }
    // SAFETY: f_fstypename is a NUL-terminated name within the struct.
    let name = unsafe { std::ffi::CStr::from_ptr(fs.f_fstypename.as_ptr()) }.to_string_lossy().into_owned();
    (name, wide(fs.f_bavail) * wide(fs.f_bsize))
}

#[cfg(target_os = "linux")]
fn file_system(dir: &std::path::Path) -> (String, u64) {
    use std::os::unix::ffi::OsStrExt;
    let Ok(path) = std::ffi::CString::new(dir.as_os_str().as_bytes()) else { return ("unknown".into(), 0) };
    let mut fs: libc::statfs = unsafe { std::mem::zeroed() };
    // SAFETY: `path` is NUL-terminated and `fs` is a zeroed statfs for the call to fill.
    if unsafe { libc::statfs(path.as_ptr(), &mut fs) } != 0 {
        return ("unknown".into(), 0);
    }
    // Magic numbers from linux/magic.h.
    let name = match wide(fs.f_type) {
        0x9123683E => "btrfs".to_string(),
        0x58465342 => "xfs".to_string(),
        0xEF53 => "ext4".to_string(),
        0x01021994 => "tmpfs".to_string(),
        0x794C7630 => "overlayfs".to_string(),
        0x2FC12FC1 => "zfs".to_string(),
        0xF2F52010 => "f2fs".to_string(),
        0x6969 => "nfs".to_string(),
        0x65735546 => "fuse".to_string(),
        0x9FA0 => "procfs".to_string(),
        other => format!("fs type {other:#x}"),
    };
    (name, wide(fs.f_bavail) * wide(fs.f_bsize))
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn file_system(_dir: &std::path::Path) -> (String, u64) {
    ("unknown".into(), 0)
}

#[derive(Debug, Serialize)]
pub struct Recorded {
    /// `null` when the workspace held no edits.
    pub change: Option<Change>,
    pub writes: Vec<Resource>,
}

/// Record a workspace and report what the new change wrote.
pub fn record(repo: &Repo, workspace: &str, opts: &Record, dispose: bool) -> Result<Recorded> {
    let base = workspace::get(repo, workspace)?.base;
    let change = change::record(repo, workspace, opts)?;
    if dispose {
        workspace::dispose(repo, workspace)?;
    }
    let writes = match &change {
        Some(c) => footprint::of_change(repo, &base, &c.id, &c.reads)?.writes.into_iter().collect(),
        None => vec![],
    };
    Ok(Recorded { change, writes })
}
