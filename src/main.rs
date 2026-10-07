use anyhow::{bail, Context};
use clap::{Args, Parser, Subcommand};
use serde::Serialize;
use std::process::ExitCode;
use zit::accept::{self, BatchOutcome, DryRun, Invalid, Outcome};
use zit::change::{self, Record};
use zit::claim::{self, Claimed};
use zit::resource::Resource;
use zit::workspace::{self, NewWorkspace, Workspace};
use zit::{api, evidence, mcp, run, tui, view, web, Repo};

/// Zit: many developers and coding agents changing one git repository at
/// once. Disposable workspaces, changes checked against what they read and
/// wrote, one accepted state, plain git underneath.
#[derive(Parser)]
#[command(name = "zit", version)]
struct Cli {
    /// Machine-readable output.
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Args)]
struct Who {
    /// What the change is meant to achieve.
    #[arg(long, default_value = "")]
    intent: String,
    /// Who is making it [default: $ZIT_AGENT or $USER].
    #[arg(long)]
    agent: Option<String>,
    /// Conversation or session id, for provenance.
    #[arg(long)]
    session: Option<String>,
    /// Change to start from [default: current].
    #[arg(long)]
    from: Option<String>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Start the graph in this git repository; REV becomes the current state.
    Init {
        #[arg(long, default_value = "HEAD")]
        from: String,
    },
    /// Create a disposable workspace holding a state; prints its path.
    #[command(visible_alias = "materialize")]
    Materialise(#[command(flatten)] Who),
    /// Declare resources observed (`path`, `path#Symbol`, `path#`).
    Read {
        #[arg(long)]
        workspace: Option<String>,
        #[arg(required = true)]
        resources: Vec<String>,
    },
    /// Claim resources you intend to write; refused if other unaccepted work holds them.
    Claim {
        #[arg(long)]
        workspace: Option<String>,
        #[arg(required_unless_present = "edit")]
        resources: Vec<String>,
        /// Claim what an edit to this path would change (by Zit's own index), instead of naming resources.
        #[arg(long, requires = "content", conflicts_with = "resources")]
        edit: Option<String>,
        /// The file's new content, for --edit.
        #[arg(long, requires = "edit")]
        content: Option<std::path::PathBuf>,
        /// With --edit: report the resources the edit would change, claim nothing.
        #[arg(long, requires = "edit")]
        dry_run: bool,
    },
    /// Snapshot a workspace into a change.
    Record {
        /// Workspace id [default: the one containing the working directory].
        #[arg(long)]
        workspace: Option<String>,
        #[arg(long)]
        intent: Option<String>,
        /// What was done and why; stored with the change.
        #[arg(long)]
        summary: Option<String>,
        /// Resource observed; repeatable.
        #[arg(long = "read")]
        reads: Vec<String>,
        /// Delete the workspace afterwards.
        #[arg(long)]
        dispose: bool,
    },
    /// Everything that exists: current, speculative changes, workspaces.
    Status,
    /// One change: what, why, dependencies, evidence.
    Show { change: String },
    /// Produce evidence for a change by running its state's checks.
    Check {
        change: String,
        /// Ignore existing evidence.
        #[arg(long)]
        rerun: bool,
        /// Run only this check (repeatable); a name no check has is an error.
        #[arg(long = "only", value_name = "NAME")]
        only: Vec<String>,
    },
    /// Make a change part of the current state. Several changes land as one batch.
    Accept {
        /// One change; or several, composed in order, checked once, landed together.
        #[arg(required_unless_present = "batch")]
        changes: Vec<String>,
        /// Every verified, non-conflicting change as one batch: compose all, check once, move current once.
        /// A change that is stale or does not compose is skipped and reported; a failing check lands nothing.
        #[arg(long)]
        batch: bool,
        /// Compose a stale change anyway; the checks on the composed state decide.
        #[arg(long)]
        allow_stale: bool,
        /// Run every check again instead of trusting existing evidence.
        #[arg(long)]
        rerun: bool,
        /// Compose as one commit on top of current, never a merge commit.
        #[arg(long)]
        linear: bool,
        /// Compose and validate only: report the outcome and which checks would run. Moves nothing, runs nothing.
        #[arg(long, conflicts_with = "batch")]
        dry_run: bool,
    },
    /// Rebuild a stale change on current, in a new workspace, to reconsider.
    Retry { change: String },
    /// Forget speculative changes.
    Discard {
        #[arg(required = true)]
        changes: Vec<String>,
    },
    /// Delete workspaces. Recorded changes are unaffected.
    Dispose {
        ids: Vec<String>,
        #[arg(long)]
        all: bool,
    },
    /// Delete every local copy zit made for this repository: workspaces,
    /// cached checkouts, verification views, caches. Changes stay in the graph.
    Clean {
        /// Also delete workspaces with unrecorded edits or a running agent.
        #[arg(long)]
        force: bool,
    },
    /// Fast-forward a git branch to the current state; with --pr, push it and open a pull request.
    Export {
        #[arg(long, default_value = "main")]
        branch: String,
        /// Push the branch and open a pull request into --base (needs the GitHub CLI, `gh`).
        #[arg(long)]
        pr: bool,
        /// The branch the pull request goes into.
        #[arg(long, default_value = "main")]
        base: String,
        /// The remote to push to.
        #[arg(long, default_value = "origin")]
        remote: String,
    },
    /// Accept a git branch's commits into current, then export current to it.
    Sync {
        #[arg(long, default_value = "main")]
        branch: String,
    },
    /// Run an agent CLI in a workspace; record its work however it ends.
    Run {
        #[command(flatten)]
        who: Who,
        /// Keep the workspace afterwards.
        #[arg(long)]
        keep: bool,
        /// Accept the recorded change.
        #[arg(long)]
        accept: bool,
        /// Stop the agent after this many seconds; its partial work is still recorded (exit 124).
        #[arg(long)]
        timeout: Option<u64>,
        /// Command to run [default: headless preset for --agent autohand|claude|codex|pi, prompted with --intent].
        #[arg(last = true)]
        command: Vec<String>,
    },
    /// Serve the graph to agents over the Model Context Protocol (stdio).
    Mcp {
        /// Also offer zit_accept and zit_discard. For the integrator, not for agents.
        #[arg(long)]
        integrator: bool,
    },
    /// Interactive view of changes and workspaces.
    Ui,
    /// Live graph in the browser: accepted line, speculative changes, workspaces, diffs.
    Web {
        /// Port to listen on (a free one is chosen if it is taken).
        #[arg(long, default_value_t = 4747)]
        port: u16,
        /// Do not open the browser.
        #[arg(long)]
        no_open: bool,
    },
}

fn default_agent() -> String {
    std::env::var("ZIT_AGENT").or_else(|_| std::env::var("USER")).unwrap_or_else(|_| "human".into())
}

fn emit<T: Serialize>(json: bool, value: &T, human: impl FnOnce()) -> anyhow::Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(value)?);
    } else {
        human();
    }
    Ok(())
}

fn print_invalid(repo: &Repo, invalid: &Invalid) {
    match invalid {
        Invalid::Stale(why) => {
            println!("rejected: stale");
            for s in why {
                let by = s.by.as_ref().and_then(|id| change::load(repo, id).ok());
                let by = by.map(|c| format!(" ({}: {})", c.agent, view::subject(&c))).unwrap_or_default();
                println!("  {}{by}", view::staleness_line(s));
            }
            println!("next: `zit retry <change>` rebuilds it on current for the agent to reconsider");
        }
        Invalid::Conflict(paths) => {
            println!("rejected: conflict (text does not merge)");
            paths.iter().for_each(|p| println!("  {p}"));
            println!("next: `zit retry <change>` opens a workspace with conflict markers");
        }
        Invalid::Failed(checks) => {
            println!("rejected: failed checks: {}", checks.join(", "));
            println!("next: `zit check <change>` shows the output; `zit accept --rerun` runs them again");
        }
        Invalid::Error(why) => println!("rejected: {why}"),
    }
}

fn print_workspace(ws: &Workspace) {
    println!("{}", ws.path().display());
    eprintln!("workspace {} on {} — edit there, then `zit record`", ws.id, ws.base.short());
}

fn workspace_id(repo: &Repo, given: Option<String>) -> anyhow::Result<String> {
    if let Some(id) = given.or_else(|| std::env::var("ZIT_WORKSPACE").ok()) {
        return Ok(id);
    }
    let cwd = std::env::current_dir()?;
    match workspace::containing(repo, &cwd) {
        Some(ws) => Ok(ws.id),
        None => bail!("not inside a workspace; pass --workspace <id> (see `zit status`)"),
    }
}

fn main() -> ExitCode {
    use clap::{CommandFactory, FromArgMatches};
    // Called as `git zit`, say so in usage and errors.
    let invoked = std::env::args_os().next().map(std::path::PathBuf::from);
    let as_git = invoked.as_deref().and_then(|p| p.file_stem()).is_some_and(|stem| stem == "git-zit");
    let command = Cli::command().bin_name(if as_git { "git zit" } else { "zit" });
    let cli = match Cli::from_arg_matches(&command.get_matches()) {
        Ok(cli) => cli,
        Err(e) => e.exit(),
    };
    match execute(cli) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("zit: {e:#}");
            ExitCode::from(2)
        }
    }
}

const OK: ExitCode = ExitCode::SUCCESS;
const NO: u8 = 1;

fn execute(cli: Cli) -> anyhow::Result<ExitCode> {
    zit::git::require_git()?;
    let json = cli.json;
    let cwd = std::env::current_dir()?;
    if let Cmd::Mcp { integrator } = cli.command {
        mcp::serve(&cwd, integrator, std::io::stdin().lock(), std::io::stdout().lock())?;
        return Ok(OK);
    }
    let repo = Repo::discover(&cwd)?;

    match cli.command {
        Cmd::Init { from } => {
            let current = repo.init(Some(&from))?;
            emit(json, &serde_json::json!({"current": current}), || println!("current is {}", current.short()))?;
        }
        Cmd::Materialise(who) => {
            let from = who.from.as_deref().map(|rev| repo.resolve(rev)).transpose()?;
            let agent = who.agent.unwrap_or_else(default_agent);
            let new = NewWorkspace {
                from: from.as_ref(),
                intent: &who.intent,
                agent: &agent,
                session: who.session.as_deref(),
            };
            let ws = workspace::materialise(&repo, &new)?;
            emit(json, &ws, || print_workspace(&ws))?;
        }
        Cmd::Read { workspace, resources } => {
            let id = workspace_id(&repo, workspace)?;
            let reads: Vec<Resource> = resources.iter().map(|r| Resource::parse(r)).collect();
            workspace::declare_reads(&repo, &id, &reads)?;
            emit(json, &serde_json::json!({"declared": reads}), || {})?;
        }
        Cmd::Claim { workspace, resources, edit, content, dry_run } => {
            let id = workspace_id(&repo, workspace)?;
            let wanted: Vec<Resource> = match (&edit, &content) {
                (Some(path), Some(content)) => {
                    if std::path::Path::new(path).components().any(|c| !matches!(c, std::path::Component::Normal(_))) {
                        anyhow::bail!("--edit takes a path inside the workspace, relative to its root: {path}");
                    }
                    let ws = workspace::get(&repo, &id)?;
                    let old = std::fs::read(ws.path().join(path)).ok();
                    let new = std::fs::read(content)?;
                    zit::footprint::edit_writes(path, old.as_deref(), &new)
                }
                _ => resources.iter().map(|r| Resource::parse(r)).collect(),
            };
            if dry_run {
                emit(json, &serde_json::json!({ "resources": wanted }), || {
                    wanted.iter().for_each(|r| println!("{r}"))
                })?;
                return Ok(OK);
            }
            let outcome = if wanted.is_empty() { Claimed::Granted } else { claim::claim(&repo, &id, &wanted)? };
            let mut shown = serde_json::to_value(&outcome)?;
            if edit.is_some() {
                shown["resources"] = serde_json::json!(wanted);
            }
            emit(json, &shown, || match &outcome {
                Claimed::Granted => println!("claimed"),
                Claimed::Refused(held) => {
                    println!("refused: nothing was claimed");
                    held.iter().for_each(|h| println!("  {}", view::held_line(h)));
                }
            })?;
            if matches!(outcome, Claimed::Refused(_)) {
                return Ok(ExitCode::from(NO));
            }
        }
        Cmd::Record { workspace, intent, summary, reads, dispose } => {
            let id = workspace_id(&repo, workspace)?;
            let opts =
                Record { intent, summary, reads: reads.iter().map(|r| Resource::parse(r)).collect(), usage: None };
            let recorded = api::record(&repo, &id, &opts, dispose)?;
            emit(json, &recorded, || match &recorded.change {
                None => println!("nothing to record"),
                Some(c) => {
                    println!("{}", c.id);
                    recorded.writes.iter().for_each(|w| eprintln!("  wrote {w}"));
                    for w in recorded.writes.iter().filter(|w| zit::symbols::unparsed_code(w.path())) {
                        eprintln!(
                            "  note: {} is not parsed: it is one resource, and Zit cannot tell what it calls",
                            w.path()
                        );
                    }
                }
            })?;
        }
        Cmd::Status => {
            let overview = api::overview(&repo)?;
            emit(json, &overview, || print!("{}", view::overview_text(&overview)))?;
        }
        Cmd::Show { change } => {
            let detail = api::detail(&repo, &repo.resolve(&change)?)?;
            emit(json, &detail, || print!("{}", view::detail_text(&detail)))?;
        }
        Cmd::Check { change, rerun, only } => {
            let verdicts = evidence::verify_only(&repo, &repo.resolve(&change)?, rerun, &only)?;
            emit(json, &verdicts, || {
                for v in &verdicts {
                    let e = &v.evidence;
                    let mark = if e.passed { "pass" } else { "FAIL" };
                    let how = if v.cached { "reused".into() } else { format!("{}ms", e.duration_ms) };
                    println!("{mark}  {}  ({how})", e.check);
                    if !e.passed {
                        e.output.lines().for_each(|l| println!("      {l}"));
                    }
                }
                if verdicts.is_empty() {
                    println!("no checks declared (add zit.toml)");
                }
            })?;
            if verdicts.iter().any(|v| !v.evidence.passed) {
                return Ok(ExitCode::from(NO));
            }
        }
        Cmd::Accept { changes, batch, allow_stale, rerun, linear, dry_run } => {
            let policy = accept::Policy { allow_stale, rerun, linear };
            if dry_run {
                let [change] = changes.as_slice() else { bail!("--dry-run takes one change") };
                let dry = accept::dry_run(&repo, &repo.resolve(change)?, &policy)?;
                emit(json, &dry, || match &dry {
                    DryRun::AlreadyAccepted => println!("already accepted"),
                    DryRun::Rejected(invalid) => print_invalid(&repo, invalid),
                    DryRun::WouldAccept { composed, checks } => {
                        let how = if *composed { "composed onto current" } else { "fast-forward" };
                        let planned: Vec<String> = checks
                            .iter()
                            .map(|c| format!("{} ({})", c.check, if c.run { "run" } else { "reused" }))
                            .collect();
                        let planned = if planned.is_empty() { "none".to_string() } else { planned.join(", ") };
                        println!("would accept {} ({how}); checks: {planned}", change);
                    }
                })?;
                return Ok(if matches!(dry, DryRun::Rejected(_)) { ExitCode::from(NO) } else { OK });
            }
            if let ([change], false) = (changes.as_slice(), batch) {
                let outcome = accept::accept_with(&repo, &repo.resolve(change)?, &policy)?;
                return report_outcome(&repo, json, &outcome);
            }
            let ids = match changes.is_empty() {
                true => accept::verified(&repo)?,
                false => changes.iter().map(|c| repo.resolve(c)).collect::<Result<_, _>>()?,
            };
            let outcome = accept::accept_batch(&repo, &ids, &policy)?;
            return report_batch(&repo, json, &outcome);
        }
        Cmd::Retry { change } => {
            let ws = accept::retry(&repo, &repo.resolve(&change)?)?;
            emit(json, &ws, || print_workspace(&ws))?;
        }
        Cmd::Discard { changes } => {
            for rev in &changes {
                change::discard(&repo, &repo.resolve(rev)?)?;
            }
            emit(json, &serde_json::json!({"discarded": changes}), || {})?;
        }
        Cmd::Dispose { ids, all } => {
            let ids = if all { workspace::list(&repo)?.into_iter().map(|w| w.id).collect() } else { ids };
            for id in &ids {
                workspace::dispose(&repo, id)?;
            }
            emit(json, &serde_json::json!({"disposed": ids}), || {})?;
        }
        Cmd::Clean { force } => {
            let report = zit::clean::clean(&repo, force)?;
            emit(json, &report, || {
                println!(
                    "deleted {} workspaces, {} cached states, {} verification views; {} MB more free on the volume",
                    report.workspaces,
                    report.cached_states,
                    report.verification_views,
                    report.freed_bytes / 1_000_000
                )
            })?;
        }
        Cmd::Export { branch, pr: true, base, remote } => {
            let url = accept::pull_request(&repo, &branch, &base, &remote)?;
            emit(json, &serde_json::json!({"branch": branch, "base": base, "pull_request": url}), || {
                println!("{url}")
            })?;
        }
        Cmd::Export { branch, .. } => {
            let current = accept::export(&repo, &branch)?;
            emit(json, &serde_json::json!({"branch": branch, "current": current}), || {
                println!("{branch} is {}", current.short())
            })?;
        }
        Cmd::Sync { branch } => {
            let tip = repo.resolve(&format!("refs/heads/{branch}"))?;
            let outcome = accept::accept(&repo, &tip)?;
            if let Outcome::Rejected(_) = outcome {
                return report_outcome(&repo, json, &outcome);
            }
            let current = accept::export(&repo, &branch)?;
            emit(json, &serde_json::json!({"branch": branch, "current": current}), || {
                println!("{branch} is {}", current.short())
            })?;
        }
        Cmd::Run { who, keep, accept, timeout, command } => {
            let agent = who.agent.unwrap_or_else(default_agent);
            // Presets that print JSON events: their final message and usage are read from them.
            let structured = match command.is_empty() {
                true => matches!(agent.as_str(), "claude" | "codex" | "autohand"),
                false => run::prints_json_events(&command),
            };
            let command = match command.is_empty() {
                false => command,
                true => run::preset(&agent, &who.intent, &[repo.home().to_path_buf()])
                    .with_context(|| format!("no command given and no preset for agent `{agent}`"))?,
            };
            let from = who.from.as_deref().map(|rev| repo.resolve(rev)).transpose()?;
            let opts = run::Run {
                agent,
                intent: who.intent,
                session: who.session,
                from,
                keep,
                accept,
                quiet_stdout: json,
                timeout: timeout.map(std::time::Duration::from_secs),
                structured,
                command,
            };
            let report = run::run(&repo, &opts)?;
            emit(json, &report, || {
                match &report.change {
                    Some(c) => eprintln!("zit: recorded {} ({})", c.id.short(), view::subject(c)),
                    None => eprintln!("zit: nothing to record"),
                }
                if let Some(Outcome::Rejected(invalid)) = &report.outcome {
                    print_invalid(&repo, invalid);
                }
            })?;
            return Ok(ExitCode::from(report.exit_code.clamp(0, 255) as u8));
        }
        Cmd::Ui => tui::run(&repo)?,
        Cmd::Web { port, no_open } => {
            let server = web::Server::bind(repo, port)?;
            let url = server.url();
            eprintln!("zit web: {url}  (read-only; Ctrl-C to stop)");
            if !no_open {
                web::open_browser(&url);
            }
            server.run();
        }
        Cmd::Mcp { .. } => unreachable!("handled above"),
    }
    Ok(OK)
}

fn report_batch(repo: &Repo, json: bool, outcome: &BatchOutcome) -> anyhow::Result<ExitCode> {
    let subject = |id: &zit::Oid| change::load(repo, id).map(|c| view::subject(&c).to_string()).unwrap_or_default();
    let print_skipped = |skipped: &[accept::Skipped]| {
        for s in skipped {
            let status = accept::Status::Invalid(s.reason.clone());
            println!("skipped  {}  {}", s.change.short(), view::status_label(&status));
            view::reasons(&status).iter().for_each(|line| println!("    {line}"));
        }
    };
    emit(json, outcome, || match outcome {
        BatchOutcome::Accepted { current, landed, skipped, verdicts } => {
            landed.iter().for_each(|id| println!("landed   {}  {}", id.short(), subject(id)));
            print_skipped(skipped);
            let reused = verdicts.iter().filter(|v| v.cached).count();
            println!(
                "current is {} ({} landed, {} skipped; checks: {} run, {reused} reused)",
                current.short(),
                landed.len(),
                skipped.len(),
                verdicts.len() - reused
            );
        }
        BatchOutcome::Rejected { failed, tried, skipped } => {
            print_skipped(skipped);
            let tried: Vec<&str> = tried.iter().map(zit::Oid::short).collect();
            println!(
                "rejected: failed checks: {} on the combined state of {}; nothing landed",
                failed.join(", "),
                tried.join(", ")
            );
            println!("next: accept one at a time to find the change responsible: `zit accept <change>`");
        }
    })?;
    Ok(match outcome {
        BatchOutcome::Rejected { .. } => ExitCode::from(NO),
        _ => OK,
    })
}

fn report_outcome(repo: &Repo, json: bool, outcome: &Outcome) -> anyhow::Result<ExitCode> {
    emit(json, outcome, || match outcome {
        Outcome::AlreadyAccepted => println!("already accepted"),
        Outcome::Rejected(invalid) => print_invalid(repo, invalid),
        Outcome::Accepted { current, composed, verdicts } => {
            let reused = verdicts.iter().filter(|v| v.cached).count();
            let how = if *composed { "composed onto current" } else { "fast-forward" };
            println!(
                "current is {} ({how}; checks: {} run, {reused} reused)",
                current.short(),
                verdicts.len() - reused
            );
        }
    })?;
    Ok(match outcome {
        Outcome::Rejected(_) => ExitCode::from(NO),
        _ => OK,
    })
}
