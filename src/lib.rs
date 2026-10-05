//! Causal Program State Graph.
//!
//! A persistent graph of program changes, stored in git:
//!
//! | Primitive | Module | Stored as |
//! |---|---|---|
//! | State | [`git`] | a git tree |
//! | Change | [`change`] | a git commit under `refs/zit/changes/` |
//! | Causality | [`footprint`] | commit parents, plus derived read/write sets |
//! | Materialisation | [`workspace`] | a disposable directory |
//! | Evidence | [`evidence`] | a blob under `refs/zit/evidence/` |
//! | Acceptance | [`accept`] | `refs/zit/current` |
//!
//! [`claim`] is the early, advisory layer in front of acceptance: what a
//! workspace intends to write, refused if other unaccepted work holds it.
//!
//! [`api`] holds the result shapes shared by the CLI, the MCP server
//! ([`mcp`]), the terminal UI ([`tui`]) and the web view ([`web`]);
//! [`run`] wraps an agent process.
//!
//! ```no_run
//! use zit::accept::{self, Outcome};
//! use zit::change::{self, Record};
//! use zit::workspace::{self, NewWorkspace};
//! use zit::Repo;
//!
//! let repo = Repo::discover(std::path::Path::new("."))?;
//! repo.init(None)?; // HEAD becomes the current state
//!
//! // A disposable view of the current state for an agent to edit.
//! let new = NewWorkspace { from: None, intent: "Raise the price", agent: "claude", session: None };
//! let ws = workspace::materialise(&repo, &new)?;
//! std::fs::write(ws.path().join("price.txt"), "42\n")?;
//!
//! // The change outlives the workspace.
//! let change = change::record(&repo, &ws.id, &Record::default())?.expect("the workspace was edited");
//! workspace::dispose(&repo, &ws.id)?;
//!
//! match accept::accept(&repo, &change.id)? {
//!     Outcome::Accepted { current, .. } => println!("current is {current}"),
//!     Outcome::Rejected(why) => println!("kept in the graph, not accepted: {why:?}"),
//!     Outcome::AlreadyAccepted => {}
//! }
//! # Ok::<(), zit::Error>(())
//! ```

pub mod accept;
pub mod api;
pub mod change;
pub mod claim;
pub mod clean;
pub mod evidence;
pub mod footprint;
pub mod git;
pub mod mcp;
pub mod resource;
pub mod run;
pub mod symbols;
pub mod tui;
pub mod view;
pub mod web;
pub mod workspace;

pub use git::{Oid, Repo};

use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("git {args}: {stderr}")]
    Git { args: String, stderr: String },
    #[error("not a git repository: {0}")]
    NotARepo(PathBuf),
    #[error("zit is not initialised in this repository; run `zit init`")]
    NotInitialised,
    #[error("unknown workspace: {0}")]
    UnknownWorkspace(String),
    #[error("unknown revision: {0}")]
    UnknownRevision(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("{0}")]
    Other(String),
}

impl Error {
    pub(crate) fn msg(text: impl Into<String>) -> Error {
        Error::Other(text.into())
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// Short content hash (first 16 hex chars of SHA-256).
pub fn hash(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes).iter().take(8).map(|b| format!("{b:02x}")).collect()
}
