//! A regression task made from a recorded run.
//!
//! The new task copies what the source run was given (its exact task version:
//! goal, criteria, verification, limits, held-out checks) and records where it
//! came from. The source task and the source run are only read. The starting
//! state is chosen so every attempt starts from an identified baseline:
//!
//! - a fixture or git-snapshot source keeps its baseline as it is;
//! - a project source whose run started on a clean git commit becomes a git
//!   snapshot of that commit (tracked files only, credential-shaped paths left
//!   out at every run);
//! - any other project source stays a project in place, and the task records
//!   that its start cannot be reproduced.

use serde::Deserialize;

use super::analysis::CauseSource;
use super::run::RunRecord;
use super::task::{
    is_valid_id, BenchTask, ExpectedOutcome, OriginCheck, TaskOrigin, WorkdirRef, TASK_SCHEMA,
};

#[derive(Debug, Clone, Deserialize)]
pub struct RegressionRequest {
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    /// A person's short description of the failure. Required.
    pub failure_description: String,
    #[serde(default)]
    pub proposed_improvement: Option<String>,
    /// `success` (default): the fix should make every check pass.
    #[serde(default)]
    pub expected_outcome: Option<String>,
}

/// Always true of a re-run against a model, whatever else is pinned.
pub const NONDETERMINISM: &str =
    "Model output is not deterministic: a re-run with the same model, \
    settings and baseline can still behave differently.";

/// A value's serialized name (`failed_to_start`, not `FailedToStart`).
fn snake<T: serde::Serialize>(v: &T) -> String {
    serde_json::to_value(v)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

pub fn build(run: &RunRecord, req: &RegressionRequest) -> Result<BenchTask, String> {
    if !is_valid_id(&req.id) {
        return Err("task id must be 1-80 lowercase letters, digits, '-', '_' or '.'".into());
    }
    let description = req.failure_description.trim();
    if description.is_empty() {
        return Err("describe the failure in a sentence or two".into());
    }
    if !run.status.is_terminal() {
        return Err("only a finished run can become a regression task".into());
    }
    let source = &run.task.snapshot;
    let mut limitations = vec![NONDETERMINISM.to_string()];
    let workdir = match &source.workdir {
        WorkdirRef::Path { path } => match run.baseline.as_ref() {
            Some(b) if b.git_dirty == Some(false) && b.git_commit.is_some() => {
                limitations.push(format!(
                    "Reproduced from commit {} of {path}: tracked files only, without \
                     credential-shaped paths.",
                    b.git_commit.as_deref().unwrap_or_default()
                ));
                WorkdirRef::GitSnapshot {
                    repo: path.clone(),
                    commit: b.git_commit.clone().unwrap_or_default(),
                }
            }
            Some(b) => {
                limitations.push(if b.git_commit.is_some() {
                    "The source project had uncommitted changes, so its start cannot be \
                     snapshotted; the regression runs on the project in place."
                        .into()
                } else {
                    "The source project is not a git repository, so its start cannot be \
                     snapshotted; the regression runs on the project in place."
                        .into()
                });
                source.workdir.clone()
            }
            None => {
                limitations.push(
                    "The source run recorded no baseline; the regression runs on the project in place."
                        .into(),
                );
                source.workdir.clone()
            }
        },
        other => other.clone(),
    };
    if run.baseline.is_none() {
        limitations.push(
            "The source run predates baseline identities; whether its start matches this \
             task's is not known."
                .into(),
        );
    }
    if run.model.is_none() {
        limitations.push("The source run recorded no model.".into());
    }
    let expected = req
        .expected_outcome
        .clone()
        .unwrap_or_else(|| "success".into());
    let primary = run.analysis.as_ref().and_then(|a| a.primary()).map(|c| {
        format!(
            "{} ({})",
            snake(&c.category),
            if c.source == CauseSource::Human {
                "reviewer"
            } else {
                "rule"
            }
        )
    });
    let mut desc = format!(
        "Regression from run {} of {} v{}: {}",
        run.id, source.id, source.version, description
    );
    if !source.description.is_empty() {
        desc.push_str("\n\nOriginal task: ");
        desc.push_str(&source.description);
    }
    let task = BenchTask {
        schema: TASK_SCHEMA.into(),
        id: req.id.clone(),
        version: 1,
        name: req
            .name
            .clone()
            .filter(|n| !n.trim().is_empty())
            .unwrap_or_else(|| format!("Regression: {}", source.name)),
        description: desc,
        goal: source.goal.clone(),
        workdir,
        success_criteria: source.success_criteria.clone(),
        evaluation: source.evaluation.clone(),
        limits: source.limits.clone(),
        builtin: false,
        created_at: String::new(),
        updated_at: String::new(),
        origin: Some(TaskOrigin {
            kind: "regression".into(),
            source_run_id: run.id.clone(),
            source_task_id: source.id.clone(),
            source_task_version: run.task.version,
            source_task_digest: run.task.digest.clone(),
            source_status: snake(&run.status),
            source_verified: run.evaluation.verified.map(|v| snake(&v)),
            source_checks: run
                .verification
                .iter()
                .map(|c| OriginCheck {
                    name: c.name.clone(),
                    outcome: snake(&c.outcome),
                })
                .collect(),
            source_primary_cause: primary,
            source_agent: run.agent.id.clone(),
            source_agent_version: run.agent.version.clone(),
            source_model: run.model.clone(),
            source_baseline: run.baseline.as_ref().map(|b| b.identity.clone()),
            failure_description: description.to_string(),
            proposed_improvement: req
                .proposed_improvement
                .clone()
                .unwrap_or_default()
                .trim()
                .to_string(),
            created_at: super::run::now(),
            limitations,
        }),
        expected: Some(ExpectedOutcome {
            outcome: expected,
            note: String::new(),
        }),
    };
    task.validate()?;
    Ok(task)
}
