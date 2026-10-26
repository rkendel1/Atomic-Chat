use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, Runtime, State};

use super::adapters::{self, AdapterInfo, RunConfig};
use super::executor::{self, Context};
use super::export;
use std::collections::HashMap;

use super::analysis::{self, HumanAssessment};
use super::evidence::Signal;
use super::regression::{self, RegressionRequest};
use super::regression_suite::{
    self, FailureSummary, Suite, SuiteComparison, SuiteEntry, SuiteRun, SuiteRunEntry,
    SuiteRunSummary, SUITE_RUN_SCHEMA, SUITE_SCHEMA,
};
use super::run::{RunRecord, SuiteRef, Verdict};
use super::store::Store;
use super::task::BenchTask;
use super::timeline;
use super::{BenchState, RUN_UPDATED_EVENT};
use crate::core::app::commands::get_jan_data_folder_path;

/// The ledger, after recovering any run an earlier process left in progress.
fn store<R: Runtime>(app: &AppHandle<R>, state: &BenchState) -> Store {
    let store = Store::new(&get_jan_data_folder_path(app.clone()));
    state
        .recovered
        .get_or_init(|| match store.recover_stale_runs() {
            Ok(ids) if !ids.is_empty() => {
                log::warn!(
                    "agent bench: marked {} interrupted run(s) inconclusive",
                    ids.len()
                )
            }
            Ok(_) => {}
            Err(e) => log::error!("agent bench: recovery failed: {e}"),
        });
    store
}

#[derive(Serialize)]
pub struct BenchOverview {
    pub adapters: &'static [AdapterInfo],
    pub fixtures: Vec<&'static str>,
    pub suite_id: &'static str,
    pub suite_version: u32,
    pub data_dir: String,
    pub app_version: String,
}

#[tauri::command]
pub fn bench_overview<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, BenchState>,
) -> BenchOverview {
    let store = store(&app, &state);
    BenchOverview {
        adapters: adapters::ADAPTERS,
        fixtures: super::suite::fixture_ids(),
        suite_id: super::suite::SUITE_ID,
        suite_version: super::suite::SUITE_VERSION,
        data_dir: store.root().to_string_lossy().into_owned(),
        app_version: app.package_info().version.to_string(),
    }
}

#[tauri::command]
pub fn bench_list_tasks<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, BenchState>,
) -> Result<Vec<BenchTask>, String> {
    store(&app, &state).list_tasks()
}

#[tauri::command]
pub fn bench_save_task<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, BenchState>,
    task: BenchTask,
) -> Result<BenchTask, String> {
    store(&app, &state).save_task(task)
}

#[tauri::command]
pub fn bench_delete_task<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, BenchState>,
    id: String,
) -> Result<(), String> {
    store(&app, &state).delete_task(&id)
}

/// Every run, newest first, without captured output.
#[tauri::command]
pub fn bench_list_runs<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, BenchState>,
) -> Result<Vec<RunRecord>, String> {
    Ok(store(&app, &state)
        .list_runs()?
        .iter()
        .map(RunRecord::summary)
        .collect())
}

#[tauri::command]
pub fn bench_get_run<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, BenchState>,
    id: String,
) -> Result<RunRecord, String> {
    store(&app, &state).get_run(&id)
}

/// Queues a run and returns its record at once; the run proceeds in the
/// background and emits [`RUN_UPDATED_EVENT`] on every change.
#[tauri::command]
pub async fn bench_start_run<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, BenchState>,
    task_id: String,
    config: RunConfig,
) -> Result<RunRecord, String> {
    let store = store(&app, &state);
    let task = store.get_task(&task_id)?;
    start(&app, &state, store, task, config, None).await
}

/// Starts one run of `task` (any stored version) and returns its queued record.
async fn start<R: Runtime>(
    app: &AppHandle<R>,
    state: &BenchState,
    store: Store,
    task: BenchTask,
    config: RunConfig,
    suite: Option<SuiteRef>,
) -> Result<RunRecord, String> {
    task.validate()?;
    let adapter = adapters::adapter(&config.agent_id)
        .ok_or_else(|| format!("{} has no Agent Bench adapter", config.agent_id))?;
    let app_version = app.package_info().version.to_string();
    let mut record = executor::new_record(&store, &task, adapter.id, &app_version);
    record.suite = suite;
    let evidence_dir = store.evidence_dir(&record.id);
    std::fs::create_dir_all(&evidence_dir)
        .map_err(|e| format!("could not create {}: {e}", evidence_dir.display()))?;

    // Resolution can block on a login shell the first time; keep it off the
    // async runtime. Its failure is recorded on the run, not returned.
    let bin = adapter.bin;
    let given = config.executable.clone();
    let task_for_plan = task.clone();
    let config_for_plan = config.clone();
    let (launch, path_env) = tokio::task::spawn_blocking(move || {
        let launch = adapters::resolve_executable(bin, given.as_deref()).and_then(|exe| {
            adapters::plan(&task_for_plan, &config_for_plan, &exe, Some(&evidence_dir))
        });
        (launch, adapters::child_path())
    })
    .await
    .map_err(|e| e.to_string())?;

    let cancel = tokio_util::sync::CancellationToken::new();
    state
        .active
        .lock()
        .map_err(|e| e.to_string())?
        .insert(record.id.clone(), cancel.clone());
    store.save_run(&record)?;

    let emitter = app.clone();
    let ctx = Context {
        store,
        notify: std::sync::Arc::new(move |run: &RunRecord| {
            let _ = emitter.emit(RUN_UPDATED_EVENT, run.id.clone());
        }),
        cancel,
        slot: state.slot.clone(),
        launch,
        path_env,
    };
    let returned = record.clone();
    let app_for_task = app.clone();
    tauri::async_runtime::spawn(async move {
        let id = record.id.clone();
        executor::execute(ctx, record).await;
        if let Some(state) = app_for_task.try_state::<BenchState>() {
            if let Ok(mut active) = state.active.lock() {
                active.remove(&id);
            }
        }
    });
    Ok(returned)
}

#[tauri::command]
pub fn bench_cancel_run(state: State<'_, BenchState>, id: String) -> Result<bool, String> {
    let active = state.active.lock().map_err(|e| e.to_string())?;
    match active.get(&id) {
        Some(token) => {
            token.cancel();
            Ok(true)
        }
        None => Ok(false),
    }
}

#[tauri::command]
pub fn bench_submit_review<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, BenchState>,
    id: String,
    verdict: Verdict,
    note: Option<String>,
) -> Result<RunRecord, String> {
    let store = store(&app, &state);
    let mut run = store.get_run(&id)?;
    run.apply_judgment(verdict, note.unwrap_or_default().trim().to_string())?;
    store.save_run(&run)?;
    let _ = app.emit(RUN_UPDATED_EVENT, run.id.clone());
    Ok(run)
}

/// Writes every finished run to a new JSONL file and returns its path.
#[tauri::command]
pub fn bench_export_runs<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, BenchState>,
) -> Result<String, String> {
    let store = store(&app, &state);
    let runs = store.list_runs()?;
    let path = store.exports_dir().join(format!(
        "agent-bench-runs-{}.jsonl",
        chrono::Utc::now().format("%Y%m%dT%H%M%SZ")
    ));
    super::store::write_atomic(&path, export::export_jsonl(&runs).as_bytes())?;
    Ok(path.to_string_lossy().into_owned())
}

// ---- Agent Workshop -----------------------------------------------------

#[derive(Serialize)]
pub struct RunInspection {
    pub run: RunRecord,
    /// The instruction the agent received.
    pub prompt: String,
    pub timeline: Vec<timeline::TimelineEntry>,
    /// The agent's evidence as its adapter normalized it.
    pub signals: Vec<Signal>,
}

/// A run with everything the inspector shows: its record, the prompt, a
/// chronological timeline and the normalized agent evidence.
#[tauri::command]
pub fn bench_inspect_run<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, BenchState>,
    id: String,
) -> Result<RunInspection, String> {
    let run = store(&app, &state).get_run(&id)?;
    Ok(RunInspection {
        prompt: run.task.snapshot.prompt(),
        timeline: timeline::build(&run),
        signals: analysis::run_signals(&run),
        run,
    })
}

/// Records a person's view of a cause. The rules' causes stay as they were.
#[tauri::command]
pub fn bench_add_assessment<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, BenchState>,
    id: String,
    assessment: HumanAssessment,
) -> Result<RunRecord, String> {
    let store = store(&app, &state);
    let mut run = store.get_run(&id)?;
    if !run.status.is_terminal() && !run.evaluation.awaiting_review {
        return Err("this run has not finished yet".into());
    }
    let mut a = run
        .analysis
        .clone()
        .unwrap_or_else(|| analysis::analyze(&run));
    analysis::add_human_cause(&mut a, assessment, &run);
    run.analysis = Some(a);
    store.save_run(&run)?;
    let _ = app.emit(RUN_UPDATED_EVENT, run.id.clone());
    Ok(run)
}

/// Makes a regression task from a finished run. The run and its task are
/// only read.
#[tauri::command]
pub fn bench_create_regression<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, BenchState>,
    run_id: String,
    request: RegressionRequest,
) -> Result<BenchTask, String> {
    let store = store(&app, &state);
    let run = store.get_run(&run_id)?;
    let task = regression::build(&run, &request)?;
    store.create_task(task)
}

#[derive(Serialize)]
pub struct SuiteRunView {
    pub suite_run: SuiteRun,
    pub summary: SuiteRunSummary,
}

#[tauri::command]
pub fn bench_list_suites<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, BenchState>,
) -> Result<Vec<Suite>, String> {
    store(&app, &state).list_suites()
}

#[tauri::command]
pub fn bench_create_suite<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, BenchState>,
    id: String,
    name: String,
    description: Option<String>,
) -> Result<Suite, String> {
    let store = store(&app, &state);
    if store.get_suite(&id).is_ok() {
        return Err(format!("a suite with id {id} already exists"));
    }
    if name.trim().is_empty() {
        return Err("a suite needs a name".into());
    }
    let now = super::run::now();
    let suite = Suite {
        schema: SUITE_SCHEMA.into(),
        id,
        name: name.trim().into(),
        description: description.unwrap_or_default(),
        version: 1,
        entries: vec![],
        created_at: now.clone(),
        updated_at: now,
    };
    store.save_suite(&suite)?;
    Ok(suite)
}

/// Adds (or re-pins) a task at its current version. The suite's version goes up.
#[tauri::command]
pub fn bench_add_to_suite<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, BenchState>,
    suite_id: String,
    task_id: String,
) -> Result<Suite, String> {
    let store = store(&app, &state);
    let mut suite = store.get_suite(&suite_id)?;
    let task = store.get_task(&task_id)?;
    let entry = SuiteEntry::from_task(&task);
    match suite.entries.iter_mut().find(|e| e.task_id == task_id) {
        Some(e) if e.task_version == entry.task_version && e.task_digest == entry.task_digest => {
            return Ok(suite)
        }
        Some(e) => *e = entry,
        None => suite.entries.push(entry),
    }
    suite.version += 1;
    suite.updated_at = super::run::now();
    store.save_suite(&suite)?;
    Ok(suite)
}

#[tauri::command]
pub fn bench_remove_from_suite<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, BenchState>,
    suite_id: String,
    task_id: String,
) -> Result<Suite, String> {
    let store = store(&app, &state);
    let mut suite = store.get_suite(&suite_id)?;
    let before = suite.entries.len();
    suite.entries.retain(|e| e.task_id != task_id);
    if suite.entries.len() != before {
        suite.version += 1;
        suite.updated_at = super::run::now();
        store.save_suite(&suite)?;
    }
    Ok(suite)
}

/// Runs every entry of a suite, at its pinned task version, with one agent
/// configuration. Runs are queued and execute one at a time.
#[tauri::command]
pub async fn bench_run_suite<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, BenchState>,
    suite_id: String,
    config: RunConfig,
    label: Option<String>,
) -> Result<SuiteRun, String> {
    let store = store(&app, &state);
    let suite = store.get_suite(&suite_id)?;
    if suite.entries.is_empty() {
        return Err("the suite has no tasks".into());
    }
    let mut suite_run = SuiteRun {
        schema: SUITE_RUN_SCHEMA.into(),
        id: super::store::new_suite_run_id(),
        suite_id: suite.id.clone(),
        suite_version: suite.version,
        label: label.unwrap_or_default().trim().to_string(),
        agent_id: config.agent_id.clone(),
        model: config.model.clone(),
        entries: vec![],
        created_at: super::run::now(),
    };
    for e in &suite.entries {
        let mut entry = SuiteRunEntry {
            task_id: e.task_id.clone(),
            task_version: e.task_version,
            expected: e.expected.clone(),
            run_id: None,
            not_started_reason: None,
        };
        let reference = SuiteRef {
            suite_id: suite.id.clone(),
            suite_version: suite.version,
            suite_run_id: suite_run.id.clone(),
        };
        match store.get_task_version(&e.task_id, e.task_version) {
            Ok(task) if task.digest() != e.task_digest => {
                entry.not_started_reason = Some(format!(
                    "{} v{} no longer matches what the suite pinned",
                    e.task_id, e.task_version
                ))
            }
            Ok(task) => match start(
                &app,
                &state,
                store.clone(),
                task,
                config.clone(),
                Some(reference),
            )
            .await
            {
                Ok(r) => entry.run_id = Some(r.id),
                Err(err) => entry.not_started_reason = Some(err),
            },
            Err(err) => entry.not_started_reason = Some(err),
        }
        suite_run.entries.push(entry);
        store.save_suite_run(&suite_run)?;
    }
    Ok(suite_run)
}

fn runs_by_id(store: &Store) -> Result<HashMap<String, RunRecord>, String> {
    Ok(store
        .list_runs()?
        .into_iter()
        .map(|r| (r.id.clone(), r))
        .collect())
}

#[tauri::command]
pub fn bench_list_suite_runs<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, BenchState>,
    suite_id: Option<String>,
) -> Result<Vec<SuiteRunView>, String> {
    let store = store(&app, &state);
    let runs = runs_by_id(&store)?;
    Ok(store
        .list_suite_runs(suite_id.as_deref())?
        .into_iter()
        .map(|sr| SuiteRunView {
            summary: regression_suite::summarize(&sr, &runs),
            suite_run: sr,
        })
        .collect())
}

#[tauri::command]
pub fn bench_compare_suite_runs<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, BenchState>,
    baseline_id: String,
    candidate_id: String,
) -> Result<SuiteComparison, String> {
    let store = store(&app, &state);
    let runs = runs_by_id(&store)?;
    let base = store.get_suite_run(&baseline_id)?;
    let cand = store.get_suite_run(&candidate_id)?;
    Ok(regression_suite::compare(&base, &cand, &runs))
}

/// Unsuccessful runs grouped by primary cause, optionally for one agent.
#[tauri::command]
pub fn bench_failure_summary<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, BenchState>,
    agent_id: Option<String>,
) -> Result<FailureSummary, String> {
    let runs = store(&app, &state).list_runs()?;
    Ok(regression_suite::failure_summary(
        &runs,
        agent_id.as_deref(),
    ))
}
