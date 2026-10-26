//! Agent Workshop runs on the stack-agent runtime manager: a run gets its
//! executable from the manager through the same `prepare_run` the app uses,
//! records the runtime it actually launched, keeps it through a refresh, and
//! stays attributable in regressions, suites and the export. The "builds" are
//! shell scripts that speak stack-agent's interface and write the commit they
//! were built from into the project, so a test can see what really ran.
#![cfg(unix)]

use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::{Arc, Mutex};

use serde_json::Value;
use tokio_util::sync::CancellationToken;

use super::adapters::RunConfig;
use super::commands::{pin_suite_runtime, prepare_run};
use super::executor::{self, Context};
use super::export;
use super::regression::{self, RegressionRequest};
use super::regression_suite::{self, SuiteRun, SuiteRunEntry, SUITE_RUN_SCHEMA};
use super::run::*;
use super::store::Store;
use super::task::*;
use crate::core::stack_agent_runtime::{
    sha256_file, Manager, PrepareError, Prepared, Selection, Source, Trigger, BIN_NAME,
    KEEP_PREVIOUS, REQUIRED_WORK_FLAGS,
};

const A: &str = "1111111111111111111111111111111111111111";
const B: &str = "2222222222222222222222222222222222222222";

/// Builds a stack-agent for `commit`. A commit in `hold` waits in `work`
/// until the project has a `release` file (after writing `started`).
struct RevisionSource {
    latest: Mutex<String>,
    hold: Mutex<Vec<String>>,
}

const TRACE: &str = r#"{"events":[{"attempt":1,"index":0,"recorded_at_ms":1793038760698,"provenance":"runtime_decided","event":{"kind":"WorkStarted","max_turns":4,"max_executions":8}},{"attempt":1,"index":1,"recorded_at_ms":1793038760800,"provenance":"host_observed","event":{"kind":"ExecutionCompleted","execution_id":"e1"}},{"attempt":1,"index":2,"recorded_at_ms":1793038761000,"provenance":"runtime_decided","event":{"kind":"WorkCompleted"}}]}"#;
const REPORT: &str = r#"{"terminal_state":"completed","verified":true,"outcome_reason":"done","executions_by_capability":{},"lists":0,"searches":0,"reads":0,"invalid_decisions":0,"malformed_replies":0,"model_failures":[],"audit":{},"evidence":{"invalidated":null,"test_files_replaced":[]},"agent_state":{"recorded":true,"task":{"task_id":"work_fake"}}}"#;

impl Source for RevisionSource {
    fn repository(&self) -> &str {
        "https://example.invalid/stack-agent"
    }
    fn latest(&self) -> Result<String, String> {
        Ok(self.latest.lock().unwrap().clone())
    }
    fn history(&self, _: &str, _: usize, _: &Path) -> Result<Vec<String>, String> {
        Ok(vec![])
    }
    fn prepare(
        &self,
        commit: &str,
        staging: &Path,
        _log: &mut dyn FnMut(&str),
    ) -> Result<Prepared, PrepareError> {
        let hold = if self.hold.lock().unwrap().iter().any(|c| c == commit) {
            "touch started; while [ ! -f release ]; do sleep 0.05; done"
        } else {
            ""
        };
        let bin = staging.join("bin").join(BIN_NAME);
        std::fs::create_dir_all(bin.parent().unwrap()).unwrap();
        let script = format!(
            r#"#!/bin/sh
case "$1" in
  --version) echo "stack-agent 0.1.0"; exit 0;;
  --test) echo "Turn completed"; exit 0;;
  agent-state)
    case "$*" in *"--state-db"*"--task-id work_fake"*"--json"*) ;; *) echo "error: bad show" >&2; exit 2;; esac
    echo '{TRACE}'; exit 0;;
esac
if [ "$2" = "--help" ]; then echo "{flags}"; exit 0; fi
[ "$1" = "work" ] || exit 2
case "$*" in *"--print-reply"*"--state-db"*) ;; *) echo "error: expected --print-reply and --state-db" >&2; exit 2;; esac
{hold}
printf 'Model reply:\n%s\n' '{{"decision":"complete"}}' >&2
echo {commit} > revision.txt
echo '{REPORT}'
"#,
            flags = REQUIRED_WORK_FLAGS.join(" ")
        );
        std::fs::write(&bin, script).unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        Ok(Prepared {
            binary: bin,
            toolchain: Some("rustc 1.97.0 (fake)".into()),
        })
    }
}

struct Env {
    data: tempfile::TempDir,
    store: Store,
    source: Arc<RevisionSource>,
    manager: Arc<Manager>,
    listener: std::net::TcpListener,
}

fn env() -> Env {
    let data = tempfile::tempdir().unwrap();
    let source = Arc::new(RevisionSource {
        latest: Mutex::new(A.into()),
        hold: Mutex::new(vec![]),
    });
    let manager = Arc::new(Manager::new(data.path().join("runtime"), source.clone()));
    Env {
        store: Store::new(data.path()),
        data,
        source,
        manager,
        listener: std::net::TcpListener::bind("127.0.0.1:0").unwrap(),
    }
}

/// A fresh copy of a bundled fixture per run; the check is that the agent
/// wrote its revision there.
fn fixture_task(id: &str) -> BenchTask {
    BenchTask {
        schema: TASK_SCHEMA.to_string(),
        id: id.to_string(),
        version: 1,
        name: "Write the revision".to_string(),
        description: String::new(),
        goal: "Write revision.txt".to_string(),
        workdir: WorkdirRef::Fixture {
            fixture: "ledger".into(),
        },
        success_criteria: vec!["revision.txt exists".to_string()],
        evaluation: Evaluation::Command {
            commands: vec![VerificationCommand {
                name: "ran".into(),
                program: "test".into(),
                args: vec!["-f".into(), "revision.txt".into()],
                timeout_secs: Some(30),
            }],
            held_out_checks: None,
        },
        limits: Limits {
            timeout_secs: Some(60),
            max_turns: Some(4),
        },
        builtin: false,
        created_at: String::new(),
        updated_at: String::new(),
        origin: None,
        expected: None,
    }
}

impl Env {
    fn refresh_to(&self, commit: &str) {
        *self.source.latest.lock().unwrap() = commit.into();
        self.manager.refresh(Trigger::Manual, &mut |_| {});
    }

    fn config(&self, executable: Option<String>) -> RunConfig {
        RunConfig {
            agent_id: "stack-agent".into(),
            model: Some("m".into()),
            api_url: Some(format!("http://{}/v1", self.listener.local_addr().unwrap())),
            api_key: None,
            executable,
        }
    }

    /// What `start` does, minus the Tauri plumbing: prepare through the
    /// runtime manager, then execute with the lease held.
    async fn run(&self, task: &BenchTask, config: RunConfig, suite: Option<SuiteRef>) -> RunRecord {
        let p = prepare_run(
            &self.store,
            task,
            &config,
            "9.9.9",
            suite,
            &self.manager,
            &|| None,
        )
        .unwrap();
        let ctx = Context {
            store: self.store.clone(),
            notify: Arc::new(|_: &RunRecord| {}),
            cancel: CancellationToken::new(),
            slot: Arc::new(tokio::sync::Semaphore::new(1)),
            launch: p.launch,
            path_env: std::env::var("PATH").unwrap_or_default(),
        };
        let _lease = p.lease;
        executor::execute(ctx, p.record).await
    }
}

fn revision_written(run: &RunRecord) -> String {
    std::fs::read_to_string(Path::new(&run.workdir.path).join("revision.txt"))
        .unwrap()
        .trim()
        .to_string()
}

#[tokio::test]
async fn a_workshop_run_launches_the_managed_runtime_it_records() {
    let e = env();
    e.refresh_to(A);
    let run = e.run(&fixture_task("t"), e.config(None), None).await;
    assert_eq!(run.status, RunStatus::Succeeded, "{:?}", run.errors);

    let rt = run.agent.runtime.clone().expect("runtime recorded");
    assert_eq!(rt.source, "managed");
    assert_eq!(rt.selection, "follow_main");
    assert_eq!(rt.commit.as_deref(), Some(A));
    // The recorded commit, path and digest are those of what ran.
    assert_eq!(revision_written(&run), A);
    assert_eq!(run.agent.executable.as_deref(), Some(rt.path.as_str()));
    assert_eq!(
        sha256_file(Path::new(&rt.path)).unwrap(),
        rt.binary_sha256.clone().unwrap()
    );
    assert!(Path::new(&rt.path).starts_with(e.manager.runtime_dir(A)));

    // The Workshop's evidence is all still there.
    let trace = run.agent_trace.as_ref().expect("trace collected");
    assert!(trace.available, "{:?}", trace.unavailable_reason);
    assert_eq!(trace.events.len(), 3);
    assert_eq!(trace.replies.len(), 1);
    let baseline = run.baseline.as_ref().expect("baseline");
    assert!(baseline.fresh_workspace);
    assert!(baseline.git_commit.is_some());
    assert!(run.analysis.is_some());
    assert!(run.config.contains_key("state_db"));
}

#[tokio::test]
async fn a_refresh_during_a_run_does_not_change_or_remove_its_runtime() {
    let e = env();
    e.source.hold.lock().unwrap().push(A.into());
    e.refresh_to(A);
    let task = fixture_task("t");
    let p = prepare_run(
        &e.store,
        &task,
        &e.config(None),
        "9.9.9",
        None,
        &e.manager,
        &|| None,
    )
    .unwrap();
    let workdir = std::path::PathBuf::from(&p.record.workdir.path);
    let ctx = Context {
        store: e.store.clone(),
        notify: Arc::new(|_: &RunRecord| {}),
        cancel: CancellationToken::new(),
        slot: Arc::new(tokio::sync::Semaphore::new(1)),
        launch: p.launch,
        path_env: std::env::var("PATH").unwrap_or_default(),
    };
    let lease = p.lease;
    let running = tokio::spawn(async move {
        let _lease = lease;
        executor::execute(ctx, p.record).await
    });
    while !workdir.join("started").exists() {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }

    // main moves on, more times than rollback keeps, while A is running.
    let later: Vec<String> = (3..=3 + KEEP_PREVIOUS as u8)
        .map(|i| char::from(b'0' + i).to_string().repeat(40))
        .collect();
    let manager = e.manager.clone();
    let source = e.source.clone();
    let later_for_refresh = later.clone();
    tokio::task::spawn_blocking(move || {
        for c in std::iter::once(B.to_string()).chain(later_for_refresh) {
            *source.latest.lock().unwrap() = c;
            manager.refresh(Trigger::Automatic, &mut |_| {});
        }
    })
    .await
    .unwrap();
    assert_eq!(
        e.manager.load_state().active.as_deref(),
        later.last().map(String::as_str)
    );
    assert!(
        e.manager.verify_cached(A).is_ok(),
        "the running run's runtime is leased, not pruned"
    );

    std::fs::write(workdir.join("release"), "").unwrap();
    let run = running.await.unwrap();
    assert_eq!(run.status, RunStatus::Succeeded, "{:?}", run.errors);
    assert_eq!(revision_written(&run), A);
    assert_eq!(run.agent.runtime.unwrap().commit.as_deref(), Some(A));

    // Once the run is over, A can go.
    e.manager.prune();
    assert!(!e.manager.runtime_dir(A).exists());
    // And the next run uses the new main.
    let next = e.run(&fixture_task("t"), e.config(None), None).await;
    assert_eq!(revision_written(&next), *later.last().unwrap());
}

#[tokio::test]
async fn an_executable_changed_after_resolution_is_never_launched() {
    let e = env();
    e.refresh_to(A);
    let task = fixture_task("t");
    let p = prepare_run(
        &e.store,
        &task,
        &e.config(None),
        "9.9.9",
        None,
        &e.manager,
        &|| None,
    )
    .unwrap();
    let path = p.record.agent.runtime.as_ref().unwrap().path.clone();
    std::fs::write(&path, "#!/bin/sh\necho tampered > revision.txt\n").unwrap();
    let ctx = Context {
        store: e.store.clone(),
        notify: Arc::new(|_: &RunRecord| {}),
        cancel: CancellationToken::new(),
        slot: Arc::new(tokio::sync::Semaphore::new(1)),
        launch: p.launch,
        path_env: std::env::var("PATH").unwrap_or_default(),
    };
    let run = executor::execute(ctx, p.record).await;
    assert_eq!(run.status, RunStatus::Failed);
    assert_eq!(run.failure_kind, Some(FailureKind::Execution));
    assert!(
        run.execution
            .as_ref()
            .and_then(|x| x.error.clone())
            .unwrap_or_default()
            .contains("changed after it was resolved"),
        "{:?}",
        run.execution
    );
    assert!(!Path::new(&run.workdir.path).join("revision.txt").exists());
}

#[tokio::test]
async fn a_pinned_historical_revision_reproduces_after_main_moves() {
    let e = env();
    e.refresh_to(A);
    let first = e.run(&fixture_task("t"), e.config(None), None).await;
    e.refresh_to(B);
    let second = e.run(&fixture_task("t"), e.config(None), None).await;
    assert_eq!(revision_written(&second), B);
    // The earlier record still names A.
    let stored = e.store.get_run(&first.id).unwrap();
    assert_eq!(stored.agent.runtime.unwrap().commit.as_deref(), Some(A));

    e.manager
        .select(Selection::Pinned { commit: A.into() })
        .unwrap();
    let again = e.run(&fixture_task("t"), e.config(None), None).await;
    assert_eq!(revision_written(&again), A);
    let rt = again.agent.runtime.unwrap();
    assert_eq!(rt.selection, "pinned");
    assert_eq!(rt.matches_latest, Some(false));
}

#[tokio::test]
async fn a_suite_run_keeps_one_runtime_and_regressions_and_suites_still_work() {
    let e = env();
    e.refresh_to(A);
    // Reproduce: a regression task from a run, then a suite of it.
    let source_run = e.run(&fixture_task("t"), e.config(None), None).await;
    let reg = e
        .store
        .create_task(
            regression::build(
                &source_run,
                &RegressionRequest {
                    id: "writes-revision".into(),
                    name: None,
                    failure_description: "Check the revision is written.".into(),
                    proposed_improvement: None,
                    expected_outcome: None,
                },
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(
        reg.origin.as_ref().unwrap().source_run_id,
        source_run.id,
        "the regression names its source run"
    );
    let pinned_task = e.store.get_task_version(&reg.id, reg.version).unwrap();

    // A suite run resolves its runtime once; a refresh mid-suite does not
    // move later entries.
    let config = pin_suite_runtime(e.config(None), &e.manager, &|| None);
    assert!(config.executable.as_deref().unwrap().contains(&format!(
        "runtimes/{}/{A}",
        crate::core::stack_agent_runtime::platform()
    )));
    let suite_ref = |id: &str| SuiteRef {
        suite_id: "regressions".into(),
        suite_version: 1,
        suite_run_id: id.into(),
    };
    let one = e
        .run(&pinned_task, config.clone(), Some(suite_ref("suiterun_a")))
        .await;
    e.refresh_to(B);
    let two = e
        .run(&pinned_task, config.clone(), Some(suite_ref("suiterun_a")))
        .await;
    for r in [&one, &two] {
        assert_eq!(r.status, RunStatus::Succeeded, "{:?}", r.errors);
        assert_eq!(revision_written(r), A);
        let rt = r.agent.runtime.as_ref().unwrap();
        assert_eq!(
            rt.source, "managed",
            "a pinned suite executable stays managed"
        );
        assert_eq!(rt.commit.as_deref(), Some(A));
        assert_eq!(r.suite.as_ref().unwrap().suite_run_id, "suiterun_a");
    }
    // A later suite run (following main) is on B; the comparison says so.
    let later_config = pin_suite_runtime(e.config(None), &e.manager, &|| None);
    let three = e
        .run(&pinned_task, later_config, Some(suite_ref("suiterun_b")))
        .await;
    assert_eq!(revision_written(&three), B);

    let suite_run = |id: &str, run: &RunRecord| SuiteRun {
        schema: SUITE_RUN_SCHEMA.into(),
        id: id.into(),
        suite_id: "regressions".into(),
        suite_version: 1,
        label: id.into(),
        agent_id: "stack-agent".into(),
        model: Some("m".into()),
        entries: vec![SuiteRunEntry {
            task_id: reg.id.clone(),
            task_version: reg.version,
            expected: "success".into(),
            run_id: Some(run.id.clone()),
            not_started_reason: None,
        }],
        created_at: now(),
    };
    let runs: HashMap<String, RunRecord> = [&one, &three]
        .iter()
        .map(|r| (r.id.clone(), (*r).clone()))
        .collect();
    let cmp = regression_suite::compare(
        &suite_run("suiterun_a", &one),
        &suite_run("suiterun_b", &three),
        &runs,
    );
    assert_eq!(cmp.baseline.verified_successes, 1);
    assert_eq!(cmp.candidate.verified_successes, 1);
    let w = cmp
        .warnings
        .iter()
        .find(|w| w.field == "agent_revision")
        .expect("revision difference is a warning");
    assert_eq!((w.baseline.as_str(), w.candidate.as_str()), (A, B));
    assert!(!cmp.directly_comparable);
}

#[test]
fn unknown_runtime_provenance_is_never_comparable_and_other_agents_are_not_asked() {
    let data = tempfile::tempdir().unwrap();
    let store = Store::new(data.path());
    let task = fixture_task("t");
    let finished = |agent: &str, commit: Option<&str>| {
        let mut r = executor::new_record(&store, &task, agent, "9.9.9");
        r.agent.runtime = commit.map(|c| crate::core::stack_agent_runtime::RuntimeProvenance {
            source: "managed".into(),
            selection: "follow_main".into(),
            path: "/x".into(),
            commit: Some(c.into()),
            version: Some("0.1.0".into()),
            platform: "x".into(),
            binary_sha256: None,
            repository: None,
            toolchain: None,
            prepared_at: None,
            latest_known_main: None,
            latest_checked_at: None,
            matches_latest: None,
        });
        r.transition(RunStatus::Running).unwrap();
        r.transition(RunStatus::Inconclusive).unwrap();
        r
    };
    let sr = |id: &str, run: &RunRecord| SuiteRun {
        schema: SUITE_RUN_SCHEMA.into(),
        id: id.into(),
        suite_id: "s".into(),
        suite_version: 1,
        label: String::new(),
        agent_id: run.agent.id.clone(),
        model: None,
        entries: vec![SuiteRunEntry {
            task_id: "t".into(),
            task_version: 1,
            expected: "success".into(),
            run_id: Some(run.id.clone()),
            not_started_reason: None,
        }],
        created_at: now(),
    };
    let revision_warning = |a: &RunRecord, b: &RunRecord| {
        let runs: HashMap<String, RunRecord> = [a, b]
            .iter()
            .map(|r| (r.id.clone(), (*r).clone()))
            .collect();
        regression_suite::compare(&sr("a", a), &sr("b", b), &runs)
            .warnings
            .into_iter()
            .find(|w| w.field == "agent_revision")
    };
    // Same known revision: no warning. Unknown on either side: a warning.
    let known = finished("stack-agent", Some(A));
    assert!(revision_warning(&known, &finished("stack-agent", Some(A))).is_none());
    let legacy = finished("stack-agent", None);
    let w = revision_warning(&known, &legacy).unwrap();
    assert_eq!(w.candidate, "unknown");
    assert!(revision_warning(&legacy, &finished("stack-agent", None)).is_some());
    // Claude Code has no managed runtime: never "unknown".
    let claude = finished("claude-code", None);
    assert!(revision_warning(&claude, &finished("claude-code", None)).is_none());

    // Export: unknown only where a stack-agent run lacks it.
    let unknown = |r: &RunRecord| export::export_line(r)["unknown"].clone();
    let has = |v: Value, k: &str| v.as_array().unwrap().contains(&Value::from(k));
    assert!(has(unknown(&legacy), "observed.agent_revision"));
    assert!(!has(unknown(&known), "observed.agent_revision"));
    assert!(!has(unknown(&claude), "observed.agent_revision"));
    assert!(!has(unknown(&claude), "observed.agent_runtime"));
    assert!(!serde_json::to_string(&claude)
        .unwrap()
        .contains("\"runtime\""));
}

#[tokio::test]
async fn the_export_keeps_workshop_sections_and_adds_the_runtime() {
    let e = env();
    e.refresh_to(A);
    let run = e.run(&fixture_task("t"), e.config(None), None).await;
    let line: Value =
        serde_json::from_str(export::export_jsonl(std::slice::from_ref(&run)).trim()).unwrap();
    assert_eq!(line["schema"], export::EXPORT_SCHEMA);
    assert_eq!(line["observed"]["agent_revision"], A);
    assert_eq!(line["observed"]["agent_runtime"]["source"], "managed");
    assert_eq!(
        line["observed"]["agent_runtime"]["binary_sha256"],
        run.agent
            .runtime
            .as_ref()
            .unwrap()
            .binary_sha256
            .clone()
            .unwrap()
    );
    for key in [
        "analysis",
        "reproducibility",
        "agent_trace",
        "evidence_gaps",
    ] {
        assert!(line.get(key).is_some(), "{key}");
    }
    assert!(line["agent_trace"]["available"].as_bool().unwrap());
    // Nothing from the data folder's credentials or the runtime state leaks.
    assert!(!line.to_string().contains("sk-"));
    drop(e.data);
}

#[test]
fn a_record_written_before_runtime_provenance_still_loads() {
    let data = tempfile::tempdir().unwrap();
    let store = Store::new(data.path());
    let run = executor::new_record(&store, &fixture_task("t"), "stack-agent", "9.9.9");
    let mut v = serde_json::to_value(&run).unwrap();
    v["agent"].as_object_mut().unwrap().remove("runtime");
    let back: RunRecord = serde_json::from_value(v).unwrap();
    assert_eq!(back.agent.runtime, None);
}
