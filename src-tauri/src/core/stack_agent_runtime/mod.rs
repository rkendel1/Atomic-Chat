//! The one place Atomic Chat gets its stack-agent executable from.
//!
//! The manager follows `main` of the official repository without anyone
//! running a build or install command:
//!
//! 1. **Resolve** `refs/heads/main` to a commit (`git ls-remote`).
//! 2. **Reuse** the cached runtime for that commit and platform, if any.
//! 3. **Prepare** a new one from that exact commit: shallow fetch of the
//!    commit, `cargo build --release --locked --bin stack-agent` (the
//!    command stack-agent's release workflow uses), in a staging
//!    directory. A branch name is never built; only the resolved commit is.
//! 4. **Validate** it: it launches, its offline self-test passes, and it
//!    offers the `work` interface Atomic Chat drives.
//! 5. **Activate** it by renaming the staging directory into the cache and
//!    then rewriting `state.json` atomically. The previous runtime stays.
//! 6. **Record** the exact revision on every run that uses it.
//! 7. **Recover**: a failed check, fetch, build or validation leaves the
//!    active runtime in place and is reported; a developer can roll back or
//!    pin a cached revision.
//!
//! Runtime directories are immutable once published, so a run that resolved
//! one keeps executing the same file even if another is activated meanwhile,
//! and a lease keeps it from being pruned until the run ends.
//!
//! Official release archives exist only for tags (v0.1.0 at the time of
//! writing), which lag `main`, and their checksum file comes from the same
//! release, so it shows integrity but not provenance. The manager therefore
//! builds `main` from source; see the ADR
//! `docs/decisions/2026-10-26-manage-the-stack-agent-runtime-from-pinned-main-commits.md`.

pub mod commands;
mod lock;
pub mod source;
mod validate;

#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub use source::{GitSource, PrepareError, Prepared, Source};
pub use validate::{validate, HealthCheck, REQUIRED_WORK_FLAGS};

pub const STATE_SCHEMA: &str = "atomic.stack_agent_runtime.state.v1";
pub const MANIFEST_SCHEMA: &str = "atomic.stack_agent_runtime.manifest.v1";
pub const REPOSITORY: &str = "https://github.com/rkendel1/stack-agent";
pub const TRACKED_REF: &str = "refs/heads/main";
pub const BIN_NAME: &str = if cfg!(windows) {
    "stack-agent.exe"
} else {
    "stack-agent"
};
/// Earlier revisions of `main` tried when its tip cannot be prepared.
pub const WALK_BACK: usize = 3;
/// Validated runtimes kept for rollback besides the active and pinned ones.
pub const KEEP_PREVIOUS: usize = 3;
/// A successful check older than this no longer shows the runtime as current.
pub const CHECK_FRESH_FOR: chrono::Duration = chrono::Duration::hours(24);
const MAX_FAILURES: usize = 20;

/// The platform a runtime was built for, e.g. `x86_64-linux`.
pub fn platform() -> String {
    format!("{}-{}", std::env::consts::ARCH, std::env::consts::OS)
}

pub fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

pub fn is_full_commit(s: &str) -> bool {
    s.len() == 40
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

pub fn sha256_file(path: &Path) -> Result<String, String> {
    let mut f =
        std::fs::File::open(path).map_err(|e| format!("could not read {}: {e}", path.display()))?;
    let mut h = Sha256::new();
    std::io::copy(&mut f, &mut h).map_err(|e| format!("could not read {}: {e}", path.display()))?;
    Ok(hex::encode(h.finalize()))
}

/// Which runtime runs use.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum Selection {
    /// The latest validated revision of `main` (the default).
    #[default]
    FollowMain,
    /// One cached revision, never replaced by a refresh.
    Pinned { commit: String },
}

/// What `runtime.json` in each runtime directory records.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    pub schema: String,
    pub commit: String,
    pub platform: String,
    pub repository: String,
    /// How the executable was produced: `source_build`.
    pub source: String,
    /// `stack-agent --version`, e.g. `0.1.0`.
    pub version: String,
    /// Path of the executable relative to the runtime directory.
    pub binary: String,
    pub binary_sha256: String,
    /// `rustc -V` of the toolchain that built it, when known.
    #[serde(default)]
    pub toolchain: Option<String>,
    pub prepared_at: String,
    pub validated_at: String,
    pub checks: Vec<HealthCheck>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RemoteCheck {
    pub commit: String,
    pub checked_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CheckAttempt {
    pub at: String,
    pub ok: bool,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PrepareFailure {
    pub commit: String,
    pub at: String,
    /// `fetch`, `toolchain`, `build`, `validate` or `publish`.
    pub stage: String,
    pub error: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct State {
    pub schema: String,
    #[serde(default)]
    pub selection: Selection,
    /// Commit of the runtime following `main` uses.
    #[serde(default)]
    pub active: Option<String>,
    /// Earlier active commits, newest first, for rollback.
    #[serde(default)]
    pub previous: Vec<String>,
    /// The last commit `main` was seen at.
    #[serde(default)]
    pub latest_remote: Option<RemoteCheck>,
    /// The last attempt to look at `main`, successful or not.
    #[serde(default)]
    pub last_check: Option<CheckAttempt>,
    /// When a refresh last ended with the latest `main` active.
    #[serde(default)]
    pub last_success_refresh_at: Option<String>,
    #[serde(default)]
    pub last_activated_at: Option<String>,
    /// Revisions that could not be prepared, newest first. An automatic
    /// refresh does not retry them; a manual one does.
    #[serde(default)]
    pub failures: Vec<PrepareFailure>,
    /// Revisions a developer rolled back from; an automatic refresh does not
    /// re-activate them.
    #[serde(default)]
    pub rolled_back: Vec<String>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            schema: STATE_SCHEMA.to_string(),
            selection: Selection::FollowMain,
            active: None,
            previous: vec![],
            latest_remote: None,
            last_check: None,
            last_success_refresh_at: None,
            last_activated_at: None,
            failures: vec![],
            rolled_back: vec![],
        }
    }
}

impl State {
    fn failure_for(&self, commit: &str) -> Option<&PrepareFailure> {
        self.failures.iter().find(|f| f.commit == commit)
    }

    fn record_failure(&mut self, f: PrepareFailure) {
        self.failures.retain(|x| x.commit != f.commit);
        self.failures.insert(0, f);
        self.failures.truncate(MAX_FAILURES);
    }

    /// Makes `commit` the runtime following `main` uses, keeping the one it
    /// replaces for rollback.
    fn activate(&mut self, commit: &str) {
        if self.active.as_deref() == Some(commit) {
            return;
        }
        if let Some(old) = self.active.take() {
            self.previous.retain(|c| c != &old);
            self.previous.insert(0, old);
        }
        self.previous.retain(|c| c != commit);
        self.active = Some(commit.to_string());
        self.last_activated_at = Some(now());
    }
}

/// How a run's executable was chosen and what it is. Recorded on the run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RuntimeProvenance {
    /// `managed` (built by Atomic Chat from `commit`), `custom_path` (set on
    /// the Launch page) or `external` (found on PATH, not managed).
    pub source: String,
    /// `follow_main`, `pinned`, `custom_path` or `external`.
    pub selection: String,
    pub path: String,
    /// The stack-agent commit, known only for managed runtimes.
    #[serde(default)]
    pub commit: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    pub platform: String,
    #[serde(default)]
    pub binary_sha256: Option<String>,
    #[serde(default)]
    pub repository: Option<String>,
    #[serde(default)]
    pub toolchain: Option<String>,
    #[serde(default)]
    pub prepared_at: Option<String>,
    /// `main` as last seen, and when.
    #[serde(default)]
    pub latest_known_main: Option<String>,
    #[serde(default)]
    pub latest_checked_at: Option<String>,
    /// Whether `commit` is that `main`; unknown when either is unknown.
    #[serde(default)]
    pub matches_latest: Option<bool>,
}

#[derive(Debug, Clone)]
pub struct Resolved {
    pub path: PathBuf,
    pub provenance: RuntimeProvenance,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StatusKind {
    /// No runtime is cached and none was found on PATH.
    NotInstalled,
    /// The active runtime is the latest `main`, checked recently.
    Current,
    UpdateAvailable,
    Refreshing,
    UpdateFailed,
    /// `main` could not be checked (recently), so whether the runtime is
    /// current is unknown.
    Stale,
    Pinned,
    /// Only an unmanaged stack-agent on PATH is available.
    External,
}

#[derive(Debug, Clone, Serialize)]
pub struct CachedRuntime {
    pub commit: String,
    pub path: String,
    pub valid: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub problem: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub manifest: Option<Manifest>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Progress {
    pub commit: Option<String>,
    pub stage: String,
    pub started_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_line: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RuntimeStatus {
    pub status: StatusKind,
    /// One sentence saying what the status means and on what evidence.
    pub detail: String,
    /// False until someone first prepares a runtime; until then nothing
    /// refreshes in the background.
    pub enabled: bool,
    pub platform: String,
    pub repository: String,
    pub tracked_ref: String,
    pub selection: Selection,
    /// What a run would use now, if anything.
    pub resolved: Option<RuntimeProvenance>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolve_error: Option<String>,
    pub active: Option<String>,
    pub previous: Vec<String>,
    pub latest_remote: Option<RemoteCheck>,
    pub last_check: Option<CheckAttempt>,
    pub last_success_refresh_at: Option<String>,
    pub refreshing: Option<Progress>,
    /// The failure for the latest `main`, if it could not be prepared.
    pub failure: Option<PrepareFailure>,
    pub rolled_back: Vec<String>,
    pub cached: Vec<CachedRuntime>,
}

/// Why a refresh did what it did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "outcome")]
pub enum RefreshOutcome {
    /// `main` could not be resolved; nothing changed.
    CheckFailed { error: String },
    /// The latest `main` was already active.
    AlreadyCurrent { commit: String },
    /// A cached runtime for the latest `main` was activated without a build.
    ActivatedCached { commit: String },
    /// A new runtime was prepared, validated and activated.
    Activated { commit: String },
    /// The latest `main` could not be prepared; an earlier revision of
    /// `main` (newer than the active runtime) was activated instead.
    ActivatedPrevious {
        commit: String,
        latest: String,
        skipped: Vec<String>,
    },
    /// Preparing the latest `main` failed; the previous runtime stays.
    PrepareFailed {
        commit: String,
        stage: String,
        error: String,
    },
    /// A revision is pinned; the latest `main` was recorded, not activated.
    Pinned { pinned: String, latest: String },
    /// An automatic refresh skipped a revision that failed before or that a
    /// developer rolled back from.
    Skipped { commit: String, reason: String },
    /// Another refresh is running.
    Busy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    Automatic,
    Manual,
}

pub type LogFn<'a> = &'a mut dyn FnMut(&str);

pub struct Manager {
    root: PathBuf,
    source: Arc<dyn Source>,
    /// Serializes refreshes and preparations in this process; the lock file
    /// serializes them across processes.
    refresh: Mutex<()>,
    /// Serializes state.json read-modify-write cycles.
    state_io: Mutex<()>,
    progress: Mutex<Option<Progress>>,
    leases: Mutex<HashMap<String, usize>>,
}

/// Keeps a runtime from being pruned while a run uses it.
pub struct Lease {
    manager: Arc<Manager>,
    commit: String,
}

impl Drop for Lease {
    fn drop(&mut self) {
        if let Ok(mut l) = self.manager.leases.lock() {
            if let Some(n) = l.get_mut(&self.commit) {
                *n -= 1;
                if *n == 0 {
                    l.remove(&self.commit);
                }
            }
        }
    }
}

impl Manager {
    pub fn new(root: PathBuf, source: Arc<dyn Source>) -> Self {
        Self {
            root,
            source,
            refresh: Mutex::new(()),
            state_io: Mutex::new(()),
            progress: Mutex::new(None),
            leases: Mutex::new(HashMap::new()),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn state_path(&self) -> PathBuf {
        self.root.join("state.json")
    }

    fn platform_dir(&self) -> PathBuf {
        self.root.join("runtimes").join(platform())
    }

    pub fn runtime_dir(&self, commit: &str) -> PathBuf {
        self.platform_dir().join(commit)
    }

    fn staging_dir(&self) -> PathBuf {
        self.root.join("staging")
    }

    /// Whether anyone has asked for a managed runtime yet.
    pub fn enabled(&self) -> bool {
        self.state_path().is_file()
    }

    pub fn load_state(&self) -> State {
        let path = self.state_path();
        let Ok(text) = std::fs::read_to_string(&path) else {
            return State::default();
        };
        match serde_json::from_str::<State>(&text) {
            Ok(s) if s.schema == STATE_SCHEMA => s,
            other => {
                // Keep the unreadable file for inspection; start over.
                let aside =
                    path.with_extension(format!("unreadable-{}", uuid::Uuid::new_v4().simple()));
                let _ = std::fs::rename(&path, &aside);
                log::error!(
                    "stack-agent runtime: state was unreadable ({}); moved to {}",
                    match other {
                        Ok(s) => format!("schema {}", s.schema),
                        Err(e) => e.to_string(),
                    },
                    aside.display()
                );
                State::default()
            }
        }
    }

    fn save_state(&self, state: &State) -> Result<(), String> {
        let text = serde_json::to_string_pretty(state).map_err(|e| e.to_string())?;
        crate::core::agent_bench::store::write_atomic(&self.state_path(), text.as_bytes())
    }

    /// Reads, changes and saves the state as one step.
    fn update_state<T>(
        &self,
        f: impl FnOnce(&mut State) -> Result<T, String>,
    ) -> Result<T, String> {
        let _io = self.state_io.lock().map_err(|e| e.to_string())?;
        let mut state = self.load_state();
        let out = f(&mut state)?;
        self.save_state(&state)?;
        Ok(out)
    }

    /// Turns the manager on (creates `state.json`) without changing anything.
    pub fn enable(&self) -> Result<(), String> {
        self.update_state(|_| Ok(()))
    }

    /// Checks a cached runtime: its manifest, platform and executable digest.
    pub fn verify_cached(&self, commit: &str) -> Result<(Manifest, PathBuf), String> {
        if !is_full_commit(commit) {
            return Err(format!("{commit} is not a full commit id"));
        }
        let dir = self.runtime_dir(commit);
        let text = std::fs::read_to_string(dir.join("runtime.json"))
            .map_err(|_| format!("revision {} is not prepared on this machine", short(commit)))?;
        let m: Manifest = serde_json::from_str(&text)
            .map_err(|e| format!("runtime {} has an unreadable manifest: {e}", short(commit)))?;
        if m.schema != MANIFEST_SCHEMA || m.commit != commit || m.platform != platform() {
            return Err(format!(
                "runtime {} does not match its directory (schema {}, commit {}, platform {})",
                short(commit),
                m.schema,
                short(&m.commit),
                m.platform
            ));
        }
        let bin = dir.join(&m.binary);
        let digest = sha256_file(&bin)?;
        if digest != m.binary_sha256 {
            return Err(format!(
                "runtime {}: the executable changed after validation (sha256 {} instead of {})",
                short(commit),
                &digest[..12],
                &m.binary_sha256[..12.min(m.binary_sha256.len())]
            ));
        }
        Ok((m, bin))
    }

    pub fn cached(&self) -> Vec<CachedRuntime> {
        let mut out: Vec<CachedRuntime> = std::fs::read_dir(self.platform_dir())
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| e.file_name().to_str().map(str::to_string))
            .filter(|n| is_full_commit(n))
            .map(|commit| {
                let path = self.runtime_dir(&commit).to_string_lossy().into_owned();
                match self.verify_cached(&commit) {
                    Ok((m, _)) => CachedRuntime {
                        commit,
                        path,
                        valid: true,
                        problem: None,
                        manifest: Some(m),
                    },
                    Err(e) => CachedRuntime {
                        commit,
                        path,
                        valid: false,
                        problem: Some(e),
                        manifest: None,
                    },
                }
            })
            .collect();
        out.sort_by(|a, b| {
            let at = |c: &CachedRuntime| c.manifest.as_ref().map(|m| m.prepared_at.clone());
            at(b).cmp(&at(a))
        });
        out
    }

    fn set_progress(&self, p: Option<Progress>) {
        if let Ok(mut g) = self.progress.lock() {
            *g = p;
        }
    }

    fn progress_line(&self, line: &str) {
        if let Ok(mut g) = self.progress.lock() {
            if let Some(p) = g.as_mut() {
                p.last_line = Some(line.chars().take(300).collect());
            }
        }
    }

    fn progress_stage(&self, commit: Option<&str>, stage: &str) {
        if let Ok(mut g) = self.progress.lock() {
            let started_at = g.as_ref().map(|p| p.started_at.clone()).unwrap_or_else(now);
            *g = Some(Progress {
                commit: commit.map(str::to_string),
                stage: stage.to_string(),
                started_at,
                last_line: None,
            });
        }
    }

    pub fn is_refreshing(&self) -> bool {
        self.progress.lock().map(|g| g.is_some()).unwrap_or(false)
    }

    /// Looks at `main` and, when it moved, prepares, validates and activates
    /// the new revision. Never blocks on another refresh: returns `Busy`.
    pub fn refresh(&self, trigger: Trigger, log: LogFn) -> RefreshOutcome {
        let Ok(_guard) = self.refresh.try_lock() else {
            return RefreshOutcome::Busy;
        };
        let Some(_file_lock) = lock::RefreshLock::acquire(&self.root) else {
            return RefreshOutcome::Busy;
        };
        self.progress_stage(None, "checking");
        let out = self.refresh_locked(trigger, log);
        self.set_progress(None);
        out
    }

    fn refresh_locked(&self, trigger: Trigger, log: LogFn) -> RefreshOutcome {
        self.clean_staging();
        log(&format!(
            "Checking {TRACKED_REF} of {}",
            self.source.repository()
        ));
        let latest = self.source.latest().and_then(|c| {
            if is_full_commit(&c) {
                Ok(c)
            } else {
                Err(format!(
                    "{TRACKED_REF} resolved to {c:?}, which is not a commit id"
                ))
            }
        });
        let latest = match latest {
            Ok(c) => c,
            Err(e) => {
                let _ = self.record_check(Err(e.clone()));
                log(&format!("Could not check for updates: {e}"));
                return RefreshOutcome::CheckFailed { error: e };
            }
        };
        if let Err(e) = self.record_check(Ok(latest.clone())) {
            return RefreshOutcome::CheckFailed { error: e };
        }
        log(&format!("{TRACKED_REF} is at {latest}"));
        let state = self.load_state();

        if let Selection::Pinned { commit } = &state.selection {
            log(&format!(
                "Revision {} is pinned; not switching.",
                short(commit)
            ));
            return RefreshOutcome::Pinned {
                pinned: commit.clone(),
                latest,
            };
        }
        if state.active.as_deref() == Some(latest.as_str()) && self.verify_cached(&latest).is_ok() {
            let _ = self.update_state(|s| {
                s.last_success_refresh_at = Some(now());
                Ok(())
            });
            log("Already up to date.");
            return RefreshOutcome::AlreadyCurrent { commit: latest };
        }
        if trigger == Trigger::Automatic {
            if state.rolled_back.contains(&latest) {
                return RefreshOutcome::Skipped {
                    commit: latest,
                    reason: "a developer rolled back from this revision".into(),
                };
            }
            if let Some(f) = state.failure_for(&latest) {
                return RefreshOutcome::Skipped {
                    commit: latest,
                    reason: format!("it failed to prepare before ({}: {})", f.stage, f.error),
                };
            }
        }
        if self.verify_cached(&latest).is_ok() {
            if let Err(e) = self.activate_following(&latest) {
                return RefreshOutcome::PrepareFailed {
                    commit: latest,
                    stage: "publish".into(),
                    error: e,
                };
            }
            log(&format!(
                "Activated the cached runtime for {}.",
                short(&latest)
            ));
            return RefreshOutcome::ActivatedCached { commit: latest };
        }
        match self.prepare_locked(&latest, log) {
            Ok(_) => match self.activate_following(&latest) {
                Ok(()) => {
                    log(&format!("Activated {}.", short(&latest)));
                    RefreshOutcome::Activated { commit: latest }
                }
                Err(e) => RefreshOutcome::PrepareFailed {
                    commit: latest,
                    stage: "publish".into(),
                    error: e,
                },
            },
            Err(e) => {
                let failed = RefreshOutcome::PrepareFailed {
                    commit: latest.clone(),
                    stage: e.stage.clone(),
                    error: e.message.clone(),
                };
                // Only a revision that cannot be built or does not pass the
                // checks says anything about `main`; a fetch or toolchain
                // problem would fail for its ancestors too.
                if e.stage == "build" || e.stage == "validate" {
                    if let Some(found) = self.walk_back(&latest, trigger, log) {
                        return found;
                    }
                }
                failed
            }
        }
    }

    /// When the tip of `main` cannot be built or validated, tries its
    /// first-parent ancestors (at most [`WALK_BACK`], never older than the
    /// active runtime) and activates the newest that passes: the latest
    /// compatible revision of `main`.
    fn walk_back(&self, latest: &str, trigger: Trigger, log: LogFn) -> Option<RefreshOutcome> {
        let state = self.load_state();
        let scratch = self
            .staging_dir()
            .join(format!("history.{}", uuid::Uuid::new_v4().simple()));
        let history = self.source.history(latest, WALK_BACK, &scratch);
        let _ = std::fs::remove_dir_all(&scratch);
        let history = match history {
            Ok(h) => h,
            Err(e) => {
                log(&format!("Could not read the history of main: {e}"));
                return None;
            }
        };
        let mut skipped = vec![latest.to_string()];
        for commit in history {
            if state.active.as_deref() == Some(commit.as_str()) {
                log(&format!(
                    "The active runtime {} is the newest that works.",
                    short(&commit)
                ));
                return None;
            }
            if trigger == Trigger::Automatic
                && (state.failure_for(&commit).is_some() || state.rolled_back.contains(&commit))
            {
                skipped.push(commit);
                continue;
            }
            log(&format!(
                "Trying the previous revision of main, {}.",
                short(&commit)
            ));
            let ready =
                self.verify_cached(&commit).is_ok() || self.prepare_locked(&commit, log).is_ok();
            if ready && self.activate_following(&commit).is_ok() {
                log(&format!(
                    "Activated {}, the newest revision of main that builds and passes the checks.",
                    short(&commit)
                ));
                return Some(RefreshOutcome::ActivatedPrevious {
                    commit,
                    latest: latest.to_string(),
                    skipped,
                });
            }
            skipped.push(commit);
        }
        None
    }

    fn record_check(&self, result: Result<String, String>) -> Result<(), String> {
        let at = now();
        self.update_state(|s| {
            match result {
                Ok(commit) => {
                    s.latest_remote = Some(RemoteCheck {
                        commit,
                        checked_at: at.clone(),
                    });
                    s.last_check = Some(CheckAttempt {
                        at,
                        ok: true,
                        error: None,
                    });
                }
                Err(e) => {
                    s.last_check = Some(CheckAttempt {
                        at,
                        ok: false,
                        error: Some(e),
                    })
                }
            }
            Ok(())
        })
    }

    fn activate_following(&self, commit: &str) -> Result<(), String> {
        self.update_state(|s| {
            s.activate(commit);
            s.rolled_back.retain(|c| c != commit);
            s.failures.retain(|f| f.commit != commit);
            s.last_success_refresh_at = Some(now());
            Ok(())
        })?;
        self.prune();
        Ok(())
    }

    /// Prepares one revision (e.g. a historical one a run recorded) without
    /// activating it. Reuses the cache when it is already there.
    pub fn prepare_revision(&self, commit: &str, log: LogFn) -> Result<Manifest, String> {
        if !is_full_commit(commit) {
            return Err(format!(
                "{commit:?} is not a full 40-character commit id; only exact commits are prepared"
            ));
        }
        let Ok(_guard) = self.refresh.try_lock() else {
            return Err("another runtime refresh is in progress; try again when it ends".into());
        };
        let Some(_file_lock) = lock::RefreshLock::acquire(&self.root) else {
            return Err(
                "another Atomic Chat process is preparing a runtime; try again later".into(),
            );
        };
        self.enable()?;
        if let Ok((m, _)) = self.verify_cached(commit) {
            return Ok(m);
        }
        self.clean_staging();
        let out = self
            .prepare_locked(commit, log)
            .map_err(|e| format!("{}: {}", e.stage, e.message));
        self.set_progress(None);
        out
    }

    /// Fetch, build, validate and publish `commit` into the cache. The caller
    /// holds the refresh locks.
    fn prepare_locked(&self, commit: &str, log: LogFn) -> Result<Manifest, PrepareError> {
        let staging = self
            .staging_dir()
            .join(format!("{commit}.{}", uuid::Uuid::new_v4().simple()));
        let result = self.prepare_into(commit, &staging, log);
        if staging.exists() {
            let _ = std::fs::remove_dir_all(&staging);
        }
        if let Err(e) = &result {
            log(&format!(
                "Preparing {} failed at {}: {}",
                short(commit),
                e.stage,
                e.message
            ));
            let failure = PrepareFailure {
                commit: commit.to_string(),
                at: now(),
                stage: e.stage.clone(),
                error: e.message.clone(),
            };
            let _ = self.update_state(|s| {
                s.record_failure(failure);
                Ok(())
            });
        }
        result
    }

    fn prepare_into(
        &self,
        commit: &str,
        staging: &Path,
        log: LogFn,
    ) -> Result<Manifest, PrepareError> {
        std::fs::create_dir_all(staging).map_err(|e| {
            PrepareError::new(
                "publish",
                format!("could not create {}: {e}", staging.display()),
            )
        })?;
        self.progress_stage(Some(commit), "building");
        let prepared = {
            let mut tee = |line: &str| {
                self.progress_line(line);
                log(line);
            };
            self.source.prepare(commit, staging, &mut tee)?
        };
        self.progress_stage(Some(commit), "validating");
        let bin_rel = Path::new("bin").join(BIN_NAME);
        let bin = staging.join(&bin_rel);
        if prepared.binary != bin {
            if let Some(parent) = bin.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| PrepareError::new("publish", e.to_string()))?;
            }
            std::fs::copy(&prepared.binary, &bin).map_err(|e| {
                PrepareError::new("publish", format!("could not copy the executable: {e}"))
            })?;
        }
        let scratch = staging.join("validate");
        let (version, checks) = validate(&bin, &scratch);
        let _ = std::fs::remove_dir_all(&scratch);
        for c in &checks {
            log(&format!(
                "check {}: {} {}",
                c.name,
                if c.passed { "passed" } else { "FAILED" },
                c.detail
            ));
        }
        let version = version.map_err(|e| PrepareError::new("validate", e))?;
        let manifest = Manifest {
            schema: MANIFEST_SCHEMA.to_string(),
            commit: commit.to_string(),
            platform: platform(),
            repository: self.source.repository().to_string(),
            source: "source_build".to_string(),
            version,
            binary: bin_rel.to_string_lossy().replace('\\', "/"),
            binary_sha256: sha256_file(&bin).map_err(|e| PrepareError::new("publish", e))?,
            toolchain: prepared.toolchain,
            prepared_at: now(),
            validated_at: now(),
            checks,
        };
        // Only the executable and the manifest are published.
        for entry in std::fs::read_dir(staging).into_iter().flatten().flatten() {
            if entry.file_name() != "bin" {
                let p = entry.path();
                let _ = if p.is_dir() {
                    std::fs::remove_dir_all(&p)
                } else {
                    std::fs::remove_file(&p)
                };
            }
        }
        let text = serde_json::to_string_pretty(&manifest)
            .map_err(|e| PrepareError::new("publish", e.to_string()))?;
        std::fs::write(staging.join("runtime.json"), text)
            .map_err(|e| PrepareError::new("publish", e.to_string()))?;
        let dest = self.runtime_dir(commit);
        std::fs::create_dir_all(self.platform_dir())
            .map_err(|e| PrepareError::new("publish", e.to_string()))?;
        if dest.exists() {
            // A leftover that failed verification; a valid one was reused
            // before getting here.
            let aside = self.staging_dir().join(format!(
                "replaced-{commit}.{}",
                uuid::Uuid::new_v4().simple()
            ));
            std::fs::rename(&dest, &aside)
                .map_err(|e| PrepareError::new("publish", e.to_string()))?;
        }
        std::fs::rename(staging, &dest).map_err(|e| {
            PrepareError::new("publish", format!("could not publish the runtime: {e}"))
        })?;
        log(&format!(
            "Prepared and validated stack-agent {} at {}.",
            manifest.version,
            short(commit)
        ));
        Ok(manifest)
    }

    /// Removes what interrupted preparations left. Never touches published
    /// runtimes. Callers hold the refresh locks.
    fn clean_staging(&self) {
        if let Ok(entries) = std::fs::read_dir(self.staging_dir()) {
            for e in entries.flatten() {
                let _ =
                    std::fs::remove_dir_all(e.path()).or_else(|_| std::fs::remove_file(e.path()));
            }
        }
    }

    /// Removes cached runtimes nobody needs: not active, not pinned, not among
    /// the last [`KEEP_PREVIOUS`], not in use by a run.
    pub fn prune(&self) {
        let state = self.load_state();
        let leased: Vec<String> = self
            .leases
            .lock()
            .map(|l| l.keys().cloned().collect())
            .unwrap_or_default();
        let mut keep: Vec<String> = state.active.iter().cloned().collect();
        keep.extend(state.previous.iter().take(KEEP_PREVIOUS).cloned());
        if let Selection::Pinned { commit } = &state.selection {
            keep.push(commit.clone());
        }
        keep.extend(leased);
        for c in self.cached() {
            if !keep.contains(&c.commit) {
                log::info!("stack-agent runtime: pruning {}", c.commit);
                let _ = std::fs::remove_dir_all(&c.path);
            }
        }
        let _ = self.update_state(|s| {
            s.previous.truncate(KEEP_PREVIOUS);
            Ok(())
        });
    }

    /// Activates the previous validated runtime and stops automatic refreshes
    /// from re-activating the one rolled back from.
    pub fn rollback(&self) -> Result<String, String> {
        let _guard = self
            .refresh
            .try_lock()
            .map_err(|_| "a runtime refresh is in progress; roll back when it ends".to_string())?;
        let state = self.load_state();
        let target = state
            .previous
            .iter()
            .find(|c| self.verify_cached(c).is_ok())
            .cloned()
            .ok_or("no previous validated runtime is cached to roll back to")?;
        self.update_state(|s| {
            if let Some(from) = s.active.clone() {
                if !s.rolled_back.contains(&from) {
                    s.rolled_back.insert(0, from);
                }
            }
            s.activate(&target);
            Ok(())
        })?;
        Ok(target)
    }

    /// Follow `main` again, or pin a cached revision.
    pub fn select(&self, selection: Selection) -> Result<(), String> {
        if let Selection::Pinned { commit } = &selection {
            self.verify_cached(commit)?;
        }
        self.update_state(|s| {
            s.selection = selection;
            Ok(())
        })
    }

    pub fn lease(self: &Arc<Self>, commit: &str) -> Lease {
        if let Ok(mut l) = self.leases.lock() {
            *l.entry(commit.to_string()).or_insert(0) += 1;
        }
        Lease {
            manager: self.clone(),
            commit: commit.to_string(),
        }
    }

    fn managed(&self, commit: &str, selection: &str, state: &State) -> Result<Resolved, String> {
        let (m, bin) = self.verify_cached(commit)?;
        let latest = state.latest_remote.as_ref();
        Ok(Resolved {
            path: bin.clone(),
            provenance: RuntimeProvenance {
                source: "managed".into(),
                selection: selection.into(),
                path: bin.to_string_lossy().into_owned(),
                commit: Some(m.commit.clone()),
                version: Some(m.version.clone()),
                platform: m.platform.clone(),
                binary_sha256: Some(m.binary_sha256.clone()),
                repository: Some(m.repository.clone()),
                toolchain: m.toolchain.clone(),
                prepared_at: Some(m.prepared_at.clone()),
                latest_known_main: latest.map(|r| r.commit.clone()),
                latest_checked_at: latest.map(|r| r.checked_at.clone()),
                matches_latest: latest.map(|r| r.commit == m.commit),
            },
        })
    }

    /// The executable a stack-agent run or the Launch page uses now, in this
    /// order: a path set on the Launch page; the pinned revision; the active
    /// managed runtime; an unmanaged `stack-agent` found by `find_external`.
    /// A pinned revision that is not usable is an error, never a fallback.
    pub fn resolve(
        &self,
        custom_path: Option<&str>,
        find_external: &dyn Fn() -> Option<PathBuf>,
    ) -> Result<Resolved, String> {
        let state = self.load_state();
        if let Some(p) = custom_path.map(str::trim).filter(|p| !p.is_empty()) {
            let path = PathBuf::from(p);
            // A managed executable passed back by the UI is still managed.
            if let Some(commit) = self.commit_of_managed_path(&path) {
                let selection = match &state.selection {
                    Selection::Pinned { commit: c } if *c == commit => "pinned",
                    _ => "follow_main",
                };
                return self.managed(&commit, selection, &state);
            }
            if !path.is_file() {
                return Err(format!(
                    "{p} does not exist; fix the path on the Launch page"
                ));
            }
            return Ok(unmanaged(path, "custom_path", &state));
        }
        if let Selection::Pinned { commit } = &state.selection {
            return self
                .managed(commit, "pinned", &state)
                .map_err(|e| format!("the pinned stack-agent revision cannot be used: {e}"));
        }
        if let Some(active) = &state.active {
            match self.managed(active, "follow_main", &state) {
                Ok(r) => return Ok(r),
                Err(e) => log::warn!("stack-agent runtime: active runtime unusable: {e}"),
            }
        }
        if let Some(found) = find_external() {
            return Ok(unmanaged(found, "external", &state));
        }
        Err(
            "stack-agent is not prepared yet. Prepare it from Agent Bench (Stack-Agent runtime) \
             or install it from the Launch page."
                .into(),
        )
    }

    /// [`Self::resolve`] for a run: also leases a managed runtime so it is
    /// not pruned before the run ends.
    pub fn resolve_for_run(
        self: &Arc<Self>,
        custom_path: Option<&str>,
        find_external: &dyn Fn() -> Option<PathBuf>,
    ) -> Result<(Resolved, Option<Lease>), String> {
        let r = self.resolve(custom_path, find_external)?;
        let lease = r.provenance.commit.as_deref().map(|c| self.lease(c));
        Ok((r, lease))
    }

    fn commit_of_managed_path(&self, path: &Path) -> Option<String> {
        let rel = path.strip_prefix(self.platform_dir()).ok()?;
        let commit = rel.components().next()?.as_os_str().to_str()?.to_string();
        is_full_commit(&commit).then_some(commit)
    }

    pub fn status(&self, find_external: &dyn Fn() -> Option<PathBuf>) -> RuntimeStatus {
        let state = self.load_state();
        let cached = self.cached();
        let progress = self.progress.lock().ok().and_then(|g| g.clone());
        let resolved = self.resolve(None, find_external);
        let valid = |c: &str| cached.iter().any(|x| x.commit == c && x.valid);
        let latest = state.latest_remote.clone();
        let failure = latest
            .as_ref()
            .and_then(|l| state.failure_for(&l.commit))
            .cloned();
        let check_failed = state.last_check.as_ref().is_some_and(|c| !c.ok);
        let fresh = latest.as_ref().is_some_and(|l| {
            chrono::DateTime::parse_from_rfc3339(&l.checked_at)
                .map(|t| chrono::Utc::now().signed_duration_since(t) < CHECK_FRESH_FOR)
                .unwrap_or(false)
        });
        let active_ok = state.active.as_deref().is_some_and(valid);
        let (status, detail) = if let Some(p) = &progress {
            (
                StatusKind::Refreshing,
                match &p.commit {
                    Some(c) => format!("{} revision {}.", capitalize(&p.stage), short(c)),
                    None => format!("Checking {TRACKED_REF} for a new revision."),
                },
            )
        } else if let Selection::Pinned { commit } = &state.selection {
            let behind = match &latest {
                Some(l) if l.commit != *commit => format!(
                    " {TRACKED_REF} was at {} when last checked; runs keep using the pinned revision.",
                    short(&l.commit)
                ),
                _ => String::new(),
            };
            (
                StatusKind::Pinned,
                format!("Pinned to revision {}.{behind}", short(commit)),
            )
        } else if !active_ok {
            match (&resolved, &failure) {
                (_, Some(f)) => (
                    StatusKind::UpdateFailed,
                    format!(
                        "Preparing {} failed at {}: {}",
                        short(&f.commit),
                        f.stage,
                        f.error
                    ),
                ),
                (Ok(r), None) if r.provenance.source == "external" => (
                    StatusKind::External,
                    format!(
                        "Using an unmanaged stack-agent at {}; its revision is unknown.",
                        r.provenance.path
                    ),
                ),
                _ => (
                    StatusKind::NotInstalled,
                    "No stack-agent runtime is prepared yet.".to_string(),
                ),
            }
        } else if check_failed || !fresh {
            let why = match &state.last_check {
                Some(c) if !c.ok => format!(
                    "the last check failed ({})",
                    c.error.clone().unwrap_or_default()
                ),
                _ if latest.is_none() => "main has not been checked yet".to_string(),
                _ => "the last successful check is more than a day old".to_string(),
            };
            (
                StatusKind::Stale,
                format!(
                    "Using {} but it may be stale: {why}.{}",
                    short(state.active.as_deref().unwrap_or_default()),
                    latest
                        .as_ref()
                        .map(|l| format!(" main was at {} on {}.", short(&l.commit), l.checked_at))
                        .unwrap_or_default()
                ),
            )
        } else {
            let latest = latest.as_ref().expect("fresh implies a check");
            let active = state.active.as_deref().unwrap_or_default();
            if latest.commit == active {
                (
                    StatusKind::Current,
                    format!(
                        "Revision {} is the latest main (checked {}).",
                        short(active),
                        latest.checked_at
                    ),
                )
            } else if let Some(f) = &failure {
                (
                    StatusKind::UpdateFailed,
                    format!(
                        "main is at {} but preparing it failed at {}: {}. Still using {}.",
                        short(&f.commit),
                        f.stage,
                        f.error,
                        short(active)
                    ),
                )
            } else if state.rolled_back.contains(&latest.commit) {
                (
                    StatusKind::UpdateAvailable,
                    format!(
                        "Rolled back from {}; using {}. Refresh to use it again.",
                        short(&latest.commit),
                        short(active)
                    ),
                )
            } else {
                (
                    StatusKind::UpdateAvailable,
                    format!(
                        "main is at {}; using {}.",
                        short(&latest.commit),
                        short(active)
                    ),
                )
            }
        };
        let (resolved, resolve_error) = match resolved {
            Ok(r) => (Some(r.provenance), None),
            Err(e) => (None, Some(e)),
        };
        RuntimeStatus {
            status,
            detail,
            enabled: self.enabled(),
            platform: platform(),
            repository: self.source.repository().to_string(),
            tracked_ref: TRACKED_REF.to_string(),
            selection: state.selection.clone(),
            resolved,
            resolve_error,
            active: state.active.clone(),
            previous: state.previous.clone(),
            latest_remote: state.latest_remote.clone(),
            last_check: state.last_check.clone(),
            last_success_refresh_at: state.last_success_refresh_at.clone(),
            refreshing: progress,
            failure,
            rolled_back: state.rolled_back.clone(),
            cached,
        }
    }
}

fn unmanaged(path: PathBuf, kind: &str, state: &State) -> Resolved {
    let latest = state.latest_remote.as_ref();
    Resolved {
        provenance: RuntimeProvenance {
            source: kind.into(),
            selection: kind.into(),
            path: path.to_string_lossy().into_owned(),
            commit: None,
            version: None,
            platform: platform(),
            binary_sha256: sha256_file(&path).ok(),
            repository: None,
            toolchain: None,
            prepared_at: None,
            latest_known_main: latest.map(|r| r.commit.clone()),
            latest_checked_at: latest.map(|r| r.checked_at.clone()),
            matches_latest: None,
        },
        path,
    }
}

pub fn short(commit: &str) -> &str {
    &commit[..commit.len().min(12)]
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    c.next()
        .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
        .unwrap_or_default()
}
