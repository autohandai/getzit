#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::TempDir;
use zit::change::{self, Change, Record};
use zit::workspace::{self, NewWorkspace, Strategy, Workspace};
use zit::{Oid, Repo};

/// A throwaway git repository with zit initialised on its first commit.
pub struct Fixture {
    pub dir: TempDir,
    pub repo: Repo,
}

pub fn git(cwd: &Path, args: &[&str]) -> String {
    let out = Command::new("git").current_dir(cwd).args(args).output().unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

pub fn write(root: &Path, files: &[(&str, &str)]) {
    for (path, body) in files {
        let p = root.join(path);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }
}

impl Fixture {
    pub fn new(files: &[(&str, &str)]) -> Fixture {
        Fixture::with_strategy(files, Strategy::Clone)
    }

    pub fn with_strategy(files: &[(&str, &str)], strategy: Strategy) -> Fixture {
        let dir = TempDir::new().unwrap();
        let root = dir.path().join("repo");
        std::fs::create_dir(&root).unwrap();
        git(&root, &["init", "-q", "-b", "main"]);
        git(&root, &["config", "user.name", "human"]);
        git(&root, &["config", "user.email", "human@example.com"]);
        write(&root, files);
        git(&root, &["add", "-A"]);
        git(&root, &["commit", "-q", "-m", "genesis"]);
        let repo = Repo::open(&root, &dir.path().join("home")).unwrap().with_strategy(strategy);
        repo.init(None).unwrap();
        Fixture { dir, repo }
    }

    pub fn root(&self) -> PathBuf {
        self.dir.path().join("repo")
    }

    pub fn workspace(&self, agent: &str) -> Workspace {
        self.workspace_from(None, agent)
    }

    pub fn workspace_from(&self, from: Option<&Oid>, agent: &str) -> Workspace {
        let new = NewWorkspace { from, intent: "work", agent, session: None };
        workspace::materialise(&self.repo, &new).unwrap()
    }

    /// One agent turn: materialise from `from` (default current), apply
    /// `files`, record, dispose.
    pub fn change_from(&self, from: Option<&Oid>, agent: &str, files: &[(&str, &str)]) -> Change {
        let ws = self.workspace_from(from, agent);
        write(ws.path(), files);
        let change = change::record(&self.repo, &ws.id, &Record::default()).unwrap().unwrap();
        workspace::dispose(&self.repo, &ws.id).unwrap();
        change
    }

    pub fn change(&self, agent: &str, files: &[(&str, &str)]) -> Change {
        self.change_from(None, agent, files)
    }
}

/// The `zit` binary pointed at a fixture.
pub struct Cli {
    pub root: PathBuf,
    pub home: PathBuf,
}

pub struct Ran {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

impl Ran {
    pub fn json(&self) -> serde_json::Value {
        serde_json::from_str(&self.stdout)
            .unwrap_or_else(|e| panic!("not json ({e}): {}\n{}", self.stdout, self.stderr))
    }
    pub fn ok(self) -> Ran {
        assert_eq!(self.code, 0, "stdout: {}\nstderr: {}", self.stdout, self.stderr);
        self
    }
}

impl Cli {
    /// A git repository with one commit; zit not yet initialised.
    pub fn new(dir: &Path, files: &[(&str, &str)]) -> Cli {
        let root = dir.join("repo");
        std::fs::create_dir(&root).unwrap();
        git(&root, &["init", "-q", "-b", "main"]);
        git(&root, &["config", "user.name", "human"]);
        git(&root, &["config", "user.email", "human@example.com"]);
        write(&root, files);
        git(&root, &["add", "-A"]);
        git(&root, &["commit", "-q", "-m", "genesis"]);
        Cli { root, home: dir.join("home") }
    }

    pub fn command(&self, cwd: &Path) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_zit"));
        cmd.current_dir(cwd).env("ZIT_HOME", &self.home).env_remove("ZIT_MATERIALISE");
        cmd
    }

    pub fn run_in(&self, cwd: &Path, args: &[&str]) -> Ran {
        let out = self.command(cwd).args(args).output().unwrap();
        Ran {
            code: out.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&out.stdout).to_string(),
            stderr: String::from_utf8_lossy(&out.stderr).to_string(),
        }
    }

    pub fn run(&self, args: &[&str]) -> Ran {
        self.run_in(&self.root, args)
    }
}
