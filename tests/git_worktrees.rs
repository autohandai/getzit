mod common;

use common::{git, write, Fixture};
use std::path::{Path, PathBuf};
use zit::worktrees::{self, GitWorktreeRow};

fn linked(fx: &Fixture, name: &str) -> PathBuf {
    let path = fx.dir.path().join(name);
    git(&fx.root(), &["worktree", "add", "-q", "-b", name, path.to_str().unwrap()]);
    path
}

fn row_at<'a>(rows: &'a [GitWorktreeRow], path: &Path) -> &'a GitWorktreeRow {
    let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    rows.iter().find(|row| row.path == path).expect("worktree is listed")
}

fn state(assessment: &zit::status::Assessment) -> String {
    serde_json::to_value(assessment).unwrap()["state"].as_str().unwrap().to_string()
}

#[test]
fn ordinary_worktrees_show_explicit_task_live_diff_and_preserve_the_index() {
    let fx = Fixture::new(&[("a.txt", "old\n")]);
    let path = linked(&fx, "agent-checkout");
    git(&fx.root(), &["config", "branch.agent-checkout.zit-task", "Implement the discount"]);
    git(&fx.root(), &["config", "branch.agent-checkout.zit-agent", "codex"]);
    write(&path, &[("a.txt", "new\n"), ("untracked.txt", "one\ntwo\n")]);
    git(&path, &["add", "a.txt"]);
    let index_path = git(&path, &["rev-parse", "--path-format=absolute", "--git-path", "index"]);
    let before = std::fs::read(&index_path).unwrap();
    let rows = worktrees::list(&fx.repo, &fx.repo.current().unwrap()).unwrap();
    let row = row_at(&rows, &path);
    assert_eq!(row.task.as_deref(), Some("Implement the discount"));
    assert_eq!(row.task_source.as_deref(), Some("branch.zit-task"));
    assert_eq!(row.agent.as_deref(), Some("codex"));
    assert_eq!(row.dirty, Some(true));
    let diff = row.diff.as_ref().expect("diff is inspected");
    assert_eq!((diff.files, diff.added, diff.removed, diff.binary), (2, 3, 1, 0));
    assert_eq!(state(&row.merge), "blocked");
    assert_eq!(state(&row.cleanup), "blocked");
    assert_eq!(std::fs::read(index_path).unwrap(), before, "status leaves the real index byte-for-byte intact");
    assert!(row_at(&rows, &fx.root()).primary);
}

#[test]
fn clean_retained_worktrees_still_require_an_owner_check_and_primary_is_protected() {
    let fx = Fixture::new(&[("a.txt", "old\n")]);
    let path = linked(&fx, "done");
    git(&fx.root(), &["config", "branch.done.description", "Fix the invoice calculation"]);
    let rows = worktrees::list(&fx.repo, &fx.repo.current().unwrap()).unwrap();
    let row = row_at(&rows, &path);
    assert_eq!(row.task.as_deref(), Some("Fix the invoice calculation"));
    assert_eq!(row.task_source.as_deref(), Some("branch.description"));
    assert_eq!(row.agent, None, "commit authors do not identify an agent");
    assert_eq!(row.dirty, Some(false));
    assert_eq!(row.diff.as_ref().unwrap().files, 0);
    assert_eq!(state(&row.merge), "not-needed");
    assert_eq!(state(&row.cleanup), "review");
    assert_eq!(state(&row_at(&rows, &fx.root()).cleanup), "blocked");
    let from_linked = zit::Repo::open(&path, &fx.dir.path().join("home")).unwrap();
    let again = worktrees::list(&from_linked, &fx.repo.current().unwrap()).unwrap();
    assert_eq!(rows.iter().map(|r| &r.path).collect::<Vec<_>>(), again.iter().map(|r| &r.path).collect::<Vec<_>>());
}

#[test]
fn binary_untracked_files_and_a_newline_in_the_worktree_path_are_not_lost() {
    let fx = Fixture::new(&[("a.txt", "old\n")]);
    let path = fx.dir.path().join("agent checkout\n");
    git(&fx.root(), &["worktree", "add", "-q", "--detach", path.to_str().unwrap()]);
    std::fs::write(path.join("image.bin"), [0_u8, 1, 2, 3]).unwrap();
    std::fs::write(path.join("empty.txt"), []).unwrap();
    let rows = worktrees::list(&fx.repo, &fx.repo.current().unwrap()).unwrap();
    let row = row_at(&rows, &path);
    assert!(row.detached);
    assert_eq!(row.dirty, Some(true));
    let diff = row.diff.as_ref().expect("newline paths are inspectable");
    assert_eq!((diff.files, diff.added, diff.removed, diff.binary), (2, 0, 0, 1));
    assert_eq!(state(&row.cleanup), "blocked");
    let json = serde_json::to_value(row).unwrap();
    assert!(json["path"].as_str().unwrap().ends_with('\n'));
}

#[test]
fn locked_missing_and_in_progress_worktrees_remain_visible_and_are_not_cleanup_ready() {
    let fx = Fixture::new(&[("a.txt", "old\n")]);
    let locked = linked(&fx, "locked");
    let missing = linked(&fx, "missing").canonicalize().unwrap();
    let merging = linked(&fx, "merging");
    git(&fx.root(), &["worktree", "lock", "--reason", "agent is still working", locked.to_str().unwrap()]);
    std::fs::remove_dir_all(&missing).unwrap();
    let admin = git(&merging, &["rev-parse", "--absolute-git-dir"]);
    std::fs::write(Path::new(&admin).join("MERGE_HEAD"), format!("{}\n", fx.repo.current().unwrap())).unwrap();
    let rows = worktrees::list(&fx.repo, &fx.repo.current().unwrap()).unwrap();
    assert_eq!(rows.len(), 4);
    let locked = row_at(&rows, &locked);
    assert_eq!(locked.locked.as_deref(), Some("agent is still working"));
    assert_eq!(state(&locked.cleanup), "blocked");
    let missing = row_at(&rows, &missing);
    assert!(missing.prunable.is_some());
    assert!(missing.error.as_deref().unwrap().contains("missing"));
    assert_eq!(missing.diff, None);
    assert_eq!(state(&missing.cleanup), "unknown");
    let merging = row_at(&rows, &merging);
    assert_eq!(state(&merging.merge), "blocked");
    assert_eq!(state(&merging.cleanup), "blocked");
}

#[test]
fn unmerged_commits_require_integration_even_when_the_patch_is_equivalent() {
    let fx = Fixture::new(&[("a.txt", "old\n")]);
    let path = linked(&fx, "agent");
    write(&path, &[("a.txt", "same patch\n")]);
    git(&path, &["add", "a.txt"]);
    git(&path, &["commit", "-q", "-m", "Agent task\n\nZit-Agent: codex"]);
    write(&fx.root(), &[("a.txt", "same patch\n")]);
    git(&fx.root(), &["add", "a.txt"]);
    git(&fx.root(), &["commit", "-q", "-m", "Human equivalent"]);
    git(&fx.root(), &["update-ref", "refs/zit/current", "HEAD"]);
    let rows = worktrees::list(&fx.repo, &fx.repo.current().unwrap()).unwrap();
    let row = row_at(&rows, &path);
    assert_eq!(row.task.as_deref(), Some("Agent task"));
    assert_eq!(row.task_source.as_deref(), Some("commit.subject"));
    assert_eq!(row.agent.as_deref(), Some("codex"));
    assert_eq!(state(&row.merge), "review");
    assert_eq!(state(&row.cleanup), "blocked", "patch equivalence is not retained ancestry");
}

#[test]
fn an_actual_text_conflict_blocks_merge_without_moving_any_ref() {
    let fx = Fixture::new(&[("a.txt", "old\n")]);
    let path = linked(&fx, "agent");
    write(&path, &[("a.txt", "agent\n")]);
    git(&path, &["add", "a.txt"]);
    git(&path, &["commit", "-q", "-m", "agent task"]);
    write(&fx.root(), &[("a.txt", "human\n")]);
    git(&fx.root(), &["add", "a.txt"]);
    git(&fx.root(), &["commit", "-q", "-m", "human task"]);
    git(&fx.root(), &["update-ref", "refs/zit/current", "HEAD"]);
    let before = git(&fx.root(), &["show-ref"]);
    let rows = worktrees::list(&fx.repo, &fx.repo.current().unwrap()).unwrap();
    let row = row_at(&rows, &path);
    assert_eq!(state(&row.merge), "blocked");
    assert!(row.merge.reasons.iter().any(|r| r.contains("conflict")));
    assert_eq!(git(&fx.root(), &["show-ref"]), before);
}

#[test]
fn a_staged_only_edit_blocks_cleanup_even_when_the_final_files_match_head() {
    let fx = Fixture::new(&[("a.txt", "old\n")]);
    let path = linked(&fx, "staged");
    write(&path, &[("a.txt", "staged\n")]);
    git(&path, &["add", "a.txt"]);
    write(&path, &[("a.txt", "old\n")]);
    let rows = worktrees::list(&fx.repo, &fx.repo.current().unwrap()).unwrap();
    let row = row_at(&rows, &path);
    assert_eq!(
        row.diff.as_ref().unwrap().files,
        0,
        "diff describes final files, while dirty includes staged-only work"
    );
    assert_eq!(row.dirty, Some(true));
    assert_eq!(state(&row.cleanup), "blocked");
}

#[cfg(target_os = "linux")]
#[test]
fn non_utf8_worktree_paths_preserve_lossless_machine_bytes() {
    use std::ffi::OsString;
    use std::os::unix::ffi::{OsStrExt, OsStringExt};
    let fx = Fixture::new(&[("a.txt", "old\n")]);
    let path = fx.dir.path().join(OsString::from_vec(b"agent-\xff".to_vec()));
    let output = std::process::Command::new("git")
        .current_dir(fx.root())
        .args(["worktree", "add", "-q", "--detach"])
        .arg(&path)
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let rows = worktrees::list(&fx.repo, &fx.repo.current().unwrap()).unwrap();
    let row = row_at(&rows, &path);
    assert!(row.error.is_none(), "{:?}", row.error);
    assert_eq!(row.path_bytes.as_deref(), Some(row.path.as_os_str().as_bytes()));
    assert!(serde_json::to_value(row).unwrap()["path_bytes"].is_array());
}

#[test]
fn status_does_not_run_clean_filters_fsmonitor_or_custom_merge_drivers() {
    use std::os::unix::fs::PermissionsExt;
    let fx = Fixture::new(&[("a.txt", "old\n"), (".gitattributes", "*.txt filter=tripwire merge=tripwire\n")]);
    let path = linked(&fx, "agent");
    write(&path, &[("a.txt", "agent\n")]);
    git(&path, &["add", "a.txt"]);
    git(&path, &["commit", "-q", "-m", "agent task"]);
    let marker = fx.dir.path().join("command-ran");
    let script = fx.dir.path().join("tripwire.sh");
    std::fs::write(&script, format!("#!/bin/sh\ntouch '{}'\ncat\n", marker.display())).unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    for key in ["filter.tripwire.clean", "core.fsmonitor", "merge.tripwire.driver"] {
        git(&fx.root(), &["config", key, script.to_str().unwrap()]);
    }
    git(&fx.root(), &["config", "filter.tripwire.required", "true"]);
    let rows = worktrees::list(&fx.repo, &fx.repo.current().unwrap()).unwrap();
    assert!(!marker.exists(), "status executed a repository command");
    let row = row_at(&rows, &path);
    assert!(row.error.is_none(), "{:?}", row.error);
    assert_eq!(state(&row.merge), "unknown");
}

#[test]
fn linked_worktree_configuration_does_not_run_checkout_commands() {
    use std::os::unix::fs::PermissionsExt;
    for key in ["filter.tripwire.clean", "filter.tripwire.process", "core.fsmonitor", "merge.tripwire.driver"] {
        let fx = Fixture::new(&[("a.txt", "old\n"), (".gitattributes", "*.txt filter=tripwire merge=tripwire\n")]);
        let path = linked(&fx, "agent");
        write(&path, &[("a.txt", "agent\n")]);
        git(&path, &["add", "a.txt"]);
        git(&path, &["commit", "-q", "-m", "agent task"]);
        git(&fx.root(), &["config", "extensions.worktreeConfig", "true"]);
        let marker = fx.dir.path().join("worktree-command-ran");
        let script = fx.dir.path().join("worktree-tripwire.sh");
        let action = if key.ends_with(".clean") { "cat" } else { "exit 1" };
        std::fs::write(&script, format!("#!/bin/sh\ntouch '{}'\n{action}\n", marker.display())).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        git(&path, &["config", "--worktree", key, script.to_str().unwrap()]);
        if key.starts_with("filter.") {
            git(&path, &["config", "--worktree", "filter.tripwire.required", "true"]);
        }
        let rows = worktrees::list(&fx.repo, &fx.repo.current().unwrap()).unwrap();
        assert!(!marker.exists(), "status executed linked worktree {key}");
        let row = row_at(&rows, &path);
        assert!(row.error.is_none(), "{key}: {:?}", row.error);
        if key.starts_with("merge.") {
            assert_eq!(state(&row.merge), "unknown", "linked merge driver must not be ignored");
        }
    }
}

#[test]
fn a_bare_repository_is_preserved_as_a_non_disposable_inventory_entry() {
    let fx = Fixture::new(&[("a.txt", "old\n")]);
    let bare = fx.dir.path().join("bare.git");
    git(fx.dir.path(), &["clone", "--bare", "-q", fx.root().to_str().unwrap(), bare.to_str().unwrap()]);
    let repo = zit::Repo::open(&bare, &fx.dir.path().join("bare-home")).unwrap();
    repo.init(None).unwrap();
    let rows = worktrees::list(&repo, &repo.current().unwrap()).unwrap();
    assert_eq!(rows.len(), 1);
    let row = row_at(&rows, &bare);
    assert!(row.bare && row.primary);
    assert_eq!(state(&row.merge), "not-needed");
    assert_eq!(state(&row.cleanup), "blocked");
    assert!(row.diff.is_none());
}

#[test]
fn sparse_checkout_exclusions_are_not_reported_as_deleted_files() {
    let fx = Fixture::new(&[("src/a.txt", "source\n"), ("docs/readme.txt", "docs\n")]);
    let path = linked(&fx, "sparse");
    git(&path, &["sparse-checkout", "set", "src"]);
    assert!(!path.join("docs/readme.txt").exists());
    let rows = worktrees::list(&fx.repo, &fx.repo.current().unwrap()).unwrap();
    let row = row_at(&rows, &path);
    assert!(row.error.is_none(), "{:?}", row.error);
    assert_eq!(row.dirty, Some(false));
    assert_eq!(row.diff.as_ref().unwrap().files, 0);
}
