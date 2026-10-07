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
    let current = change::load(repo, &repo.current()?)?;
    accept::prune_accepted(repo)?;
    let speculative = change::speculative(repo)?;
    let evidence = evidence::fingerprint(repo)?;
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
            let alive = workspace.pid.map(workspace::process_alive);
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
    pub evidence: Vec<CheckResult>,
}

/// What changed, why, what it depends on, and the evidence for it.
pub fn detail(repo: &Repo, id: &Oid) -> Result<Detail> {
    let change = change::load(repo, id)?;
    let writes = match change.parents.first() {
        Some(parent) => footprint::between(repo, parent, id)?.writes.into_iter().collect(),
        None => vec![],
    };
    let evidence = evidence::lookup(repo, &change.state)?
        .into_iter()
        .map(|(check, evidence)| CheckResult { check: check.name, evidence })
        .collect();
    Ok(Detail { status: accept::status(repo, id)?, change, writes, evidence })
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
