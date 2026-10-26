//! Fresh, reasoned status observations. They are advice at a named snapshot,
//! never authorisation to skip acceptance or deletion guards.

use crate::accept::{self, DryRun, Status};
use crate::git::{run, Oid, Repo};
use crate::workspace::{self, Workspace};
use crate::{Error, Result};
use serde::Serialize;
use std::path::PathBuf;
use std::process::Command;

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
pub struct DiffStats {
    pub files: u64,
    pub added: u64,
    pub removed: u64,
    /// Binary files are counted as files, never as zero-line text changes.
    pub binary: u64,
}

impl DiffStats {
    pub(crate) fn between(repo: &Repo, from: &Oid, to: &Oid) -> Result<Self> {
        let output = run(inspection_command(repo, repo.cmd())?.args([
            "diff",
            "--numstat",
            "-z",
            "--no-renames",
            "--no-ext-diff",
            "--no-textconv",
            from.as_str(),
            to.as_str(),
            "--",
        ]))?;
        let mut stats = Self::default();
        // --no-renames gives exactly one NUL-delimited record per path;
        // only the first two tabs are separators (a path may contain tabs).
        for record in output.as_bytes().split(|b| *b == 0).filter(|r| !r.is_empty()) {
            let mut fields = record.splitn(3, |b| *b == b'\t');
            let added = fields.next().unwrap_or_default();
            let removed = fields.next().ok_or_else(|| Error::msg("malformed git numstat"))?;
            if fields.next().is_none() {
                return Err(Error::msg("missing path in git numstat"));
            }
            stats.files += 1;
            if added == b"-" && removed == b"-" {
                stats.binary += 1;
            } else {
                let count = |s| {
                    std::str::from_utf8(s)
                        .ok()
                        .and_then(|s| s.parse::<u64>().ok())
                        .ok_or_else(|| Error::msg("invalid line count in git numstat"))
                };
                stats.added += count(added)?;
                stats.removed += count(removed)?;
            }
        }
        Ok(stats)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Readiness {
    Ready,
    Blocked,
    Review,
    NotNeeded,
    Unknown,
}

impl Readiness {
    pub fn label(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::Blocked => "blocked",
            Self::Review => "review",
            Self::NotNeeded => "not-needed",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Assessment {
    pub state: Readiness,
    pub reasons: Vec<String>,
}

impl Assessment {
    pub fn new(state: Readiness, reason: impl Into<String>) -> Self {
        Self { state, reasons: vec![reason.into()] }
    }
}

struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Reading a checkout must not invoke its fsmonitor, hooks, or clean filters.
/// Empty filter commands fall back to the file's bytes; required filters are
/// disabled for this observation only. Normal record/accept behavior is unchanged.
pub(crate) fn inspection_command(_repo: &Repo, mut command: Command) -> Result<Command> {
    command
        .args(["-c", "core.fsmonitor=false", "-c", "core.hooksPath=/dev/null", "-c", "maintenance.auto=false"])
        .env("GIT_OPTIONAL_LOCKS", "0");
    let output = copy_command(&command)
        .args(["config", "--null", "--name-only", "--get-regexp", "^filter\\..*\\.(clean|process|required)$"])
        .output()?;
    if !output.status.success() && output.status.code() != Some(1) {
        return Err(Error::msg(format!(
            "cannot read inspection configuration: {}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    for name in output.stdout.split(|b| *b == 0).filter(|s| !s.is_empty()) {
        let name = std::str::from_utf8(name).map_err(|_| Error::msg("non-UTF8 filter configuration key"))?;
        let value = if name.ends_with(".required") { "false" } else { "" };
        command.arg("-c").arg(format!("{name}={value}"));
    }
    Ok(command)
}

fn copy_command(source: &Command) -> Command {
    let mut command = Command::new(source.get_program());
    command.args(source.get_args());
    if let Some(dir) = source.get_current_dir() {
        command.current_dir(dir);
    }
    for (key, value) in source.get_envs() {
        match value {
            Some(value) => {
                command.env(key, value);
            }
            None => {
                command.env_remove(key);
            }
        }
    }
    command
}

/// merge-tree can execute repository-configured merge drivers. Status must
/// report the missing assessment instead of running those programs.
pub(crate) fn require_builtin_merges(repo: &Repo) -> Result<()> {
    require_builtin_merges_in(repo.cmd())
}

pub(crate) fn require_builtin_merges_in(mut command: Command) -> Result<()> {
    let out = command.args(["config", "--name-only", "--get-regexp", "^merge\\..*\\.driver$"]).output()?;
    match out.status.code() {
        Some(1) => Ok(()),
        Some(0) => Err(Error::msg(
            "custom merge drivers configured; status does not execute them; inspect with zit accept --dry-run",
        )),
        _ => Err(Error::msg("cannot inspect merge driver configuration")),
    }
}

/// Snapshot the checkout using an isolated index and object store. The real
/// index, refs, and object store are never staged into or written by this scan.
pub(crate) fn snapshot<T>(
    repo: &Repo,
    git: impl Fn() -> Command,
    base: &Oid,
    inspect: impl FnOnce(&Repo, &Oid) -> Result<T>,
) -> Result<T> {
    let root = repo.home().join("inspection");
    std::fs::create_dir_all(&root)?;
    let scratch = Scratch(root.join(workspace::fresh_id()));
    std::fs::create_dir(&scratch.0)?;
    let objects = scratch.0.join("objects");
    std::fs::create_dir(&objects)?;
    let index = scratch.0.join("index");
    let real = repo.object_dir()?;
    let command = || -> Result<Command> {
        let mut cmd = inspection_command(repo, git())?;
        cmd.env("GIT_INDEX_FILE", &index)
            .env("GIT_OBJECT_DIRECTORY", &objects)
            .env("GIT_ALTERNATE_OBJECT_DIRECTORIES", &real)
            .env("GIT_OPTIONAL_LOCKS", "0");
        Ok(cmd)
    };
    run(command()?.args(["read-tree", base.as_str()]))?;
    run(command()?.args(["add", "-A", "--", "."]))?;
    let tree = Oid::new(run(command()?.arg("write-tree"))?);
    inspect(&repo.reading_also(&objects), &tree)
}

pub(crate) struct WorkspaceInspection {
    pub diff: DiffStats,
    pub writes: Vec<crate::resource::Resource>,
    pub dirty: bool,
}

pub(crate) fn inspect_workspace(repo: &Repo, ws: &Workspace) -> Result<WorkspaceInspection> {
    // Porcelain also sees staged-only changes, empty untracked files and
    // deletions. Semantic footprints alone cannot establish cleanliness.
    let dirty = !run(inspection_command(repo, ws.git())?.args([
        "status",
        "--porcelain",
        "--untracked-files=all",
        "--ignore-submodules=none",
    ]))?
    .is_empty();
    snapshot(
        repo,
        || ws.git(),
        &ws.base_state,
        |reader, tree| {
            let diff = DiffStats::between(reader, &ws.base_state, tree)?;
            let writes = crate::footprint::between_states(reader, &ws.base_state, tree, &ws.declared_reads())?
                .writes
                .into_iter()
                .collect();
            Ok(WorkspaceInspection { dirty: dirty || diff.files != 0, diff, writes })
        },
    )
}

/// Preview acceptance of a recorded change, including composed-state evidence.
pub(crate) fn recorded_merge(repo: &Repo, id: &Oid, status: &Status) -> Assessment {
    use Readiness::*;
    if let Err(e) = require_builtin_merges(repo) {
        return Assessment::new(Unknown, e.to_string());
    }
    if matches!(status, Status::Invalid(_)) {
        return Assessment {
            state: Blocked,
            reasons: std::iter::once(crate::view::status_label(status)).chain(crate::view::reasons(status)).collect(),
        };
    }
    match accept::dry_run(repo, id, &accept::Policy::default()) {
        Ok(DryRun::AlreadyAccepted) => Assessment::new(NotNeeded, "already included in current"),
        Ok(DryRun::Rejected(why)) => {
            let status = Status::Invalid(why);
            Assessment { state: Blocked, reasons: crate::view::reasons(&status) }
        }
        Ok(DryRun::WouldAccept { checks, .. }) => {
            let failed: Vec<_> =
                checks.iter().filter(|c| !c.run && c.evidence_passed == Some(false)).map(|c| c.check.clone()).collect();
            if !failed.is_empty() {
                return Assessment::new(Blocked, format!("composed-state checks failed: {}", failed.join(", ")));
            }
            let pending: Vec<_> = checks.into_iter().filter(|c| c.run).map(|c| c.check).collect();
            if pending.is_empty() {
                Assessment::new(Ready, "acceptance preview passes; zit accept revalidates before landing")
            } else {
                Assessment::new(Review, format!("composed-state checks required: {}", pending.join(", ")))
            }
        }
        Err(e) => Assessment::new(Unknown, e.to_string()),
    }
}
