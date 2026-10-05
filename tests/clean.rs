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
    if cfg!(target_os = "macos") {
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
