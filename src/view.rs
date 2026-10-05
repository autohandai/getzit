//! Plain-text rendering shared by the CLI and the UI.

use crate::accept::{Invalid, Staleness, Status};
use crate::api::{Detail, Overview, WorkspaceRow};
use crate::change::Change;
use crate::footprint::ConflictKind;
use std::fmt::Write;

pub fn age(then: u64) -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    match now.saturating_sub(then) {
        s @ 0..=59 => format!("{s}s"),
        s @ 60..=3599 => format!("{}m", s / 60),
        s @ 3600..=86399 => format!("{}h", s / 3600),
        s => format!("{}d", s / 86400),
    }
}

pub fn subject(change: &Change) -> &str {
    change.intent.lines().next().unwrap_or_default()
}

pub fn status_label(status: &Status) -> String {
    match status {
        Status::Speculative => "speculative".into(),
        Status::Verified => "verified".into(),
        Status::Accepted => "accepted".into(),
        Status::Current => "current".into(),
        Status::Invalid(Invalid::Stale(_)) => "invalid: stale".into(),
        Status::Invalid(Invalid::Conflict(_)) => "invalid: conflict".into(),
        Status::Invalid(Invalid::Failed(_)) => "invalid: failed".into(),
        Status::Invalid(Invalid::Error(_)) => "invalid: error".into(),
    }
}

pub fn kind_label(kind: ConflictKind) -> &'static str {
    match kind {
        ConflictKind::WriteWrite => "written here and",
        ConflictKind::ReadWrite => "read here, written",
        ConflictKind::WriteRead => "written here, read",
    }
}

pub fn staleness_line(s: &Staleness) -> String {
    let by = s.by.as_ref().map_or("current", |id| id.short());
    format!("{} {} by {by}", s.resource, kind_label(s.kind))
}

/// Why a change is invalid, one line per reason.
pub fn reasons(status: &Status) -> Vec<String> {
    match status {
        Status::Invalid(Invalid::Stale(why)) => why.iter().map(staleness_line).collect(),
        Status::Invalid(Invalid::Conflict(paths)) => paths.iter().map(|p| format!("{p} does not merge")).collect(),
        Status::Invalid(Invalid::Failed(checks)) => checks.iter().map(|c| format!("check {c} failed")).collect(),
        Status::Invalid(Invalid::Error(why)) => vec![why.clone()],
        _ => vec![],
    }
}

pub fn held_line(held: &crate::claim::Held) -> String {
    match &held.by {
        crate::claim::Holder::Workspace { id, agent } => {
            format!("{} is claimed by {agent} (workspace {id})", held.resource)
        }
        crate::claim::Holder::Change { id, agent } => {
            format!("{} is already written by {agent} (change {}, not accepted yet)", held.resource, id.short())
        }
    }
}

pub fn workspace_state(row: &WorkspaceRow) -> String {
    let state = if row.dirty { "dirty" } else { "clean" };
    match row.alive {
        Some(true) => format!("{state}, running"),
        Some(false) => format!("{state}, owner gone"),
        None => state.to_string(),
    }
}

/// Reasons shown per change in the overview; `show` lists them all.
const MAX_REASONS: usize = 3;

pub fn overview_text(o: &Overview) -> String {
    let mut out = String::new();
    let c = &o.current;
    let _ = writeln!(out, "current  {}  {}  {}  {}", c.id.short(), c.agent, age(c.time), subject(c));

    let _ = writeln!(out, "\nCHANGES ({})", o.changes.len());
    for row in &o.changes {
        let c = &row.change;
        let _ = writeln!(
            out,
            "  {}  {:<17}  {:<10}  {:>3}  {}",
            c.id.short(),
            status_label(&row.status),
            c.agent,
            age(c.time),
            subject(c)
        );
        let why = reasons(&row.status);
        for reason in why.iter().take(MAX_REASONS) {
            let _ = writeln!(out, "              {reason}");
        }
        if why.len() > MAX_REASONS {
            let _ = writeln!(out, "              … and {} more (zit show {})", why.len() - MAX_REASONS, c.id.short());
        }
    }

    let _ = writeln!(out, "\nWORKSPACES ({})", o.workspaces.len());
    let list =
        |items: &[crate::resource::Resource]| items.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ");
    for row in &o.workspaces {
        let w = &row.workspace;
        let _ = writeln!(
            out,
            "  {}  on {}  {:<10}  {:>3}  {:<18}  {}",
            w.id,
            w.base.short(),
            w.agent,
            age(w.created),
            workspace_state(row),
            w.path().display()
        );
        if !row.claims.is_empty() {
            let _ = writeln!(out, "              claims {}", list(&row.claims));
        }
        if !row.writes.is_empty() {
            let _ = writeln!(out, "              writing {}", list(&row.writes));
        }
        for overlap in &row.overlaps {
            let _ = writeln!(
                out,
                "              overlap: {} is also held by {} ({})",
                overlap.resource,
                overlap.with,
                overlap.holder.get(..10).unwrap_or(&overlap.holder)
            );
        }
    }
    out
}

pub fn detail_text(d: &Detail) -> String {
    let mut out = String::new();
    let c = &d.change;
    let _ = writeln!(out, "change   {}", c.id);
    let _ = writeln!(out, "status   {}", status_label(&d.status));
    for reason in reasons(&d.status) {
        let _ = writeln!(out, "         {reason}");
    }
    let _ = writeln!(out, "intent   {}", c.intent.replace('\n', "\n         "));
    if let Some(summary) = &c.summary {
        let _ = writeln!(out, "reported {}", summary.replace('\n', "\n         "));
    }
    let _ = writeln!(
        out,
        "agent    {}{}",
        c.agent,
        c.session.as_ref().map(|s| format!(" (session {s})")).unwrap_or_default()
    );
    let _ = writeln!(out, "state    {}", c.state);
    for parent in &c.parents {
        let _ = writeln!(out, "parent   {parent}");
    }
    for write in &d.writes {
        let _ = writeln!(out, "wrote    {write}");
    }
    for read in &c.reads {
        let _ = writeln!(out, "read     {read}");
    }
    for e in &d.evidence {
        let verdict = match &e.evidence {
            None => "no evidence".to_string(),
            Some(ev) if ev.passed => format!("pass ({}ms on {})", ev.duration_ms, ev.state.short()),
            Some(ev) => format!("FAIL exit {} ({}ms on {})", ev.exit_code, ev.duration_ms, ev.state.short()),
        };
        let _ = writeln!(out, "check    {}: {verdict}", e.check);
    }
    out
}
