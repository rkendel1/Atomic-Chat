//! What a run changed in its working directory: a content snapshot before the
//! agent starts, compared with one after it stops. It needs no version
//! control, so it works the same on a fixture copy and on any project.

use std::collections::BTreeMap;
use std::path::Path;

use sha2::{Digest, Sha256};

use super::run::ChangeSummary;

/// Files beyond this many are not compared; the summary says so.
pub const MAX_FILES: usize = 20_000;
/// Larger files are compared by size and modification time instead of content.
const MAX_HASHED_BYTES: u64 = 4 * 1024 * 1024;
/// Build output, dependency trees and agents' own bookkeeping: never part of
/// the work, and counting them would skew a comparison between agents.
const SKIPPED_DIRS: &[&str] = &[
    ".git",
    "target",
    "node_modules",
    ".venv",
    "__pycache__",
    ".stack-agent",
    ".claude",
];

#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    files: BTreeMap<String, String>,
    incomplete: bool,
}

pub fn snapshot(root: &Path) -> Snapshot {
    let mut snap = Snapshot::default();
    let walker = ignore::WalkBuilder::new(root)
        .hidden(false)
        .git_ignore(true)
        .git_global(false)
        .git_exclude(false)
        .require_git(false)
        .filter_entry(|e| {
            !(e.file_type().is_some_and(|t| t.is_dir())
                && SKIPPED_DIRS.contains(&e.file_name().to_string_lossy().as_ref()))
        })
        .build();
    for entry in walker.flatten() {
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        if snap.files.len() >= MAX_FILES {
            snap.incomplete = true;
            break;
        }
        let path = entry.path();
        let Ok(rel) = path.strip_prefix(root) else {
            continue;
        };
        let rel = rel.to_string_lossy().replace('\\', "/");
        if let Some(fp) = fingerprint(path) {
            snap.files.insert(rel, fp);
        }
    }
    snap
}

fn fingerprint(path: &Path) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    if meta.len() > MAX_HASHED_BYTES {
        let modified = meta
            .modified()
            .ok()
            .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        return Some(format!("size:{}:mtime:{modified}", meta.len()));
    }
    let bytes = std::fs::read(path).ok()?;
    Some(hex::encode(Sha256::digest(&bytes)))
}

pub fn compare(before: &Snapshot, after: &Snapshot) -> ChangeSummary {
    let mut summary = ChangeSummary {
        incomplete: before.incomplete || after.incomplete,
        ..Default::default()
    };
    for (path, fp) in &after.files {
        match before.files.get(path) {
            None => summary.added.push(path.clone()),
            Some(old) if old != fp => summary.modified.push(path.clone()),
            Some(_) => {}
        }
    }
    for path in before.files.keys() {
        if !after.files.contains_key(path) {
            summary.deleted.push(path.clone());
        }
    }
    summary
}

impl Snapshot {
    /// SHA-256 over every compared file's path and fingerprint, in path order:
    /// equal digests mean the same files with the same contents.
    pub fn digest(&self) -> String {
        let mut h = Sha256::new();
        for (path, fp) in &self.files {
            h.update(path.as_bytes());
            h.update([0]);
            h.update(fp.as_bytes());
            h.update(b"\n");
        }
        hex::encode(h.finalize())
    }

    pub fn file_count(&self) -> usize {
        self.files.len()
    }

    pub fn incomplete(&self) -> bool {
        self.incomplete
    }
}
