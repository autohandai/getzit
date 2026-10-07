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

/// Take `file`'s lock exclusively, trying until `deadline`.
fn lock_by(file: &fs::File, deadline: std::time::Instant) -> bool {
    use std::os::fd::AsRawFd;
    // SAFETY: `file` is an open descriptor we own.
    while unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    true
}

/// Free space Zit insists on before it materialises anything, in MB, unless
/// `git config zit.minFreeMB` says otherwise (0 disables the guard).
pub const DEFAULT_MIN_FREE_MB: i64 = 512;

/// Refuse to write a workspace or verification view when the volume holding
/// Zit's home has less than `zit.minFreeMB` free. A full disk makes git
/// fail halfway through and leaves agents' work unlanded.
pub fn require_free_space(repo: &Repo) -> Result<()> {
    let min_mb = repo
        .git(&["config", "--int", "zit.minFreeMB"])
        .ok()
        .and_then(|v| v.trim().parse::<i64>().ok())
        .unwrap_or(DEFAULT_MIN_FREE_MB);
    if min_mb <= 0 {
        return Ok(());
    }
    // The home may not exist yet: measure the nearest directory that does.
    let home = repo.home();
    let Some(free) = home.ancestors().find(|p| p.exists()).and_then(free_bytes) else {
        return Ok(());
    };
    let free_mb = free / 1_000_000;
    if free_mb < min_mb {
        return Err(Error::msg(format!(
            "refusing to materialise: {free_mb} MB free on the volume holding {}, less than zit.minFreeMB = {min_mb} (`git config zit.minFreeMB 0` disables this guard)",
            home.display()
        )));
    }
    Ok(())
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
            if workspace::alive(ws) {
                blocked.push(format!("{} ({}) is running", ws.id, ws.agent));
            } else if ws.path().is_dir() && workspace::is_dirty(repo, ws).unwrap_or(true) {
                // A view deleted by hand holds no edits to lose.
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
    // Take the locks clones and verifications hold, without waiting: if one is
    // busy, something is using these files right now. Held until deleted.
    let mut held = Vec::new();
    let mut busy = Vec::new();
    let lock_files = fs::read_dir(home.join("verify"))
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "lock"))
        .chain([home.join("trees/.lock")].into_iter().filter(|p| p.exists()));
    // A lock that frees within a second was not a clone or verification: on macOS a process
    // spawned anywhere holds every open descriptor, locks included, until it execs.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
    for path in lock_files {
        let Ok(file) = fs::File::open(&path) else { continue };
        if lock_by(&file, deadline) {
            held.push(file);
        } else {
            busy.push(path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
        }
    }
    if !busy.is_empty() && !force {
        return Err(Error::msg(format!(
            "not cleaning: a clone or a verification is in progress ({}). Try again when it is done, or pass --force.",
            busy.join(", ")
        )));
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
    drop(held);
    let freed = before.zip(free_bytes(anchor)).map_or(0, |(b, a)| a - b);
    Ok(Report { freed_bytes: freed, ..report })
}
