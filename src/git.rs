//! The store. Git's object database holds every state (tree) and change
//! (commit); `refs/zit/*` keeps them alive and makes them replicable.

use crate::workspace::Strategy;
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::ffi::OsStr;
use std::fmt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

pub const CURRENT: &str = "refs/zit/current";
pub const CHANGES: &str = "refs/zit/changes";
pub const EVIDENCE: &str = "refs/zit/evidence";

/// Content address of a git object.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Oid(String);

impl Oid {
    pub fn new(s: impl Into<String>) -> Oid {
        Oid(s.into())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
    pub fn short(&self) -> &str {
        &self.0[..self.0.len().min(10)]
    }
}

impl fmt::Display for Oid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<OsStr> for Oid {
    fn as_ref(&self) -> &OsStr {
        self.0.as_ref()
    }
}

/// Handle to one repository's graph.
#[derive(Debug, Clone)]
pub struct Repo {
    git_dir: PathBuf,
    home: PathBuf,
    pub(crate) strategy: Strategy,
    /// Extra objects to read, written somewhere other than the repository.
    alternate: Option<PathBuf>,
    /// Content of `<id>:<path>` names read so far: an id's tree never
    /// changes, so each is read from git once per process.
    remembered: Arc<Mutex<HashMap<String, Option<Vec<u8>>>>>,
}

/// How many remembered objects to keep before starting over (a long-lived
/// `zit mcp` sees many ids; each entry is one small file).
const REMEMBER_LIMIT: usize = 4096;

/// The oldest git Zit works with: `merge-tree --write-tree` arrived in 2.38.
pub const MIN_GIT: (u32, u32) = (2, 38);

/// Fail early, naming the version needed, when git is older than [`MIN_GIT`].
pub fn require_git() -> Result<()> {
    let out = git_command().arg("--version").output().map_err(|e| Error::msg(format!("cannot run git: {e}")))?;
    let text = String::from_utf8_lossy(&out.stdout);
    let version = text.split_whitespace().nth(2).unwrap_or_default();
    let mut parts = version.split('.').map(|p| p.parse::<u32>().unwrap_or(0));
    let found = (parts.next().unwrap_or(0), parts.next().unwrap_or(0));
    if found < MIN_GIT {
        return Err(Error::msg(format!(
            "Zit needs git {}.{} or newer (for `merge-tree --write-tree`); found {}",
            MIN_GIT.0,
            MIN_GIT.1,
            text.trim()
        )));
    }
    Ok(())
}

/// A `git` invocation (binary overridable with `$ZIT_GIT`) isolated from
/// any ambient repository environment.
pub(crate) fn git_command() -> Command {
    let mut cmd = Command::new(std::env::var_os("ZIT_GIT").unwrap_or_else(|| "git".into()));
    for var in ["GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE", "GIT_COMMON_DIR", "GIT_PREFIX"] {
        cmd.env_remove(var);
    }
    cmd
}

/// With `$ZIT_TRACE` set, report how long each git call took on stderr.
pub(crate) struct Trace(Option<(std::time::Instant, String)>);

impl Trace {
    pub(crate) fn start(cmd: &Command) -> Trace {
        Trace(std::env::var_os("ZIT_TRACE").map(|_| {
            let args: Vec<_> = cmd.get_args().map(|a| a.to_string_lossy().into_owned()).collect();
            (std::time::Instant::now(), args.join(" "))
        }))
    }
}

impl Drop for Trace {
    fn drop(&mut self) {
        if let Some((started, args)) = &self.0 {
            let args =
                args.strip_prefix("--git-dir ").and_then(|a| a.split_once(' ')).map_or(args.as_str(), |(_, rest)| rest);
            let shown: String = args.chars().take(110).collect();
            eprintln!("zit-trace {:>6.1}ms git {shown}", started.elapsed().as_secs_f64() * 1000.0);
        }
    }
}

/// A full object id, which names its content forever.
fn is_id(rev: &str) -> bool {
    rev.len() == 40 && rev.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Run to completion; trimmed stdout on success.
pub(crate) fn run(cmd: &mut Command) -> Result<String> {
    let _trace = Trace::start(cmd);
    let out = cmd.stdin(Stdio::null()).output()?;
    if !out.status.success() {
        return Err(Error::Git {
            args: cmd.get_args().map(|a| a.to_string_lossy()).collect::<Vec<_>>().join(" "),
            stderr: String::from_utf8_lossy(&out.stderr).trim().to_string(),
        });
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim_end().to_string())
}

/// The common git directory of the checkout, linked worktree or zit
/// workspace containing `cwd`, found without running git.
fn find_git_dir(cwd: &Path) -> Option<PathBuf> {
    let resolve = |base: &Path, text: &str| {
        let path = Path::new(text.trim());
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            base.join(path)
        }
    };
    for dir in cwd.ancestors() {
        let dot_git = dir.join(".git");
        if dot_git.is_dir() {
            return Some(dot_git);
        }
        if let Ok(pointer) = std::fs::read_to_string(&dot_git) {
            let git_dir = resolve(dir, pointer.strip_prefix("gitdir:")?);
            return Some(match std::fs::read_to_string(git_dir.join("commondir")) {
                Ok(common) => resolve(&git_dir, &common),
                Err(_) => git_dir,
            });
        }
    }
    None
}

impl Repo {
    /// Open the repository containing `cwd`; local state lives under
    /// `$ZIT_HOME` (default `~/.zit`).
    pub fn discover(cwd: &Path) -> Result<Repo> {
        let home = match std::env::var_os("ZIT_HOME") {
            Some(h) => PathBuf::from(h),
            None => PathBuf::from(std::env::var_os("HOME").ok_or_else(|| Error::msg("HOME is not set"))?).join(".zit"),
        };
        let mut repo = Repo::open(cwd, &home)?;
        if std::env::var("ZIT_MATERIALISE").as_deref() == Ok("checkout") {
            repo.strategy = Strategy::Checkout;
        }
        Ok(repo)
    }

    pub fn open(cwd: &Path, home_root: &Path) -> Result<Repo> {
        let git_dir = match find_git_dir(cwd) {
            Some(dir) => dir,
            // Bare repositories and unusual layouts: ask git.
            None => {
                run(git_command().current_dir(cwd).args(["rev-parse", "--path-format=absolute", "--git-common-dir"]))
                    .map(PathBuf::from)
                    .map_err(|_| Error::NotARepo(cwd.to_path_buf()))?
            }
        };
        let git_dir = git_dir.canonicalize()?;
        let name = match git_dir.file_name().and_then(OsStr::to_str) {
            Some(".git") => git_dir.parent().and_then(Path::file_name).and_then(OsStr::to_str),
            other => other,
        };
        let key = format!("{}-{}", name.unwrap_or("repo"), crate::hash(git_dir.as_os_str().as_encoded_bytes()));
        Ok(Repo {
            git_dir,
            home: home_root.join(key),
            strategy: Strategy::Clone,
            alternate: None,
            remembered: Arc::default(),
        })
    }

    pub fn with_strategy(mut self, strategy: Strategy) -> Repo {
        self.strategy = strategy;
        self
    }

    pub fn git_dir(&self) -> &Path {
        &self.git_dir
    }

    /// Local, disposable state: workspaces and caches. Never the source of truth.
    pub fn home(&self) -> &Path {
        &self.home
    }

    /// A directory that outlives workspaces, for caches agents and checks
    /// want to share (build output, package stores). Exported to both as
    /// `$ZIT_CACHE_DIR`.
    pub fn cache_dir(&self) -> Result<PathBuf> {
        let dir = self.home.join("shared");
        std::fs::create_dir_all(&dir)?;
        Ok(dir)
    }

    pub(crate) fn cmd(&self) -> Command {
        let mut cmd = git_command();
        // Anchored to the store: the caller's cwd may be a workspace that no longer exists.
        cmd.current_dir(&self.git_dir).arg("--git-dir").arg(&self.git_dir);
        if let Some(objects) = &self.alternate {
            cmd.env("GIT_ALTERNATE_OBJECT_DIRECTORIES", objects);
        }
        cmd
    }

    /// The same repository, also reading objects from `objects`. What it
    /// reads from there is not remembered: those objects may be discarded.
    pub(crate) fn reading_also(&self, objects: &Path) -> Repo {
        Repo { alternate: Some(objects.to_path_buf()), remembered: Arc::default(), ..self.clone() }
    }

    /// The object store: where git writes objects for this repository.
    pub(crate) fn object_dir(&self) -> Result<PathBuf> {
        let common = self.git(&["rev-parse", "--git-common-dir"])?;
        Ok(self.git_dir.join(common.trim()).join("objects"))
    }

    pub(crate) fn git<S: AsRef<OsStr>>(&self, args: &[S]) -> Result<String> {
        run(self.cmd().args(args))
    }

    /// Like `git`, with `input` on stdin.
    pub(crate) fn git_stdin<S: AsRef<OsStr>>(&self, args: &[S], env: &[(&str, &str)], input: &str) -> Result<String> {
        use std::io::Write;
        let mut cmd = self.cmd();
        cmd.args(args).envs(env.iter().copied());
        let _trace = Trace::start(&cmd);
        let mut child = cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
        child.stdin.take().expect("piped").write_all(input.as_bytes())?;
        let out = child.wait_with_output()?;
        if !out.status.success() {
            return Err(Error::Git {
                args: cmd.get_args().map(|a| a.to_string_lossy()).collect::<Vec<_>>().join(" "),
                stderr: String::from_utf8_lossy(&out.stderr).trim().to_string(),
            });
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim_end().to_string())
    }

    /// Start the graph: `rev` (default `HEAD`) becomes the current state.
    /// Idempotent; an initialised graph is left untouched.
    pub fn init(&self, rev: Option<&str>) -> Result<Oid> {
        if let Ok(cur) = self.current() {
            return Ok(cur);
        }
        let genesis = self.resolve(rev.unwrap_or("HEAD"))?;
        self.git(&["update-ref", CURRENT, genesis.as_str(), ""])?;
        self.current()
    }

    /// The one canonical accepted change.
    pub fn current(&self) -> Result<Oid> {
        self.git(&["rev-parse", "--verify", "--quiet", CURRENT]).map(Oid).map_err(|_| Error::NotInitialised)
    }

    /// Resolve any revision (id prefix, ref, `HEAD`, `current`) to a change.
    pub fn resolve(&self, rev: &str) -> Result<Oid> {
        // A full id needs no lookup; a wrong one fails where it is used.
        if is_id(rev) {
            return Ok(Oid::new(rev));
        }
        if rev.starts_with('-') {
            return Err(Error::UnknownRevision(rev.to_string()));
        }
        let rev = if rev == "current" { CURRENT } else { rev };
        self.git(&["rev-parse", "--verify", "--quiet", &format!("{rev}^{{commit}}")])
            .map(Oid)
            .map_err(|_| Error::UnknownRevision(rev.to_string()))
    }

    /// Resolve a revision to (change, resulting state) in one git call.
    pub fn resolve_state(&self, rev: &str) -> Result<(Oid, Oid)> {
        if rev.starts_with('-') {
            return Err(Error::UnknownRevision(rev.to_string()));
        }
        let rev = if rev == "current" { CURRENT } else { rev };
        let out =
            self.git(&["rev-parse", "--quiet", &format!("{rev}^{{commit}}"), &format!("{rev}^{{tree}}")]).map_err(
                |_| if rev == CURRENT { Error::NotInitialised } else { Error::UnknownRevision(rev.to_string()) },
            )?;
        match out.split_once('\n') {
            Some((change, state)) => Ok((Oid::new(change), Oid::new(state))),
            None => Err(Error::UnknownRevision(rev.to_string())),
        }
    }

    /// The state a change results in.
    pub fn tree_of(&self, change: &Oid) -> Result<Oid> {
        self.git(&["rev-parse", "--verify", &format!("{change}^{{tree}}")]).map(Oid)
    }

    pub fn is_ancestor(&self, ancestor: &Oid, of: &Oid) -> Result<bool> {
        let status = self
            .cmd()
            .args(["merge-base", "--is-ancestor", ancestor.as_str(), of.as_str()])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .status()?;
        Ok(status.success())
    }

    pub fn merge_base(&self, a: &Oid, b: &Oid) -> Result<Option<Oid>> {
        let mut cmd = self.cmd();
        cmd.args(["merge-base", a.as_str(), b.as_str()]);
        let _trace = Trace::start(&cmd);
        let out = cmd.stdin(Stdio::null()).output()?;
        let oid = String::from_utf8_lossy(&out.stdout).trim().to_string();
        Ok((out.status.success() && !oid.is_empty()).then_some(Oid(oid)))
    }

    /// Apply ref updates atomically (`git update-ref --stdin` commands, one
    /// per line); `false` if any precondition no longer holds.
    pub(crate) fn transaction(&self, commands: &[String]) -> Result<bool> {
        use std::io::Write;
        let mut cmd = self.cmd();
        cmd.args(["update-ref", "--stdin"]);
        let _trace = Trace::start(&cmd);
        let mut child = cmd.stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null()).spawn()?;
        let mut stdin = child.stdin.take().expect("piped");
        // A failed precondition makes git exit early; the broken pipe is the answer, not an error.
        let _ = stdin.write_all((commands.join("\n") + "\n").as_bytes());
        drop(stdin);
        Ok(child.wait()?.success())
    }

    /// The content of `name`, read once per process when `name` is
    /// `<id>:<path>` (immutable); a name through a ref is read every time.
    pub(crate) fn read_immutable(&self, name: &str) -> Result<Option<Vec<u8>>> {
        Ok(self.read_immutable_all(std::slice::from_ref(&name.to_string()))?.pop().expect("one answer per name"))
    }

    /// As `read_immutable` for several names, in their order, with at most
    /// one `cat-file` session for all the names not remembered yet.
    pub(crate) fn read_immutable_all(&self, names: &[String]) -> Result<Vec<Option<Vec<u8>>>> {
        let mut found: Vec<Option<Option<Vec<u8>>>> = vec![None; names.len()];
        {
            let remembered = self.remembered.lock().expect("not poisoned");
            for (name, slot) in names.iter().zip(&mut found) {
                if let Some(known) = remembered.get(name) {
                    *slot = Some(known.clone());
                }
            }
        }
        if found.iter().any(Option::is_none) {
            let mut objects = self.objects()?;
            let mut remembered = self.remembered.lock().expect("not poisoned");
            for (name, slot) in names.iter().zip(&mut found) {
                if slot.is_none() {
                    let content = objects.read(name)?;
                    if name.split_once(':').is_some_and(|(rev, _)| is_id(rev)) {
                        if remembered.len() >= REMEMBER_LIMIT {
                            remembered.clear();
                        }
                        remembered.insert(name.clone(), content.clone());
                    }
                    *slot = Some(content);
                }
            }
        }
        Ok(found.into_iter().map(|slot| slot.expect("filled above")).collect())
    }

    #[cfg(test)]
    fn remembered(&self) -> usize {
        self.remembered.lock().expect("not poisoned").len()
    }

    /// Open a session for reading many objects with one process.
    pub(crate) fn objects(&self) -> Result<Objects> {
        let mut child =
            self.cmd().args(["cat-file", "--batch"]).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn()?;
        let stdin = child.stdin.take().expect("piped");
        let stdout = std::io::BufReader::new(child.stdout.take().expect("piped"));
        Ok(Objects { child, stdin, stdout })
    }
}

/// A `git cat-file --batch` session.
pub(crate) struct Objects {
    child: std::process::Child,
    stdin: std::process::ChildStdin,
    stdout: std::io::BufReader<std::process::ChildStdout>,
}

/// Parse a `cat-file --batch` header: the object's size, or `None` if git
/// says it is missing. Anything else, including EOF, means the session is broken.
fn object_size(header: &str) -> Result<Option<usize>> {
    let mut fields = header.split_whitespace();
    match (fields.next(), fields.next(), fields.next()) {
        (Some(_), Some("missing"), None) => Ok(None),
        (Some(_), Some(_), Some(size)) => size.parse().map(Some).map_err(|_| Error::msg("git cat-file: bad header")),
        _ => Err(Error::msg("git cat-file ended unexpectedly")),
    }
}

impl Objects {
    /// The content of `name` (an id, ref or `rev:path`); `None` if it does not exist.
    pub(crate) fn read(&mut self, name: &str) -> Result<Option<Vec<u8>>> {
        use std::io::{BufRead, Read, Write};
        writeln!(self.stdin, "{name}")?;
        self.stdin.flush()?;
        let mut header = String::new();
        self.stdout.read_line(&mut header)?;
        let Some(size) = object_size(&header)? else {
            return Ok(None);
        };
        let mut body = vec![0; size + 1];
        self.stdout.read_exact(&mut body)?;
        body.pop();
        Ok(Some(body))
    }
}

impl Drop for Objects {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sh(cwd: &Path, script: &str) {
        assert!(Command::new("sh").current_dir(cwd).args(["-c", script]).status().unwrap().success());
    }

    #[test]
    fn only_content_named_by_an_id_is_remembered() {
        let dir = tempfile::tempdir().unwrap();
        sh(
            dir.path(),
            "git init -q -b main . && echo one > f && git add f && git -c user.name=t -c user.email=t@t commit -qm one",
        );
        let repo = Repo::open(dir.path(), &dir.path().join("home")).unwrap();
        let first = repo.resolve("HEAD").unwrap();
        assert_eq!(repo.read_immutable(&format!("{first}:f")).unwrap().as_deref(), Some(&b"one\n"[..]));
        assert_eq!(repo.read_immutable("HEAD:f").unwrap().as_deref(), Some(&b"one\n"[..]));
        sh(dir.path(), "echo two > f && git -c user.name=t -c user.email=t@t commit -qam two");
        assert_eq!(repo.read_immutable("HEAD:f").unwrap().as_deref(), Some(&b"two\n"[..]), "a ref moves");
        assert_eq!(repo.read_immutable(&format!("{first}:f")).unwrap().as_deref(), Some(&b"one\n"[..]));
        assert_eq!(repo.read_immutable(&format!("{first}:missing")).unwrap(), None);
        assert_eq!(repo.remembered(), 2, "the two names under an id; nothing under HEAD");
    }

    #[test]
    fn a_dead_batch_process_is_an_error_not_a_missing_object() {
        assert_eq!(object_size("9f6f29 blob 12\n").unwrap(), Some(12));
        assert_eq!(object_size("refs/zit/evidence/abc missing\n").unwrap(), None);
        assert!(object_size("").is_err(), "EOF means git died");
        assert!(object_size("garbage\n").is_err());
    }
}
