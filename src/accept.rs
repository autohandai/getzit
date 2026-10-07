//! Acceptance: selecting which change becomes the current state. Status is
//! derived from the graph on demand, never stored.

use crate::change::{self, Change};
use crate::evidence::{self, Verdict};
use crate::footprint::{self, ConflictKind};
use crate::git::{Oid, Repo, CHANGES, CURRENT};
use crate::resource::Resource;
use crate::workspace::{self, NewWorkspace, Workspace};
use crate::{Error, Result};
use serde::Serialize;
use std::process::Stdio;

/// One reason a change is stale against current.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Staleness {
    pub resource: Resource,
    pub kind: ConflictKind,
    /// The accepted change on the other side of the conflict.
    pub by: Option<Oid>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case", tag = "reason", content = "detail")]
pub enum Invalid {
    /// It read or wrote something current has since changed.
    Stale(Vec<Staleness>),
    /// It composes semantically but its text does not merge; the paths.
    Conflict(Vec<String>),
    /// These checks failed.
    Failed(Vec<String>),
    /// Its state cannot be evaluated (for example an unreadable `zit.toml`).
    Error(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case", tag = "status")]
pub enum Status {
    /// Recorded; evidence incomplete.
    Speculative,
    /// Does not conflict with current, and every check of its own state
    /// passed. Accepting still verifies the composed state.
    Verified,
    Invalid(Invalid),
    /// Part of current's history.
    Accepted,
    Current,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "kebab-case", tag = "outcome")]
pub enum Outcome {
    Accepted {
        current: Oid,
        /// A compose change was created because current had moved on.
        composed: bool,
        verdicts: Vec<Verdict>,
    },
    AlreadyAccepted,
    Rejected(Invalid),
}

/// A step of acceptance that can refuse the change (`Err(Invalid)`) as well as fail.
type Checked<T> = Result<std::result::Result<T, Invalid>>;

/// Text-level three-way composition of two changes.
struct Merged {
    state: Oid,
    conflicts: Vec<String>,
}

/// Text-level composition, where conflicts inside generated files do not
/// count: those files are regenerated on the result.
fn merge(repo: &Repo, ours: &Oid, theirs: &Oid) -> Result<Merged> {
    let mut merged = merge_text(repo, ours, theirs)?;
    let generated = generated_paths(repo, ours)?;
    merged.conflicts.retain(|path| !generated.contains(path));
    Ok(merged)
}

/// Paths of the generated files declared by `rev`'s `zit.toml`.
pub(crate) fn generated_paths(repo: &Repo, rev: &Oid) -> Result<Vec<String>> {
    Ok(evidence::derived(repo, rev)?.into_iter().map(|d| d.path).collect())
}

/// Prose has no read semantics; two edits to one section are decided by
/// whether their text merges.
fn is_prose(path: &str) -> bool {
    matches!(path.rsplit_once('.').map(|(_, ext)| ext), Some("md" | "mdx" | "markdown"))
}

fn merge_text(repo: &Repo, ours: &Oid, theirs: &Oid) -> Result<Merged> {
    let mut cmd = repo.cmd();
    cmd.args(["merge-tree", "--write-tree", "--name-only", "--no-messages", ours.as_str(), theirs.as_str()]);
    let _trace = crate::git::Trace::start(&cmd);
    let out = cmd.stdin(Stdio::null()).output()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let mut lines = text.lines();
    match (out.status.code(), lines.next()) {
        (Some(0 | 1), Some(state)) => {
            Ok(Merged { state: Oid::new(state), conflicts: lines.map(str::to_string).collect() })
        }
        _ => Err(Error::Git {
            args: "merge-tree".into(),
            stderr: String::from_utf8_lossy(&out.stderr).trim().to_string(),
        }),
    }
}

/// Why `change` cannot be composed onto `current`, given their merge base,
/// with the accepted change responsible for each reason.
fn staleness(repo: &Repo, base: &Oid, change: &Oid, current: &Oid) -> Result<Vec<Staleness>> {
    let mine = footprint::between(repo, base, change)?;
    let generated = generated_paths(repo, current)?;
    let mut conflicts = mine.conflicts(&footprint::between(repo, base, current)?);
    conflicts.retain(|c| {
        let path = c.resource.path();
        let text_decides =
            is_prose(path) || matches!(&c.resource, Resource::Symbol(_, name) if name == crate::symbols::IMPORTS);
        !generated.iter().any(|g| g == path) && (c.kind != ConflictKind::WriteWrite || !text_decides)
    });
    if conflicts.is_empty() {
        return Ok(vec![]);
    }

    let range = format!("{base}..{current}");
    let log = repo.git(&["rev-list", "--first-parent", "--reverse", "--parents", &range])?;
    let mut steps = Vec::new();
    for line in log.lines() {
        let mut ids = line.split_whitespace().map(Oid::new);
        if let (Some(id), Some(parent)) = (ids.next(), ids.next()) {
            steps.push((footprint::between(repo, &parent, &id)?, id));
        }
    }
    Ok(conflicts
        .into_iter()
        .map(|c| {
            let by = steps.iter().find(|(fp, _)| match c.kind {
                ConflictKind::WriteRead => fp.reads(&c.resource, mine.signatures.contains(&c.resource)),
                _ => fp.writes.iter().any(|w| w.overlaps(&c.resource)),
            });
            Staleness { resource: c.resource, kind: c.kind, by: by.map(|(_, id)| id.clone()) }
        })
        .collect())
}

/// Files both sides changed since `base` that parsed on each side and do not
/// parse in `merged`: errors the composition itself introduced.
fn unparsable(repo: &Repo, base: &Oid, ours: &Oid, theirs: &Oid, merged: &Oid) -> Result<Vec<String>> {
    let changed = |tip: &Oid| -> Result<std::collections::BTreeSet<String>> {
        let out = repo.git(&["diff-tree", "-r", "--name-only", "--no-renames", base.as_str(), tip.as_str()])?;
        Ok(out.lines().map(str::to_string).collect())
    };
    let both: Vec<String> = changed(ours)?.intersection(&changed(theirs)?).cloned().collect();
    let mut objects = repo.objects()?;
    let mut bad = Vec::new();
    for path in both {
        let mut errors = |rev: &Oid| -> Result<Option<bool>> {
            Ok(objects.read(&format!("{rev}:{path}"))?.and_then(|src| crate::symbols::has_syntax_errors(&path, &src)))
        };
        if errors(merged)? == Some(true) && errors(ours)? == Some(false) && errors(theirs)? == Some(false) {
            bad.push(path);
        }
    }
    Ok(bad)
}

/// Compose `change` onto `current` without writing anything: staleness, the
/// text merge, and the parse check (unless current's zit.toml turns it off).
fn validate(repo: &Repo, change: &Oid, base: &Oid, current: &Oid, policy: &Policy) -> Checked<Merged> {
    if !policy.allow_stale {
        let stale = staleness(repo, base, change, current)?;
        if !stale.is_empty() {
            return Ok(Err(Invalid::Stale(stale)));
        }
    }
    let merged = merge(repo, current, change)?;
    if !merged.conflicts.is_empty() {
        return Ok(Err(Invalid::Conflict(merged.conflicts)));
    }
    if evidence::accept_rules(repo, current)?.parse_check {
        let bad = unparsable(repo, base, current, change, &merged.state)?;
        if !bad.is_empty() {
            return Ok(Err(Invalid::Error(format!("does not parse after composing: {}", bad.join(", ")))));
        }
    }
    Ok(Ok(merged))
}

/// Write the compose change for a validated merge and regenerate its
/// generated files: the candidate and its state.
fn land(repo: &Repo, source: &Change, current: &Oid, merged: &Merged, policy: &Policy) -> Checked<(Oid, Oid)> {
    let change = &source.id;
    let subject = source.intent.lines().next().unwrap_or_default();
    let linear = policy.linear || evidence::accept_rules(repo, current)?.linear;
    let mut msg = change::message(
        &match linear {
            true => subject.to_string(),
            false => format!("Compose {}: {subject}", change.short()),
        },
        source.summary.as_deref(),
        &source.agent,
        None,
        &[],
    );
    if linear {
        msg.push_str(&format!("Zit-Change: {change}\n"));
    }
    let parents: Vec<&Oid> = if linear { vec![current] } else { vec![current, change] };
    let id = change::commit(repo, &merged.state, &parents, &source.agent, &msg, workspace::now())?;
    let rules = evidence::derived(repo, &id)?;
    if rules.is_empty() {
        return Ok(Ok((id, merged.state.clone())));
    }
    Ok(match evidence::regenerate(repo, &id, &merged.state, &rules)? {
        Ok(state) => Ok((change::commit(repo, &state, &parents, &source.agent, &msg, workspace::now())?, state)),
        Err(failed) => Err(Invalid::Failed(vec![failed])),
    })
}

fn no_shared_history(change: &Oid) -> Error {
    Error::msg(format!("{} shares no history with current", change.short()))
}

/// A change together with the current change, in one git call. The second
/// is `None` when they are the same.
fn load_with_current(repo: &Repo, change: &Oid) -> Result<(Change, Option<Oid>)> {
    let mut loaded = match change::load_revs(repo, &[change.as_str(), CURRENT]) {
        Ok(loaded) => loaded.into_iter(),
        Err(e) => return Err(repo.current().err().unwrap_or(e)),
    };
    let source = loaded.next().ok_or_else(|| Error::UnknownRevision(change.to_string()))?;
    Ok((source, loaded.next().map(|c| c.id)))
}

/// Where a change stands relative to current and the evidence at hand.
pub fn status(repo: &Repo, change: &Oid) -> Result<Status> {
    match load_with_current(repo, change)? {
        (_, None) => Ok(Status::Current),
        (source, Some(current)) => Ok(evaluate(repo, &source, &current)),
    }
}

/// Status of one change. A change that cannot be evaluated is reported as
/// invalid rather than failing the whole listing.
pub(crate) fn evaluate(repo: &Repo, change: &Change, current: &Oid) -> Status {
    try_evaluate(repo, change, current).unwrap_or_else(|e| Status::Invalid(Invalid::Error(e.to_string())))
}

fn try_evaluate(repo: &Repo, change: &Change, current: &Oid) -> Result<Status> {
    if change.id == *current {
        return Ok(Status::Current);
    }
    let base = repo.merge_base(&change.id, current)?.ok_or_else(|| no_shared_history(&change.id))?;
    if base == change.id || landed_linearly(repo, &change.id, &base, current)? {
        return Ok(Status::Accepted);
    }
    if base != *current {
        let stale = staleness(repo, &base, &change.id, current)?;
        if !stale.is_empty() {
            return Ok(Status::Invalid(Invalid::Stale(stale)));
        }
        let merged = merge(repo, current, &change.id)?;
        if !merged.conflicts.is_empty() {
            return Ok(Status::Invalid(Invalid::Conflict(merged.conflicts)));
        }
    }
    let found = evidence::lookup(repo, &change.state)?;
    let failed: Vec<String> =
        found.iter().filter(|(_, e)| e.as_ref().is_some_and(|e| !e.passed)).map(|(c, _)| c.name.clone()).collect();
    Ok(if !failed.is_empty() {
        Status::Invalid(Invalid::Failed(failed))
    } else if found.iter().all(|(_, e)| e.is_some()) {
        Status::Verified
    } else {
        Status::Speculative
    })
}

/// Make `change` part of the current state: compose it onto current if
/// current has moved, verify the result, then advance current atomically.
/// Rejection destroys nothing; the change stays in the graph.
pub fn accept(repo: &Repo, change: &Oid) -> Result<Outcome> {
    accept_with(repo, change, &Policy::default())
}

#[derive(Debug, Default, Clone, Copy)]
pub struct Policy {
    /// Compose a stale change anyway and let the checks on the composed
    /// state decide. Text must still merge. The default refuses stale changes.
    pub allow_stale: bool,
    /// Run every check again instead of trusting existing evidence (a flaky failure).
    pub rerun: bool,
    /// Compose as one commit on top of current instead of a merge commit.
    /// Also on when current's zit.toml says `[accept] linear = true`.
    pub linear: bool,
}

/// The commit on current's first-parent line that landed `change` linearly, if any.
fn landed_linearly(repo: &Repo, change: &Oid, base: &Oid, current: &Oid) -> Result<bool> {
    let grep = format!("--grep=^Zit-Change: {change}$");
    let found = repo.git(&["log", "--first-parent", "-1", "--format=%H", &grep, &format!("{base}..{current}")])?;
    Ok(!found.trim().is_empty())
}

/// Failed ref updates with current unchanged before giving up.
const STUCK_LIMIT: usize = 10;

pub fn accept_with(repo: &Repo, change: &Oid, policy: &Policy) -> Result<Outcome> {
    let mut stuck = 0;
    loop {
        let (source, Some(current)) = load_with_current(repo, change)? else {
            return Ok(Outcome::AlreadyAccepted);
        };
        let base = repo.merge_base(change, &current)?.ok_or_else(|| no_shared_history(change))?;
        if base == *change || landed_linearly(repo, change, &base, &current)? {
            return Ok(Outcome::AlreadyAccepted);
        }

        let composed = base != current;
        let (candidate, state) = if composed {
            let merged = match validate(repo, change, &base, &current, policy)? {
                Ok(merged) => merged,
                Err(invalid) => return Ok(Outcome::Rejected(invalid)),
            };
            match land(repo, &source, &current, &merged, policy)? {
                Ok(landed) => landed,
                Err(invalid) => return Ok(Outcome::Rejected(invalid)),
            }
        } else {
            (change.clone(), source.state.clone())
        };

        let verdicts = evidence::verify_state(repo, &candidate, &state, Some(&current), policy.rerun)?;
        let failed: Vec<String> =
            verdicts.iter().filter(|v| !v.evidence.passed).map(|v| v.evidence.check.clone()).collect();
        if !failed.is_empty() {
            return Ok(Outcome::Rejected(Invalid::Failed(failed)));
        }

        // Advance current and retire the speculative ref in one atomic step.
        let landed = repo
            .transaction(&[format!("update {CURRENT} {candidate} {current}"), format!("delete {CHANGES}/{change}")])?;
        if landed {
            return Ok(Outcome::Accepted { current: candidate, composed, verdicts });
        }
        // Someone else advanced current first: re-evaluate against the new one.
        // If current did not move, the update itself is failing (a stale
        // lock, permissions); do not spin on it.
        if repo.current()? == current {
            stuck += 1;
            if stuck >= STUCK_LIMIT {
                return Err(Error::msg(format!(
                    "could not move {CURRENT}; check for a stale {CURRENT}.lock in {}",
                    repo.git_dir().display()
                )));
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        } else {
            stuck = 0;
        }
    }
}

/// Drop speculative refs of changes that current already contains (the
/// ancestors of an accepted chain).
pub(crate) fn prune_accepted(repo: &Repo) -> Result<()> {
    let merged = repo.git(&["for-each-ref", "--merged", CURRENT, "--format=delete %(refname)", CHANGES])?;
    if !merged.is_empty() {
        // A racing prune may have removed the same refs; that is the desired end state.
        let _ = repo.transaction(&merged.lines().map(str::to_string).collect::<Vec<_>>());
    }
    Ok(())
}

/// Re-run a change against the new current: a workspace on current holding
/// the change's edits (conflict markers where text collides), for its agent
/// to reconsider. Recording it yields a change with both as parents.
pub fn retry(repo: &Repo, change: &Oid) -> Result<Workspace> {
    let current = repo.current()?;
    let source = change::load(repo, change)?;
    let merged = merge(repo, &current, change)?;
    let new = NewWorkspace {
        from: Some(&current),
        intent: &source.intent,
        agent: &source.agent,
        session: source.session.as_deref(),
    };
    let mut ws = workspace::materialise(repo, &new)?;
    crate::git::run(ws.git().args(["read-tree", "-m", "-u", ws.base_state.as_str(), merged.state.as_str()]))?;
    ws.merge_parent = Some(change.clone());
    ws.save()?;
    Ok(ws)
}

/// Publish current to a git branch (fast-forward only). If the branch is
/// checked out somewhere, that checkout's files move with it.
/// Export current to `branch`, push it to `remote` and open a pull request
/// into `base` whose body carries every change's intent and reason. Uses
/// the GitHub CLI (`gh`, or `$ZIT_GH`). Returns what `gh` printed: the URL.
pub fn pull_request(repo: &Repo, branch: &str, base: &str, remote: &str) -> Result<String> {
    if branch == base {
        return Err(Error::msg(format!("the pull request needs its own branch: `--branch <name>`, not {base}")));
    }
    let current = export(repo, branch)?;
    repo.git(&["push", "--quiet", remote, &format!("refs/heads/{branch}:refs/heads/{branch}")])?;

    // What the pull request brings: first-parent commits not on the base yet.
    let upstream = [format!("refs/remotes/{remote}/{base}"), format!("refs/heads/{base}")]
        .into_iter()
        .find(|r| repo.git(&["rev-parse", "--verify", "--quiet", r]).is_ok());
    let range = upstream.map_or(current.to_string(), |u| format!("{u}..{current}"));
    let log = repo.git(&["log", "--first-parent", "--reverse", "--format=%B%x01", &range])?;
    let changes: Vec<(String, String)> = log
        .split('\x01')
        .map(str::trim)
        .filter(|m| !m.is_empty())
        .map(|m| {
            let text: Vec<&str> = m.lines().filter(|l| !l.starts_with("Zit-")).collect();
            let text = text.join("\n");
            let (subject, rest) = text.split_once("\n\n").unwrap_or((&text, ""));
            let subject =
                subject.split_once(": ").filter(|(p, _)| p.starts_with("Compose ")).map_or(subject, |(_, s)| s);
            (subject.trim().to_string(), rest.trim().to_string())
        })
        .collect();
    let title = match changes.as_slice() {
        [(only, _)] => only.clone(),
        many => format!("{} changes", many.len()),
    };
    let body: String = changes
        .iter()
        .map(|(subject, reason)| match reason.is_empty() {
            true => format!("### {subject}\n\n"),
            false => format!("### {subject}\n\n{reason}\n\n"),
        })
        .collect();

    let gh = std::env::var_os("ZIT_GH").unwrap_or_else(|| "gh".into());
    let workdir =
        repo.git_dir().parent().map(std::path::Path::to_path_buf).unwrap_or_else(|| repo.git_dir().to_path_buf());
    let out = std::process::Command::new(&gh)
        .current_dir(workdir)
        .args(["pr", "create", "--base", base, "--head", branch, "--title", &title, "--body", body.trim_end()])
        .stdin(Stdio::null())
        .output()
        .map_err(|e| Error::msg(format!("cannot run the GitHub CLI ({}): {e}", gh.to_string_lossy())))?;
    if !out.status.success() {
        return Err(Error::msg(format!("gh pr create failed: {}", String::from_utf8_lossy(&out.stderr).trim())));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

pub fn export(repo: &Repo, branch: &str) -> Result<Oid> {
    let current = repo.current()?;
    let name = format!("refs/heads/{branch}");
    let Ok(tip) = repo.git(&["rev-parse", "--verify", "--quiet", &name]).map(Oid::new) else {
        repo.git(&["update-ref", &name, current.as_str(), ""])?;
        return Ok(current);
    };
    if !repo.is_ancestor(&tip, &current)? {
        return Err(Error::msg(format!("branch {branch} has commits current does not; `zit accept {branch}` first")));
    }

    let worktrees = repo.git(&["worktree", "list", "--porcelain"])?;
    let checked_out = worktrees.split("\n\n").find_map(|entry| {
        let path = entry.lines().find_map(|l| l.strip_prefix("worktree "))?;
        entry.lines().any(|l| l.strip_prefix("branch ") == Some(&name)).then(|| path.to_string())
    });
    match checked_out {
        Some(path) => {
            crate::git::run(crate::git::git_command().current_dir(path).args([
                "merge",
                "--ff-only",
                "--quiet",
                current.as_str(),
            ]))?;
        }
        None => {
            repo.git(&["update-ref", &name, current.as_str(), tip.as_str()])?;
        }
    }
    Ok(current)
}
