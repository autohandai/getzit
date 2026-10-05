mod common;

use common::{git, write, Fixture};
use std::fs;
use zit::change::{self, Record};
use zit::resource::Resource;
use zit::workspace::{self, NewWorkspace, Strategy};

const FILES: &[(&str, &str)] = &[("src/a.rs", "fn a() {}\n"), ("README.md", "hello\n")];

fn both(test: impl Fn(Fixture)) {
    test(Fixture::with_strategy(FILES, Strategy::Clone));
    test(Fixture::with_strategy(FILES, Strategy::Checkout));
}

#[test]
fn materialise_gives_a_filesystem_view_of_current() {
    both(|fx| {
        let ws = fx.workspace("claude");
        assert_eq!(fs::read_to_string(ws.path().join("src/a.rs")).unwrap(), "fn a() {}\n");
        assert_eq!(ws.base, fx.repo.current().unwrap());
        assert_eq!(git(ws.path(), &["status", "--porcelain"]), "");
    });
}

#[test]
fn materialise_registers_no_git_worktree_and_no_branch() {
    let fx = Fixture::new(FILES);
    fx.workspace("claude");
    assert_eq!(git(&fx.root(), &["worktree", "list"]).lines().count(), 1);
    assert_eq!(git(&fx.root(), &["branch", "--format=%(refname:short)"]), "main");
    assert_eq!(git(&fx.root(), &["status", "--porcelain"]), "");
}

#[test]
fn record_turns_edits_into_a_change() {
    both(|fx| {
        let base = fx.repo.current().unwrap();
        let new = NewWorkspace { from: None, intent: "Add b", agent: "codex", session: Some("s-1") };
        let ws = workspace::materialise(&fx.repo, &new).unwrap();
        write(ws.path(), &[("src/b.rs", "fn b() {}\n")]);
        fs::remove_file(ws.path().join("README.md")).unwrap();

        let reads = vec![Resource::parse("src/a.rs#a")];
        let c = change::record(&fx.repo, &ws.id, &Record { reads, ..Default::default() }).unwrap().unwrap();

        assert_eq!(c.parents, vec![base]);
        assert_eq!((c.intent.as_str(), c.agent.as_str()), ("Add b", "codex"));
        assert_eq!(c.session.as_deref(), Some("s-1"));
        assert_eq!(c.reads, vec![Resource::parse("src/a.rs#a")]);
        assert_eq!(git(&fx.root(), &["ls-tree", "-r", "--name-only", c.id.as_str()]), "src/a.rs\nsrc/b.rs");
        assert_eq!(change::speculative(&fx.repo).unwrap(), vec![c.clone()]);
        assert_eq!(change::load(&fx.repo, &c.id).unwrap(), c);
    });
}

#[test]
fn record_without_edits_is_nothing() {
    both(|fx| {
        let ws = fx.workspace("claude");
        assert!(change::record(&fx.repo, &ws.id, &Record::default()).unwrap().is_none());
        assert!(change::speculative(&fx.repo).unwrap().is_empty());
    });
}

#[test]
fn record_sees_a_same_size_edit_made_right_after_materialising() {
    both(|fx| {
        let ws = fx.workspace("claude");
        write(ws.path(), &[("src/a.rs", "fn z() {}\n")]);
        assert!(change::record(&fx.repo, &ws.id, &Record::default()).unwrap().is_some());
    });
}

#[test]
fn ignored_files_are_not_part_of_the_state() {
    let fx = Fixture::new(&[(".gitignore", "target/\n"), ("a.rs", "fn a() {}\n")]);
    let ws = fx.workspace("claude");
    write(ws.path(), &[("target/out.bin", "x")]);
    assert!(change::record(&fx.repo, &ws.id, &Record::default()).unwrap().is_none());
}

#[test]
fn a_change_outlives_its_workspace() {
    both(|fx| {
        let c = fx.change("claude", &[("src/a.rs", "fn a() { 1 }\n")]);
        assert!(workspace::list(&fx.repo).unwrap().is_empty());
        let ws = fx.workspace_from(Some(&c.id), "codex");
        assert_eq!(fs::read_to_string(ws.path().join("src/a.rs")).unwrap(), "fn a() { 1 }\n");
        assert_eq!(git(ws.path(), &["status", "--porcelain"]), "");
    });
}

#[test]
fn recording_twice_chains_changes() {
    let fx = Fixture::new(FILES);
    let ws = fx.workspace("claude");
    write(ws.path(), &[("src/a.rs", "fn a() { 1 }\n")]);
    let c1 = change::record(&fx.repo, &ws.id, &Record::default()).unwrap().unwrap();
    write(ws.path(), &[("src/a.rs", "fn a() { 2 }\n")]);
    let c2 = change::record(&fx.repo, &ws.id, &Record::default()).unwrap().unwrap();
    assert_eq!(c2.parents, vec![c1.id]);
}

#[test]
fn list_tracks_every_live_workspace() {
    let fx = Fixture::new(FILES);
    let a = fx.workspace("claude");
    let b = fx.workspace("codex");
    write(b.path(), &[("src/a.rs", "fn a() { 1 }\n")]);

    let mut listed = workspace::list(&fx.repo).unwrap();
    listed.sort_by(|x, y| x.agent.cmp(&y.agent));
    assert_eq!(listed.iter().map(|w| w.agent.as_str()).collect::<Vec<_>>(), ["claude", "codex"]);
    assert!(!workspace::is_dirty(&fx.repo, &a).unwrap());
    assert!(workspace::is_dirty(&fx.repo, &b).unwrap());

    workspace::dispose(&fx.repo, &a.id).unwrap();
    assert_eq!(workspace::list(&fx.repo).unwrap().len(), 1);
    assert!(!a.path().exists());
}

#[test]
fn declared_reads_are_carried_into_the_change() {
    let fx = Fixture::new(FILES);
    let ws = fx.workspace("claude");
    workspace::declare_reads(&fx.repo, &ws.id, &[Resource::parse("README.md")]).unwrap();
    write(ws.path(), &[("src/a.rs", "fn a() { 1 }\n")]);
    let c = change::record(&fx.repo, &ws.id, &Record::default()).unwrap().unwrap();
    assert_eq!(c.reads, vec![Resource::parse("README.md")]);
}

#[test]
fn materialising_a_new_state_reuses_the_cached_one() {
    let fx = Fixture::new(FILES);
    let c = fx.change("claude", &[("src/new.rs", "fn n() {}\n"), ("src/a.rs", "fn a() { 9 }\n")]);
    let ws = fx.workspace_from(Some(&c.id), "codex");
    assert_eq!(fs::read_to_string(ws.path().join("src/new.rs")).unwrap(), "fn n() {}\n");
    assert_eq!(fs::read_to_string(ws.path().join("src/a.rs")).unwrap(), "fn a() { 9 }\n");
    assert_eq!(fs::read_to_string(ws.path().join("README.md")).unwrap(), "hello\n");
    assert_eq!(git(ws.path(), &["status", "--porcelain"]), "");
}

#[test]
fn a_workspace_id_is_never_a_path() {
    let fx = Fixture::new(FILES);
    let ws = fx.workspace("claude");
    let sneaky = format!("../ws/{}", ws.id);
    assert!(workspace::dispose(&fx.repo, &sneaky).is_err());
    assert!(workspace::get(&fx.repo, &sneaky).is_err());
    assert!(ws.path().exists());
}

#[test]
fn a_same_size_edit_right_after_a_delta_materialisation_is_recorded() {
    let fx = Fixture::new(FILES);
    fx.workspace("warm-the-cache");
    let c = fx.change("claude", &[("src/a.rs", "fn b() {}\n")]);
    let ws = fx.workspace_from(Some(&c.id), "codex");
    write(ws.path(), &[("src/a.rs", "fn c() {}\n")]);
    assert!(change::record(&fx.repo, &ws.id, &Record::default()).unwrap().is_some());
}

#[test]
fn an_agent_needs_a_name_git_will_accept() {
    let fx = Fixture::new(FILES);
    for bad in ["", " ", "."] {
        let new = NewWorkspace { from: None, intent: "x", agent: bad, session: None };
        assert!(workspace::materialise(&fx.repo, &new).is_err(), "{bad:?}");
    }
    assert!(workspace::list(&fx.repo).unwrap().is_empty(), "a refused materialise leaves nothing behind");
}

#[test]
fn a_revision_is_never_an_option() {
    let fx = Fixture::new(FILES);
    assert!(matches!(fx.repo.resolve("--all"), Err(zit::Error::UnknownRevision(_))));
    assert!(matches!(fx.repo.resolve_state("-h"), Err(zit::Error::UnknownRevision(_))));
}

/// Tools leave files behind (Python bytecode, editor and OS files). They are
/// never source, so they are never part of a change.
#[test]
fn tool_byproducts_are_never_recorded() {
    let fx = Fixture::new(FILES);
    let ws = fx.workspace("claude");
    write(
        ws.path(),
        &[
            ("scripts/__pycache__/gen.cpython-310.pyc", "x"),
            ("src/.DS_Store", "x"),
            (".pytest_cache/v/x", "x"),
            ("a.py.swp", "x"),
        ],
    );
    assert!(change::record(&fx.repo, &ws.id, &Record::default()).unwrap().is_none());
}

#[test]
fn a_project_can_ignore_more_for_agents() {
    let fx = Fixture::new(&[("zit.toml", "ignore = [\"agent-notes/\", \"*.log\"]\n"), ("a.txt", "a\n")]);
    let ws = fx.workspace("claude");
    write(ws.path(), &[("agent-notes/plan.md", "x"), ("debug.log", "x")]);
    assert!(change::record(&fx.repo, &ws.id, &Record::default()).unwrap().is_none());
    write(ws.path(), &[("a.txt", "b\n")]);
    assert!(change::record(&fx.repo, &ws.id, &Record::default()).unwrap().is_some(), "real edits still count");
}

#[test]
fn ignoring_never_hides_a_tracked_file() {
    let fx = Fixture::new(&[("vendored/__pycache__/keep.pyc", "v1"), ("a.txt", "a\n")]);
    let ws = fx.workspace("claude");
    write(ws.path(), &[("vendored/__pycache__/keep.pyc", "v2")]);
    assert!(change::record(&fx.repo, &ws.id, &Record::default()).unwrap().is_some());
}

/// Coding agents keep session state in the project; it is the agent's, not the work.
#[test]
fn agents_session_state_is_never_recorded() {
    let fx = Fixture::new(FILES);
    let ws = fx.workspace("autohand");
    write(
        ws.path(),
        &[
            (".autohand/memory/index.json", "{}"),
            (".autohand/settings.local.json", "{}"),
            (".autohand/goals.local.json", "{}"),
            (".autohand/session-permissions.json", "{}"),
            (".claude/settings.local.json", "{}"),
        ],
    );
    assert!(change::record(&fx.repo, &ws.id, &Record::default()).unwrap().is_none());
}
