//! Regression suites (`atomic.bench.suite.v1`) and their runs
//! (`atomic.bench.suite_run.v1`), their aggregation, their comparison, and the
//! failure summary across runs.
//!
//! Counting rules, so nothing is silently a success or a failure:
//! - the **verified success rate** is verified successes over runs whose
//!   checks established success or failure; that denominator is shown;
//! - interrupted, cancelled, inconclusive, still running, not started, agent
//!   could not run, and human-judged runs are listed as **exclusions** with a
//!   reason and never enter the rate;
//! - a comparison reports an improvement or regression only for a task both
//!   sides verified; everything else is "not comparable", with the reason.

use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize};

use super::analysis::{self, Category, CauseSource, Confidence};
use super::run::{FailureKind, RunRecord, RunStatus, Verdict};
use super::task::{BenchTask, Evaluation, WorkdirRef};

pub const SUITE_SCHEMA: &str = "atomic.bench.suite.v1";
pub const SUITE_RUN_SCHEMA: &str = "atomic.bench.suite_run.v1";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Suite {
    pub schema: String,
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// Increases whenever the entries change.
    pub version: u32,
    #[serde(default)]
    pub entries: Vec<SuiteEntry>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SuiteEntry {
    pub task_id: String,
    /// The task version this entry runs; later edits do not change it.
    pub task_version: u32,
    pub task_digest: String,
    pub task_name: String,
    /// `success` or `failure`.
    pub expected: String,
    /// The verification procedure, as it reads.
    #[serde(default)]
    pub verification: Vec<String>,
    /// The starting state the task names (`fixture:ledger`, `git:<commit>`, `path:<dir>`).
    pub baseline: String,
    /// The run the task was made from, for a regression task.
    #[serde(default)]
    pub source_run_id: Option<String>,
    pub added_at: String,
}

impl SuiteEntry {
    pub fn from_task(task: &BenchTask) -> Self {
        SuiteEntry {
            task_id: task.id.clone(),
            task_version: task.version,
            task_digest: task.digest(),
            task_name: task.name.clone(),
            expected: task
                .expected
                .as_ref()
                .map(|e| e.outcome.clone())
                .unwrap_or_else(|| "success".into()),
            verification: match &task.evaluation {
                Evaluation::Command { commands, .. } => {
                    commands.iter().map(|c| c.display()).collect()
                }
                Evaluation::Manual { .. } => vec!["manual review".into()],
            },
            baseline: match &task.workdir {
                WorkdirRef::Fixture { fixture } => format!("fixture:{fixture}"),
                WorkdirRef::GitSnapshot { commit, .. } => format!("git:{commit}"),
                WorkdirRef::Path { path } => format!("path:{path}"),
            },
            source_run_id: task.origin.as_ref().map(|o| o.source_run_id.clone()),
            added_at: super::run::now(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SuiteRun {
    pub schema: String,
    pub id: String,
    pub suite_id: String,
    pub suite_version: u32,
    /// A person's label, e.g. "baseline" or "after the decision-format fix".
    #[serde(default)]
    pub label: String,
    pub agent_id: String,
    #[serde(default)]
    pub model: Option<String>,
    pub entries: Vec<SuiteRunEntry>,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SuiteRunEntry {
    pub task_id: String,
    pub task_version: u32,
    pub expected: String,
    #[serde(default)]
    pub run_id: Option<String>,
    /// Why no run was started for this entry.
    #[serde(default)]
    pub not_started_reason: Option<String>,
}

// ---- aggregation --------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResultClass {
    VerifiedSuccess,
    VerifiedFailure,
    /// Verification ran but established nothing.
    Inconclusive,
    /// The agent could not run; the task was never attempted.
    ExecutionFailure,
    Interrupted,
    Cancelled,
    InProgress,
    /// Judged by a person, not verified; reported apart.
    HumanJudged,
    /// No run was started for this entry.
    NotStarted,
}

impl ResultClass {
    pub fn of(run: &RunRecord) -> Self {
        match run.status {
            RunStatus::Queued | RunStatus::Running => Self::InProgress,
            RunStatus::Evaluating => Self::InProgress,
            RunStatus::Cancelled => Self::Cancelled,
            _ if run.failure_kind == Some(FailureKind::Interrupted) => Self::Interrupted,
            _ if run.failure_kind == Some(FailureKind::Execution) => Self::ExecutionFailure,
            _ if run.evaluation.method == "manual" => Self::HumanJudged,
            _ => match run.evaluation.verified {
                Some(Verdict::Success) => Self::VerifiedSuccess,
                Some(Verdict::Failure) => Self::VerifiedFailure,
                _ => Self::Inconclusive,
            },
        }
    }

    fn verified(self) -> bool {
        matches!(self, Self::VerifiedSuccess | Self::VerifiedFailure)
    }

    fn exclusion_reason(self) -> &'static str {
        match self {
            Self::Inconclusive => "verification established no outcome",
            Self::ExecutionFailure => "the agent could not run",
            Self::Interrupted => "Atomic Chat stopped during the run",
            Self::Cancelled => "cancelled",
            Self::InProgress => "still running",
            Self::HumanJudged => "judged by a person, not verified",
            Self::NotStarted => "not started",
            Self::VerifiedSuccess | Self::VerifiedFailure => "",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EntryResult {
    pub task_id: String,
    pub task_version: u32,
    pub expected: String,
    #[serde(default)]
    pub run_id: Option<String>,
    pub class: ResultClass,
    /// Whether the verified result matches the expectation; `None` when unverified.
    #[serde(default)]
    pub met_expectation: Option<bool>,
    #[serde(default)]
    pub duration_ms: Option<u64>,
    #[serde(default)]
    pub primary_cause: Option<Category>,
    #[serde(default)]
    pub baseline: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Exclusion {
    pub task_id: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SuiteRunSummary {
    pub suite_run_id: String,
    pub entries: Vec<EntryResult>,
    pub total: usize,
    pub verified_successes: usize,
    pub verified_failures: usize,
    /// Verified successes and failures: the rate's denominator.
    pub denominator: usize,
    /// `verified_successes / denominator`, absent when nothing was verified.
    #[serde(default)]
    pub verified_success_rate: Option<f64>,
    pub exclusions: Vec<Exclusion>,
    pub counts: BTreeMap<String, usize>,
    /// Verified entries that matched their expected outcome, over `denominator`.
    pub met_expectation: usize,
    /// Sum of the durations that were recorded, and how many were.
    pub total_duration_ms: u64,
    pub durations_recorded: usize,
}

pub fn summarize(suite_run: &SuiteRun, runs: &HashMap<String, RunRecord>) -> SuiteRunSummary {
    let mut entries = Vec::new();
    for e in &suite_run.entries {
        let run = e.run_id.as_ref().and_then(|id| runs.get(id));
        let class = match (run, &e.run_id) {
            (Some(r), _) => ResultClass::of(r),
            (None, _) => ResultClass::NotStarted,
        };
        let met = class
            .verified()
            .then(|| (class == ResultClass::VerifiedSuccess) == (e.expected == "success"));
        entries.push(EntryResult {
            task_id: e.task_id.clone(),
            task_version: e.task_version,
            expected: e.expected.clone(),
            run_id: e.run_id.clone(),
            class,
            met_expectation: met,
            duration_ms: run.and_then(|r| r.duration_ms),
            primary_cause: run
                .and_then(|r| r.analysis.as_ref())
                .and_then(|a| a.primary())
                .map(|c| c.category),
            baseline: run
                .and_then(|r| r.baseline.as_ref())
                .map(|b| b.identity.clone()),
            note: match (run, &e.run_id) {
                (None, Some(id)) => Some(format!("run {id} is not in the ledger")),
                (None, None) => e.not_started_reason.clone(),
                _ => None,
            },
        });
    }
    let successes = entries
        .iter()
        .filter(|e| e.class == ResultClass::VerifiedSuccess)
        .count();
    let failures = entries
        .iter()
        .filter(|e| e.class == ResultClass::VerifiedFailure)
        .count();
    let denominator = successes + failures;
    let mut counts = BTreeMap::new();
    for e in &entries {
        let key = serde_json::to_value(e.class)
            .ok()
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_default();
        *counts.entry(key).or_insert(0) += 1;
    }
    let durations: Vec<u64> = entries.iter().filter_map(|e| e.duration_ms).collect();
    SuiteRunSummary {
        suite_run_id: suite_run.id.clone(),
        total: entries.len(),
        verified_successes: successes,
        verified_failures: failures,
        denominator,
        verified_success_rate: (denominator > 0).then(|| successes as f64 / denominator as f64),
        exclusions: entries
            .iter()
            .filter(|e| !e.class.verified())
            .map(|e| Exclusion {
                task_id: e.task_id.clone(),
                reason: e
                    .note
                    .clone()
                    .unwrap_or_else(|| e.class.exclusion_reason().into()),
            })
            .collect(),
        counts,
        met_expectation: entries
            .iter()
            .filter(|e| e.met_expectation == Some(true))
            .count(),
        total_duration_ms: durations.iter().sum(),
        durations_recorded: durations.len(),
        entries,
    }
}

// ---- comparison -----------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Change {
    Improvement,
    Regression,
    StillSucceeds,
    StillFails,
    NotComparable,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskChange {
    pub task_id: String,
    pub baseline: Option<ResultClass>,
    pub candidate: Option<ResultClass>,
    pub change: Change,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub baseline_duration_ms: Option<u64>,
    #[serde(default)]
    pub candidate_duration_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Warning {
    pub field: String,
    pub baseline: String,
    pub candidate: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SuiteComparison {
    pub baseline: SuiteRunSummary,
    pub candidate: SuiteRunSummary,
    pub tasks: Vec<TaskChange>,
    pub improvements: usize,
    pub regressions: usize,
    pub not_comparable: usize,
    /// Conditions that differ between the two suite runs.
    pub warnings: Vec<Warning>,
    /// True when no warning applies.
    pub directly_comparable: bool,
}

/// One value per field across a suite run's runs, `unknown` when unrecorded
/// and `mixed: a | b` when they disagree.
fn conditions(sr: &SuiteRun, runs: &HashMap<String, RunRecord>) -> BTreeMap<&'static str, String> {
    let rs: Vec<&RunRecord> = sr
        .entries
        .iter()
        .filter_map(|e| e.run_id.as_ref().and_then(|id| runs.get(id)))
        .collect();
    let pick = |f: &dyn Fn(&RunRecord) -> String| -> String {
        let mut vals: Vec<String> = rs.iter().map(|r| f(r)).collect();
        vals.sort();
        vals.dedup();
        match vals.len() {
            0 => "unknown".into(),
            1 => vals.remove(0),
            _ => format!("mixed: {}", vals.join(" | ")),
        }
    };
    let mut m = BTreeMap::new();
    m.insert("suite_version", sr.suite_version.to_string());
    m.insert("agent", sr.agent_id.clone());
    m.insert(
        "agent_version",
        pick(&|r| r.agent.version.clone().unwrap_or_else(|| "unknown".into())),
    );
    m.insert(
        "model",
        pick(&|r| r.model.clone().unwrap_or_else(|| "unknown".into())),
    );
    m.insert(
        "provider",
        pick(&|r| r.provider.clone().unwrap_or_else(|| "unknown".into())),
    );
    m.insert(
        "config",
        pick(&|r| {
            r.config
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join(",")
        }),
    );
    m.insert("app_version", pick(&|r| r.app_version.clone()));
    m
}

pub fn compare(
    base: &SuiteRun,
    cand: &SuiteRun,
    runs: &HashMap<String, RunRecord>,
) -> SuiteComparison {
    let bs = summarize(base, runs);
    let cs = summarize(cand, runs);
    let mut warnings = Vec::new();
    if base.suite_id != cand.suite_id {
        warnings.push(Warning {
            field: "suite".into(),
            baseline: base.suite_id.clone(),
            candidate: cand.suite_id.clone(),
        });
    }
    let (bc, cc) = (conditions(base, runs), conditions(cand, runs));
    for (k, bv) in &bc {
        let cv = &cc[k];
        // Configuration naturally differs between agents; compare it only for one agent.
        if *k == "config" && base.agent_id != cand.agent_id {
            continue;
        }
        if bv != cv || bv.starts_with("mixed") || bv == "unknown" && *k != "config" {
            let (baseline, candidate) =
                if *k == "config" && !bv.starts_with("mixed") && !cv.starts_with("mixed") {
                    config_difference(bv, cv)
                } else {
                    (bv.clone(), cv.clone())
                };
            warnings.push(Warning {
                field: (*k).into(),
                baseline,
                candidate,
            });
        }
    }
    let mut tasks = Vec::new();
    let mut ids: Vec<&String> = bs.entries.iter().map(|e| &e.task_id).collect();
    for e in &cs.entries {
        if !ids.contains(&&e.task_id) {
            ids.push(&e.task_id);
        }
    }
    for id in ids {
        let b = bs.entries.iter().find(|e| &e.task_id == id);
        let c = cs.entries.iter().find(|e| &e.task_id == id);
        let (change, reason) = match (b, c) {
            (Some(b), Some(c)) if b.task_version != c.task_version => (
                Change::NotComparable,
                Some(format!(
                    "task version v{} vs v{}",
                    b.task_version, c.task_version
                )),
            ),
            (Some(b), Some(c))
                if b.baseline.is_some() && c.baseline.is_some() && b.baseline != c.baseline =>
            {
                (
                    Change::NotComparable,
                    Some("the runs started from different baselines".into()),
                )
            }
            (Some(b), Some(c)) if b.class.verified() && c.class.verified() => {
                let bs = b.class == ResultClass::VerifiedSuccess;
                let cs = c.class == ResultClass::VerifiedSuccess;
                (
                    match (bs, cs) {
                        (false, true) => Change::Improvement,
                        (true, false) => Change::Regression,
                        (true, true) => Change::StillSucceeds,
                        (false, false) => Change::StillFails,
                    },
                    None,
                )
            }
            (Some(b), Some(c)) => (
                Change::NotComparable,
                Some(
                    format!(
                        "{}{}",
                        if b.class.verified() {
                            String::new()
                        } else {
                            format!("baseline: {}; ", b.class.exclusion_reason())
                        },
                        if c.class.verified() {
                            String::new()
                        } else {
                            format!("candidate: {}", c.class.exclusion_reason())
                        },
                    )
                    .trim_end_matches("; ")
                    .to_string(),
                ),
            ),
            (Some(_), None) => (
                Change::NotComparable,
                Some("not in the candidate run".into()),
            ),
            (None, _) => (
                Change::NotComparable,
                Some("not in the baseline run".into()),
            ),
        };
        tasks.push(TaskChange {
            task_id: id.clone(),
            baseline: b.map(|e| e.class),
            candidate: c.map(|e| e.class),
            change,
            reason,
            baseline_duration_ms: b.and_then(|e| e.duration_ms),
            candidate_duration_ms: c.and_then(|e| e.duration_ms),
        });
    }
    let count = |ch: Change| tasks.iter().filter(|t| t.change == ch).count();
    SuiteComparison {
        improvements: count(Change::Improvement),
        regressions: count(Change::Regression),
        not_comparable: count(Change::NotComparable),
        directly_comparable: warnings.is_empty(),
        warnings,
        tasks,
        baseline: bs,
        candidate: cs,
    }
}

/// Only the `key=value` settings that differ between two configurations.
fn config_difference(a: &str, b: &str) -> (String, String) {
    let set = |s: &str| -> std::collections::BTreeSet<String> {
        s.split(',')
            .filter(|p| !p.is_empty())
            .map(str::to_string)
            .collect()
    };
    let (sa, sb) = (set(a), set(b));
    let only = |x: &std::collections::BTreeSet<String>, y: &std::collections::BTreeSet<String>| {
        let d: Vec<&str> = x.difference(y).map(String::as_str).collect();
        if d.is_empty() {
            "(not set)".to_string()
        } else {
            d.join(", ")
        }
    };
    (only(&sa, &sb), only(&sb, &sa))
}

// ---- failure summary ---------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CategoryExample {
    pub run_id: String,
    /// Whether this category is the run's primary cause.
    pub primary: bool,
    pub task_id: String,
    pub task_version: u32,
    pub agent_id: String,
    #[serde(default)]
    pub model: Option<String>,
    pub source: CauseSource,
    pub confidence: Confidence,
    pub explanation: String,
    /// Up to three evidence lines.
    pub evidence: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CategorySummary {
    pub category: Category,
    pub label: String,
    /// Runs whose primary cause is this category.
    pub primary_count: usize,
    /// Runs where it is a contributing (non-primary) cause.
    pub contributing_count: usize,
    pub examples: Vec<CategoryExample>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FailureSummary {
    pub runs_considered: usize,
    pub succeeded: usize,
    /// Failed or inconclusive runs, each counted once by its primary cause.
    pub not_succeeded: usize,
    pub excluded: usize,
    pub categories: Vec<CategorySummary>,
}

/// Groups finished, unsuccessful runs by their primary cause. Cancelled and
/// in-progress runs are counted as excluded.
pub fn failure_summary(runs: &[RunRecord], agent: Option<&str>) -> FailureSummary {
    let runs: Vec<&RunRecord> = runs
        .iter()
        .filter(|r| agent.map_or(true, |a| r.agent.id == a))
        .collect();
    let mut by: BTreeMap<Category, CategorySummary> = BTreeMap::new();
    let (mut succeeded, mut failed, mut excluded) = (0, 0, 0);
    for r in &runs {
        match r.status {
            RunStatus::Succeeded => {
                succeeded += 1;
                continue;
            }
            RunStatus::Failed | RunStatus::Inconclusive => failed += 1,
            _ => {
                excluded += 1;
                continue;
            }
        }
        let Some(a) = r.analysis.as_ref() else {
            continue;
        };
        for c in &a.causes {
            let entry = by.entry(c.category).or_insert_with(|| CategorySummary {
                category: c.category,
                label: analysis::label(c.category).into(),
                primary_count: 0,
                contributing_count: 0,
                examples: vec![],
            });
            if c.primary {
                entry.primary_count += 1;
            } else {
                entry.contributing_count += 1;
            }
            if entry.examples.len() < 5 && !entry.examples.iter().any(|e| e.run_id == r.id) {
                entry.examples.push(CategoryExample {
                    run_id: r.id.clone(),
                    primary: c.primary,
                    task_id: r.task.id.clone(),
                    task_version: r.task.version,
                    agent_id: r.agent.id.clone(),
                    model: r.model.clone(),
                    source: c.source,
                    confidence: c.confidence,
                    explanation: c.explanation.clone(),
                    evidence: c
                        .evidence
                        .iter()
                        .take(3)
                        .map(|e| e.detail.clone())
                        .collect(),
                });
            }
        }
    }
    let mut categories: Vec<CategorySummary> = by.into_values().collect();
    for c in &mut categories {
        // Runs where the category is the primary cause first.
        c.examples.sort_by_key(|e| !e.primary);
    }
    categories.sort_by(|a, b| {
        b.primary_count
            .cmp(&a.primary_count)
            .then(b.contributing_count.cmp(&a.contributing_count))
    });
    FailureSummary {
        runs_considered: runs.len(),
        succeeded,
        not_succeeded: failed,
        excluded,
        categories,
    }
}
