mod common;

use common::{git, write, Cli};
use tempfile::TempDir;

#[test]
fn status_exposes_task_fresh_diff_and_actionable_safety_without_changing_the_index() {
    let dir = TempDir::new().unwrap();
    let cli = Cli::new(dir.path(), &[("a.txt", "one\ntwo\n")]);
    cli.run(&["init"]).ok();
    let ws = cli.run(&["--json", "materialise", "--agent", "codex", "--intent", "Fix the pricing bug"]).ok().json();
    let path = std::path::Path::new(ws["path"].as_str().unwrap());
    // Warm the advisory footprint cache before editing: safety must still be fresh.
    cli.run(&["--json", "status"]).ok();
    write(path, &[("a.txt", "one\nthree\n"), ("new.txt", "new\n")]);
    git(path, &["add", "a.txt"]);
    let before = git(path, &["diff", "--cached"]);
    let report = cli.run(&["--json", "status"]).ok().json();
    let row = &report["workspaces"][0];
    assert_eq!(row["intent"], "Fix the pricing bug");
    assert_eq!(row["diff"]["files"], 2, "{report}");
    assert_eq!(row["diff"]["added"], 2);
    assert_eq!(row["diff"]["removed"], 1);
    assert_eq!(row["cleanup"]["state"], "blocked");
    assert_eq!(row["merge"]["state"], "blocked");
    assert_eq!(before, git(path, &["diff", "--cached"]));
    let text = cli.run(&["status"]).ok().stdout;
    assert!(text.contains("Fix the pricing bug") && text.contains("2 files +2 -1"), "{text}");
    assert!(text.contains("merge: blocked") && text.contains("cleanup: blocked"), "{text}");
}

#[test]
fn recorded_workspace_can_be_disposed_while_its_change_still_needs_acceptance() {
    let fx = common::Fixture::new(&[("a.txt", "old\n")]);
    let ws = fx.workspace("codex");
    write(ws.path(), &[("a.txt", "new\n")]);
    let change = zit::change::record(&fx.repo, &ws.id, &Default::default()).unwrap().unwrap();
    let report = zit::api::overview(&fx.repo).unwrap();
    let row = &report.workspaces[0];
    assert!(!row.dirty);
    assert_eq!(row.diff.as_ref().unwrap().files, 0);
    assert_eq!(row.cleanup.state, zit::status::Readiness::Review);
    assert_eq!(row.merge.state, zit::status::Readiness::Ready);
    assert_eq!(report.changes[0].diff.as_ref().unwrap().added, 1);
    zit::accept::accept(&fx.repo, &change.id).unwrap();
    assert_eq!(zit::api::overview(&fx.repo).unwrap().workspaces[0].merge.state, zit::status::Readiness::NotNeeded);
}

#[test]
fn status_reports_failed_inspection_instead_of_a_clean_workspace() {
    let fx = common::Fixture::new(&[("a.txt", "old\n")]);
    let ws = fx.workspace("codex");
    std::fs::remove_dir_all(ws.path()).unwrap();
    let report = zit::api::overview(&fx.repo).unwrap();
    let row = &report.workspaces[0];
    assert!(row.dirty);
    assert!(row.diff.is_none() && row.inspection_error.is_some());
    assert_eq!(row.cleanup.state, zit::status::Readiness::Unknown);
    assert_eq!(row.merge.state, zit::status::Readiness::Unknown);
}

#[test]
fn running_owner_blocks_merge_and_cleanup_even_with_no_edits() {
    let fx = common::Fixture::new(&[("a.txt", "old\n")]);
    let ws = fx.workspace("codex");
    let _owner = ws.hold().unwrap();
    let report = zit::api::overview(&fx.repo).unwrap();
    assert_eq!(report.workspaces[0].alive, Some(true));
    assert_eq!(report.workspaces[0].cleanup.state, zit::status::Readiness::Blocked);
    assert_eq!(report.workspaces[0].merge.state, zit::status::Readiness::Blocked);
}

#[test]
fn verified_own_state_is_not_ready_when_composed_state_needs_checks() {
    let fx = common::Fixture::new(&[
        ("zit.toml", "[[check]]\nname = \"all\"\nrun = \"true\"\n"),
        ("a.txt", "old\n"),
        ("b.txt", "old\n"),
    ]);
    let first = fx.change("a", &[("a.txt", "new\n")]);
    let second = fx.change("b", &[("b.txt", "new\n")]);
    zit::evidence::verify(&fx.repo, &second.id, false).unwrap();
    zit::accept::accept(&fx.repo, &first.id).unwrap();
    let report = zit::api::overview(&fx.repo).unwrap();
    let row = report.changes.iter().find(|r| r.change.id == second.id).unwrap();
    assert_eq!(row.status, zit::accept::Status::Verified);
    assert_eq!(row.merge.state, zit::status::Readiness::Review);
    assert!(row.merge.reasons.join(" ").contains("all"));
}

#[test]
fn staged_only_edits_empty_untracked_and_binary_files_are_not_safe_to_drop() {
    let fx = common::Fixture::new(&[("a.txt", "old\n")]);
    let ws = fx.workspace("codex");
    write(ws.path(), &[("a.txt", "staged\n")]);
    git(ws.path(), &["add", "a.txt"]);
    write(ws.path(), &[("a.txt", "old\n"), ("empty.txt", "")]);
    std::fs::write(ws.path().join("image.bin"), b"\0\x01\x02").unwrap();
    let report = zit::api::overview(&fx.repo).unwrap();
    let row = &report.workspaces[0];
    let diff = row.diff.as_ref().unwrap();
    assert_eq!(diff.files, 2);
    assert_eq!(diff.binary, 1);
    assert!(row.dirty);
    assert_eq!(row.cleanup.state, zit::status::Readiness::Blocked);
}

#[test]
fn status_does_not_execute_clean_filters_or_fsmonitor() {
    let fx = common::Fixture::new(&[("a.txt", "old\n"), (".gitattributes", "*.txt filter=hostile\n")]);
    let marker = fx.dir.path().join("FILTER_RAN");
    let filter = format!("echo filter >> '{}'; cat", marker.display());
    let monitor = format!("echo fsmonitor >> '{}'", marker.display());
    git(&fx.root(), &["config", "filter.hostile.clean", &filter]);
    git(&fx.root(), &["config", "core.fsmonitor", &monitor]);
    let ws = fx.workspace("codex");
    // Materialisation is outside the status contract.
    let _ = std::fs::remove_file(&marker);
    write(ws.path(), &[("a.txt", "new\n")]);
    let report = zit::api::overview(&fx.repo).unwrap();
    assert!(report.workspaces[0].diff.is_some());
    assert!(!marker.exists(), "status executed repository commands: {:?}", std::fs::read_to_string(&marker));
}

#[test]
fn status_leaves_all_refs_and_the_primary_index_unchanged() {
    let fx = common::Fixture::new(&[("a.txt", "old\n")]);
    let change = fx.change("codex", &[("a.txt", "new\n")]);
    zit::accept::accept(&fx.repo, &change.id).unwrap();
    // A fetched clone may retain an already-accepted change ref.
    git(&fx.root(), &["update-ref", &format!("refs/zit/changes/{}", change.id), change.id.as_str()]);
    let before = git(&fx.root(), &["show-ref"]);
    let index = std::fs::read(fx.root().join(".git/index")).unwrap();
    zit::api::overview(&fx.repo).unwrap();
    assert_eq!(before, git(&fx.root(), &["show-ref"]));
    assert_eq!(index, std::fs::read(fx.root().join(".git/index")).unwrap());
}

#[test]
fn failed_composed_state_evidence_blocks_an_individually_verified_change() {
    let fx = common::Fixture::new(&[
        ("zit.toml", "[[check]]\nname = \"compatible\"\nrun = \"! (grep -q new a.txt && grep -q new b.txt)\"\n"),
        ("a.txt", "old\n"),
        ("b.txt", "old\n"),
    ]);
    let first = fx.change("a", &[("a.txt", "new\n")]);
    let second = fx.change("b", &[("b.txt", "new\n")]);
    zit::evidence::verify(&fx.repo, &second.id, false).unwrap();
    zit::accept::accept(&fx.repo, &first.id).unwrap();
    assert!(matches!(zit::accept::accept(&fx.repo, &second.id).unwrap(), zit::accept::Outcome::Rejected(_)));
    let report = zit::api::overview(&fx.repo).unwrap();
    let row = report.changes.iter().find(|r| r.change.id == second.id).unwrap();
    assert_eq!(row.status, zit::accept::Status::Verified);
    assert_eq!(row.merge.state, zit::status::Readiness::Blocked);
    assert!(row.merge.reasons.join(" ").contains("compatible"));
}

#[test]
fn completed_managed_run_is_cleanup_ready_and_its_recorded_change_survives() {
    let dir = TempDir::new().unwrap();
    let cli = Cli::new(dir.path(), &[("a.txt", "old\n")]);
    cli.run(&["init"]).ok();
    cli.run(&[
        "run",
        "--keep",
        "--agent",
        "codex",
        "--intent",
        "Fix pricing",
        "--",
        "sh",
        "-c",
        "printf 'new\\n' > a.txt",
    ])
    .ok();
    let report = cli.run(&["--json", "status"]).ok().json();
    let ws = &report["workspaces"][0];
    assert_eq!(ws["alive"], false);
    assert_eq!(ws["cleanup"]["state"], "ready");
    assert_eq!(ws["merge"]["state"], "ready");
    let base = ws["base"].as_str().unwrap();
    cli.run(&["dispose", ws["id"].as_str().unwrap()]).ok();
    let after = cli.run(&["--json", "status"]).ok().json();
    assert!(after["workspaces"].as_array().unwrap().is_empty());
    assert!(after["changes"].as_array().unwrap().iter().any(|c| c["id"] == base));
}
