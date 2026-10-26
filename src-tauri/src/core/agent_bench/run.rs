//! One run of one task by one agent: the ledger record, `atomic.bench.run.v1`.
//!
//! Lifecycle: `queued → running → evaluating → succeeded | failed |
//! inconclusive | cancelled`. A run is `succeeded` only when its evaluation
//! says so (every verification command passed, or a person judged it
//! successful); an agent's exit status or reply never decides it.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::task::BenchTask;

pub const RUN_SCHEMA: &str = "atomic.bench.run.v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Queued,
    Running,
    Evaluating,
    Succeeded,
    Failed,
    Inconclusive,
    Cancelled,
}

impl RunStatus {
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Succeeded | Self::Failed | Self::Inconclusive | Self::Cancelled
        )
    }

    pub fn can_become(self, next: RunStatus) -> bool {
        use RunStatus::*;
        match self {
            Queued => matches!(next, Running | Failed | Inconclusive | Cancelled),
            Running => matches!(next, Evaluating | Failed | Inconclusive | Cancelled),
            Evaluating => matches!(next, Succeeded | Failed | Inconclusive | Cancelled),
            Succeeded | Failed | Inconclusive | Cancelled => false,
        }
    }
}

/// Why a run did not succeed, kept apart from the status so an agent that
/// crashed is never confused with a change that failed its checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureKind {
    /// The agent could not be run, or its run failed before anything could be
    /// verified (missing executable, unreachable model, preflight).
    Execution,
    /// The agent ran; the verification commands did not all pass.
    Verification,
    /// A verification command could not run, so nothing was established.
    VerificationUnavailable,
    /// A person judged the result unsuccessful.
    HumanJudgment,
    /// The app stopped while the run was in progress.
    Interrupted,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionOutcome {
    /// The process exited with status 0.
    Exited,
    /// The process exited with a non-zero status.
    ExitedNonZero,
    /// Stopped because the task's time limit passed.
    TimedOut,
    Cancelled,
    /// Never started (no executable, preflight failure, spawn error).
    FailedToStart,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CapturedText {
    pub text: String,
    /// Bytes the process wrote, including any dropped by truncation.
    pub total_bytes: u64,
    /// When true, `text` holds the start and end with the middle removed.
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExecutionRecord {
    pub outcome: ExecutionOutcome,
    /// The command as it reads to a person, credentials removed.
    pub command_line: String,
    pub exit_code: Option<i32>,
    /// Unix signal that ended the process, when one did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signal: Option<i32>,
    pub duration_ms: u64,
    pub stdout: CapturedText,
    pub stderr: CapturedText,
    /// The agent's own structured report when it prints one (stack-agent
    /// `--json`, Claude Code `--output-format json`). These are the agent's
    /// claims, never evidence of success.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_report: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckOutcome {
    Passed,
    Failed,
    TimedOut,
    /// The command could not be started (e.g. the program is not installed).
    FailedToStart,
    /// Not run because the run was cancelled first.
    Skipped,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CheckResult {
    pub name: String,
    pub command_line: String,
    pub outcome: CheckOutcome,
    pub exit_code: Option<i32>,
    pub duration_ms: u64,
    pub stdout: CapturedText,
    pub stderr: CapturedText,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Success,
    Failure,
    Inconclusive,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HumanJudgment {
    pub verdict: Verdict,
    #[serde(default)]
    pub note: String,
    pub judged_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EvaluationRecord {
    /// `command` or `manual`, copied from the task.
    pub method: String,
    /// The outcome the verification commands established; `None` for manual
    /// evaluation or when verification never ran.
    #[serde(default)]
    pub verified: Option<Verdict>,
    /// A person's judgment. Decides a manual run; on a command run it is kept
    /// beside the verified outcome and does not replace it.
    #[serde(default)]
    pub human: Option<HumanJudgment>,
    /// A manual run waits in `evaluating` for its judgment.
    #[serde(default)]
    pub awaiting_review: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ChangeSummary {
    pub added: Vec<String>,
    pub modified: Vec<String>,
    pub deleted: Vec<String>,
    /// More files than the snapshot limit: the lists are incomplete.
    pub incomplete: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Artifact {
    pub kind: String,
    pub reference: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskRef {
    pub id: String,
    pub version: u32,
    pub digest: String,
    pub name: String,
    /// The exact task the run was given.
    pub snapshot: BenchTask,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentRef {
    pub id: String,
    pub name: String,
    /// How the agent was driven, e.g. `cli:stack-agent work --json`.
    pub execution_method: String,
    #[serde(default)]
    pub executable: Option<String>,
    /// First line of `<agent> --version`, when it answered.
    #[serde(default)]
    pub version: Option<String>,
    /// stack-agent only: the exact runtime that ran, from the runtime manager.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime: Option<crate::core::stack_agent_runtime::RuntimeProvenance>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkdirInfo {
    /// `fixture` or `path`.
    pub source: String,
    pub path: String,
    #[serde(default)]
    pub fixture: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Cancellation {
    pub requested_at: String,
    /// `user` or `timeout`.
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Interruption {
    pub detected_at: String,
    pub previous_status: RunStatus,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunRecord {
    pub schema: String,
    pub id: String,
    pub task: TaskRef,
    pub agent: AgentRef,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub endpoint: Option<String>,
    /// Settings that shape the run, credentials excluded.
    #[serde(default)]
    pub config: BTreeMap<String, String>,
    /// Whether a credential was handed to the agent (never what it was).
    #[serde(default)]
    pub credential_supplied: bool,
    pub app_version: String,
    pub workdir: WorkdirInfo,
    pub status: RunStatus,
    #[serde(default)]
    pub failure_kind: Option<FailureKind>,
    pub created_at: String,
    #[serde(default)]
    pub started_at: Option<String>,
    #[serde(default)]
    pub finished_at: Option<String>,
    #[serde(default)]
    pub duration_ms: Option<u64>,
    #[serde(default)]
    pub execution: Option<ExecutionRecord>,
    #[serde(default)]
    pub verification: Vec<CheckResult>,
    #[serde(default)]
    pub changes: Option<ChangeSummary>,
    pub evaluation: EvaluationRecord,
    #[serde(default)]
    pub artifacts: Vec<Artifact>,
    #[serde(default)]
    pub errors: Vec<String>,
    #[serde(default)]
    pub cancellation: Option<Cancellation>,
    #[serde(default)]
    pub interruption: Option<Interruption>,
    /// Status changes in order, each with when it happened.
    #[serde(default)]
    pub history: Vec<StatusChange>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StatusChange {
    pub status: RunStatus,
    pub at: String,
}

pub fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

impl RunRecord {
    /// Moves the run to `next`, refusing a transition the lifecycle forbids.
    pub fn transition(&mut self, next: RunStatus) -> Result<(), String> {
        if !self.status.can_become(next) {
            return Err(format!(
                "run {}: {:?} cannot become {:?}",
                self.id, self.status, next
            ));
        }
        self.status = next;
        let at = now();
        if next == RunStatus::Running && self.started_at.is_none() {
            self.started_at = Some(at.clone());
        }
        if next.is_terminal() {
            self.finish(&at);
        }
        self.history.push(StatusChange { status: next, at });
        Ok(())
    }

    fn finish(&mut self, at: &str) {
        self.finished_at = Some(at.to_string());
        if let Some(start) = self.started_at.as_deref() {
            if let (Ok(s), Ok(f)) = (
                chrono::DateTime::parse_from_rfc3339(start),
                chrono::DateTime::parse_from_rfc3339(at),
            ) {
                self.duration_ms = Some((f - s).num_milliseconds().max(0) as u64);
            }
        }
    }

    /// Marks a run the app stopped mid-way as inconclusive. A manual run
    /// waiting for its judgment was not interrupted and is left alone.
    pub fn recover_if_stale(&mut self) -> bool {
        if self.status.is_terminal()
            || (self.status == RunStatus::Evaluating && self.evaluation.awaiting_review)
        {
            return false;
        }
        let previous = self.status;
        self.interruption = Some(Interruption {
            detected_at: now(),
            previous_status: previous,
        });
        self.failure_kind = Some(FailureKind::Interrupted);
        self.errors.push(format!(
            "Atomic Chat stopped while this run was {}; its outcome is unknown.",
            serde_json::to_value(previous)
                .ok()
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_default()
        ));
        self.evaluation.awaiting_review = false;
        // Every non-terminal status may become inconclusive.
        let _ = self.transition(RunStatus::Inconclusive);
        true
    }

    /// Records a person's judgment. On a manual run waiting for review it
    /// decides the final status; otherwise it is stored beside the outcome.
    pub fn apply_judgment(&mut self, verdict: Verdict, note: String) -> Result<(), String> {
        if !self.status.is_terminal() && !self.evaluation.awaiting_review {
            return Err("this run has not finished yet".to_string());
        }
        self.evaluation.human = Some(HumanJudgment {
            verdict,
            note,
            judged_at: now(),
        });
        if self.evaluation.awaiting_review {
            self.evaluation.awaiting_review = false;
            let (status, failure) = match verdict {
                Verdict::Success => (RunStatus::Succeeded, None),
                Verdict::Failure => (RunStatus::Failed, Some(FailureKind::HumanJudgment)),
                Verdict::Inconclusive => (RunStatus::Inconclusive, None),
            };
            self.failure_kind = failure;
            self.transition(status)?;
        }
        Ok(())
    }

    /// A copy without captured output, for listings.
    pub fn summary(&self) -> RunRecord {
        let mut s = self.clone();
        if let Some(e) = s.execution.as_mut() {
            e.stdout.text.clear();
            e.stderr.text.clear();
            e.agent_report = None;
        }
        for c in &mut s.verification {
            c.stdout.text.clear();
            c.stderr.text.clear();
        }
        s
    }
}
