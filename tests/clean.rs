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

#[test]
fn clean_refuses_to_pull_a_workspace_from_under_a_running_agent() {
    let fx = Fixture::new(&[("a.txt", "a\n")]);
    let mut ws = fx.workspace("claude");
    let mut agent = std::process::Command::new("sleep").arg("30").spawn().unwrap();
    ws.pid = Some(agent.id());
    std::fs::write(ws.path().parent().unwrap().join("meta.json"), serde_json::to_vec(&ws).unwrap()).unwrap();
    let err = zit::clean::clean(&fx.repo, false).unwrap_err().to_string();
    assert!(err.contains("running"), "{err}");
    agent.kill().unwrap();
    agent.wait().unwrap();
    assert_eq!(zit::clean::clean(&fx.repo, false).unwrap().workspaces, 1);
    assert!(workspace::list(&fx.repo).unwrap().is_empty());
}

/// `dispose --orphaned` removes workspaces whose `zit run` is gone, keeping
/// those with unrecorded edits unless forced.
#[test]
fn dispose_orphaned_removes_workspaces_whose_owner_is_gone() {
    let dir = tempfile::tempdir().unwrap();
    let cli = common::Cli::new(dir.path(), &[("a.txt", "a\n")]);
    cli.run(&["init"]).ok();
    let mut gone = std::process::Command::new("true").spawn().unwrap();
    gone.wait().unwrap();
    let make = |agent: &str, owner: Option<u32>| -> String {
        let path = cli.run(&["materialise", "--agent", agent]).ok().stdout.trim().to_string();
        let meta_path = std::path::Path::new(&path).parent().unwrap().join("meta.json");
        let mut meta: serde_json::Value = serde_json::from_slice(&std::fs::read(&meta_path).unwrap()).unwrap();
        meta["pid"] = serde_json::json!(owner);
        std::fs::write(&meta_path, serde_json::to_vec(&meta).unwrap()).unwrap();
        path
    };
    let clean_orphan = make("clean-orphan", Some(gone.id()));
    let dirty_orphan = make("dirty-orphan", Some(gone.id()));
    std::fs::write(std::path::Path::new(&dirty_orphan).join("a.txt"), "edited\n").unwrap();
    let owned = make("owned", Some(std::process::id()));
    let unowned = make("unowned", None);
    let id =
        |path: &str| std::path::Path::new(path).parent().unwrap().file_name().unwrap().to_str().unwrap().to_string();

    let out = cli.run(&["dispose", "--orphaned"]).ok();
    assert!(out.stdout.contains(&format!("disposed {} (clean-orphan)", id(&clean_orphan))), "{}", out.stdout);
    assert!(
        out.stdout.contains(&format!("kept {} (dirty-orphan): unrecorded edits", id(&dirty_orphan))),
        "{}",
        out.stdout
    );
    assert!(!std::path::Path::new(&clean_orphan).exists());
    for still in [&dirty_orphan, &owned, &unowned] {
        assert!(std::path::Path::new(still).exists(), "{still}");
    }

    let forced = cli.run(&["dispose", "--orphaned", "--force", "--json"]).ok().json();
    assert_eq!(forced["disposed"], serde_json::json!([id(&dirty_orphan)]));
    assert_eq!(forced["kept"], serde_json::json!([]));
    assert!(!std::path::Path::new(&dirty_orphan).exists());
    assert!(std::path::Path::new(&owned).exists() && std::path::Path::new(&unowned).exists());
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
