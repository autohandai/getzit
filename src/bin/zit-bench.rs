//! Benchmarks: agents working through zit versus through git worktrees.
//! Both arms are driven through their CLIs with the same `git` binary.
//!
//!   lifecycle  cost of creating and destroying isolated workspaces
//!   workflow   N agents x M speculative changes on one repository, integrated
//!              one by one behind checks (a merge queue)
//!   agents     real agent CLIs (claude, codex, autohand) doing one edit each

use clap::{Parser, Subcommand};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Instant;
use zit::accept;
use zit::workspace::{self, NewWorkspace, Strategy};

#[derive(Parser)]
struct Cli {
    /// Scratch directory (must allow copy-on-write clones for the clone arm).
    #[arg(long)]
    dir: Option<PathBuf>,
    /// The git binary both arms use.
    #[arg(long, default_value = "git")]
    git: String,
    /// The python3 binary the checks run.
    #[arg(long, default_value = "python3")]
    python: String,
    /// Lifecycle only: drive zit by spawning its CLI for every operation.
    /// Default: in-process, as the long-lived `zit mcp` server does. (The
    /// workflow experiment has a `zit-cli` arm instead.)
    #[arg(long)]
    zit_cli: bool,
    /// Write results as JSON here.
    #[arg(long)]
    out: Option<PathBuf>,
    /// Keep the scratch directory for inspection.
    #[arg(long)]
    keep: bool,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    Lifecycle {
        #[arg(long, value_delimiter = ',', default_value = "1000,10000")]
        files: Vec<usize>,
        #[arg(long, value_delimiter = ',', default_value = "1,10")]
        agents: Vec<usize>,
        #[arg(long, default_value_t = 3)]
        rounds: usize,
    },
    Workflow {
        #[arg(long, default_value_t = 10)]
        agents: usize,
        #[arg(long, default_value_t = 10)]
        changes: usize,
        #[arg(long, default_value_t = 10)]
        modules: usize,
        #[arg(long, default_value_t = 5)]
        functions: usize,
        #[arg(long, default_value_t = 1)]
        seed: u64,
        #[arg(
            long,
            value_delimiter = ',',
            default_value = "worktree-full,worktree-affected,zit,zit-allow-stale,zit-cli"
        )]
        arms: Vec<String>,
        #[arg(long, default_value_t = 3)]
        rounds: usize,
    },
    Agents {
        #[arg(long, value_delimiter = ',', default_value = "claude,codex,autohand")]
        agents: Vec<String>,
    },
}

struct Env {
    git: String,
    zit: PathBuf,
    python: String,
    work: PathBuf,
    cli: bool,
}

/// zit for one repository, in-process or through the CLI.
struct Graph<'a> {
    env: &'a Env,
    root: PathBuf,
    repo: zit::Repo,
    strategy: Strategy,
    /// Spawn the `zit` CLI for every operation instead of calling the library.
    cli: bool,
}

impl<'a> Graph<'a> {
    fn open(env: &'a Env, root: &Path, strategy: Strategy, cli: bool) -> Graph<'a> {
        let repo = zit::Repo::open(root, &root.with_file_name("home")).expect("open").with_strategy(strategy);
        Graph { env, root: root.to_path_buf(), repo, strategy, cli }
    }

    fn cli(&self) -> Command {
        let mut cmd = self.env.zit(&self.root);
        if self.strategy == Strategy::Checkout {
            cmd.env("ZIT_MATERIALISE", "checkout");
        }
        cmd
    }

    fn json(&self, args: &[&str]) -> Value {
        let (_, stdout, stderr) = output(self.cli().args(args).arg("--json"));
        serde_json::from_str(&stdout).unwrap_or_else(|e| panic!("zit {args:?}: {e}\n{stdout}\n{stderr}"))
    }

    fn init(&self) {
        match self.cli {
            true => drop(must(self.cli().arg("init"))),
            false => drop(self.repo.init(None).expect("init")),
        }
    }

    /// (workspace id, path)
    fn materialise(&self, from: Option<&str>, agent: &str, intent: &str) -> (String, PathBuf) {
        if self.cli {
            let mut args = vec!["materialise", "--agent", agent, "--intent", intent];
            if let Some(from) = from {
                args.extend(["--from", from]);
            }
            let ws = self.json(&args);
            return (ws["id"].as_str().expect("id").to_string(), PathBuf::from(ws["path"].as_str().expect("path")));
        }
        let from = from.map(|rev| self.repo.resolve(rev).expect("resolve"));
        let new = NewWorkspace { from: from.as_ref(), intent, agent, session: None };
        let ws = workspace::materialise(&self.repo, &new).expect("materialise");
        (ws.id.clone(), ws.path().to_path_buf())
    }

    fn dispose(&self, id: &str) {
        match self.cli {
            true => drop(must(self.cli().args(["dispose", id]))),
            false => workspace::dispose(&self.repo, id).expect("dispose"),
        }
    }

    /// Record and dispose; the new change id.
    fn record(&self, ws: &str) -> String {
        if self.cli {
            let rec = self.json(&["record", "--workspace", ws, "--dispose"]);
            return rec["change"]["id"].as_str().expect("change").to_string();
        }
        let rec = zit::api::record(&self.repo, ws, &Default::default(), true).expect("record");
        rec.change.expect("change").id.to_string()
    }

    /// "first-try", or the rejection reason.
    fn accept(&self, change: &str, allow_stale: bool) -> String {
        let outcome = if self.cli {
            let mut args = vec!["accept", change];
            if allow_stale {
                args.push("--allow-stale");
            }
            self.json(&args)
        } else {
            let id = self.repo.resolve(change).expect("resolve");
            json!(accept::accept_with(&self.repo, &id, &accept::Policy { allow_stale, rerun: false }).expect("accept"))
        };
        match outcome["outcome"].as_str() {
            Some("accepted") => "first-try".to_string(),
            _ => outcome["reason"].as_str().expect("reason").to_string(),
        }
    }

    fn discard(&self, change: &str) {
        match self.cli {
            true => drop(must(self.cli().args(["discard", change]))),
            false => zit::change::discard(&self.repo, &self.repo.resolve(change).expect("resolve")).expect("discard"),
        }
    }

    /// (speculative changes, live workspaces)
    fn status(&self) -> (usize, usize) {
        if self.cli {
            let status = self.json(&["status"]);
            return (
                status["changes"].as_array().map_or(0, Vec::len),
                status["workspaces"].as_array().map_or(0, Vec::len),
            );
        }
        let overview = zit::api::overview(&self.repo).expect("status");
        (overview.changes.len(), overview.workspaces.len())
    }

    fn export(&self) {
        match self.cli {
            true => drop(must(self.cli().args(["export", "--branch", "main"]))),
            false => drop(accept::export(&self.repo, "main").expect("export")),
        }
    }
}

fn output(cmd: &mut Command) -> (bool, String, String) {
    let out = cmd.stdin(Stdio::null()).output().expect("spawn");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).trim().to_string(),
        String::from_utf8_lossy(&out.stderr).trim().to_string(),
    )
}

fn must(cmd: &mut Command) -> String {
    let (ok, stdout, stderr) = output(cmd);
    assert!(ok, "{cmd:?} failed:\n{stdout}\n{stderr}");
    stdout
}

fn ms(since: Instant) -> f64 {
    since.elapsed().as_secs_f64() * 1000.0
}

fn median(values: &[f64]) -> f64 {
    let mut v = values.to_vec();
    v.sort_by(|a, b| a.partial_cmp(b).expect("no NaN"));
    v[v.len() / 2]
}

fn round1(x: f64) -> f64 {
    (x * 10.0).round() / 10.0
}

impl Env {
    fn git(&self, cwd: &Path) -> Command {
        let mut cmd = Command::new(&self.git);
        cmd.current_dir(cwd);
        cmd
    }

    fn zit(&self, root: &Path) -> Command {
        let mut cmd = Command::new(&self.zit);
        cmd.current_dir(root).env("ZIT_HOME", root.with_file_name("home")).env("ZIT_GIT", &self.git);
        cmd.env_remove("ZIT_MATERIALISE");
        cmd
    }

    fn init_repo(&self, root: &Path) {
        fs::create_dir_all(root).expect("mkdir");
        must(self.git(root).args(["init", "-q", "-b", "main"]));
        must(self.git(root).args(["config", "user.name", "bench"]));
        must(self.git(root).args(["config", "user.email", "bench@example.com"]));
    }

    fn commit_all(&self, cwd: &Path, message: &str) {
        must(self.git(cwd).args(["add", "-A"]));
        must(self.git(cwd).args(["commit", "-q", "-m", message]));
    }

    /// `git worktree add`, retried: concurrent adds race inside git (one reads
    /// another's half-written admin directory). Returns how many retries it took.
    fn worktree_add(&self, root: &Path, path: &Path, branch: Option<&str>, from: &str) -> usize {
        let mut retries = 0;
        loop {
            let mut cmd = self.git(root);
            cmd.args(["worktree", "add", "-q"]);
            match branch {
                Some(branch) => cmd.args(["-b", branch]),
                None => cmd.arg("--detach"),
            };
            let (ok, _, err) = output(cmd.arg(path).arg(from));
            if ok {
                return retries;
            }
            retries += 1;
            assert!(retries < 50, "git worktree add keeps failing: {err}");
            let _ = fs::remove_dir_all(path);
            let _ = output(self.git(root).args(["worktree", "prune"]));
            if let Some(branch) = branch {
                let _ = output(self.git(root).args(["branch", "-D", branch]));
            }
        }
    }

    /// `git worktree remove --force`, retried for the same reason. Returns the retries.
    fn worktree_remove(&self, root: &Path, path: &Path) -> usize {
        let mut retries = 0;
        loop {
            let (ok, _, err) = output(self.git(root).args(["worktree", "remove", "--force"]).arg(path));
            if ok || !path.exists() {
                return retries;
            }
            retries += 1;
            assert!(retries < 50, "git worktree remove keeps failing: {err}");
        }
    }

    fn free_bytes(&self) -> u64 {
        use std::os::unix::ffi::OsStrExt;
        let path = std::ffi::CString::new(self.work.as_os_str().as_bytes()).expect("path");
        // SAFETY: `path` is a valid C string and `stat` is a properly sized out-parameter.
        unsafe {
            let mut stat: libc::statfs = std::mem::zeroed();
            assert_eq!(libc::statfs(path.as_ptr(), &mut stat), 0);
            stat.f_bavail as u64 * stat.f_bsize as u64
        }
    }
}

/// Run `f(i)` for `0..n` on `n` threads; results in index order.
fn concurrently<T: Send>(n: usize, f: impl Fn(usize) -> T + Sync) -> Vec<T> {
    std::thread::scope(|scope| {
        let handles: Vec<_> = (0..n)
            .map(|i| {
                scope.spawn({
                    let f = &f;
                    move || f(i)
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().expect("worker panicked")).collect()
    })
}

// ---------------------------------------------------------------- lifecycle

fn lifecycle(env: &Env, files: &[usize], agents: &[usize], rounds: usize) -> Value {
    let mut rows = Vec::new();
    for &count in files {
        let root = env.work.join(format!("life-{count}/repo"));
        env.init_repo(&root);
        for i in 0..count {
            let path = root.join(format!("d{}/f{}.py", i / 100, i));
            fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
            let body: String = (0..10).map(|l| format!("def f{i}_{l}(x):\n    return x + {l}\n\n\n")).collect();
            fs::write(path, body).expect("write");
        }
        env.commit_all(&root, "genesis");
        let clone = Graph::open(env, &root, Strategy::Clone, env.cli);
        let checkout = Graph::open(env, &root, Strategy::Checkout, env.cli);
        clone.init();

        // The clone arm needs the state cached once; that one-off cost is reported separately.
        let started = Instant::now();
        let (first, _) = clone.materialise(None, "bench", "");
        let cold_ms = ms(started);
        clone.dispose(&first);

        for &n in agents {
            for arm in ["worktree", "zit-clone", "zit-checkout"] {
                let (mut create_wall, mut create_op, mut destroy_wall, mut disk) = (vec![], vec![], vec![], vec![]);
                let retries = std::sync::atomic::AtomicUsize::new(0);
                for round in 0..rounds {
                    let free_before = env.free_bytes();
                    let started = Instant::now();
                    let made: Vec<(String, f64)> = concurrently(n, |i| {
                        let op = Instant::now();
                        let handle = match arm {
                            "worktree" => {
                                let path = root.with_file_name(format!("wt-{n}-{round}-{i}"));
                                let n = env.worktree_add(&root, &path, None, "HEAD");
                                retries.fetch_add(n, std::sync::atomic::Ordering::Relaxed);
                                path.display().to_string()
                            }
                            "zit-clone" => clone.materialise(None, "bench", "").0,
                            _ => checkout.materialise(None, "bench", "").0,
                        };
                        (handle, ms(op))
                    });
                    create_wall.push(ms(started));
                    create_op.extend(made.iter().map(|(_, t)| *t));
                    disk.push(free_before.saturating_sub(env.free_bytes()) as f64 / 1e6);

                    let started = Instant::now();
                    concurrently(n, |i| match arm {
                        "worktree" => {
                            let n = env.worktree_remove(&root, Path::new(&made[i].0));
                            retries.fetch_add(n, std::sync::atomic::Ordering::Relaxed);
                        }
                        _ => clone.dispose(&made[i].0),
                    });
                    destroy_wall.push(ms(started));
                }
                rows.push(json!({
                    "files": count, "concurrent": n, "arm": arm,
                    "create_wall_ms": round1(median(&create_wall)),
                    "create_op_ms": round1(median(&create_op)),
                    "destroy_wall_ms": round1(median(&destroy_wall)),
                    "disk_mb": round1(median(&disk)),
                    "cold_cache_ms": if arm == "zit-clone" { json!(round1(cold_ms)) } else { Value::Null },
                    "worktree_retries": retries.into_inner(),
                }));
                eprintln!("lifecycle files={count} n={n} {arm}: create {:.0}ms", median(&create_wall));
            }
        }
    }
    json!(rows)
}

// ----------------------------------------------------------------- workflow

#[derive(Clone, Copy, PartialEq, Debug)]
enum Kind {
    /// Change one function's body.
    Body,
    /// Add a new file that calls one function.
    Caller,
    /// Change one function's signature and every caller visible at the time.
    Signature,
}

#[derive(Clone, Copy, Debug)]
struct Task {
    agent: usize,
    n: usize,
    module: usize,
    func: usize,
    kind: Kind,
}

struct Rng(u64);

impl Rng {
    fn next(&mut self, bound: usize) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 % bound as u64) as usize
    }
}

fn tasks(agents: usize, changes: usize, modules: usize, functions: usize, seed: u64) -> Vec<Task> {
    let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
    let mut all = Vec::new();
    // Round-robin over agents: the integration order interleaves them.
    for n in 0..changes {
        for agent in 0..agents {
            let kind = match rng.next(100) {
                0..=49 => Kind::Body,
                50..=84 => Kind::Caller,
                _ => Kind::Signature,
            };
            all.push(Task { agent, n, module: rng.next(modules), func: rng.next(functions), kind });
        }
    }
    all
}

/// The agent's edit: deterministic, and correct against the state it sees in `dir`.
fn apply(task: &Task, dir: &Path) {
    let module = dir.join(format!("mod{}", task.module));
    let name = format!("m{}_f{}", task.module, task.func);
    let lib = fs::read_to_string(module.join("lib.py")).expect("lib.py");
    let scaled = lib.contains(&format!("def {name}(x, scale):"));
    let salt = 1000 + task.agent * 100 + task.n;
    let rewrite = |new_def: &str, new_body: &str| {
        let mut out = Vec::new();
        let mut lines = lib.lines();
        while let Some(line) = lines.next() {
            if line.starts_with(&format!("def {name}(")) {
                out.push(new_def.to_string());
                out.push(new_body.to_string());
                lines.next();
            } else {
                out.push(line.to_string());
            }
        }
        fs::write(module.join("lib.py"), out.join("\n") + "\n").expect("write lib.py");
    };
    match (task.kind, scaled) {
        (Kind::Caller, _) => {
            let args = if scaled { "1, 2" } else { "1" };
            let body = format!("import lib\n\n\ndef run():\n    return lib.{name}({args})\n");
            fs::write(module.join(format!("use_a{}_{}.py", task.agent, task.n)), body).expect("write caller");
        }
        (Kind::Body, false) => rewrite(&format!("def {name}(x):"), &format!("    return x + {salt}")),
        (Kind::Body, true) | (Kind::Signature, true) => {
            rewrite(&format!("def {name}(x, scale):"), &format!("    return x * scale + {salt}"))
        }
        (Kind::Signature, false) => {
            rewrite(&format!("def {name}(x, scale):"), &format!("    return x * scale + {}", task.func));
            for entry in fs::read_dir(&module).expect("module dir").flatten() {
                if entry.file_name().to_string_lossy().starts_with("use_") {
                    let text = fs::read_to_string(entry.path()).expect("caller");
                    fs::write(entry.path(), text.replace(&format!("{name}(1)"), &format!("{name}(1, 2)")))
                        .expect("write");
                }
            }
        }
    }
}

const TEST_PY: &str = r#"import glob, importlib.util, os, sys
here = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, here)
for path in sorted(glob.glob(os.path.join(here, "use_*.py"))):
    spec = importlib.util.spec_from_file_location(os.path.basename(path)[:-3], path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    assert isinstance(module.run(), int)
"#;

fn check_command(env: &Env, module: usize) -> String {
    format!("echo m{module} >> \"$ZIT_BENCH_LOG\"; PYTHONDONTWRITEBYTECODE=1 exec {} mod{module}/test.py", env.python)
}

fn make_project(env: &Env, root: &Path, modules: usize, functions: usize) {
    env.init_repo(root);
    let mut config = String::new();
    for m in 0..modules {
        let dir = root.join(format!("mod{m}"));
        fs::create_dir_all(&dir).expect("mkdir");
        let lib: Vec<String> = (0..functions).map(|f| format!("def m{m}_f{f}(x):\n    return x + {f}\n")).collect();
        fs::write(dir.join("lib.py"), lib.join("\n\n")).expect("write");
        fs::write(dir.join("test.py"), TEST_PY).expect("write");
        for f in 0..functions {
            let body = format!("import lib\n\n\ndef run():\n    return lib.m{m}_f{f}(1)\n");
            fs::write(dir.join(format!("use_base_{f}.py")), body).expect("write");
        }
        config.push_str(&format!(
            "[[check]]\nname = \"m{m}\"\nrun = '''{}'''\ninputs = [\"mod{m}\"]\n\n",
            check_command(env, m)
        ));
    }
    fs::write(root.join("zit.toml"), config).expect("write");
    fs::write(root.join(".gitignore"), "__pycache__/\n").expect("write");
    env.commit_all(root, "genesis");
}

/// Run module checks in `cwd`; true when all pass. A failing check's output
/// is kept next to the log, so an unexpected failure can be explained.
fn run_checks(env: &Env, cwd: &Path, log: &Path, modules: impl Iterator<Item = usize>) -> bool {
    let mut ok = true;
    for m in modules {
        let out = Command::new("sh")
            .arg("-c")
            .arg(check_command(env, m))
            .current_dir(cwd)
            .env("ZIT_BENCH_LOG", log)
            .stdin(Stdio::null())
            .output()
            .expect("sh");
        if !out.status.success() {
            ok = false;
            let note = format!("m{m}: {}\n", String::from_utf8_lossy(&out.stderr).lines().last().unwrap_or_default());
            let failures = log.with_file_name("failures.log");
            let _ = fs::OpenOptions::new().create(true).append(true).open(failures).map(|mut f| {
                use std::io::Write;
                f.write_all(note.as_bytes())
            });
        }
    }
    ok
}

/// One arm of the workflow experiment, advanced step by step so that arms
/// can be interleaved and machine noise hits them equally.
struct ArmRun<'a> {
    env: &'a Env,
    arm: String,
    root: PathBuf,
    log: PathBuf,
    base: String,
    graph: Graph<'a>,
    modules: usize,
    /// Change handle (branch name or change id) per task, in task order.
    handles: Vec<String>,
    add_retries: std::sync::atomic::AtomicUsize,
    phase_a_ms: f64,
    status_ms: Option<f64>,
    phase_b_ms: f64,
    decisions: Vec<String>,
}

impl<'a> ArmRun<'a> {
    fn new(env: &'a Env, arm: &str, round: usize, modules: usize, functions: usize) -> ArmRun<'a> {
        let root = env.work.join(format!("wf-{arm}-{round}/repo"));
        make_project(env, &root, modules, functions);
        let graph = Graph::open(env, &root, Strategy::Clone, arm == "zit-cli");
        if arm.starts_with("zit") {
            graph.init();
        }
        ArmRun {
            env,
            arm: arm.to_string(),
            log: root.with_file_name("checks.log"),
            base: must(env.git(&root).args(["rev-parse", "HEAD"])),
            root,
            graph,
            modules,
            handles: Vec::new(),
            add_retries: Default::default(),
            phase_a_ms: 0.0,
            status_ms: None,
            phase_b_ms: 0.0,
            decisions: Vec::new(),
        }
    }

    fn is_zit(&self) -> bool {
        self.arm.starts_with("zit")
    }

    /// Checks append to the arm's log; in-process zit inherits it from our environment.
    fn select(&self) {
        std::env::set_var("ZIT_BENCH_LOG", &self.log);
    }

    /// One agent turn against `from`: isolate, edit, record, tear down. Returns the change handle.
    fn turn(&self, t: &Task, from: &str, tag: &str) -> String {
        let intent = format!("{:?} m{}_f{}", t.kind, t.module, t.func);
        if self.is_zit() {
            let (ws, path) = self.graph.materialise(Some(from), &format!("a{}", t.agent), &intent);
            apply(t, &path);
            self.graph.record(&ws)
        } else {
            let branch = format!("{tag}-a{}-{}", t.agent, t.n);
            let path = self.root.with_file_name(format!("wt-{branch}"));
            let n = self.env.worktree_add(&self.root, &path, Some(&branch), from);
            self.add_retries.fetch_add(n, std::sync::atomic::Ordering::Relaxed);
            apply(t, &path);
            self.env.commit_all(&path, &intent);
            let n = self.env.worktree_remove(&self.root, &path);
            self.add_retries.fetch_add(n, std::sync::atomic::Ordering::Relaxed);
            branch
        }
    }

    /// Phase A: every agent works concurrently; all changes are speculative on the same base.
    fn agents_work(&mut self, all: &[Task], agents: usize) {
        self.select();
        let started = Instant::now();
        let per_agent: Vec<Vec<(usize, String)>> = concurrently(agents, |agent| {
            let mine = all.iter().enumerate().filter(|(_, t)| t.agent == agent);
            mine.map(|(i, t)| (i, self.turn(t, &self.base, "t"))).collect()
        });
        self.phase_a_ms = ms(started);
        let mut handles: Vec<(usize, String)> = per_agent.into_iter().flatten().collect();
        handles.sort();
        self.handles = handles.into_iter().map(|(_, h)| h).collect();

        if self.is_zit() {
            let started = Instant::now();
            assert_eq!(self.graph.status().0, all.len());
            self.status_ms = Some(ms(started));
        }
    }

    fn checks_ok(&self) -> bool {
        let git = |args: &[&str]| must(self.env.git(&self.root).args(args));
        match self.arm.as_str() {
            "worktree-full" => run_checks(self.env, &self.root, &self.log, 0..self.modules),
            _ => {
                let changed = git(&["diff", "--name-only", "HEAD~1", "HEAD"]);
                let touched: BTreeSet<usize> =
                    changed.lines().filter_map(|p| p.strip_prefix("mod")?.split('/').next()?.parse().ok()).collect();
                run_checks(self.env, &self.root, &self.log, touched.into_iter())
            }
        }
    }

    /// Phase B, one step: integrate task `index`; a rejected change is redone on the new tip.
    fn integrate(&mut self, index: usize, t: &Task) {
        self.select();
        let started = Instant::now();
        let change = self.handles[index].clone();
        let git = |args: &[&str]| output(self.env.git(&self.root).args(args));
        let decision = if self.is_zit() {
            self.graph.accept(&change, self.arm == "zit-allow-stale")
        } else if !git(&["merge", "--no-ff", "--no-edit", "-q", &change]).0 {
            assert!(git(&["merge", "--abort"]).0);
            "conflict".to_string()
        } else if !self.checks_ok() {
            assert!(git(&["reset", "-q", "--hard", "HEAD~1"]).0);
            "failed".to_string()
        } else {
            "first-try".to_string()
        };

        if decision != "first-try" {
            if self.is_zit() {
                self.graph.discard(&change);
                let redo = self.turn(t, "current", "r");
                assert_eq!(
                    self.graph.accept(&redo, false),
                    "first-try",
                    "{}: retry of {t:?} was not accepted",
                    self.arm
                );
            } else {
                let redo = self.turn(t, "main", "r");
                assert!(git(&["merge", "--ff-only", "-q", &redo]).0);
                assert!(self.checks_ok(), "{}: retry of {t:?} fails its checks", self.arm);
            }
        }
        self.decisions.push(decision);
        self.phase_b_ms += ms(started);
    }

    fn finish(self) -> Value {
        if self.is_zit() {
            self.graph.export();
        }
        let checks_run = fs::read_to_string(&self.log).map(|s| s.lines().count()).unwrap_or(0);
        let final_green = run_checks(self.env, &self.root, &self.root.with_file_name("final.log"), 0..self.modules);
        let leftovers = if self.is_zit() {
            let (changes, workspaces) = self.graph.status();
            changes + workspaces
        } else {
            must(self.env.git(&self.root).args(["branch", "--format=%(refname:short)"])).lines().count() - 1
        };
        let count = |what: &str| self.decisions.iter().filter(|d| *d == what).count();
        json!({
            "arm": self.arm,
            "phase_a_ms": round1(self.phase_a_ms),
            "status_ms": self.status_ms.map(round1),
            "phase_b_ms": round1(self.phase_b_ms),
            "checks_run": checks_run,
            "first_try": count("first-try"),
            "rejected_conflict": count("conflict"),
            "rejected_stale": count("stale"),
            "rejected_failed": count("failed"),
            "worktree_retries": self.add_retries.into_inner(),
            "final_green": final_green,
            "final_tree": must(self.env.git(&self.root).args(["rev-parse", "main^{tree}"])),
            "leftover_branches_or_changes": leftovers,
            "decisions": self.decisions,
        })
    }
}

/// The size of a workflow experiment.
struct Shape {
    agents: usize,
    changes: usize,
    modules: usize,
    functions: usize,
    seed: u64,
}

fn workflow(env: &Env, shape: &Shape, arms: &[String], rounds: usize) -> Value {
    let Shape { agents, changes, modules, functions, seed } = *shape;
    let all = tasks(agents, changes, modules, functions, seed);
    let mix = |k: Kind| all.iter().filter(|t| t.kind == k).count();

    let mut per_arm: Vec<Vec<Value>> = vec![Vec::new(); arms.len()];
    for round in 0..rounds {
        let mut runs: Vec<ArmRun> = arms.iter().map(|arm| ArmRun::new(env, arm, round, modules, functions)).collect();
        for run in &mut runs {
            run.agents_work(&all, agents);
        }
        // Integrate change by change, every arm in turn, rotating who goes
        // first: whatever else the machine is doing slows all arms alike.
        for (index, task) in all.iter().enumerate() {
            for k in 0..runs.len() {
                let turn = (index + k) % runs.len();
                runs[turn].integrate(index, task);
            }
        }
        for (slot, run) in per_arm.iter_mut().zip(runs) {
            let result = run.finish();
            eprintln!(
                "workflow round {round} {}: A {}ms B {}ms checks {}",
                result["arm"], result["phase_a_ms"], result["phase_b_ms"], result["checks_run"]
            );
            slot.push(result);
        }
    }

    let arms_out: Vec<Value> = per_arm
        .into_iter()
        .map(|runs| {
            let all_of = |key: &str| runs.iter().map(|r| r[key].as_f64().unwrap_or(0.0)).collect::<Vec<_>>();
            let mut summary = runs[0].clone();
            for key in ["phase_a_ms", "phase_b_ms"] {
                summary[key] = json!(round1(median(&all_of(key))));
                summary[format!("{key}_rounds")] = json!(all_of(key));
            }
            if summary["status_ms"].is_number() {
                summary["status_ms"] = json!(round1(median(&all_of("status_ms"))));
            }
            // Counts are deterministic; say so if a round disagrees.
            summary["deterministic"] = json!(runs.iter().all(|r| {
                r["decisions"] == runs[0]["decisions"]
                    && r["final_tree"] == runs[0]["final_tree"]
                    && r["checks_run"] == runs[0]["checks_run"]
            }));
            summary
        })
        .collect();
    json!({
        "agents": agents, "changes_per_agent": changes, "modules": modules, "functions_per_module": functions,
        "seed": seed, "rounds": rounds,
        "task_mix": {"body": mix(Kind::Body), "caller": mix(Kind::Caller), "signature": mix(Kind::Signature)},
        "arms": arms_out,
    })
}

// ------------------------------------------------------------------- agents

fn real_agents(env: &Env, agents: &[String]) -> Value {
    let mut rows = Vec::new();
    for arm in ["worktree", "zit"] {
        let root = env.work.join(format!("agents-{arm}/repo"));
        make_project(env, &root, 2, agents.len().max(2));
        if arm == "zit" {
            must(env.zit(&root).arg("init"));
        }
        let log = root.with_file_name("checks.log");
        std::env::set_var("ZIT_BENCH_LOG", &log);
        let prompt = |i: usize| {
            format!(
                "In mod0/lib.py change the function m0_f{i} so that it returns x + {} instead of x + {i}. Edit only that file. Do not run any commands.",
                100 + i
            )
        };
        // All agents work at the same time on the same file, one function each.
        let started = Instant::now();
        let turns: Vec<(f64, String, bool)> = concurrently(agents.len(), |i| {
            let agent = &agents[i];
            let argv = zit::run::preset(agent, &prompt(i), &[]).unwrap_or_else(|| panic!("no preset for {agent}"));
            let op = Instant::now();
            if arm == "zit" {
                let mut cmd = env.zit(&root);
                let (_, stdout, stderr) = output(cmd.args(["run", "--agent", agent, "--intent", &prompt(i), "--json"]));
                let report: Value = serde_json::from_str(&stdout).unwrap_or_else(|e| panic!("{agent}: {e}\n{stderr}"));
                (ms(op), report["change"]["id"].as_str().unwrap_or_default().to_string(), report["exit_code"] == 0)
            } else {
                let path = root.with_file_name(format!("wt-{agent}"));
                env.worktree_add(&root, &path, Some(agent), "main");
                let (ok, _, _) = output(Command::new(&argv[0]).args(&argv[1..]).current_dir(&path));
                must(env.git(&path).args(["add", "-A"]));
                let committed = output(env.git(&path).args(["commit", "-q", "-m", &prompt(i)])).0;
                env.worktree_remove(&root, &path);
                (ms(op), if committed { agent.clone() } else { String::new() }, ok)
            }
        });
        let wall_ms = ms(started);

        let started = Instant::now();
        let mut integrated = Vec::new();
        for (_, change, _) in &turns {
            let ok = !change.is_empty()
                && if arm == "zit" {
                    let mut cmd = env.zit(&root);
                    output(cmd.env("ZIT_BENCH_LOG", &log).args(["accept", change])).0
                } else {
                    output(env.git(&root).args(["merge", "--no-ff", "--no-edit", "-q", change])).0
                        && run_checks(env, &root, &log, 0..1)
                };
            integrated.push(ok);
        }
        if arm == "zit" {
            must(env.zit(&root).args(["export", "--branch", "main"]));
        }
        let integrate_ms = ms(started);
        let lib = fs::read_to_string(root.join("mod0/lib.py")).unwrap_or_default();
        for (i, agent) in agents.iter().enumerate() {
            rows.push(json!({
                "arm": arm, "agent": agent,
                "turn_ms": round1(turns[i].0),
                "agent_exit_ok": turns[i].2,
                "produced_change": !turns[i].1.is_empty(),
                "integrated": integrated[i],
                "edit_is_in_main": lib.contains(&format!("return x + {}", 100 + i)),
            }));
        }
        rows.push(json!({"arm": arm, "agent": "(all, concurrent)", "wall_ms": round1(wall_ms), "integrate_ms": round1(integrate_ms)}));
        eprintln!("agents {arm}: {wall_ms:.0}ms");
    }
    json!(rows)
}

fn main() {
    let cli = Cli::parse();
    let work = cli.dir.unwrap_or_else(|| std::env::temp_dir().join(format!("zit-bench-{}", std::process::id())));
    fs::create_dir_all(&work).expect("mkdir work");
    let work = work.canonicalize().expect("canonical work dir");
    let zit = std::env::current_exe().expect("exe").with_file_name("zit");
    assert!(zit.is_file(), "build the zit binary first: {}", zit.display());
    let python = must(Command::new(&cli.python).args(["-c", "import sys; print(sys.executable)"]));
    // In-process zit uses the same git binary as the worktree arm.
    std::env::set_var("ZIT_GIT", &cli.git);
    let env = Env { git: cli.git, zit, python: cli.python.clone(), work, cli: cli.zit_cli };
    let _ = python;

    // How long this machine takes to start each binary, right now: [median, 90th percentile] in ms.
    let spawn_ms = |program: &str, arg: &str| {
        let mut samples: Vec<f64> = (0..40)
            .map(|_| {
                let started = Instant::now();
                let _ = output(Command::new(program).arg(arg));
                ms(started)
            })
            .collect();
        samples.sort_by(|a, b| a.partial_cmp(b).expect("no NaN"));
        json!([round1(samples[samples.len() / 2]), round1(samples[samples.len() * 9 / 10])])
    };
    let calibration = json!({
        "true": spawn_ms("/usr/bin/true", ""),
        "git": spawn_ms(&env.git, "--version"),
        "zit": spawn_ms(&env.zit.display().to_string(), "--version"),
        "python": spawn_ms(&env.python, "-c pass"),
    });

    let results = match &cli.cmd {
        Cmd::Lifecycle { files, agents, rounds } => lifecycle(&env, files, agents, *rounds),
        Cmd::Workflow { agents, changes, modules, functions, seed, arms, rounds } => {
            let shape =
                Shape { agents: *agents, changes: *changes, modules: *modules, functions: *functions, seed: *seed };
            workflow(&env, &shape, arms, *rounds)
        }
        Cmd::Agents { agents } => real_agents(&env, agents),
    };
    let report = json!({
        "git": must(Command::new(&env.git).arg("--version")),
        "zit": env!("CARGO_PKG_VERSION"),
        "os": must(Command::new("uname").arg("-srm")),
        "cpus": std::thread::available_parallelism().map_or(0, |n| n.get()),
        "load_average": must(Command::new("sysctl").args(["-n", "vm.loadavg"])),
        "zit_driven": if env.cli { "cli (one process per operation)" } else { "in-process (as the MCP server)" },
        "spawn_ms": calibration,
        "results": results,
    });
    let text = serde_json::to_string_pretty(&report).expect("json");
    if let Some(out) = cli.out {
        fs::write(out, &text).expect("write results");
    }
    println!("{text}");
    if !cli.keep {
        let _ = fs::remove_dir_all(&env.work);
    }
}
