//! The durable task and run ledger: one JSON file per task and per run under
//! `<Atomic Chat data folder>/agent_bench/`, the same file-per-record layout
//! desktop threads use. Every write goes to a temporary file that is then
//! renamed over the record, so a crash never leaves half a record behind.

use std::path::{Path, PathBuf};

use super::run::{RunRecord, RUN_SCHEMA};
use super::suite;
use super::task::{is_valid_id, BenchTask, TASK_SCHEMA};

pub const BENCH_DIR: &str = "agent_bench";

#[derive(Debug, Clone)]
pub struct Store {
    root: PathBuf,
}

impl Store {
    pub fn new(data_folder: &Path) -> Self {
        Self {
            root: data_folder.join(BENCH_DIR),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
    fn tasks_dir(&self) -> PathBuf {
        self.root.join("tasks")
    }
    fn runs_dir(&self) -> PathBuf {
        self.root.join("runs")
    }
    pub fn workspaces_dir(&self) -> PathBuf {
        self.root.join("workspaces")
    }
    pub fn exports_dir(&self) -> PathBuf {
        self.root.join("exports")
    }

    // ---- tasks ----------------------------------------------------------

    /// The bundled tasks followed by the saved ones, by name.
    pub fn list_tasks(&self) -> Result<Vec<BenchTask>, String> {
        let mut saved: Vec<BenchTask> = read_all(&self.tasks_dir())?
            .into_iter()
            .filter(|t: &BenchTask| t.schema == TASK_SCHEMA)
            .collect();
        saved.sort_by_key(|t| t.name.to_lowercase());
        let mut out = suite::builtin_tasks();
        out.extend(saved);
        Ok(out)
    }

    pub fn get_task(&self, id: &str) -> Result<BenchTask, String> {
        if let Some(t) = suite::builtin_tasks().into_iter().find(|t| t.id == id) {
            return Ok(t);
        }
        if !is_valid_id(id) {
            return Err(format!("invalid task id {id:?}"));
        }
        read_json(&self.tasks_dir().join(format!("{id}.json")))
    }

    /// Saves a task. A changed task gets the next version; saving the same
    /// content again keeps its version. Bundled tasks are read-only.
    pub fn save_task(&self, mut task: BenchTask) -> Result<BenchTask, String> {
        if suite::builtin_tasks().iter().any(|b| b.id == task.id) {
            return Err(format!(
                "{} is a bundled task and cannot be changed; duplicate it instead",
                task.id
            ));
        }
        task.builtin = false;
        task.schema = TASK_SCHEMA.to_string();
        if task.version == 0 {
            task.version = 1;
        }
        task.validate()?;
        let path = self.tasks_dir().join(format!("{}.json", task.id));
        let now = super::run::now();
        match read_json::<BenchTask>(&path) {
            Ok(existing) => {
                task.created_at = existing.created_at.clone();
                if existing.digest() == task.digest() {
                    task.version = existing.version;
                    task.updated_at = existing.updated_at.clone();
                } else {
                    task.version = existing.version + 1;
                    task.updated_at = now;
                }
            }
            Err(_) => {
                task.version = 1;
                task.created_at = now.clone();
                task.updated_at = now;
            }
        }
        write_json(&path, &task)?;
        Ok(task)
    }

    pub fn delete_task(&self, id: &str) -> Result<(), String> {
        if suite::builtin_tasks().iter().any(|b| b.id == id) {
            return Err(format!("{id} is a bundled task and cannot be deleted"));
        }
        if !is_valid_id(id) {
            return Err(format!("invalid task id {id:?}"));
        }
        let path = self.tasks_dir().join(format!("{id}.json"));
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(format!("could not delete {}: {e}", path.display())),
        }
    }

    // ---- runs -----------------------------------------------------------

    /// Every recorded run, newest first.
    pub fn list_runs(&self) -> Result<Vec<RunRecord>, String> {
        let mut runs: Vec<RunRecord> = read_all(&self.runs_dir())?
            .into_iter()
            .filter(|r: &RunRecord| r.schema == RUN_SCHEMA)
            .collect();
        runs.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        Ok(runs)
    }

    pub fn get_run(&self, id: &str) -> Result<RunRecord, String> {
        if !is_valid_run_id(id) {
            return Err(format!("invalid run id {id:?}"));
        }
        read_json(&self.runs_dir().join(format!("{id}.json")))
    }

    pub fn save_run(&self, run: &RunRecord) -> Result<(), String> {
        if !is_valid_run_id(&run.id) {
            return Err(format!("invalid run id {:?}", run.id));
        }
        write_json(&self.runs_dir().join(format!("{}.json", run.id)), run)
    }

    /// Marks every run left in progress by an earlier process as
    /// inconclusive. Call once per process, before any run starts.
    pub fn recover_stale_runs(&self) -> Result<Vec<String>, String> {
        let mut recovered = Vec::new();
        for mut run in self.list_runs()? {
            if run.recover_if_stale() {
                self.save_run(&run)?;
                recovered.push(run.id.clone());
            }
        }
        Ok(recovered)
    }
}

pub fn is_valid_run_id(id: &str) -> bool {
    id.starts_with("run_")
        && id.len() <= 64
        && id[4..]
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

pub fn new_run_id() -> String {
    format!(
        "run_{}_{}",
        chrono::Utc::now().format("%Y%m%dT%H%M%S"),
        &uuid::Uuid::new_v4().simple().to_string()[..8]
    )
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("could not read {}: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("could not parse {}: {e}", path.display()))
}

/// Every `*.json` record in `dir` that parses; a damaged file is logged and
/// skipped so one bad record never hides the rest.
fn read_all<T: serde::de::DeserializeOwned>(dir: &Path) -> Result<Vec<T>, String> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("could not read {}: {e}", dir.display())),
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        match read_json(&path) {
            Ok(v) => out.push(v),
            Err(e) => log::warn!("agent bench: skipping {e}"),
        }
    }
    Ok(out)
}

pub(crate) fn write_json<T: serde::Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let text = serde_json::to_string_pretty(value).map_err(|e| e.to_string())?;
    write_atomic(path, text.as_bytes())
}

pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let dir = path
        .parent()
        .ok_or_else(|| format!("{} has no parent directory", path.display()))?;
    std::fs::create_dir_all(dir).map_err(|e| format!("could not create {}: {e}", dir.display()))?;
    let tmp = dir.join(format!(
        ".{}.{}.tmp",
        path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("record"),
        uuid::Uuid::new_v4().simple()
    ));
    std::fs::write(&tmp, bytes).map_err(|e| format!("could not write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("could not replace {}: {e}", path.display())
    })
}
