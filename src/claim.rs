//! Claims: early, coarse coordination. A workspace announces what it means
//! to write; overlapping claims are refused while the holder is alive.
//! Claims prevent wasted work; correctness still rests on validation at accept.

use crate::change;
use crate::footprint;
use crate::git::{Oid, Repo};
use crate::resource::Resource;
use crate::workspace;
use crate::{Error, Result};
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case", tag = "kind")]
pub enum Holder {
    /// A live workspace claimed it.
    Workspace { id: String, agent: String },
    /// A recorded, not yet accepted change already wrote it.
    Change { id: Oid, agent: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Held {
    pub resource: Resource,
    pub by: Holder,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case", tag = "claim", content = "held")]
pub enum Claimed {
    Granted,
    /// Nothing was claimed; these are already held.
    Refused(Vec<Held>),
}

/// Everything currently held: claims and in-flight writes of live workspaces,
/// and writes of speculative changes.
pub fn held(repo: &Repo) -> Result<Vec<Held>> {
    let mut all = work_held(repo)?;
    all.extend(claims_held(repo)?);
    Ok(all)
}

/// What open workspaces are writing and recorded changes wrote. Slow-ish;
/// computed outside the claims lock.
fn work_held(repo: &Repo) -> Result<Vec<Held>> {
    let mut all = Vec::new();
    let spaces = workspace::list(repo)?;
    let window = workspace::AWARENESS_WINDOW;
    let writing = crate::api::parallel(&spaces, |ws| workspace::in_flight_within(repo, ws, window).unwrap_or_default());
    for (ws, writes) in spaces.iter().zip(writing) {
        let by = Holder::Workspace { id: ws.id.clone(), agent: ws.agent.clone() };
        all.extend(writes.into_iter().map(|resource| Held { resource, by: by.clone() }));
    }
    for c in change::speculative(repo)? {
        let Some(parent) = c.parents.first() else { continue };
        let by = Holder::Change { id: c.id.clone(), agent: c.agent.clone() };
        let writes = footprint::of_change(repo, parent, &c.id, &c.reads)?.writes;
        all.extend(writes.into_iter().map(|resource| Held { resource, by: by.clone() }));
    }
    Ok(all)
}

/// Claims recorded by open workspaces: a few small files.
fn claims_held(repo: &Repo) -> Result<Vec<Held>> {
    let mut all = Vec::new();
    for ws in workspace::list(repo)? {
        let by = Holder::Workspace { id: ws.id.clone(), agent: ws.agent.clone() };
        all.extend(ws.claims().into_iter().map(|resource| Held { resource, by: by.clone() }));
    }
    Ok(all)
}

/// The path of `resource` as the workspace's git names it: relative to the
/// workspace root, no `.` or `..`, no trailing slash. Agents pass absolute
/// paths and `./a`; one file must be one claim however it is spelt.
fn normalise(ws: &workspace::Workspace, resource: &Resource) -> Result<Resource> {
    use std::path::{Component, Path};
    let given = Path::new(resource.path());
    let relative = given.strip_prefix(ws.path()).unwrap_or(given);
    let mut parts: Vec<&str> = Vec::new();
    for component in relative.components() {
        match component {
            Component::Normal(name) => parts.push(name.to_str().unwrap_or_default()),
            Component::CurDir => {}
            Component::ParentDir if parts.pop().is_some() => {}
            _ => return Err(Error::msg(format!("`{resource}` is not a path inside the workspace"))),
        }
    }
    if parts.is_empty() {
        return Err(Error::msg(format!("`{resource}` names no file")));
    }
    let path = parts.join("/");
    Ok(match resource {
        Resource::File(_) => Resource::File(path),
        Resource::Top(_) => Resource::Top(path),
        Resource::Symbol(_, name) => Resource::Symbol(path, name.clone()),
    })
}

/// Claim `resources` for `workspace`, all or nothing. A claim lapses when
/// its workspace is disposed; nothing has to release it.
pub fn claim(repo: &Repo, workspace: &str, resources: &[Resource]) -> Result<Claimed> {
    use std::io::Write;
    use std::os::fd::AsRawFd;
    let ws = workspace::get(repo, workspace)?;
    let resources: Vec<Resource> = resources.iter().map(|r| normalise(&ws, r)).collect::<Result<_>>()?;
    // The slow part happens before the lock; only claims are re-read under it.
    let mut held = work_held(repo)?;
    // One claimant at a time decides, across processes.
    let lock = std::fs::File::create(repo.home().join("ws/.claims.lock"))?;
    // SAFETY: `lock` is an open descriptor we own; it unlocks when dropped.
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }

    let mine = Holder::Workspace { id: ws.id.clone(), agent: ws.agent.clone() };
    // Generated files are rebuilt on compose, so nobody needs to hold them.
    let generated = crate::accept::generated_paths(repo, &repo.current()?)?;
    let free = |r: &Resource| generated.iter().any(|g| g == r.path());
    held.extend(claims_held(repo)?);
    let taken: Vec<Held> = held
        .into_iter()
        .filter(|h| h.by != mine && !free(&h.resource))
        .filter(|h| resources.iter().any(|r| !free(r) && r.held_with(&h.resource)))
        .collect();
    if !taken.is_empty() {
        return Ok(Claimed::Refused(taken));
    }
    let mut file = std::fs::OpenOptions::new().create(true).append(true).open(ws.claims_file())?;
    for resource in &resources {
        writeln!(file, "{resource}")?;
    }
    Ok(Claimed::Granted)
}
