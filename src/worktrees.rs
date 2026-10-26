//! Ordinary Git worktrees alongside Zit's unregistered workspace views.
//!
//! The inventory is authoritative; assessments are a point-in-time preview
//! against `refs/zit/current`, never permission to delete somebody's files.

use crate::git::{git_command, run, Oid, Repo};
use crate::status::{self, Assessment, DiffStats, Readiness};
use crate::{Error, Result};
use serde::{Serialize, Serializer};
use std::ffi::OsString;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Debug, Serialize)]
pub struct GitWorktreeRow {
    /// Display form; `path_bytes` preserves a path that is not UTF-8.
    #[serde(serialize_with = "serialize_path")]
    pub path: PathBuf,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path_bytes: Option<Vec<u8>>,
    /// Short local branch name, absent for detached and bare entries.
    pub branch: Option<String>,
    pub head: Option<Oid>,
    pub primary: bool,
    pub bare: bool,
    pub detached: bool,
    pub locked: Option<String>,
    pub prunable: Option<String>,
    pub task: Option<String>,
    /// `branch.zit-task`, `branch.description`, or the inferred `commit.subject`.
    pub task_source: Option<String>,
    /// Explicit branch metadata or a `Zit-Agent` trailer, never a guessed author.
    pub agent: Option<String>,
    /// Final checkout contents relative to its merge base with current.
    pub diff: Option<DiffStats>,
    /// Fresh tracked, staged and untracked status; `None` when inspection failed.
    pub dirty: Option<bool>,
    pub merge: Assessment,
    pub cleanup: Assessment,
    pub error: Option<String>,
}

fn serialize_path<S: Serializer>(path: &Path, serializer: S) -> std::result::Result<S::Ok, S::Error> {
    serializer.serialize_str(&path.to_string_lossy())
}

impl GitWorktreeRow {
    fn new(path: PathBuf, primary: bool) -> Self {
        Self {
            path_bytes: path.to_str().is_none().then(|| path.as_os_str().as_bytes().to_vec()),
            path,
            branch: None,
            head: None,
            primary,
            bare: false,
            detached: false,
            locked: None,
            prunable: None,
            task: None,
            task_source: None,
            agent: None,
            diff: None,
            dirty: None,
            merge: Assessment::new(Readiness::Unknown, "worktree has not been inspected"),
            cleanup: Assessment::new(Readiness::Unknown, "worktree has not been inspected"),
            error: None,
        }
    }
}

/// All entries in Git's worktree registry, including broken/missing paths.
/// A failed inspection remains a row with unknown readiness rather than
/// turning into an apparently clean worktree or disappearing from the list.
pub fn list(repo: &Repo, current: &Oid) -> Result<Vec<GitWorktreeRow>> {
    let out = repo.cmd().args(["worktree", "list", "--porcelain", "-z"]).stdin(Stdio::null()).output()?;
    if !out.status.success() {
        return Err(Error::msg(format!("cannot list git worktrees: {}", String::from_utf8_lossy(&out.stderr).trim())));
    }
    let rows = parse(&out.stdout)?;
    Ok(crate::api::parallel(&rows, |row| inspect(repo, current, row)))
}

fn parse(bytes: &[u8]) -> Result<Vec<GitWorktreeRow>> {
    let mut rows = Vec::new();
    for field in bytes.split(|&b| b == 0) {
        if field.is_empty() {
            continue;
        }
        if let Some(path) = field.strip_prefix(b"worktree ") {
            rows.push(GitWorktreeRow::new(PathBuf::from(OsString::from_vec(path.to_vec())), rows.is_empty()));
            continue;
        }
        let row = rows.last_mut().ok_or_else(|| Error::msg("invalid git worktree inventory: missing path"))?;
        if let Some(head) = field.strip_prefix(b"HEAD ") {
            let head = std::str::from_utf8(head).map_err(|_| Error::msg("invalid worktree HEAD"))?;
            if head.bytes().any(|b| b != b'0') {
                row.head = Some(Oid::new(head));
            }
        } else if let Some(branch) = field.strip_prefix(b"branch refs/heads/") {
            row.branch = Some(String::from_utf8_lossy(branch).into_owned());
        } else if field == b"bare" {
            row.bare = true;
        } else if field == b"detached" {
            row.detached = true;
        } else if field == b"locked" || field.starts_with(b"locked ") {
            row.locked = Some(String::from_utf8_lossy(field.strip_prefix(b"locked ").unwrap_or_default()).into_owned());
        } else if field == b"prunable" || field.starts_with(b"prunable ") {
            row.prunable =
                Some(String::from_utf8_lossy(field.strip_prefix(b"prunable ").unwrap_or_default()).into_owned());
        }
    }
    Ok(rows)
}

/// Recreate a row without sharing mutable filesystem state between workers.
fn inspect(repo: &Repo, current: &Oid, registered: &GitWorktreeRow) -> GitWorktreeRow {
    let mut row = GitWorktreeRow::new(registered.path.clone(), registered.primary);
    row.branch.clone_from(&registered.branch);
    row.head.clone_from(&registered.head);
    row.bare = registered.bare;
    row.detached = registered.detached;
    row.locked.clone_from(&registered.locked);
    row.prunable.clone_from(&registered.prunable);
    metadata(repo, &mut row);
    if row.bare {
        row.merge = Assessment::new(Readiness::NotNeeded, "bare repository has no checkout to merge");
        row.cleanup = Assessment::new(Readiness::Blocked, "bare repository owns the object store");
        return row;
    }
    if let Err(error) = inspect_checkout(repo, current, &mut row) {
        let message = error.to_string();
        row.merge = Assessment::new(Readiness::Unknown, format!("cannot inspect worktree: {message}"));
        row.cleanup = Assessment::new(Readiness::Unknown, format!("cannot inspect worktree: {message}"));
        row.error = Some(message);
    }
    row
}

fn checkout(path: &Path) -> Command {
    let mut command = git_command();
    command.current_dir(path).env("GIT_OPTIONAL_LOCKS", "0");
    command
}

fn metadata(repo: &Repo, row: &mut GitWorktreeRow) {
    if let Some(branch) = &row.branch {
        for (key, source) in [("zit-task", "branch.zit-task"), ("description", "branch.description")] {
            if let Ok(task) = repo.git(&["config", "--get", &format!("branch.{branch}.{key}")]) {
                if !task.trim().is_empty() {
                    row.task = Some(task);
                    row.task_source = Some(source.into());
                    break;
                }
            }
        }
        row.agent =
            repo.git(&["config", "--get", &format!("branch.{branch}.zit-agent")]).ok().filter(|v| !v.trim().is_empty());
    }
    if let Some(head) = &row.head {
        if let Ok(summary) = repo.git(&["log", "-1", "--format=%s%n%(trailers:key=Zit-Agent,valueonly)", head.as_str()])
        {
            let mut lines = summary.lines();
            if let Some(subject) = lines.next().filter(|s| !s.is_empty()) {
                if row.task.is_none() {
                    row.task = Some(subject.into());
                    row.task_source = Some("commit.subject".into());
                }
            }
            if row.agent.is_none() {
                row.agent = lines.find(|line| !line.trim().is_empty()).map(str::to_owned);
            }
        }
    }
}

fn inspect_checkout(repo: &Repo, current: &Oid, row: &mut GitWorktreeRow) -> Result<()> {
    if !row.path.is_dir() {
        return Err(Error::msg("worktree path is missing or inaccessible"));
    }
    let head = row.head.as_ref().ok_or_else(|| Error::msg("worktree HEAD has no commit"))?;
    // Check registry identity before inspecting: a path may now contain a
    // different repository, whose files are not this registry entry's work.
    let common = git_path(checkout(&row.path).args(["rev-parse", "--path-format=absolute", "--git-common-dir"]))?;
    if common.canonicalize()? != repo.git_dir() {
        return Err(Error::msg("worktree path belongs to a different repository"));
    }
    let live_head = run(checkout(&row.path).args(["rev-parse", "--verify", "HEAD"]))?;
    if live_head != head.as_str() {
        return Err(Error::msg("worktree HEAD changed during inspection; run status again"));
    }
    let porcelain = run(status::inspection_command(repo, checkout(&row.path))?.args([
        "status",
        "--porcelain=v1",
        "-z",
        "--untracked-files=all",
        "--ignore-submodules=none",
    ]))?;
    row.dirty = Some(!porcelain.is_empty());
    let admin = git_path(checkout(&row.path).args(["rev-parse", "--absolute-git-dir"]))?;
    let operation =
        ["MERGE_HEAD", "CHERRY_PICK_HEAD", "REVERT_HEAD", "BISECT_START", "rebase-merge", "rebase-apply", "sequencer"]
            .into_iter()
            .find(|name| admin.join(name).exists());

    let base =
        merge_base(repo, head, current)?.ok_or_else(|| Error::msg("worktree and current have unrelated histories"))?;
    let head_state = repo.tree_of(head)?;
    let (diff, changed) = status::snapshot(
        repo,
        || checkout(&row.path),
        head,
        |shadow, state| Ok((DiffStats::between(shadow, &base, state)?, *state != head_state)),
    )?;
    row.diff = Some(diff);
    row.dirty = Some(row.dirty == Some(true) || changed);
    let retained = is_ancestor(repo, head, current)?;
    row.merge = if let Some(operation) = operation {
        Assessment::new(Readiness::Blocked, format!("Git operation in progress: {operation}"))
    } else if row.dirty == Some(true) {
        Assessment::new(Readiness::Blocked, "commit or record the uncommitted edits before merging")
    } else if retained {
        Assessment::new(Readiness::NotNeeded, "HEAD is already an ancestor of current")
    } else {
        merge_preview(repo, current, head, &row.path)?
    };
    row.cleanup = if row.primary {
        Assessment::new(Readiness::Blocked, "primary checkout owns the repository; it is not disposable")
    } else if let Some(reason) = &row.locked {
        Assessment::new(
            Readiness::Blocked,
            format!("worktree is locked{}", if reason.is_empty() { String::new() } else { format!(": {reason}") }),
        )
    } else if let Some(operation) = operation {
        Assessment::new(Readiness::Blocked, format!("Git operation in progress: {operation}"))
    } else if row.dirty == Some(true) {
        Assessment::new(Readiness::Blocked, "worktree has staged, unstaged or untracked edits")
    } else if !retained {
        Assessment::new(Readiness::Blocked, "HEAD has commits not retained in current; integrate or retain them first")
    } else {
        Assessment::new(Readiness::Review, "tracked/untracked files clean and HEAD retained in current; confirm no agent is using this ordinary Git worktree and inspect ignored files before removing it")
    };
    Ok(())
}

fn is_ancestor(repo: &Repo, ancestor: &Oid, of: &Oid) -> Result<bool> {
    let output = repo
        .cmd()
        .args(["merge-base", "--is-ancestor", ancestor.as_str(), of.as_str()])
        .stdin(Stdio::null())
        .output()?;
    match output.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => Err(Error::msg(format!(
            "cannot inspect commit ancestry: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ))),
    }
}

fn merge_base(repo: &Repo, a: &Oid, b: &Oid) -> Result<Option<Oid>> {
    let output = repo.cmd().args(["merge-base", a.as_str(), b.as_str()]).stdin(Stdio::null()).output()?;
    match output.status.code() {
        Some(0) => Ok(Some(Oid::new(String::from_utf8_lossy(&output.stdout).trim().to_string()))),
        Some(1) => Ok(None),
        _ => Err(Error::msg(format!("cannot inspect merge base: {}", String::from_utf8_lossy(&output.stderr).trim()))),
    }
}

fn merge_preview(repo: &Repo, current: &Oid, head: &Oid, path: &Path) -> Result<Assessment> {
    // Repository-defined merge drivers are executable commands. Status
    // neither runs them nor treats bypassing them as a clean merge.
    if let Err(why) = status::require_builtin_merges_in(checkout(path)) {
        return Ok(Assessment::new(Readiness::Unknown, why.to_string()));
    }
    struct Scratch(PathBuf);
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let scratch = Scratch(repo.home().join("inspection").join(crate::workspace::fresh_id()));
    let objects = scratch.0.join("objects");
    std::fs::create_dir_all(&objects)?;
    let output = status::inspection_command(repo, checkout(path))?
        .env("GIT_OBJECT_DIRECTORY", &objects)
        .env("GIT_ALTERNATE_OBJECT_DIRECTORIES", repo.object_dir()?)
        .args(["merge-tree", "--write-tree", "--name-only", current.as_str(), head.as_str()])
        .stdin(Stdio::null())
        .output()?;
    match output.status.code() {
        Some(0) => Ok(Assessment::new(Readiness::Review, "text merges with current; ordinary Git worktrees have no Zit acceptance evidence, so record and verify before integrating")),
        Some(1) => Ok(Assessment::new(Readiness::Blocked, "HEAD conflicts with current; resolve the merge before integrating")),
        _ => Err(Error::msg(format!("cannot preview merge: {}", String::from_utf8_lossy(&output.stderr).trim()))),
    }
}

/// Git's path plumbing emits a path followed by one newline; trimming all
/// whitespace would corrupt a directory whose name itself ends in a newline.
fn git_path(command: &mut Command) -> Result<PathBuf> {
    let output = command.stdin(Stdio::null()).output()?;
    if !output.status.success() {
        return Err(Error::msg(format!(
            "cannot inspect worktree administration: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let bytes = output.stdout.strip_suffix(b"\n").unwrap_or(&output.stdout);
    Ok(PathBuf::from(OsString::from_vec(bytes.to_vec())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nul_inventory_preserves_non_utf8_and_newline_path_bytes() {
        let rows = parse(b"worktree /tmp/agent-\xff\n\0HEAD 123456\0detached\0\0").unwrap();
        assert_eq!(rows[0].path.as_os_str().as_bytes(), b"/tmp/agent-\xff\n");
        let json = serde_json::to_value(&rows[0]).unwrap();
        let bytes: Vec<u8> = serde_json::from_value(json["path_bytes"].clone()).unwrap();
        assert_eq!(bytes, b"/tmp/agent-\xff\n");
    }
}
