//! Footprints: what a span of the graph read and wrote, at symbol
//! granularity. Two concurrent spans compose iff their footprints do not
//! conflict.

use crate::git::{Objects, Oid, Repo};
use crate::resource::Resource;
use crate::{symbols, Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::Path;

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
            || (interface_changed && matches!(written, Resource::Symbol(_, name) if self.names(name)))
    }

    /// Whether this span mentions `symbol`: by its name, or, for a method
    /// (`Type::method`), by its type.
    fn names(&self, symbol: &str) -> bool {
        self.refs.contains(symbol) || symbol.split_once("::").is_some_and(|(owner, _)| self.refs.contains(owner))
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

/// What writing `new` over `old` (absent: a new file) at `path` would change,
/// by the same index footprints use: symbols, methods, imports and
/// module-level code; the whole file when it is new or cannot be parsed.
pub fn edit_writes(path: &str, old: Option<&[u8]>, new: &[u8]) -> Vec<Resource> {
    let whole = || vec![Resource::File(path.to_string())];
    let Some(old) = old else { return whole() };
    if old == new {
        return vec![];
    }
    let index = |body: &[u8]| if body.len() > MAX_PARSE_BYTES { None } else { symbols::index(path, body) };
    let (Some(before), Some(after)) = (index(old), index(new)) else { return whole() };
    let mut writes = BTreeSet::new();
    for name in before.symbols.keys().chain(after.symbols.keys()) {
        if before.symbols.get(name).map(|u| &u.hash) != after.symbols.get(name).map(|u| &u.hash) {
            writes.insert(Resource::Symbol(path.to_string(), name.clone()));
        }
    }
    if before.top.hash != after.top.hash {
        writes.insert(Resource::Top(path.to_string()));
    }
    writes.into_iter().collect()
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

/// The cached part of a footprint: what the diff of two trees says, which
/// every caller shares whatever reads it declares. Versioned by what is extracted.
fn diff_cache(repo: &Repo, base: &Oid, tip: &Oid) -> std::path::PathBuf {
    repo.home().join("cache/footprint-v3").join(format!("{base}-{tip}.json"))
}

fn read_cached<T: serde::de::DeserializeOwned>(path: &Path) -> Option<T> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

fn write_cached<T: Serialize>(path: &Path, value: &T) {
    if let (Some(dir), Ok(bytes)) = (path.parent(), serde_json::to_vec(value)) {
        let _ = std::fs::create_dir_all(dir);
        let _ = std::fs::write(path, bytes);
    }
}

fn compute(repo: &Repo, base: &Oid, tip: &Oid, declared: Option<&[Resource]>) -> Result<Footprint> {
    let cache = diff_cache(repo, base, tip);
    let mut fp = match read_cached::<Footprint>(&cache) {
        Some(fp) => fp,
        None => {
            let fp = diff(repo, base, tip)?;
            write_cached(&cache, &fp);
            fp
        }
    };
    fp.reads = match declared {
        Some(reads) => reads.iter().cloned().collect(),
        None => {
            // The commits of base..tip never change either.
            let cache = cache.with_extension("reads.json");
            match read_cached(&cache) {
                Some(reads) => reads,
                None => {
                    let log = repo.git(&["log", "--format=%B", &format!("{base}..{tip}")])?;
                    let reads: BTreeSet<Resource> =
                        log.lines().filter_map(|l| l.strip_prefix("Zit-Read: ")).map(Resource::parse).collect();
                    write_cached(&cache, &reads);
                    reads
                }
            }
        }
    };
    Ok(fp)
}

/// Writes, refs and signatures of `base..tip`, with no reads: what the two trees say.
fn diff(repo: &Repo, base: &Oid, tip: &Oid) -> Result<Footprint> {
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
    Ok(fp)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn sh(cwd: &Path, script: &str) {
        assert!(Command::new("sh").current_dir(cwd).args(["-c", script]).status().unwrap().success());
    }

    /// `of_change` (reads known) and `between` (reads from the trailers)
    /// share one cached diff; the reads are each caller's own.
    #[test]
    fn the_diff_of_a_span_is_shared_but_its_reads_are_not() {
        let dir = tempfile::tempdir().unwrap();
        sh(dir.path(), "git init -q -b main . && printf 'fn a() {}\\n' > a.rs && git add a.rs && git -c user.name=t -c user.email=t@t commit -qm one && printf 'fn a() { 1 }\\n' > a.rs && git -c user.name=t -c user.email=t@t commit -qam 'two\n\nZit-Read: b.rs'");
        let repo = Repo::open(dir.path(), &dir.path().join("home")).unwrap();
        let (base, tip) = (repo.resolve("HEAD~1").unwrap(), repo.resolve("HEAD").unwrap());
        let declared = of_change(&repo, &base, &tip, &[Resource::File("c.rs".into())]).unwrap();
        assert!(diff_cache(&repo, &base, &tip).is_file());
        let trailers = between(&repo, &base, &tip).unwrap();
        assert_eq!(declared.writes, trailers.writes);
        assert_eq!(declared.reads, [Resource::File("c.rs".into())].into_iter().collect());
        assert_eq!(trailers.reads, [Resource::File("b.rs".into())].into_iter().collect());
        assert_eq!(between(&repo, &base, &tip).unwrap(), trailers, "the same from the cache");
    }
}
