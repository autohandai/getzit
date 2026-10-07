mod common;

use common::{write, Fixture};
use zit::{change, evidence, workspace};

const CHECK: &str = "[[check]]\nname = \"t\"\nrun = \"true\"\n";

fn local_dirs(fx: &Fixture) -> Vec<String> {
    let mut found: Vec<String> = std::fs::read_dir(fx.repo.home())
        .map(|d| d.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect())
        .unwrap_or_default();
    found.sort();
    found
}

#[test]
fn clean_deletes_every_local_copy_and_keeps_the_graph() {
    let fx = Fixture::new(&[("zit.toml", CHECK), ("a.txt", "a\n")]);
    let c = fx.change("claude", &[("a.txt", "b\n")]);
    evidence::verify(&fx.repo, &c.id, false).unwrap();
    fx.workspace("codex");
    std::fs::write(fx.repo.cache_dir().unwrap().join("build-cache"), "x").unwrap();
    assert!(!local_dirs(&fx).is_empty());

    let report = zit::clean::clean(&fx.repo, false).unwrap();
    assert_eq!(report.workspaces, 1);
    assert!(report.verification_views >= 1, "{report:?}");
    // Checkouts are cached for cloning only where copy-on-write exists.
    if zit::workspace::copy_on_write(&std::env::temp_dir()) {
        assert!(report.cached_states >= 1, "{report:?}");
    }
    assert!(local_dirs(&fx).is_empty(), "nothing left: {:?}", local_dirs(&fx));

    assert_eq!(change::speculative(&fx.repo).unwrap(), vec![c.clone()], "recorded changes are untouched");
    assert!(evidence::lookup(&fx.repo, &c.state).unwrap()[0].1.is_some(), "so is the evidence");
    let ws = fx.workspace("again");
    assert!(ws.path().join("a.txt").exists(), "zit still works afterwards");
}

#[test]
fn clean_refuses_to_delete_unrecorded_work() {
    let fx = Fixture::new(&[("a.txt", "a\n")]);
    let ws = fx.workspace("claude");
    write(ws.path(), &[("a.txt", "unsaved\n")]);
    let err = zit::clean::clean(&fx.repo, false).unwrap_err().to_string();
    assert!(err.contains(&ws.id) && err.contains("--force"), "{err}");
    assert!(ws.path().exists());
    zit::clean::clean(&fx.repo, true).unwrap();
    assert!(!ws.path().exists());
}

/// A view someone deleted by hand has no unrecorded edits to protect.
#[test]
fn clean_does_not_mistake_a_hand_deleted_view_for_unrecorded_edits() {
    let fx = Fixture::new(&[("a.txt", "a\n")]);
    let ws = fx.workspace("claude");
    std::fs::remove_dir_all(ws.path()).unwrap();
    assert_eq!(zit::clean::clean(&fx.repo, false).unwrap().workspaces, 1);
    assert!(workspace::list(&fx.repo).unwrap().is_empty());
}

#[test]
fn clean_refuses_to_pull_a_workspace_from_under_a_running_agent() {
    let fx = Fixture::new(&[("a.txt", "a\n")]);
    let mut ws = fx.workspace("claude");
    ws.pid = Some(std::process::id());
    std::fs::write(ws.path().parent().unwrap().join("meta.json"), serde_json::to_vec(&ws).unwrap()).unwrap();
    let owner = ws.hold().unwrap();
    assert!(workspace::running(&ws));
    let err = zit::clean::clean(&fx.repo, false).unwrap_err().to_string();
    assert!(err.contains("running"), "{err}");
    drop(owner);
    assert!(!workspace::running(&ws));
    assert_eq!(zit::clean::clean(&fx.repo, false).unwrap().workspaces, 1);
    assert!(workspace::list(&fx.repo).unwrap().is_empty());
}

/// A `zit run` that died (SIGKILL, a crash) leaves its pid in meta.json; when the
/// system hands that pid to another process, the workspace is not running.
#[test]
fn a_reused_pid_is_not_a_running_agent() {
    let fx = Fixture::new(&[("a.txt", "a\n")]);
    let mut ws = fx.workspace("claude");
    ws.pid = Some(std::process::id());
    std::fs::write(ws.path().parent().unwrap().join("meta.json"), serde_json::to_vec(&ws).unwrap()).unwrap();
    assert!(!workspace::running(&ws));
    assert_eq!(zit::clean::clean(&fx.repo, false).unwrap().workspaces, 1);
}

/// A verification in progress holds its view's lock; clean waits for nobody and refuses instead.
#[test]
fn clean_refuses_while_a_verification_holds_its_view() {
    use std::os::fd::AsRawFd;
    let fx = Fixture::new(&[("zit.toml", CHECK), ("a.txt", "a\n")]);
    let c = fx.change("claude", &[("a.txt", "b\n")]);
    evidence::verify(&fx.repo, &c.id, false).unwrap();
    let lock = std::fs::File::open(fx.repo.home().join("verify/0.lock")).unwrap();
    assert_eq!(unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) }, 0);
    let refused = zit::clean::clean(&fx.repo, false).unwrap_err().to_string();
    assert!(refused.contains("in progress"), "{refused}");
    drop(lock);
    zit::clean::clean(&fx.repo, false).unwrap();
}

/// A lock released moments after clean looks is not a verification in progress. On macOS a
/// concurrent process spawn briefly holds every open descriptor, locks included.
#[test]
fn clean_waits_out_a_lock_held_for_a_moment() {
    use std::os::fd::AsRawFd;
    let fx = Fixture::new(&[("zit.toml", CHECK), ("a.txt", "a\n")]);
    let c = fx.change("claude", &[("a.txt", "b\n")]);
    evidence::verify(&fx.repo, &c.id, false).unwrap();
    let lock = std::fs::File::open(fx.repo.home().join("verify/0.lock")).unwrap();
    assert_eq!(unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) }, 0);
    let release = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(100));
        drop(lock);
    });
    zit::clean::clean(&fx.repo, false).unwrap();
    release.join().unwrap();
}
