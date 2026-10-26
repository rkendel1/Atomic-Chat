use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, Runtime, State};

use super::adapters::{self, AdapterInfo, RunConfig};
use super::executor::{self, Context};
use super::export;
use super::run::{RunRecord, Verdict};
use super::store::Store;
use super::task::BenchTask;
use super::{BenchState, RUN_UPDATED_EVENT};
use crate::core::app::commands::get_jan_data_folder_path;
use crate::core::stack_agent_runtime;

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
    task.validate()?;
    let adapter = adapters::adapter(&config.agent_id)
        .ok_or_else(|| format!("{} has no Agent Bench adapter", config.agent_id))?;
    let app_version = app.package_info().version.to_string();
    let mut record = executor::new_record(&store, &task, adapter.id, &app_version);

    // Resolution can block on a login shell the first time; keep it off the
    // async runtime. Its failure is recorded on the run, not returned.
    // stack-agent's executable comes from the runtime manager, once, here: the
    // run keeps that exact runtime (and a lease on it) until it ends.
    let bin = adapter.bin;
    let given = config.executable.clone();
    let task_for_plan = task.clone();
    let config_for_plan = config.clone();
    let runtime_app = app.clone();
    let (launch, path_env, runtime) = tokio::task::spawn_blocking(move || {
        let (exe, runtime) = if bin == "stack-agent" {
            match stack_agent_runtime::commands::manager(&runtime_app).resolve_for_run(
                given.as_deref(),
                &stack_agent_runtime::commands::find_external,
            ) {
                Ok((r, lease)) => (Ok(r.path), Some((r.provenance, lease))),
                Err(e) => (Err(e), None),
            }
        } else {
            (adapters::resolve_executable(bin, given.as_deref()), None)
        };
        let launch = exe.and_then(|exe| adapters::plan(&task_for_plan, &config_for_plan, &exe));
        (launch, adapters::child_path(), runtime)
    })
    .await
    .map_err(|e| e.to_string())?;
    let (provenance, lease) = runtime.unzip();
    record.agent.runtime = provenance;

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
        let _lease = lease;
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
