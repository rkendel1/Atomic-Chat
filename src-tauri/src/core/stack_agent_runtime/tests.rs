//! The runtime lifecycle against a fake source whose "builds" are small
//! shell scripts, so every step runs for real except git and cargo.
#![cfg(unix)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use super::*;

const A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const C: &str = "cccccccccccccccccccccccccccccccccccccccc";

#[derive(Clone, Copy, PartialEq)]
enum Build {
    Good,
    /// cargo fails.
    Fails,
    /// Builds, but `--version` fails.
    NoLaunch,
    /// Builds, but lacks a flag Atomic Chat passes.
    OldInterface,
    /// Waits until released (to hold the refresh lock).
    Slow,
}

struct Fake {
    latest: Mutex<Result<String, String>>,
    /// First-parent history of main, newest first.
    history: Mutex<Vec<String>>,
    builds: Mutex<HashMap<String, Build>>,
    prepared: AtomicUsize,
    /// What state.json said while a build ran.
    seen_active_during_build: Mutex<Vec<Option<String>>>,
    root: PathBuf,
    release: Mutex<Option<std::sync::mpsc::Receiver<()>>>,
    started: Mutex<Option<std::sync::mpsc::Sender<()>>>,
}

fn script(version_ok: bool, flags: &str) -> String {
    format!(
        "#!/bin/sh\ncase \"$1\" in\n  --version) {};;\n  --test) echo 'Chip'; echo 'Turn completed';;\n  work) echo 'usage: stack-agent work {flags}';;\nesac\n",
        if version_ok {
            "echo 'stack-agent 0.1.0'"
        } else {
            "echo 'boom' >&2; exit 3"
        }
    )
}

impl Source for Fake {
    fn repository(&self) -> &str {
        "https://example.invalid/stack-agent"
    }
    fn latest(&self) -> Result<String, String> {
        self.latest.lock().unwrap().clone()
    }
    fn history(&self, commit: &str, n: usize, _: &Path) -> Result<Vec<String>, String> {
        let h = self.history.lock().unwrap();
        let at = h.iter().position(|c| c == commit).ok_or("unknown commit")?;
        Ok(h[at + 1..].iter().take(n).cloned().collect())
    }
    fn prepare(
        &self,
        commit: &str,
        staging: &Path,
        log: &mut dyn FnMut(&str),
    ) -> Result<Prepared, PrepareError> {
        self.prepared.fetch_add(1, Ordering::SeqCst);
        let state: Option<State> = std::fs::read_to_string(self.root.join("state.json"))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok());
        self.seen_active_during_build
            .lock()
            .unwrap()
            .push(state.and_then(|s| s.active));
        log(&format!("building {commit}"));
        let kind = *self
            .builds
            .lock()
            .unwrap()
            .get(commit)
            .unwrap_or(&Build::Good);
        if kind == Build::Slow {
            if let Some(tx) = self.started.lock().unwrap().take() {
                let _ = tx.send(());
            }
            let rx = self.release.lock().unwrap().take();
            if let Some(rx) = rx {
                let _ = rx.recv();
            }
        }
        if kind == Build::Fails {
            return Err(PrepareError::new(
                "build",
                "error[E0425]: cannot find value",
            ));
        }
        let all = REQUIRED_WORK_FLAGS.join(" ");
        let flags = if kind == Build::OldInterface {
            all.replace("--state-db", "")
        } else {
            all
        };
        let bin = staging.join("bin").join(BIN_NAME);
        std::fs::create_dir_all(bin.parent().unwrap()).unwrap();
        std::fs::write(&bin, script(kind != Build::NoLaunch, &flags)).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        Ok(Prepared {
            binary: bin,
            toolchain: Some("rustc 1.97.0 (fake)".into()),
        })
    }
}

struct Env {
    _tmp: tempfile::TempDir,
    fake: Arc<Fake>,
    m: Arc<Manager>,
}

fn env() -> Env {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("runtime");
    let fake = Arc::new(Fake {
        latest: Mutex::new(Ok(A.to_string())),
        history: Mutex::new(vec![]),
        builds: Mutex::new(HashMap::new()),
        prepared: AtomicUsize::new(0),
        seen_active_during_build: Mutex::new(vec![]),
        root: root.clone(),
        release: Mutex::new(None),
        started: Mutex::new(None),
    });
    let m = Arc::new(Manager::new(root, fake.clone()));
    Env { _tmp: tmp, fake, m }
}

impl Env {
    fn set_latest(&self, c: &str) {
        *self.fake.latest.lock().unwrap() = Ok(c.to_string());
    }
    fn offline(&self) {
        *self.fake.latest.lock().unwrap() = Err("could not resolve host: github.com".into());
    }
    fn build(&self, c: &str, b: Build) {
        self.fake.builds.lock().unwrap().insert(c.to_string(), b);
    }
    fn refresh(&self, t: Trigger) -> RefreshOutcome {
        self.m.refresh(t, &mut |_| {})
    }
    fn prepared(&self) -> usize {
        self.fake.prepared.load(Ordering::SeqCst)
    }
    fn status(&self) -> RuntimeStatus {
        self.m.status(&no_external)
    }
    fn resolve(&self) -> Result<Resolved, String> {
        self.m.resolve(None, &no_external)
    }
}

fn no_external() -> Option<PathBuf> {
    None
}

#[test]
fn first_refresh_acquires_validates_and_activates_a_runtime() {
    let e = env();
    assert!(!e.m.enabled());
    assert_eq!(e.status().status, StatusKind::NotInstalled);
    assert!(e.resolve().is_err());

    assert_eq!(
        e.refresh(Trigger::Manual),
        RefreshOutcome::Activated { commit: A.into() }
    );
    let s = e.status();
    assert_eq!(s.status, StatusKind::Current, "{}", s.detail);
    assert_eq!(s.active.as_deref(), Some(A));
    assert!(s.last_success_refresh_at.is_some());
    let r = e.resolve().unwrap();
    assert_eq!(r.provenance.source, "managed");
    assert_eq!(r.provenance.selection, "follow_main");
    assert_eq!(r.provenance.commit.as_deref(), Some(A));
    assert_eq!(r.provenance.version.as_deref(), Some("0.1.0"));
    assert_eq!(r.provenance.matches_latest, Some(true));
    assert_eq!(
        r.provenance.toolchain.as_deref(),
        Some("rustc 1.97.0 (fake)")
    );
    assert!(r.path.starts_with(e.m.runtime_dir(A)));
    let (m, _) = e.m.verify_cached(A).unwrap();
    assert_eq!(m.source, "source_build");
    assert_eq!(
        m.checks
            .iter()
            .map(|c| (c.name.as_str(), c.passed))
            .collect::<Vec<_>>(),
        vec![
            ("launches", true),
            ("self_test", true),
            ("work_interface", true)
        ]
    );
    // Only the executable and the manifest are published.
    let mut names: Vec<String> = std::fs::read_dir(e.m.runtime_dir(A))
        .unwrap()
        .map(|d| d.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(names, vec!["bin", "runtime.json"]);
}

#[test]
fn an_unchanged_revision_is_reused_without_rebuilding() {
    let e = env();
    e.refresh(Trigger::Manual);
    assert_eq!(
        e.refresh(Trigger::Automatic),
        RefreshOutcome::AlreadyCurrent { commit: A.into() }
    );
    assert_eq!(
        e.refresh(Trigger::Manual),
        RefreshOutcome::AlreadyCurrent { commit: A.into() }
    );
    assert_eq!(e.prepared(), 1);
}

#[test]
fn a_newer_main_is_built_beside_the_active_one_and_activated_after_validation() {
    let e = env();
    e.refresh(Trigger::Manual);
    e.set_latest(B);
    assert_eq!(
        e.refresh(Trigger::Automatic),
        RefreshOutcome::Activated { commit: B.into() }
    );
    // While B was being built, A was still the active runtime.
    assert_eq!(
        *e.fake.seen_active_during_build.lock().unwrap(),
        vec![None, Some(A.to_string())]
    );
    let s = e.status();
    assert_eq!(s.active.as_deref(), Some(B));
    assert_eq!(s.previous, vec![A.to_string()]);
    assert!(e.m.verify_cached(A).is_ok(), "the previous runtime is kept");
    assert_eq!(e.prepared(), 2);
}

#[test]
fn a_failed_build_keeps_the_last_known_good_runtime_and_is_not_retried_automatically() {
    let e = env();
    e.refresh(Trigger::Manual);
    e.set_latest(B);
    e.build(B, Build::Fails);
    match e.refresh(Trigger::Automatic) {
        RefreshOutcome::PrepareFailed { commit, stage, .. } => {
            assert_eq!(commit, B);
            assert_eq!(stage, "build");
        }
        o => panic!("{o:?}"),
    }
    let s = e.status();
    assert_eq!(s.status, StatusKind::UpdateFailed);
    assert!(
        s.detail.contains("Still using aaaaaaaaaaaa"),
        "{}",
        s.detail
    );
    assert_eq!(e.resolve().unwrap().provenance.commit.as_deref(), Some(A));
    assert_eq!(e.resolve().unwrap().provenance.matches_latest, Some(false));
    assert!(!e.m.runtime_dir(B).exists());
    assert_eq!(
        std::fs::read_dir(e.m.root().join("staging"))
            .unwrap()
            .count(),
        0
    );

    assert!(matches!(
        e.refresh(Trigger::Automatic),
        RefreshOutcome::Skipped { .. }
    ));
    assert_eq!(e.prepared(), 2);
    // A manual refresh retries; once it builds, it is activated.
    e.build(B, Build::Good);
    assert_eq!(
        e.refresh(Trigger::Manual),
        RefreshOutcome::Activated { commit: B.into() }
    );
    assert!(e.status().failure.is_none());
}

#[test]
fn a_broken_main_falls_back_to_its_newest_revision_that_builds() {
    // As observed on the real repository: main's tip and its parent do not
    // compile; two merges back does.
    let e = env();
    let d = "dddddddddddddddddddddddddddddddddddddddd";
    *e.fake.history.lock().unwrap() = vec![C.into(), B.into(), d.into(), A.into()];
    e.set_latest(C);
    e.build(C, Build::Fails);
    e.build(B, Build::Fails);
    match e.refresh(Trigger::Manual) {
        RefreshOutcome::ActivatedPrevious {
            commit,
            latest,
            skipped,
        } => {
            assert_eq!(commit, d);
            assert_eq!(latest, C);
            assert_eq!(skipped, vec![C.to_string(), B.to_string()]);
        }
        o => panic!("{o:?}"),
    }
    let s = e.status();
    assert_eq!(s.active.as_deref(), Some(d));
    // Never "current": main itself could not be prepared.
    assert_eq!(s.status, StatusKind::UpdateFailed, "{}", s.detail);
    assert_eq!(e.resolve().unwrap().provenance.matches_latest, Some(false));
    assert!(!e.m.runtime_dir(C).exists() && !e.m.runtime_dir(B).exists());

    // The walk stops at the active runtime and does not rebuild failures.
    let before = e.prepared();
    assert!(matches!(
        e.refresh(Trigger::Automatic),
        RefreshOutcome::Skipped { .. }
    ));
    assert_eq!(e.prepared(), before);
}

#[test]
fn walking_back_never_goes_past_the_active_runtime_or_retries_a_fetch_failure() {
    let e = env();
    *e.fake.history.lock().unwrap() = vec![C.into(), B.into(), A.into()];
    e.refresh(Trigger::Manual);
    e.set_latest(C);
    e.build(C, Build::Fails);
    e.build(B, Build::Fails);
    assert!(matches!(
        e.refresh(Trigger::Manual),
        RefreshOutcome::PrepareFailed { .. }
    ));
    assert_eq!(e.status().active.as_deref(), Some(A));
    // A, B attempted: C and B; A is active and not rebuilt.
    assert_eq!(e.prepared(), 3);
}

#[test]
fn a_runtime_that_fails_its_health_checks_is_never_published() {
    for (kind, check) in [
        (Build::NoLaunch, "launches"),
        (Build::OldInterface, "work_interface"),
    ] {
        let e = env();
        e.refresh(Trigger::Manual);
        e.set_latest(B);
        e.build(B, kind);
        match e.refresh(Trigger::Manual) {
            RefreshOutcome::PrepareFailed { stage, error, .. } => {
                assert_eq!(stage, "validate");
                assert!(error.contains(check), "{error}");
            }
            o => panic!("{o:?}"),
        }
        assert!(!e.m.runtime_dir(B).exists());
        assert_eq!(e.status().active.as_deref(), Some(A));
    }
}

#[test]
fn a_runtime_changed_after_validation_is_rejected() {
    let e = env();
    e.refresh(Trigger::Manual);
    let bin = e.resolve().unwrap().path;
    std::fs::write(&bin, "#!/bin/sh\necho 'stack-agent 0.1.0'\n").unwrap();
    assert!(e
        .m
        .verify_cached(A)
        .unwrap_err()
        .contains("changed after validation"));
    assert!(e.resolve().is_err(), "a tampered runtime is never used");
    assert_ne!(e.status().status, StatusKind::Current);
}

#[test]
fn an_interrupted_preparation_leaves_nothing_usable_and_is_cleaned_up() {
    let e = env();
    e.refresh(Trigger::Manual);
    e.set_latest(B);
    // A crash mid-build left a staging directory, and a crash mid-publish a
    // runtime directory without a manifest.
    let staging = e.m.root().join("staging").join(format!("{B}.dead"));
    std::fs::create_dir_all(staging.join("bin")).unwrap();
    std::fs::create_dir_all(e.m.runtime_dir(B).join("bin")).unwrap();
    std::fs::write(e.m.runtime_dir(B).join("bin").join(BIN_NAME), "partial").unwrap();
    assert!(e.m.verify_cached(B).is_err());
    assert_eq!(e.resolve().unwrap().provenance.commit.as_deref(), Some(A));

    assert_eq!(
        e.refresh(Trigger::Automatic),
        RefreshOutcome::Activated { commit: B.into() }
    );
    assert!(!staging.exists());
    assert!(e.m.verify_cached(B).is_ok());
}

#[test]
fn rollback_restores_the_previous_runtime_and_automatic_refresh_respects_it() {
    let e = env();
    e.refresh(Trigger::Manual);
    e.set_latest(B);
    e.refresh(Trigger::Manual);
    assert_eq!(e.m.rollback().unwrap(), A);
    let s = e.status();
    assert_eq!(s.active.as_deref(), Some(A));
    assert_eq!(s.rolled_back, vec![B.to_string()]);
    assert_eq!(s.status, StatusKind::UpdateAvailable);
    assert!(s.detail.contains("Rolled back from"), "{}", s.detail);
    assert!(matches!(
        e.refresh(Trigger::Automatic),
        RefreshOutcome::Skipped { .. }
    ));
    assert_eq!(e.resolve().unwrap().provenance.commit.as_deref(), Some(A));
    // A manual refresh brings B back from the cache, without a build.
    assert_eq!(
        e.refresh(Trigger::Manual),
        RefreshOutcome::ActivatedCached { commit: B.into() }
    );
    assert_eq!(e.prepared(), 2);
}

#[test]
fn rollback_needs_a_previous_runtime() {
    let e = env();
    e.refresh(Trigger::Manual);
    assert!(e.m.rollback().unwrap_err().contains("no previous"));
}

#[test]
fn only_one_refresh_runs_at_a_time() {
    let e = env();
    e.build(A, Build::Slow);
    let (tx_start, rx_start) = std::sync::mpsc::channel();
    let (tx_release, rx_release) = std::sync::mpsc::channel();
    *e.fake.started.lock().unwrap() = Some(tx_start);
    *e.fake.release.lock().unwrap() = Some(rx_release);
    let m = e.m.clone();
    let worker = std::thread::spawn(move || m.refresh(Trigger::Manual, &mut |_| {}));
    rx_start.recv().unwrap();

    assert!(e.m.is_refreshing());
    assert_eq!(e.status().status, StatusKind::Refreshing);
    assert_eq!(e.refresh(Trigger::Manual), RefreshOutcome::Busy);
    assert!(e
        .m
        .prepare_revision(B, &mut |_| {})
        .unwrap_err()
        .contains("in progress"));
    assert!(e.m.rollback().is_err());

    tx_release.send(()).unwrap();
    assert_eq!(
        worker.join().unwrap(),
        RefreshOutcome::Activated { commit: A.into() }
    );
    assert_eq!(e.prepared(), 1);
    assert!(!e.m.root().join("refresh.lock").exists());
}

#[test]
fn another_live_process_holding_the_lock_blocks_a_refresh_and_a_dead_ones_lock_is_taken_over() {
    let e = env();
    std::fs::create_dir_all(e.m.root()).unwrap();
    let lock = e.m.root().join("refresh.lock");
    // pid 1 is always alive.
    std::fs::write(&lock, "1").unwrap();
    assert_eq!(e.refresh(Trigger::Manual), RefreshOutcome::Busy);
    // A pid that cannot exist.
    std::fs::write(&lock, "4194399").unwrap();
    assert_eq!(
        e.refresh(Trigger::Manual),
        RefreshOutcome::Activated { commit: A.into() }
    );
}

#[test]
fn offline_startup_uses_the_cached_runtime_and_says_it_may_be_stale() {
    let e = env();
    e.refresh(Trigger::Manual);
    // A new process, offline.
    let m2 = Manager::new(e.m.root().to_path_buf(), e.fake.clone());
    e.offline();
    assert!(matches!(
        m2.refresh(Trigger::Automatic, &mut |_| {}),
        RefreshOutcome::CheckFailed { .. }
    ));
    let s = m2.status(&no_external);
    assert_eq!(s.status, StatusKind::Stale);
    assert!(s.detail.contains("may be stale"), "{}", s.detail);
    assert!(s.detail.contains("could not resolve host"), "{}", s.detail);
    let r = m2.resolve(None, &no_external).unwrap();
    assert_eq!(r.provenance.commit.as_deref(), Some(A));
    // The last revision actually seen, not a claim about now.
    assert_eq!(r.provenance.latest_known_main.as_deref(), Some(A));
    assert_eq!(e.prepared(), 1);
}

#[test]
fn a_runtime_is_never_current_without_a_recent_check() {
    let e = env();
    e.refresh(Trigger::Manual);
    e.m.update_state(|s| {
        s.latest_remote.as_mut().unwrap().checked_at = "2020-01-01T00:00:00.000Z".into();
        Ok(())
    })
    .unwrap();
    let s = e.status();
    assert_eq!(s.status, StatusKind::Stale);
    assert!(s.detail.contains("more than a day old"), "{}", s.detail);
}

#[test]
fn a_pinned_historical_revision_is_prepared_on_demand_and_never_replaced() {
    let e = env();
    e.set_latest(B);
    e.refresh(Trigger::Manual);
    // Reproduce a run recorded against A.
    let m = e.m.prepare_revision(A, &mut |_| {}).unwrap();
    assert_eq!(m.commit, A);
    assert_eq!(
        e.status().active.as_deref(),
        Some(B),
        "preparing does not activate"
    );
    e.m.select(Selection::Pinned { commit: A.into() }).unwrap();

    e.set_latest(C);
    assert_eq!(
        e.refresh(Trigger::Automatic),
        RefreshOutcome::Pinned {
            pinned: A.into(),
            latest: C.into()
        }
    );
    let r = e.resolve().unwrap();
    assert_eq!(r.provenance.commit.as_deref(), Some(A));
    assert_eq!(r.provenance.selection, "pinned");
    assert_eq!(r.provenance.matches_latest, Some(false));
    let s = e.status();
    assert_eq!(s.status, StatusKind::Pinned);
    assert!(s.detail.contains("cccccccccccc"), "{}", s.detail);
    // Pruning keeps the pinned revision.
    e.m.prune();
    assert!(e.m.verify_cached(A).is_ok());

    e.m.select(Selection::FollowMain).unwrap();
    assert_eq!(
        e.refresh(Trigger::Automatic),
        RefreshOutcome::Activated { commit: C.into() }
    );
}

#[test]
fn pinning_requires_a_validated_cached_revision() {
    let e = env();
    e.refresh(Trigger::Manual);
    assert!(e
        .m
        .select(Selection::Pinned { commit: B.into() })
        .unwrap_err()
        .contains("not prepared"));
    assert!(e
        .m
        .prepare_revision("abc1234", &mut |_| {})
        .unwrap_err()
        .contains("full 40-character"));
}

#[test]
fn a_pinned_revision_that_disappears_is_an_error_not_a_silent_fallback() {
    let e = env();
    e.refresh(Trigger::Manual);
    e.m.select(Selection::Pinned { commit: A.into() }).unwrap();
    std::fs::remove_dir_all(e.m.runtime_dir(A)).unwrap();
    let err =
        e.m.resolve(None, &|| Some(PathBuf::from("/usr/bin/true")))
            .unwrap_err();
    assert!(err.contains("pinned"), "{err}");
}

#[test]
fn a_runtime_in_use_is_not_pruned() {
    let e = env();
    let commits = [
        A,
        B,
        C,
        "dddddddddddddddddddddddddddddddddddddddd",
        "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee",
    ];
    e.refresh(Trigger::Manual);
    let lease = e.m.lease(A);
    for c in &commits[1..] {
        e.set_latest(c);
        e.refresh(Trigger::Manual);
    }
    // A fell out of the last KEEP_PREVIOUS, but a run still uses it.
    assert!(e.m.verify_cached(A).is_ok());
    drop(lease);
    e.m.prune();
    assert!(!e.m.runtime_dir(A).exists());
    assert_eq!(e.status().previous.len(), KEEP_PREVIOUS);
}

#[test]
fn an_unmanaged_stack_agent_is_used_only_without_a_managed_runtime_and_is_labelled() {
    let e = env();
    let tmp = tempfile::tempdir().unwrap();
    let ext = tmp.path().join("stack-agent");
    std::fs::write(&ext, script(true, "")).unwrap();
    let find = || Some(ext.clone());
    let r = e.m.resolve(None, &find).unwrap();
    assert_eq!(r.provenance.source, "external");
    assert_eq!(r.provenance.commit, None);
    assert_eq!(r.provenance.matches_latest, None);
    assert!(r.provenance.binary_sha256.is_some());
    assert_eq!(e.m.status(&find).status, StatusKind::External);

    e.refresh(Trigger::Manual);
    assert_eq!(
        e.m.resolve(None, &find).unwrap().provenance.source,
        "managed"
    );
    // A Launch-page path wins, and is labelled as such.
    let r = e.m.resolve(Some(ext.to_str().unwrap()), &find).unwrap();
    assert_eq!(r.provenance.source, "custom_path");
    // The managed executable passed back by the UI stays managed.
    let managed = e.resolve().unwrap().path;
    let r = e.m.resolve(Some(managed.to_str().unwrap()), &find).unwrap();
    assert_eq!(r.provenance.source, "managed");
    assert_eq!(r.provenance.commit.as_deref(), Some(A));
}

#[test]
fn an_unreadable_state_file_is_set_aside_not_trusted() {
    let e = env();
    e.refresh(Trigger::Manual);
    std::fs::write(e.m.root().join("state.json"), "{not json").unwrap();
    assert_eq!(e.m.load_state(), State::default());
    assert!(std::fs::read_dir(e.m.root()).unwrap().any(|d| d
        .unwrap()
        .file_name()
        .to_string_lossy()
        .starts_with("state.unreadable-")));
    // The cached runtime is still there to re-activate without a build.
    assert_eq!(
        e.refresh(Trigger::Manual),
        RefreshOutcome::ActivatedCached { commit: A.into() }
    );
    assert_eq!(e.prepared(), 1);
}

#[test]
fn a_malformed_remote_answer_is_a_failed_check() {
    let e = env();
    e.set_latest("main");
    assert!(matches!(
        e.refresh(Trigger::Manual),
        RefreshOutcome::CheckFailed { .. }
    ));
    assert_eq!(e.prepared(), 0);
}

#[test]
fn versions_are_parsed_strictly() {
    assert_eq!(
        validate::parse_version("stack-agent 0.1.0\n").as_deref(),
        Some("0.1.0")
    );
    assert_eq!(
        validate::parse_version("stack-agent 1.2.3-rc.1").as_deref(),
        Some("1.2.3-rc.1")
    );
    assert_eq!(validate::parse_version("stack-agent"), None);
    assert_eq!(validate::parse_version("pax 0.1.0"), None);
    assert_eq!(validate::parse_version("stack-agent 0.1"), None);
}

#[test]
fn state_and_provenance_carry_no_credentials() {
    let e = env();
    std::env::set_var("CHIP_API_KEY", "sk-should-not-appear");
    e.refresh(Trigger::Manual);
    std::env::remove_var("CHIP_API_KEY");
    let state = std::fs::read_to_string(e.m.root().join("state.json")).unwrap();
    let manifest = std::fs::read_to_string(e.m.runtime_dir(A).join("runtime.json")).unwrap();
    let prov = serde_json::to_string(&e.resolve().unwrap().provenance).unwrap();
    for text in [state, manifest, prov] {
        assert!(!text.contains("sk-should-not-appear"));
    }
}

/// The real thing: resolve the official `main`, fetch exactly that commit,
/// build it with the repository's toolchain and lockfile, validate and
/// activate it; then reuse it without a rebuild, and start "offline" from
/// the cache. Needs network, git and rustup, and takes minutes.
///
///   ATOMIC_RUNTIME_ROOT=/some/dir cargo test --lib live_acquire_main -- --ignored --nocapture
#[test]
#[ignore]
fn live_acquire_main() {
    let root =
        PathBuf::from(std::env::var("ATOMIC_RUNTIME_ROOT").expect("set ATOMIC_RUNTIME_ROOT"));
    let path_env = std::env::var("PATH").unwrap_or_default();
    let source = |repository: &str| {
        Arc::new(GitSource {
            repository: repository.to_string(),
            build_dir: root.join("build").join(platform()),
            path_env: path_env.clone(),
        })
    };
    let m = Arc::new(Manager::new(root.clone(), source(REPOSITORY)));

    let t = std::time::Instant::now();
    let first = m.refresh(Trigger::Manual, &mut |l| eprintln!("  | {l}"));
    eprintln!(
        "first refresh: {first:?} in {:.1}s",
        t.elapsed().as_secs_f64()
    );
    let status = m.status(&no_external);
    eprintln!("{}", serde_json::to_string_pretty(&status).unwrap());
    let commit = status.active.clone().expect("a runtime is active");
    let latest = status
        .latest_remote
        .clone()
        .expect("main was resolved")
        .commit;
    let resolved = m.resolve(None, &no_external).unwrap().provenance;
    if commit == latest {
        assert_eq!(status.status, StatusKind::Current, "{}", status.detail);
        assert_eq!(resolved.matches_latest, Some(true));
    } else {
        // main's tip could not be prepared; an earlier revision of main runs,
        // and nothing claims it is current.
        assert!(
            matches!(first, RefreshOutcome::ActivatedPrevious { .. }),
            "{first:?}"
        );
        assert_eq!(status.status, StatusKind::UpdateFailed, "{}", status.detail);
        assert_eq!(resolved.matches_latest, Some(false));
    }

    let t = std::time::Instant::now();
    let before = std::fs::metadata(m.runtime_dir(&commit).join("runtime.json"))
        .unwrap()
        .modified()
        .unwrap();
    let second = m.refresh(Trigger::Automatic, &mut |_| {});
    eprintln!(
        "second refresh: {second:?} in {:.1}s",
        t.elapsed().as_secs_f64()
    );
    if commit == latest {
        assert_eq!(
            second,
            RefreshOutcome::AlreadyCurrent {
                commit: commit.clone()
            }
        );
    } else {
        assert!(
            matches!(second, RefreshOutcome::Skipped { .. }),
            "{second:?}"
        );
    }
    let after = std::fs::metadata(m.runtime_dir(&commit).join("runtime.json"))
        .unwrap()
        .modified()
        .unwrap();
    assert_eq!(before, after, "nothing was rebuilt");

    let offline = Manager::new(
        root.clone(),
        source("https://unreachable.invalid/stack-agent"),
    );
    let o = offline.refresh(Trigger::Automatic, &mut |_| {});
    eprintln!("offline refresh: {o:?}");
    assert!(matches!(o, RefreshOutcome::CheckFailed { .. }));
    let s = offline.status(&no_external);
    eprintln!("offline status: {:?}: {}", s.status, s.detail);
    assert_eq!(s.status, StatusKind::Stale);
    let r = offline.resolve(None, &no_external).unwrap();
    assert_eq!(r.provenance.commit.as_deref(), Some(commit.as_str()));
    eprintln!(
        "offline resolve: {}",
        serde_json::to_string_pretty(&r.provenance).unwrap()
    );
}

/// The real repository with `main`'s tip held at a given commit (one that
/// does not build), to see the walk back to the newest revision that does.
///
///   ATOMIC_RUNTIME_ROOT=/fresh/dir ATOMIC_RUNTIME_BUILD_DIR=/shared/build \
///   ATOMIC_RUNTIME_TIP=<commit> cargo test --lib live_walk_back -- --ignored --nocapture
#[test]
#[ignore]
fn live_walk_back_from_a_broken_tip() {
    struct FixedTip {
        inner: GitSource,
        tip: String,
    }
    impl Source for FixedTip {
        fn repository(&self) -> &str {
            self.inner.repository()
        }
        fn latest(&self) -> Result<String, String> {
            Ok(self.tip.clone())
        }
        fn history(&self, c: &str, n: usize, scratch: &Path) -> Result<Vec<String>, String> {
            self.inner.history(c, n, scratch)
        }
        fn prepare(
            &self,
            c: &str,
            staging: &Path,
            log: &mut dyn FnMut(&str),
        ) -> Result<Prepared, PrepareError> {
            self.inner.prepare(c, staging, log)
        }
    }
    let var = |k: &str| std::env::var(k).unwrap_or_else(|_| panic!("set {k}"));
    let root = PathBuf::from(var("ATOMIC_RUNTIME_ROOT"));
    let source = Arc::new(FixedTip {
        inner: GitSource {
            repository: REPOSITORY.to_string(),
            build_dir: PathBuf::from(var("ATOMIC_RUNTIME_BUILD_DIR")),
            path_env: std::env::var("PATH").unwrap_or_default(),
        },
        tip: var("ATOMIC_RUNTIME_TIP"),
    });
    let m = Manager::new(root, source);
    let t = std::time::Instant::now();
    let out = m.refresh(Trigger::Manual, &mut |l| {
        if !l.trim_start().starts_with("Compiling") {
            eprintln!("  | {l}")
        }
    });
    eprintln!("refresh: {out:?} in {:.1}s", t.elapsed().as_secs_f64());
    let s = m.status(&no_external);
    eprintln!("status: {:?}: {}", s.status, s.detail);
    eprintln!("failures: {:#?}", m.load_state().failures);
    assert!(
        matches!(out, RefreshOutcome::ActivatedPrevious { .. }),
        "{out:?}"
    );
    assert_eq!(s.status, StatusKind::UpdateFailed);
}
