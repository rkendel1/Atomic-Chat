//! Tauri commands, the background refresh, and the accessors the Launch page
//! and Agent Bench use to reach the one runtime manager.

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use tauri::{AppHandle, Emitter, Manager as _, Runtime};

use super::{
    GitSource, Manager, RefreshOutcome, Resolved, RuntimeStatus, Selection, Trigger, REPOSITORY,
};
use crate::core::app::commands::get_jan_data_folder_path;

/// Emitted (without payload) whenever the runtime status may have changed.
pub const STATUS_EVENT: &str = "stack_agent_runtime:status";
/// Emitted with one line of refresh or build output.
pub const LOG_EVENT: &str = "stack_agent_runtime:log";

/// The first automatic check waits for the app to settle.
const FIRST_CHECK_DELAY: Duration = Duration::from_secs(20);
const CHECK_EVERY: Duration = Duration::from_secs(6 * 60 * 60);

#[derive(Default)]
pub struct RuntimeState {
    manager: OnceLock<Arc<Manager>>,
}

pub fn manager<R: Runtime>(app: &AppHandle<R>) -> Arc<Manager> {
    let state = app.state::<RuntimeState>();
    state
        .manager
        .get_or_init(|| {
            let root = get_jan_data_folder_path(app.clone()).join("stack-agent-runtime");
            let source = GitSource {
                repository: REPOSITORY.to_string(),
                build_dir: root.join("build").join(super::platform()),
                path_env: crate::core::agent_bench::adapters::child_path(),
            };
            Arc::new(Manager::new(root, Arc::new(source)))
        })
        .clone()
}

/// An unmanaged `stack-agent` on the login-shell PATH or where its installers
/// put it. Used only when no managed runtime exists.
pub fn find_external() -> Option<PathBuf> {
    crate::core::agent_bench::adapters::search_path(
        "stack-agent",
        &crate::core::agent_bench::adapters::child_path(),
    )
    .or_else(|| crate::core::system::commands::resolve_off_path("stack-agent"))
}

/// Resolves the executable for a run or a terminal. Blocking.
pub fn resolve<R: Runtime>(
    app: &AppHandle<R>,
    custom_path: Option<&str>,
) -> Result<Resolved, String> {
    manager(app).resolve(custom_path, &find_external)
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Result<T, String> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| e.to_string())
}

/// Runs a refresh, streaming its lines to [`LOG_EVENT`] and `extra`.
pub async fn refresh_now<R: Runtime>(
    app: &AppHandle<R>,
    trigger: Trigger,
    extra_event: Option<String>,
) -> Result<RefreshOutcome, String> {
    let m = manager(app);
    let emitter = app.clone();
    let _ = app.emit(STATUS_EVENT, ());
    let ticker = spawn_status_ticker(app.clone());
    let outcome = blocking(move || {
        let _ = m.enable();
        let mut log = |line: &str| {
            let _ = emitter.emit(LOG_EVENT, line.to_string());
            if let Some(ev) = &extra_event {
                let _ = emitter.emit(ev, line.to_string());
            }
        };
        m.refresh(trigger, &mut log)
    })
    .await;
    ticker.abort();
    let _ = app.emit(STATUS_EVENT, ());
    if let Ok(o) = &outcome {
        log::info!("stack-agent runtime refresh ({trigger:?}): {o:?}");
    }
    outcome
}

/// Nudges the UI while a long step runs so progress lines show.
fn spawn_status_ticker<R: Runtime>(app: AppHandle<R>) -> tauri::async_runtime::JoinHandle<()> {
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(2)).await;
            let _ = app.emit(STATUS_EVENT, ());
        }
    })
}

/// Checks `main` shortly after start and then periodically, once someone has
/// prepared a runtime. Never blocks the interface.
pub fn spawn_background_refresh<R: Runtime>(app: AppHandle<R>) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(FIRST_CHECK_DELAY).await;
        loop {
            if manager(&app).enabled() {
                let _ = refresh_now(&app, Trigger::Automatic, None).await;
            }
            tokio::time::sleep(CHECK_EVERY).await;
        }
    });
}

#[tauri::command]
pub async fn stack_agent_runtime_status<R: Runtime>(
    app: AppHandle<R>,
) -> Result<RuntimeStatus, String> {
    let m = manager(&app);
    blocking(move || m.status(&find_external)).await
}

/// Starts a manual refresh in the background and returns at once; progress
/// arrives as [`STATUS_EVENT`] and [`LOG_EVENT`].
#[tauri::command]
pub async fn stack_agent_runtime_refresh<R: Runtime>(
    app: AppHandle<R>,
) -> Result<RuntimeStatus, String> {
    let m = manager(&app);
    if !m.is_refreshing() {
        let bg = app.clone();
        tauri::async_runtime::spawn(async move {
            let _ = refresh_now(&bg, Trigger::Manual, None).await;
        });
        // Let the refresh mark itself as running before reporting status.
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
    blocking(move || m.status(&find_external)).await
}

/// Prepares one exact revision (for reproducing a recorded run) in the
/// background without activating it.
#[tauri::command]
pub async fn stack_agent_runtime_prepare_revision<R: Runtime>(
    app: AppHandle<R>,
    commit: String,
) -> Result<RuntimeStatus, String> {
    let commit = commit.trim().to_ascii_lowercase();
    if !super::is_full_commit(&commit) {
        return Err("Give the full 40-character commit id the run recorded.".into());
    }
    let m = manager(&app);
    if m.is_refreshing() {
        return Err("Another runtime refresh is in progress; try again when it ends.".into());
    }
    let bg = app.clone();
    let mb = m.clone();
    tauri::async_runtime::spawn(async move {
        let ticker = spawn_status_ticker(bg.clone());
        let emitter = bg.clone();
        let result = blocking(move || {
            let mut log = |line: &str| {
                let _ = emitter.emit(LOG_EVENT, line.to_string());
            };
            mb.prepare_revision(&commit, &mut log)
        })
        .await;
        ticker.abort();
        if let Ok(Err(e)) = result {
            let _ = bg.emit(LOG_EVENT, format!("Preparing the revision failed: {e}"));
        }
        let _ = bg.emit(STATUS_EVENT, ());
    });
    tokio::time::sleep(Duration::from_millis(150)).await;
    blocking(move || m.status(&find_external)).await
}

#[tauri::command]
pub async fn stack_agent_runtime_select<R: Runtime>(
    app: AppHandle<R>,
    selection: Selection,
) -> Result<RuntimeStatus, String> {
    let m = manager(&app);
    let status = blocking(move || {
        m.select(selection)?;
        Ok::<_, String>(m.status(&find_external))
    })
    .await??;
    let _ = app.emit(STATUS_EVENT, ());
    Ok(status)
}

#[tauri::command]
pub async fn stack_agent_runtime_rollback<R: Runtime>(
    app: AppHandle<R>,
) -> Result<RuntimeStatus, String> {
    let m = manager(&app);
    let status = blocking(move || {
        m.rollback()?;
        Ok::<_, String>(m.status(&find_external))
    })
    .await??;
    let _ = app.emit(STATUS_EVENT, ());
    Ok(status)
}
