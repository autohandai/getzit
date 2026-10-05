//! Footprints: what a span of the graph read and wrote, at symbol
//! granularity. Two concurrent spans compose iff their footprints do not
//! conflict.

use crate::git::{Objects, Oid, Repo};
use crate::resource::Resource;
use crate::{symbols, Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Footprint {
    /// Resources whose content differs between base and tip.
    pub writes: BTreeSet<Resource>,
    /// Identifiers mentioned by the written symbols: inferred reads, matched
    /// by name against symbols written concurrently.
    pub refs: BTreeSet<String>,
    /// Reads declared by the changes in the span.
    pub reads: BTreeSet<Resource>,
    /// Written symbols whose interface changed (everything but function
    /// bodies). Only these make code that merely names them stale.
    #[serde(default)]
    pub signatures: BTreeSet<Resource>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ConflictKind {
    /// Both sides wrote the resource.
    WriteWrite,
    /// This side read what the other side wrote.
    ReadWrite,
    /// This side wrote what the other side read.
    WriteRead,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Conflict {
    pub resource: Resource,
    pub kind: ConflictKind,
}

impl Footprint {
    /// Did this span depend on `written`? A declared read sees any change; a
    /// mention by name sees only a change to the symbol's interface.
    pub fn reads(&self, written: &Resource, interface_changed: bool) -> bool {
        self.reads.iter().any(|r| r.overlaps(written))
            || (interface_changed && matches!(written, Resource::Symbol(_, name) if self.refs.contains(name)))
    }

    /// Why this span cannot be composed with a concurrent `other`; empty when it can.
    pub fn conflicts(&self, other: &Footprint) -> Vec<Conflict> {
        let mut found = Vec::new();
        for theirs in &other.writes {
            if self.writes.iter().any(|mine| mine.overlaps(theirs)) {
                found.push(Conflict { resource: theirs.clone(), kind: ConflictKind::WriteWrite });
            } else if self.reads(theirs, other.signatures.contains(theirs)) {
                found.push(Conflict { resource: theirs.clone(), kind: ConflictKind::ReadWrite });
            }
        }
        for mine in &self.writes {
            if !other.writes.iter().any(|theirs| theirs.overlaps(mine))
                && other.reads(mine, self.signatures.contains(mine))
            {
                found.push(Conflict { resource: mine.clone(), kind: ConflictKind::WriteRead });
            }
        }
        found
    }
}

/// Files larger than this are treated as one indivisible resource.
const MAX_PARSE_BYTES: usize = 1 << 20;

const NULL_OID: &str = "0000000000000000000000000000000000000000";

fn index_blob(blobs: &mut Objects, path: &str, mode: &str, oid: &str) -> Result<Option<symbols::FileIndex>> {
    if oid == NULL_OID {
        return Ok(Some(symbols::FileIndex::default()));
    }
    if !mode.starts_with("100") {
        return Ok(None); // symlink or submodule
    }
    let body = blobs.read(oid)?.ok_or_else(|| Error::msg(format!("git object {oid} is missing")))?;
    Ok(if body.len() > MAX_PARSE_BYTES { None } else { symbols::index(path, &body) })
}

/// The footprint of `base..tip`. Pure in its arguments, so cached.
pub fn between(repo: &Repo, base: &Oid, tip: &Oid) -> Result<Footprint> {
    compute(repo, base, tip, None)
}

/// As `between`, for a single change whose declared reads are already known.
pub(crate) fn of_change(repo: &Repo, parent: &Oid, change: &Oid, reads: &[Resource]) -> Result<Footprint> {
    compute(repo, parent, change, Some(reads))
}

/// The footprint between two states (trees), for work that is not a change yet.
pub(crate) fn between_states(repo: &Repo, base: &Oid, state: &Oid, reads: &[Resource]) -> Result<Footprint> {
    compute(repo, base, state, Some(reads))
}

fn compute(repo: &Repo, base: &Oid, tip: &Oid, declared: Option<&[Resource]>) -> Result<Footprint> {
    // Versioned by what is extracted; keyed by the declared reads too.
    let declared_key = match declared {
        None => "trailers".to_string(),
        Some(reads) => crate::hash(reads.iter().map(|r| format!("{r}\n")).collect::<String>().as_bytes()),
    };
    let cache = repo.home().join("cache/footprint-v2").join(format!("{base}-{tip}-{declared_key}.json"));
    if let Ok(bytes) = std::fs::read(&cache) {
        if let Ok(fp) = serde_json::from_slice(&bytes) {
            return Ok(fp);
        }
    }

    let mut fp = Footprint::default();
    let diff = repo.git(&["diff-tree", "-r", "-z", "--no-renames", base.as_str(), tip.as_str()])?;
    let mut blobs = repo.objects()?;
    let mut fields = diff.split('\0');
    while let (Some(meta), Some(path)) = (fields.next(), fields.next()) {
        let meta: Vec<&str> = meta.trim_start_matches(':').split(' ').collect();
        let [old_mode, new_mode, old_oid, new_oid, _status] = meta[..] else {
            return Err(Error::msg(format!("unexpected diff-tree record: {meta:?}")));
        };
        let old = index_blob(&mut blobs, path, old_mode, old_oid)?;
        let new = index_blob(&mut blobs, path, new_mode, new_oid)?;
        let (Some(old), Some(new)) = (old, new) else {
            fp.writes.insert(Resource::File(path.to_string()));
            continue;
        };
        for name in old.symbols.keys().chain(new.symbols.keys()) {
            let (before, after) = (old.symbols.get(name), new.symbols.get(name));
            if before.map(|u| &u.hash) != after.map(|u| &u.hash) {
                let written = Resource::Symbol(path.to_string(), name.clone());
                if before.map(|u| &u.sig) != after.map(|u| &u.sig) {
                    fp.signatures.insert(written.clone());
                }
                fp.writes.insert(written);
                fp.refs.extend(after.into_iter().flat_map(|u| u.refs.iter().cloned()));
            }
        }
        if old.top.hash != new.top.hash {
            fp.writes.insert(Resource::Top(path.to_string()));
            fp.refs.extend(new.top.refs.iter().cloned());
        }
    }

    fp.reads = match declared {
        Some(reads) => reads.iter().cloned().collect(),
        None => {
            let log = repo.git(&["log", "--format=%B", &format!("{base}..{tip}")])?;
            log.lines().filter_map(|l| l.strip_prefix("Zit-Read: ")).map(Resource::parse).collect()
        }
    };

    if let Some(dir) = cache.parent() {
        let _ = std::fs::create_dir_all(dir);
        let _ = std::fs::write(&cache, serde_json::to_vec(&fp)?);
    }
    Ok(fp)
}
