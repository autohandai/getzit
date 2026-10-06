//! Materialisation: disposable filesystem views of a state. A workspace is
//! a cache of the graph, never the source of truth.

use crate::git::{git_command, run, Oid, Repo};
use crate::resource::Resource;
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// How a state becomes files on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Strategy {
    /// Copy-on-write clone of a cached checkout (APFS `clonefile`); falls
    /// back to `Checkout` where the filesystem cannot clone.
    Clone,
    /// Plain checkout from the object database.
    Checkout,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Workspace {
    pub id: String,
    /// The change this view was materialised from; the parent of the next record.
    pub base: Oid,
    /// The state `base` results in.
    pub base_state: Oid,
    /// Second parent of the next record, set when composing by hand (`retry`).
    pub merge_parent: Option<Oid>,
    pub intent: String,
    pub agent: String,
    pub session: Option<String>,
    pub created: u64,
    /// Process that owns the view, if any (`zit run`).
    pub pid: Option<u32>,
    path: PathBuf,
}

pub struct NewWorkspace<'a> {
    pub from: Option<&'a Oid>,
    pub intent: &'a str,
    pub agent: &'a str,
    pub session: Option<&'a str>,
}

impl Workspace {
    /// The directory an agent works in.
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn dir(&self) -> &Path {
        self.path.parent().expect("workspace path has a parent")
    }

    /// Git inside the view, trusting size+mtime so cloned files are not
    /// re-hashed, and ignoring tool by-products.
    pub(crate) fn git(&self) -> Command {
        let mut cmd = git_command();
        cmd.current_dir(&self.path).args(["-c", "core.checkStat=minimal", "-c", "core.trustctime=false"]);
        let excludes = self.dir().join("git/zit-ignore");
        if excludes.is_file() {
            cmd.arg("-c").arg(format!("core.excludesFile={}", excludes.display()));
        }
        cmd
    }

    pub(crate) fn save(&self) -> Result<()> {
        fs::write(self.dir().join("git/HEAD"), format!("{}\n", self.base))?;
        // Write then rename: a reader never sees half a file and drops the workspace.
        let tmp = self.dir().join(format!("meta.json.{}", fresh_id()));
        fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        fs::rename(&tmp, self.dir().join("meta.json"))?;
        Ok(())
    }

    pub(crate) fn reads_file(&self) -> PathBuf {
        self.dir().join("reads")
    }

    /// A temp directory of its own, removed with the workspace. Agents and
    /// checks get it as `$TMPDIR` so that concurrent runs on one machine do
    /// not collide on fixed temp-file names.
    pub fn temp_dir(&self) -> Result<PathBuf> {
        let dir = self.dir().join("tmp");
        fs::create_dir_all(&dir)?;
        Ok(dir)
    }

    /// Where an agent may leave its account of the work (`$ZIT_SUMMARY_FILE`).
    pub fn summary_file(&self) -> PathBuf {
        self.dir().join("summary.txt")
    }

    pub(crate) fn claims_file(&self) -> PathBuf {
        self.dir().join("claims")
    }

    /// Resources this workspace has claimed for writing.
    pub fn claims(&self) -> Vec<Resource> {
        let text = fs::read_to_string(self.claims_file()).unwrap_or_default();
        let mut claims: Vec<Resource> = text.lines().filter(|l| !l.is_empty()).map(Resource::parse).collect();
        claims.sort();
        claims.dedup();
        claims
    }

    pub fn declared_reads(&self) -> Vec<Resource> {
        let text = fs::read_to_string(self.reads_file()).unwrap_or_default();
        let mut reads: Vec<Resource> = text.lines().filter(|l| !l.is_empty()).map(Resource::parse).collect();
        reads.sort();
        reads.dedup();
        reads
    }
}

fn root(repo: &Repo) -> PathBuf {
    repo.home().join("ws")
}

/// Does process `pid` exist? A process we are not allowed to signal (another
/// user's, or ours seen from inside a sandbox) still exists.
pub fn process_alive(pid: u32) -> bool {
    // SAFETY: signal 0 only probes; it delivers nothing.
    let probed = unsafe { libc::kill(pid as i32, 0) };
    probed == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

pub(crate) fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

pub(crate) fn fresh_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let seed = format!("{}-{}-{}", std::process::id(), nanos, COUNTER.fetch_add(1, Ordering::Relaxed));
    crate::hash(seed.as_bytes())[..8].to_string()
}

/// Materialise a state (default: current) as a fresh workspace.
pub fn materialise(repo: &Repo, new: &NewWorkspace) -> Result<Workspace> {
    let (base, tree) = repo.resolve_state(new.from.map_or("current", Oid::as_str))?;
    materialise_at(repo, base, tree, new)
}

/// As `materialise`, for a change whose state is already known.
pub(crate) fn materialise_at(repo: &Repo, base: Oid, tree: Oid, new: &NewWorkspace) -> Result<Workspace> {
    fs::create_dir_all(root(repo))?;
    let (id, dir) = loop {
        let id = fresh_id();
        let dir = root(repo).join(&id);
        if !dir.exists() {
            break (id, dir);
        }
    };
    // Only the current state is worth caching: it is what agents fork from.
    materialise_in(repo, &dir, id, base, tree, new, new.from.is_none())
}

/// Build a workspace in `dir`, which must not exist yet.
pub(crate) fn materialise_in(
    repo: &Repo,
    dir: &Path,
    id: String,
    base: Oid,
    tree: Oid,
    new: &NewWorkspace,
    cache_it: bool,
) -> Result<Workspace> {
    // git refuses to record an author without a letter or digit; refuse early.
    if !new.agent.chars().any(char::is_alphanumeric) {
        return Err(Error::msg(format!("agent name `{}` must contain a letter or digit", new.agent)));
    }
    let ws = Workspace {
        id,
        base,
        base_state: tree,
        merge_parent: None,
        intent: new.intent.to_string(),
        agent: new.agent.to_string(),
        session: new.session.map(str::to_string),
        created: now(),
        pid: None,
        path: dir.join("tree"),
    };
    let built = build(repo, &ws, cache_it);
    if built.is_err() {
        // Never leave a half-made directory that no command can see or remove.
        let _ = fs::remove_dir_all(dir);
    }
    built.map(|()| ws)
}

/// Load the workspace that lives in `dir`.
pub(crate) fn open(dir: &Path) -> Result<Workspace> {
    Ok(serde_json::from_slice(&fs::read(dir.join("meta.json"))?)?)
}

fn build(repo: &Repo, ws: &Workspace, cache_it: bool) -> Result<()> {
    let dir = ws.dir();
    populate(repo, &ws.base_state, dir, cache_it)?;

    // An unregistered git view: git works inside, `git worktree list` stays clean.
    let admin = dir.join("git");
    fs::create_dir(&admin)?;
    fs::rename(dir.join("index"), admin.join("index"))?;
    fs::write(admin.join("commondir"), format!("{}\n", repo.git_dir().display()))?;
    fs::write(admin.join("gitdir"), format!("{}\n", dir.join("tree/.git").display()))?;
    fs::write(dir.join("tree/.git"), format!("gitdir: {}\n", admin.display()))?;
    fs::write(admin.join("zit-ignore"), excludes(repo, &ws.base)?)?;
    let _ = fs::remove_file(dir.join("zit-ignore"));
    ws.save()
}

/// Files tools leave behind that are never source.
const BYPRODUCTS: &[&str] = &[
    ".DS_Store",
    "__pycache__/",
    "*.py[cod]",
    ".pytest_cache/",
    ".mypy_cache/",
    ".ruff_cache/",
    "*.swp",
    // Coding agents' own session state, written into the project as they run.
    "/.autohand/memory/",
    "/.autohand/goals.local.json",
    "/.autohand/settings.local.json",
    "/.autohand/session-permissions.json",
    "/.claude/settings.local.json",
];

/// The exclude file for a workspace: the user's own global excludes (which
/// this replaces for git calls made by zit), the built-in by-products, and
/// the project's `ignore` list.
fn excludes(repo: &Repo, base: &Oid) -> Result<String> {
    let global = run(git_command().args(["config", "--path", "--get", "core.excludesFile"]))
        .ok()
        .map(PathBuf::from)
        .or_else(|| {
            let config = std::env::var_os("XDG_CONFIG_HOME")
                .map(PathBuf::from)
                .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
            Some(config.join("git/ignore"))
        });
    let mut text = global.and_then(|p| fs::read_to_string(p).ok()).unwrap_or_default();
    text.push_str("\n# zit: tool by-products\n");
    for pattern in BYPRODUCTS.iter().map(|p| p.to_string()).chain(crate::evidence::ignored(repo, base)?) {
        text.push_str(&pattern);
        text.push('\n');
    }
    Ok(text)
}

/// Fill `dir` with `tree/` (the files) and `index` (git's view of them).
fn populate(repo: &Repo, tree: &Oid, dir: &Path, cache_it: bool) -> Result<()> {
    let trace = std::env::var_os("ZIT_TRACE").is_some();
    let cloned = match repo.strategy {
        Strategy::Clone => match cache::clone_into(repo, tree, dir) {
            Ok(source) => {
                if trace {
                    eprintln!("zit: workspace of {} from the cache: {source:?}", tree.short());
                }
                Some(source)
            }
            Err(e) => {
                if trace {
                    eprintln!("zit: cache clone of {} failed, checking out instead: {e}", tree.short());
                }
                None
            }
        },
        Strategy::Checkout => None,
    };
    if cloned.is_none() {
        let _ = fs::remove_dir_all(dir);
        checkout(repo, tree, dir)?;
    }
    let installed = prepare(repo, tree, dir)?;
    // Keep what was built for the next workspace: a new state of current, a
    // first checkout, or a cached state that has just had dependencies installed.
    let worth_keeping = match cloned {
        Some(cache::Source::Exact) => installed,
        Some(cache::Source::Delta) => cache_it,
        Some(cache::Source::Scratch) => true,
        None => false,
    };
    if worth_keeping {
        cache::publish_and_evict(repo, tree, dir, installed);
    }
    Ok(())
}

/// Install dependencies into `dir` if the state asks for it and `dir` does
/// not already hold that exact install. True if it ran.
fn prepare(repo: &Repo, tree: &Oid, dir: &Path) -> Result<bool> {
    let Some(step) = crate::evidence::prepare(repo, tree)? else {
        return Ok(false);
    };
    let key = crate::evidence::prepare_key(repo, tree, &step)?;
    let marker = dir.join(".zit-prepared");
    let found = fs::read_to_string(&marker).ok();
    if found.as_deref() == Some(key.as_str()) {
        return Ok(false);
    }
    if std::env::var_os("ZIT_TRACE").is_some() {
        eprintln!("zit: installing dependencies for {} (marker {:?}, key {key})", tree.short(), found);
    }
    let tmp = dir.join("tmp");
    fs::create_dir_all(&tmp)?;
    let out = Command::new("sh")
        .arg("-c")
        .arg(format!("exec 2>&1\n{}", step.run))
        .current_dir(dir.join("tree"))
        .env("ZIT_CACHE_DIR", repo.cache_dir()?)
        .env("TMPDIR", &tmp)
        .stdin(std::process::Stdio::null())
        .output()?;
    let tail = |bytes: &[u8]| {
        let text = String::from_utf8_lossy(bytes);
        text.lines().rev().take(5).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n")
    };
    if !out.status.success() {
        return Err(Error::msg(format!("zit.toml prepare step failed ({}):\n{}", out.status, tail(&out.stdout))));
    }
    // An install must not change the code: whatever it creates has to be ignored.
    let excludes_file = dir.join("zit-ignore");
    fs::write(&excludes_file, excludes(repo, tree)?)?;
    let git = |args: &[&str]| {
        run(repo
            .cmd()
            .arg("--work-tree")
            .arg(dir.join("tree"))
            .env("GIT_INDEX_FILE", dir.join("index"))
            .args(["-c", "core.checkStat=minimal", "-c", "core.trustctime=false", "-c"])
            .arg(format!("core.excludesFile={}", excludes_file.display()))
            .args(args))
    };
    // Tracked files it rewrote, and new files no ignore rule covers.
    let _ = git(&["update-index", "-q", "--refresh"]);
    let status =
        [git(&["diff-files", "--name-only"])?, git(&["ls-files", "--others", "--exclude-standard"])?].join("\n");
    let status = status.trim();
    if !status.is_empty() {
        let paths: Vec<&str> = status.lines().take(5).collect();
        return Err(Error::msg(format!(
            "zit.toml prepare step changed files that are not ignored: {}. Add them to .gitignore or to `ignore` in zit.toml, and install without rewriting tracked files (for example `npm ci`, not `npm install`).",
            paths.join(", ")
        )));
    }
    fs::write(marker, key)?;
    Ok(true)
}

fn checkout(repo: &Repo, tree: &Oid, dir: &Path) -> Result<()> {
    fs::create_dir_all(dir.join("tree"))?;
    let git = || {
        let mut cmd = repo.cmd();
        cmd.arg("--work-tree").arg(dir.join("tree")).env("GIT_INDEX_FILE", dir.join("index"));
        cmd
    };
    run(git().args(["read-tree", tree.as_str()]))?;
    run(git().args(["checkout-index", "-a", "-u"]))?;
    Ok(())
}

#[cfg(target_os = "macos")]
fn clonefile(src: &Path, dst: &Path) -> std::io::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let src = CString::new(src.as_os_str().as_bytes())?;
    let dst = CString::new(dst.as_os_str().as_bytes())?;
    // SAFETY: both pointers are valid NUL-terminated paths for the duration of the call.
    match unsafe { libc::clonefile(src.as_ptr(), dst.as_ptr(), 0) } {
        0 => Ok(()),
        _ => Err(std::io::Error::last_os_error()),
    }
}

/// Linux: a directory tree whose files are reflinks (`FICLONE`) of the
/// source's, so they share blocks until written. Tested on btrfs and XFS
/// (reflink=1); other `FICLONE` file systems are untested. Anywhere else the first file fails, the partial
/// copy is removed and the error returned, so callers fall back as they would
/// without copy-on-write.
#[cfg(target_os = "linux")]
fn clonefile(src: &Path, dst: &Path) -> std::io::Result<()> {
    if dst.symlink_metadata().is_ok() {
        return Err(std::io::ErrorKind::AlreadyExists.into());
    }
    let cloned = reflink_tree(src, dst);
    if cloned.is_err() {
        let _ = fs::remove_dir_all(dst).or_else(|_| fs::remove_file(dst));
    }
    cloned
}

#[cfg(target_os = "linux")]
fn reflink_tree(src: &Path, dst: &Path) -> std::io::Result<()> {
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::{symlink, PermissionsExt};
    let meta = src.symlink_metadata()?;
    if meta.file_type().is_symlink() {
        return symlink(fs::read_link(src)?, dst);
    }
    if meta.is_dir() {
        fs::create_dir(dst)?;
        for entry in fs::read_dir(src)? {
            let entry = entry?;
            reflink_tree(&entry.path(), &dst.join(entry.file_name()))?;
        }
        return fs::set_permissions(dst, meta.permissions());
    }
    let from = fs::File::open(src)?;
    let to = fs::OpenOptions::new().write(true).create_new(true).open(dst)?;
    // SAFETY: both descriptors are open for the duration of the call.
    if unsafe { libc::ioctl(to.as_raw_fd(), libc::FICLONE, from.as_raw_fd()) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    to.set_permissions(fs::Permissions::from_mode(meta.permissions().mode()))?;
    // Keep the modification time, as clonefile does: git compares it to decide
    // whether a file changed, so a fresh time makes every file look modified.
    to.set_modified(meta.modified()?)
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn clonefile(_src: &Path, _dst: &Path) -> std::io::Result<()> {
    Err(std::io::ErrorKind::Unsupported.into())
}

/// Whether the file system holding `dir` supports the copy-on-write clones
/// workspaces are made of (APFS on macOS; a `FICLONE` file system such as btrfs on Linux).
pub fn copy_on_write(dir: &Path) -> bool {
    let probe = dir.join(format!(".zit-cow-probe-{}", fresh_id()));
    let (src, dst) = (probe.join("a"), probe.join("b"));
    let supported =
        fs::create_dir_all(&src).is_ok() && fs::write(src.join("f"), b"zit").is_ok() && clonefile(&src, &dst).is_ok();
    let _ = fs::remove_dir_all(&probe);
    supported
}

/// Pristine checkouts by state id, the source of copy-on-write clones.
mod cache {
    use super::*;

    const KEEP: usize = 2;

    #[derive(Debug, PartialEq)]
    pub(super) enum Source {
        /// The exact state was cached.
        Exact,
        /// Cloned another cached state and moved it by touching only the paths that differ.
        Delta,
        /// Nothing cached: checked out from the object database.
        Scratch,
    }

    fn root(repo: &Repo) -> PathBuf {
        repo.home().join("trees")
    }

    /// Cached states, newest first.
    fn entries(repo: &Repo) -> Vec<PathBuf> {
        let mut found: Vec<(SystemTime, PathBuf)> = fs::read_dir(root(repo))
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
            .filter_map(|e| Some((e.metadata().ok()?.created().ok()?, e.path())))
            .collect();
        found.sort_by(|a, b| b.0.cmp(&a.0));
        found.into_iter().map(|(_, p)| p).collect()
    }

    /// Readers clone under a shared lock; eviction needs it exclusively, so
    /// an entry is never deleted while another process is cloning it.
    struct Lock(#[allow(dead_code)] fs::File);

    impl Lock {
        fn acquire(repo: &Repo, operation: libc::c_int) -> Option<Lock> {
            use std::os::fd::AsRawFd;
            let file =
                fs::OpenOptions::new().create(true).truncate(false).write(true).open(root(repo).join(".lock")).ok()?;
            // SAFETY: `file` is an open descriptor we own for the duration of the call.
            (unsafe { libc::flock(file.as_raw_fd(), operation) } == 0).then_some(Lock(file))
        }
    }

    /// Atomically add `src` (a pristine `tree/` + `index`) to the cache as
    /// `tree`. With `replace`, an existing entry for `tree` is swapped out.
    fn publish(repo: &Repo, tree: &Oid, src: &Path, replace: bool) {
        let entry = root(repo).join(tree.as_str());
        let tmp = root(repo).join(format!(".tmp-{}", fresh_id()));
        if clonefile(src, &tmp).is_err() {
            let _ = fs::remove_dir_all(&tmp);
            return;
        }
        if replace && entry.exists() {
            let old = root(repo).join(format!(".tmp-{}", fresh_id()));
            let _ = fs::rename(&entry, &old);
        }
        // Losing a race is fine: another process published the same state.
        if fs::rename(&tmp, &entry).is_err() {
            let _ = fs::remove_dir_all(&tmp);
        }
    }

    pub(super) fn publish_and_evict(repo: &Repo, tree: &Oid, src: &Path, replace: bool) {
        publish(repo, tree, src, replace);
        evict(repo);
    }

    /// Drop all but the newest entries, and leftovers of interrupted
    /// publishes, unless someone is using the cache right now.
    fn evict(repo: &Repo) {
        let Some(_exclusive) = Lock::acquire(repo, libc::LOCK_EX | libc::LOCK_NB) else {
            return;
        };
        for stale in entries(repo).into_iter().skip(KEEP) {
            let _ = fs::remove_dir_all(stale);
        }
        for entry in fs::read_dir(root(repo)).into_iter().flatten().flatten() {
            if entry.file_name().to_string_lossy().starts_with(".tmp-") {
                let _ = fs::remove_dir_all(entry.path());
            }
        }
    }

    /// Make `dir` a copy-on-write clone holding `tree`. Publishing to the
    /// cache is the caller's decision.
    pub(super) fn clone_into(repo: &Repo, tree: &Oid, dir: &Path) -> Result<Source> {
        fs::create_dir_all(root(repo))?;
        let _shared = Lock::acquire(repo, libc::LOCK_SH);
        if clonefile(&root(repo).join(tree.as_str()), dir).is_ok() {
            return Ok(Source::Exact);
        }
        let trace = std::env::var_os("ZIT_TRACE").is_some();
        let seeds = entries(repo);
        if trace {
            eprintln!("zit: no cached checkout of {}; {} seed(s) to clone from", tree.short(), seeds.len());
        }
        for seed in seeds {
            let seed_tree = seed.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_string();
            if let Err(e) = clonefile(&seed, dir) {
                if trace {
                    eprintln!("zit: cloning cached checkout {} failed: {e}", seed.display());
                }
                continue;
            }
            // Ignored files (installed dependencies, build output) come along.
            let moved = run(repo
                .cmd()
                .arg("--work-tree")
                .arg(dir.join("tree"))
                .env("GIT_INDEX_FILE", dir.join("index"))
                .args(["-c", "core.checkStat=minimal", "-c", "core.trustctime=false"])
                .args(["read-tree", "-m", "-u", &seed_tree, tree.as_str()]));
            match moved {
                Ok(_) => return Ok(Source::Delta),
                Err(e) if trace => eprintln!("zit: moving a clone of {seed_tree} to {} failed: {e}", tree.short()),
                Err(_) => {}
            }
            let _ = fs::remove_dir_all(dir);
        }
        // Empty cache: check out once. Where the filesystem cannot clone,
        // publishing fails and this stays a plain checkout.
        checkout(repo, tree, dir)?;
        Ok(Source::Scratch)
    }
}

pub fn get(repo: &Repo, id: &str) -> Result<Workspace> {
    // Ids are generated here; anything else (a path, say) names no workspace.
    if id.is_empty() || !id.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return Err(Error::UnknownWorkspace(id.to_string()));
    }
    let meta = fs::read(root(repo).join(id).join("meta.json")).map_err(|_| Error::UnknownWorkspace(id.to_string()))?;
    Ok(serde_json::from_slice(&meta)?)
}

/// Every live workspace of this repository, oldest first.
pub fn list(repo: &Repo) -> Result<Vec<Workspace>> {
    let mut all: Vec<Workspace> = fs::read_dir(root(repo))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| get(repo, &e.file_name().to_string_lossy()).ok())
        .collect();
    all.sort_by(|a, b| (a.created, &a.id).cmp(&(b.created, &b.id)));
    Ok(all)
}

/// The workspace containing `cwd`, if any.
pub fn containing(repo: &Repo, cwd: &Path) -> Option<Workspace> {
    let cwd = cwd.canonicalize().ok()?;
    let rel = cwd.strip_prefix(root(repo).canonicalize().ok()?).ok()?;
    get(repo, rel.components().next()?.as_os_str().to_str()?).ok()
}

/// Delete the view. Recorded changes are unaffected.
pub fn dispose(repo: &Repo, id: &str) -> Result<()> {
    let ws = get(repo, id)?;
    fs::remove_dir_all(ws.dir())?;
    Ok(())
}

/// Does the view differ from its base state?
pub fn is_dirty(_repo: &Repo, ws: &Workspace) -> Result<bool> {
    Ok(!run(ws.git().args(["status", "--porcelain"]))?.is_empty())
}

/// How long a workspace's live write set is reused before it is recomputed.
pub const AWARENESS_WINDOW: std::time::Duration = std::time::Duration::from_secs(2);

#[derive(Serialize, Deserialize)]
struct Snapshot {
    base: Oid,
    at_ms: u128,
    writes: Vec<Resource>,
}

/// What the workspace is writing, reusing an answer computed less than
/// `max_age` ago from the same base. Cheap enough to ask for every open
/// workspace on every `status` and `claim`.
pub fn in_flight_within(repo: &Repo, ws: &Workspace, max_age: std::time::Duration) -> Result<Vec<Resource>> {
    let file = ws.dir().join("inflight.json");
    let now_ms = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis());
    if let Some(snapshot) = fs::read(&file).ok().and_then(|bytes| serde_json::from_slice::<Snapshot>(&bytes).ok()) {
        if snapshot.base == ws.base && now_ms.saturating_sub(snapshot.at_ms) < max_age.as_millis() {
            return Ok(snapshot.writes);
        }
    }
    let writes: Vec<Resource> = match in_flight(repo, ws) {
        Ok(fp) => fp.writes.into_iter().collect(),
        Err(e) => {
            if std::env::var_os("ZIT_TRACE").is_some() {
                eprintln!("zit-trace workspace {}: cannot see its edits: {e}", ws.id);
            }
            return Err(e);
        }
    };
    let snapshot = Snapshot { base: ws.base.clone(), at_ms: now_ms, writes: writes.clone() };
    // Written whole, then renamed: readers never see half a snapshot.
    let tmp = ws.dir().join(format!("inflight.{}.tmp", fresh_id()));
    if fs::write(&tmp, serde_json::to_vec(&snapshot)?).is_ok() {
        let _ = fs::rename(&tmp, &file);
    }
    Ok(writes)
}

/// What the workspace has written so far, without recording it and without
/// touching the agent's own git index.
pub fn in_flight(repo: &Repo, ws: &Workspace) -> Result<crate::footprint::Footprint> {
    // Outside the git admin directory: sandboxes may protect it (Codex's does).
    let peek = ws.temp_dir()?.join(format!("index.peek-{}", fresh_id()));
    // A plain read and write: sandboxes (Codex's, for one) may refuse the
    // clone and copy-file calls `fs::copy` uses on macOS.
    fs::write(&peek, fs::read(ws.dir().join("git/index"))?)?;
    // Unrecorded drafts are hashed into a throwaway object store, never the repository's,
    // so watching work in progress leaves nothing behind and needs no write access to it.
    let objects = ws.temp_dir()?.join(format!("objects.peek-{}", fresh_id()));
    fs::create_dir_all(&objects)?;
    let real = repo.object_dir()?;
    let snapshot = (|| {
        let git = || {
            let mut cmd = ws.git();
            cmd.env("GIT_INDEX_FILE", &peek)
                .env("GIT_OBJECT_DIRECTORY", &objects)
                .env("GIT_ALTERNATE_OBJECT_DIRECTORIES", &real);
            cmd
        };
        run(git().args(["add", "-A"]))?;
        run(git().args(["write-tree"])).map(Oid::new)
    })();
    let _ = fs::remove_file(&peek);
    let result = snapshot.and_then(|state| match state == ws.base_state {
        true => Ok(Default::default()),
        false => {
            crate::footprint::between_states(&repo.reading_also(&objects), &ws.base_state, &state, &ws.declared_reads())
        }
    });
    let _ = fs::remove_dir_all(&objects);
    result
}

/// Declare resources the agent observed; carried into the next record.
pub fn declare_reads(repo: &Repo, id: &str, reads: &[Resource]) -> Result<()> {
    use std::io::Write;
    let ws = get(repo, id)?;
    let mut file = fs::OpenOptions::new().create(true).append(true).open(ws.reads_file())?;
    for read in reads {
        writeln!(file, "{read}")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use cache::Source;
    use std::fs;

    fn sh(cwd: &Path, script: &str) {
        assert!(Command::new("sh").current_dir(cwd).args(["-c", script]).status().unwrap().success());
    }

    #[test]
    fn states_are_cloned_from_the_cache_exactly_or_by_delta() {
        let dir = tempfile::tempdir().unwrap();
        if !copy_on_write(dir.path()) {
            eprintln!("skipped: no copy-on-write where the temp directory is");
            return;
        }
        sh(dir.path(), "git init -q -b main . && echo one > a && echo keep > b && git add -A && git -c user.name=t -c user.email=t@t commit -qm one && echo two > a && git -c user.name=t -c user.email=t@t commit -qam two");
        let repo = Repo::open(dir.path(), &dir.path().join("home")).unwrap();
        let first = repo.tree_of(&repo.resolve("HEAD~1").unwrap()).unwrap();
        let second = repo.tree_of(&repo.resolve("HEAD").unwrap()).unwrap();
        let out = |n: &str| dir.path().join(n);

        assert_eq!(cache::clone_into(&repo, &first, &out("w1")).unwrap(), Source::Scratch);
        assert_eq!(cache::clone_into(&repo, &first, &out("w2")).unwrap(), Source::Scratch, "nothing published yet");
        cache::publish_and_evict(&repo, &first, &out("w1"), false);
        assert_eq!(cache::clone_into(&repo, &first, &out("w3")).unwrap(), Source::Exact);
        assert_eq!(cache::clone_into(&repo, &second, &out("w4")).unwrap(), Source::Delta);
        assert_eq!(fs::read_to_string(out("w4/tree/a")).unwrap(), "two\n");
        assert_eq!(fs::read_to_string(out("w4/tree/b")).unwrap(), "keep\n");
        cache::publish_and_evict(&repo, &second, &out("w4"), false);
        assert_eq!(cache::clone_into(&repo, &second, &out("w5")).unwrap(), Source::Exact);
    }
}

#[cfg(test)]
mod liveness {
    use super::{clonefile, copy_on_write, process_alive};
    use std::{fs, path::Path};

    #[test]
    fn a_process_we_may_not_signal_is_still_alive() {
        assert!(process_alive(std::process::id()));
        // pid 1 belongs to root: signalling it is refused, but it exists.
        assert!(process_alive(1));
        let mut child = std::process::Command::new("true").spawn().unwrap();
        let pid = child.id();
        child.wait().unwrap();
        assert!(!process_alive(pid));
    }

    /// git compares files by modification time; a clone must keep it, as APFS
    /// clonefile does, or every cloned file looks modified (seen on XFS).
    #[test]
    fn a_clone_keeps_modification_times() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        fs::create_dir_all(&src).unwrap();
        let file = fs::File::create(src.join("f")).unwrap();
        let old = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000_000);
        file.set_modified(old).unwrap();
        drop(file);
        let dst = dir.path().join("dst");
        if clonefile(&src, &dst).is_ok() {
            assert_eq!(fs::metadata(dst.join("f")).unwrap().modified().unwrap(), old);
        }
    }

    #[test]
    fn a_clone_is_a_faithful_copy_or_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        fs::create_dir_all(src.join("sub")).unwrap();
        fs::write(src.join("sub/f"), "data").unwrap();
        std::os::unix::fs::symlink("sub/f", src.join("link")).unwrap();
        let dst = dir.path().join("dst");
        match clonefile(&src, &dst) {
            Ok(()) => {
                assert_eq!(fs::read_to_string(dst.join("sub/f")).unwrap(), "data");
                assert_eq!(fs::read_link(dst.join("link")).unwrap(), Path::new("sub/f"));
                fs::write(dst.join("sub/f"), "changed").unwrap();
                assert_eq!(fs::read_to_string(src.join("sub/f")).unwrap(), "data", "writes do not reach the source");
                assert!(copy_on_write(dir.path()));
            }
            Err(e) => {
                // CI runs this on btrfs and sets ZIT_EXPECT_COW: there it must work.
                assert!(std::env::var_os("ZIT_EXPECT_COW").is_none(), "copy-on-write expected: {e}");
                assert!(!dst.exists(), "a failed clone leaves nothing behind");
                assert!(!copy_on_write(dir.path()));
            }
        }
    }
}
