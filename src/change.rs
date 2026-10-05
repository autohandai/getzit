//! Changes: immutable program transitions. A change is a git commit whose
//! tree is the resulting state and whose parents are the changes it builds on.

use crate::git::{Oid, Repo, CHANGES};
use crate::resource::Resource;
use crate::workspace::{self, Workspace};
use crate::Result;
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Change {
    pub id: Oid,
    /// The resulting state.
    pub state: Oid,
    pub parents: Vec<Oid>,
    pub intent: String,
    /// What its author reported doing, and why: an agent's final message, or
    /// the body of a git commit.
    pub summary: Option<String>,
    pub agent: String,
    pub session: Option<String>,
    /// Resources the agent declared it observed.
    pub reads: Vec<Resource>,
    pub time: u64,
}

#[derive(Debug, Default)]
pub struct Record {
    /// Overrides the workspace intent.
    pub intent: Option<String>,
    /// What was done and why.
    pub summary: Option<String>,
    /// Observed resources, in addition to those declared on the workspace.
    pub reads: Vec<Resource>,
}

const FORMAT: &str = "--format=%H%x00%T%x00%P%x00%an%x00%ct%x00%B%x01";

fn parse(record: &str) -> Option<Change> {
    let mut fields = record.trim_start().splitn(6, '\0');
    let id = Oid::new(fields.next()?);
    let state = Oid::new(fields.next()?);
    let parents = fields.next()?.split_whitespace().map(Oid::new).collect();
    let author = fields.next()?.to_string();
    let time = fields.next()?.parse().ok()?;
    let message = fields.next()?;

    let (mut agent, mut session, mut reads, mut text) = (author, None, Vec::new(), Vec::new());
    for line in message.lines() {
        if let Some(v) = line.strip_prefix("Zit-Agent: ") {
            agent = v.to_string();
        } else if let Some(v) = line.strip_prefix("Zit-Session: ") {
            session = Some(v.to_string());
        } else if let Some(v) = line.strip_prefix("Zit-Read: ") {
            reads.push(Resource::parse(v));
        } else {
            text.push(line);
        }
    }
    // The first paragraph is the intent; the rest is the author's account.
    let text = text.join("\n");
    let text = text.trim();
    let (intent, summary) = match text.split_once("\n\n") {
        Some((intent, rest)) => (intent.trim().to_string(), Some(rest.trim().to_string()).filter(|s| !s.is_empty())),
        None => (text.to_string(), None),
    };
    Some(Change { id, state, parents, intent, summary, agent, session, reads, time })
}

fn load_many(repo: &Repo, ids: &str) -> Result<Vec<Change>> {
    if ids.trim().is_empty() {
        return Ok(vec![]);
    }
    let out = repo.git_stdin(&["log", "--no-walk=unsorted", "--stdin", FORMAT], &[], ids)?;
    Ok(out.split('\x01').filter_map(parse).collect())
}

/// Load several revisions with one git call, in the order given.
pub(crate) fn load_revs(repo: &Repo, revs: &[&str]) -> Result<Vec<Change>> {
    load_many(repo, &revs.join("\n"))
}

/// Any commit is a change; plain git commits simply carry no zit metadata.
pub fn load(repo: &Repo, id: &Oid) -> Result<Change> {
    let unknown = || crate::Error::UnknownRevision(id.to_string());
    load_many(repo, id.as_str()).map_err(|_| unknown())?.pop().ok_or_else(unknown)
}

/// Recorded changes that are not (yet) part of the current state's history.
pub fn speculative(repo: &Repo) -> Result<Vec<Change>> {
    let format =
        "--format=%(objectname)%00%(tree)%00%(parent)%00%(authorname)%00%(committerdate:unix)%00%(contents)%01";
    let out = repo.git(&["for-each-ref", "--no-merged", crate::git::CURRENT, format, CHANGES])?;
    let mut changes: Vec<Change> = out.split('\x01').filter_map(parse).collect();
    changes.sort_by(|a, b| (a.time, &a.id).cmp(&(b.time, &b.id)));
    Ok(changes)
}

/// Accepted history, newest first.
pub fn accepted(repo: &Repo, limit: usize) -> Result<Vec<Change>> {
    let out = repo.git(&["log", "--first-parent", &format!("-n{limit}"), FORMAT, crate::git::CURRENT])?;
    Ok(out.split('\x01').filter_map(parse).collect())
}

/// The intent as one paragraph: blank lines would end it.
pub(crate) fn one_paragraph(text: &str) -> String {
    text.lines().map(str::trim_end).filter(|l| !l.trim().is_empty()).collect::<Vec<_>>().join("\n")
}

/// Control characters (other than newline and tab) break parsing and terminals; drop them.
fn printable(text: &str) -> String {
    text.chars().filter(|c| !c.is_control() || *c == '\n' || *c == '\t').collect()
}

/// A line that looks like a trailer would be read back as one.
fn safe(text: &str) -> String {
    text.lines()
        .map(|l| if l.starts_with("Zit-") { format!(" {l}") } else { l.to_string() })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Commit message: the intent, the author's account if any, then trailers.
pub(crate) fn message(
    intent: &str,
    summary: Option<&str>,
    agent: &str,
    session: Option<&str>,
    reads: &[Resource],
) -> String {
    let mut msg = safe(&one_paragraph(&printable(intent)));
    if let Some(summary) = Some(printable(summary.unwrap_or_default())).filter(|s| !s.trim().is_empty()) {
        msg.push_str("\n\n");
        msg.push_str(&safe(summary.trim()));
    }
    msg.push_str(&format!("\n\nZit-Agent: {agent}\n"));
    if let Some(session) = session {
        msg.push_str(&format!("Zit-Session: {session}\n"));
    }
    for read in reads {
        msg.push_str(&format!("Zit-Read: {read}\n"));
    }
    msg
}

/// Write a change object. It is not referenced yet: `keep` it as a
/// speculative change, or make it current.
pub(crate) fn commit(repo: &Repo, state: &Oid, parents: &[&Oid], agent: &str, message: &str, time: u64) -> Result<Oid> {
    let mut args = vec!["commit-tree".to_string(), state.to_string()];
    for parent in parents {
        args.extend(["-p".to_string(), parent.to_string()]);
    }
    let email = format!("{}@zit", agent.replace(|c: char| c.is_whitespace() || c == '<' || c == '>', "-"));
    let date = format!("@{time} +0000");
    let env = [
        ("GIT_AUTHOR_NAME", agent),
        ("GIT_AUTHOR_EMAIL", email.as_str()),
        ("GIT_AUTHOR_DATE", date.as_str()),
        ("GIT_COMMITTER_NAME", "zit"),
        ("GIT_COMMITTER_EMAIL", "zit@localhost"),
        ("GIT_COMMITTER_DATE", date.as_str()),
    ];
    Ok(Oid::new(repo.git_stdin(&args, &env, message)?))
}

pub(crate) fn keep(repo: &Repo, id: &Oid) -> Result<()> {
    repo.git(&["update-ref", &format!("{CHANGES}/{id}"), id.as_str()])?;
    Ok(())
}

/// Snapshot a workspace into a change. `None` when nothing differs from its base.
pub fn record(repo: &Repo, workspace: &str, opts: &Record) -> Result<Option<Change>> {
    let mut ws: Workspace = workspace::get(repo, workspace)?;
    crate::git::run(ws.git().args(["add", "-A"]))?;
    let state = Oid::new(crate::git::run(ws.git().args(["write-tree"]))?);
    if state == ws.base_state && ws.merge_parent.is_none() {
        return Ok(None);
    }

    let mut reads = ws.declared_reads();
    reads.extend(opts.reads.iter().cloned());
    reads.sort();
    reads.dedup();
    let intent = one_paragraph(opts.intent.as_deref().unwrap_or(&ws.intent));
    let summary = opts.summary.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
    let msg = message(&intent, summary.as_deref(), &ws.agent, ws.session.as_deref(), &reads);
    let mut parents = vec![ws.base.clone()];
    parents.extend(ws.merge_parent.clone());
    let time = workspace::now();
    let id = commit(repo, &state, &parents.iter().collect::<Vec<_>>(), &ws.agent, &msg, time)?;
    keep(repo, &id)?;
    let change = Change {
        id: id.clone(),
        state: state.clone(),
        parents,
        intent,
        summary,
        agent: ws.agent.clone(),
        session: ws.session.clone(),
        reads,
        time,
    };

    // The view now sits on the new change; further edits chain from it.
    ws.base = id;
    ws.base_state = state;
    ws.merge_parent = None;
    ws.save()?;
    let _ = std::fs::remove_file(ws.reads_file());
    Ok(Some(change))
}

/// Forget a speculative change. The object stays until git collects it.
pub fn discard(repo: &Repo, id: &Oid) -> Result<()> {
    repo.git(&["update-ref", "-d", &format!("{CHANGES}/{id}")])?;
    Ok(())
}
