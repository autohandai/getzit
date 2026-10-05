//! Delete everything zit keeps locally for a repository: workspaces, cached
//! checkouts, verification views and shared caches. The graph (changes,
//! their reasons, evidence) lives in git and is never touched.

use crate::git::Repo;
use crate::workspace;
use crate::{Error, Result};
use serde::Serialize;
use std::fs;
use std::path::Path;

#[derive(Debug, Default, Serialize)]
pub struct Report {
    pub workspaces: usize,
    pub cached_states: usize,
    pub verification_views: usize,
    /// Rise in free space on the volume, as reported by the file system.
    /// Approximate: other programs write too, and clones shared blocks.
    pub freed_bytes: i64,
}

fn count(dir: &Path) -> usize {
    fs::read_dir(dir).map(|d| d.flatten().filter(|e| e.path().is_dir()).count()).unwrap_or(0)
}

fn free_bytes(path: &Path) -> Option<i64> {
    use std::os::unix::ffi::OsStrExt;
    let path = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    // SAFETY: `path` is a valid C string and `stat` a properly sized out-parameter.
    unsafe {
        let mut stat: libc::statfs = std::mem::zeroed();
        (libc::statfs(path.as_ptr(), &mut stat) == 0).then(|| stat.f_bavail as i64 * stat.f_bsize as i64)
    }
}

/// Refuses while a workspace holds unrecorded edits or belongs to a running
/// `zit run`, unless `force`.
pub fn clean(repo: &Repo, force: bool) -> Result<Report> {
    let home = repo.home();
    let spaces = workspace::list(repo)?;
    if !force {
        let mut blocked = Vec::new();
        for ws in &spaces {
            if ws.pid.is_some_and(workspace::process_alive) {
                blocked.push(format!("{} ({}) is running", ws.id, ws.agent));
            } else if workspace::is_dirty(repo, ws).unwrap_or(true) {
                blocked.push(format!("{} ({}) has unrecorded edits", ws.id, ws.agent));
            }
        }
        if !blocked.is_empty() {
            return Err(Error::msg(format!(
                "not cleaning: {}. Record them (`zit record --workspace <id>`) or pass --force to delete them anyway.",
                blocked.join("; ")
            )));
        }
    }
    let report = Report {
        workspaces: spaces.len(),
        cached_states: count(&home.join("trees")),
        verification_views: count(&home.join("verify")),
        freed_bytes: 0,
    };
    let anchor = home.parent().unwrap_or(home);
    let before = free_bytes(anchor);
    if home.exists() {
        fs::remove_dir_all(home)?;
    }
    let freed = before.zip(free_bytes(anchor)).map_or(0, |(b, a)| a - b);
    Ok(Report { freed_bytes: freed, ..report })
}
