//! Where every attempt starts, and how that start is identified.
//!
//! - **Fixture**: a fresh copy of the bundled files, then a git repository
//!   with one baseline commit made with a fixed author, date and config, so
//!   the same fixture always yields the same commit id.
//! - **Git snapshot**: the tracked files of one commit of a local repository
//!   (`git archive`), minus credential-shaped paths, in a fresh workspace,
//!   committed the same way. Nothing outside that commit is copied.
//! - **Path**: the project itself, in place. Its starting state is identified
//!   (content digest, and git commit and cleanliness when it is a repository)
//!   but cannot be restored; the record says so.
//!
//! A workspace is never reused: a fixture or snapshot run refuses a directory
//! that already exists.

use std::path::Path;
use std::process::Command;

use serde::{Deserialize, Serialize};

use super::changes;
use super::suite;
use super::task::WorkdirRef;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Baseline {
    /// `fixture`, `git_snapshot` or `path`.
    pub kind: String,
    /// Stable identity of the starting state, e.g. `fixture:ledger@<sha256>`.
    pub identity: String,
    /// SHA-256 over every file's path and content at the start.
    pub content_digest: String,
    pub file_count: usize,
    /// The baseline commit in the workspace (fixture, snapshot) or the
    /// project's HEAD (path).
    #[serde(default)]
    pub git_commit: Option<String>,
    /// Whether a path project had uncommitted changes at the start.
    #[serde(default)]
    pub git_dirty: Option<bool>,
    /// The commit a git snapshot was taken from.
    #[serde(default)]
    pub source_commit: Option<String>,
    /// True when the attempt started in a workspace created for it.
    pub fresh_workspace: bool,
    /// Paths left out of a snapshot because they may hold credentials.
    #[serde(default)]
    pub excluded_paths: Vec<String>,
    /// What this baseline cannot guarantee.
    #[serde(default)]
    pub limitations: Vec<String>,
}

/// Path components and names never copied into a snapshot: they may hold
/// credentials or private keys.
const SENSITIVE_NAMES: &[&str] = &[
    ".ssh",
    ".aws",
    ".gnupg",
    ".netrc",
    ".npmrc",
    ".pypirc",
    ".docker",
    "id_rsa",
    "id_dsa",
    "id_ecdsa",
    "id_ed25519",
    "credentials",
    "credentials.json",
    "secrets.toml",
    "secrets.yaml",
    "secrets.yml",
    "secrets.json",
    "terraform.tfstate",
];
const SENSITIVE_SUFFIXES: &[&str] = &[".pem", ".key", ".p12", ".pfx", ".keystore", ".jks"];

pub fn is_sensitive_path(rel: &str) -> bool {
    rel.split('/').any(|part| {
        let lower = part.to_ascii_lowercase();
        lower == ".env"
            || lower.starts_with(".env.")
            || SENSITIVE_NAMES.contains(&lower.as_str())
            || SENSITIVE_SUFFIXES.iter().any(|s| lower.ends_with(s))
    })
}

/// A git command with no user or system configuration, no hooks, a fixed
/// identity and fixed dates, so its result depends only on the files.
fn git(dir: &Path, path_env: &str) -> Command {
    let mut c =
        Command::new(super::adapters::search_path("git", path_env).unwrap_or_else(|| "git".into()));
    c.current_dir(dir)
        .env("PATH", path_env)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env(
            "GIT_CONFIG_GLOBAL",
            if cfg!(windows) { "NUL" } else { "/dev/null" },
        )
        .env("GIT_AUTHOR_NAME", "Atomic Chat Agent Bench")
        .env("GIT_AUTHOR_EMAIL", "agent-bench@atomic-chat.invalid")
        .env("GIT_COMMITTER_NAME", "Atomic Chat Agent Bench")
        .env("GIT_COMMITTER_EMAIL", "agent-bench@atomic-chat.invalid")
        .env("GIT_AUTHOR_DATE", "2026-01-01T00:00:00Z")
        .env("GIT_COMMITTER_DATE", "2026-01-01T00:00:00Z")
        .args([
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.autocrlf=false",
            "-c",
            "core.hooksPath=/dev/null",
        ]);
    crate::core::process_env::sanitize_std_command(&mut c);
    c
}

fn run_git(dir: &Path, path_env: &str, args: &[&str]) -> Result<String, String> {
    let out = git(dir, path_env)
        .args(args)
        .output()
        .map_err(|e| format!("git could not run: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        Err(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

/// Makes `dir` a repository with one commit of everything in it. Returns the
/// commit id.
fn commit_baseline(dir: &Path, path_env: &str, message: &str) -> Result<String, String> {
    run_git(
        dir,
        path_env,
        &["init", "--quiet", "--template=", "--initial-branch=main"],
    )
    .or_else(|_| run_git(dir, path_env, &["init", "--quiet", "--template="]))?;
    run_git(dir, path_env, &["add", "--all"])?;
    run_git(
        dir,
        path_env,
        &[
            "commit",
            "--quiet",
            "--no-verify",
            "--allow-empty",
            "-m",
            message,
        ],
    )?;
    run_git(dir, path_env, &["rev-parse", "HEAD"])
}

/// Prepares the working directory for a run and identifies its start.
pub fn prepare(workdir: &WorkdirRef, dir: &Path, path_env: &str) -> Result<Baseline, String> {
    match workdir {
        WorkdirRef::Fixture { fixture } => {
            refuse_existing(dir)?;
            suite::materialize(fixture, dir)?;
            let snap = changes::snapshot(dir);
            let mut b = Baseline {
                kind: "fixture".into(),
                identity: format!("fixture:{fixture}@{}", snap.digest()),
                content_digest: snap.digest(),
                file_count: snap.file_count(),
                git_commit: None,
                git_dirty: None,
                source_commit: None,
                fresh_workspace: true,
                excluded_paths: vec![],
                limitations: vec![],
            };
            match commit_baseline(dir, path_env, &format!("Agent Bench baseline: fixture {fixture}")) {
                Ok(c) => {
                    b.git_commit = Some(c);
                    b.git_dirty = Some(false);
                }
                Err(e) => b.limitations.push(format!(
                    "The workspace is not a git repository ({e}); agents' git capabilities will fail."
                )),
            }
            Ok(b)
        }
        WorkdirRef::GitSnapshot { repo, commit } => {
            refuse_existing(dir)?;
            let repo_dir = Path::new(repo);
            let full = run_git(
                repo_dir,
                path_env,
                &["rev-parse", "--verify", &format!("{commit}^{{commit}}")],
            )
            .map_err(|e| format!("{repo} has no commit {commit}: {e}"))?;
            let archive = git(repo_dir, path_env)
                .args(["archive", "--format=tar", &full])
                .output()
                .map_err(|e| format!("git archive could not run: {e}"))?;
            if !archive.status.success() {
                return Err(format!(
                    "git archive failed: {}",
                    String::from_utf8_lossy(&archive.stderr).trim()
                ));
            }
            std::fs::create_dir_all(dir)
                .map_err(|e| format!("could not create {}: {e}", dir.display()))?;
            let excluded = extract_tar(&archive.stdout, dir)?;
            let snap = changes::snapshot(dir);
            let mut b = Baseline {
                kind: "git_snapshot".into(),
                identity: format!("git:{full}"),
                content_digest: snap.digest(),
                file_count: snap.file_count(),
                git_commit: None,
                git_dirty: Some(false),
                source_commit: Some(full.clone()),
                fresh_workspace: true,
                limitations: if excluded.is_empty() {
                    vec![]
                } else {
                    vec![format!(
                        "{} credential-shaped paths were left out of the snapshot.",
                        excluded.len()
                    )]
                },
                excluded_paths: excluded,
            };
            match commit_baseline(dir, path_env, &format!("Agent Bench baseline: {full}")) {
                Ok(c) => b.git_commit = Some(c),
                Err(e) => b
                    .limitations
                    .push(format!("The workspace is not a git repository ({e}).")),
            }
            Ok(b)
        }
        WorkdirRef::Path { path } => {
            let dir = Path::new(path);
            if !dir.is_dir() {
                return Err(format!("project directory {path} does not exist"));
            }
            let snap = changes::snapshot(dir);
            let head = run_git(dir, path_env, &["rev-parse", "HEAD"]).ok();
            let dirty = head.as_ref().and_then(|_| {
                run_git(
                    dir,
                    path_env,
                    &["status", "--porcelain", "--untracked-files=normal"],
                )
                .ok()
                .map(|s| !s.is_empty())
            });
            let mut limitations = vec![
                "The agent works in the project itself; its starting state is identified but cannot be restored for a re-run.".to_string(),
            ];
            match (&head, dirty) {
                (None, _) => limitations.push(
                    "The project is not a git repository; only its content digest identifies the start.".into(),
                ),
                (Some(_), Some(true)) => limitations.push(
                    "The project had uncommitted changes at the start (dirty working tree).".into(),
                ),
                _ => {}
            }
            if snap.incomplete() {
                limitations.push(format!(
                    "The project has more than {} files; the content digest covers the first ones only.",
                    changes::MAX_FILES
                ));
            }
            Ok(Baseline {
                kind: "path".into(),
                identity: match (&head, dirty) {
                    (Some(c), Some(false)) => format!("git:{c}"),
                    _ => format!("path:{}", snap.digest()),
                },
                content_digest: snap.digest(),
                file_count: snap.file_count(),
                git_commit: head,
                git_dirty: dirty,
                source_commit: None,
                fresh_workspace: false,
                excluded_paths: vec![],
                limitations,
            })
        }
    }
}

fn refuse_existing(dir: &Path) -> Result<(), String> {
    if dir.exists() {
        Err(format!(
            "workspace {} already exists; refusing to reuse it",
            dir.display()
        ))
    } else {
        Ok(())
    }
}

/// Unpacks a `git archive` tar into `dir`, skipping credential-shaped paths
/// and anything that is not a plain file or directory inside `dir`. Returns
/// the skipped sensitive paths.
fn extract_tar(bytes: &[u8], dir: &Path) -> Result<Vec<String>, String> {
    let mut excluded = Vec::new();
    let mut archive = tar::Archive::new(bytes);
    for entry in archive.entries().map_err(|e| e.to_string())? {
        let mut entry = entry.map_err(|e| e.to_string())?;
        let rel = entry
            .path()
            .map_err(|e| e.to_string())?
            .to_string_lossy()
            .replace('\\', "/");
        let kind = entry.header().entry_type();
        if !(kind.is_file() || kind.is_dir())
            || rel.starts_with('/')
            || rel.split('/').any(|p| p == "..")
        {
            continue;
        }
        if is_sensitive_path(&rel) {
            excluded.push(rel);
            continue;
        }
        entry
            .unpack_in(dir)
            .map_err(|e| format!("could not unpack {rel}: {e}"))?;
    }
    Ok(excluded)
}

/// Whether two baselines are the same starting state.
pub fn same_start(a: &Baseline, b: &Baseline) -> bool {
    a.identity == b.identity
}
