//! Agent Bench: run coding agents against repeatable tasks, verify the
//! result with explicit commands or a person's judgment, and keep every run
//! in a durable, exportable ledger. See docs/agent-bench.md.

pub mod adapters;
pub mod analysis;
pub mod baseline;
pub mod changes;
pub mod commands;
pub mod evidence;
pub mod executor;
pub mod export;
pub mod process;
pub mod redact;
pub mod regression;
pub mod regression_suite;
pub mod run;
pub mod store;
pub mod suite;
pub mod task;
pub mod timeline;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_runtime;
#[cfg(test)]
mod tests_workshop;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use tokio_util::sync::CancellationToken;

/// Event emitted with the run id whenever a run record changes.
pub const RUN_UPDATED_EVENT: &str = "agent_bench:run_updated";

pub struct BenchState {
    /// Cancellation for each run this process started and has not finished.
    pub active: Mutex<HashMap<String, CancellationToken>>,
    pub slot: Arc<tokio::sync::Semaphore>,
    /// Stale runs are recovered once, on first use in this process.
    pub recovered: OnceLock<()>,
}

impl Default for BenchState {
    fn default() -> Self {
        Self {
            active: Mutex::new(HashMap::new()),
            slot: Arc::new(tokio::sync::Semaphore::new(1)),
            recovered: OnceLock::new(),
        }
    }
}
