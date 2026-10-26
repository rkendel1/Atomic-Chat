//! Export of the run ledger for analysis: JSON Lines, one run per line, under
//! `atomic.bench.export.v1` (documented in docs/agent-bench.md).
//!
//! Each line keeps four kinds of information apart:
//! - `observed`: facts Atomic Chat saw while running the agent;
//! - `verified`: what the task's verification commands established;
//! - `human`: a person's judgment;
//! - `agent_claims`: what the agent said about its own work (never evidence);
//!
//! and lists in `unknown` every field whose value was not available, so a
//! missing value is never mistaken for a zero or a failure.

use serde_json::{json, Map, Value};

use super::run::{RunRecord, RunStatus};

pub const EXPORT_SCHEMA: &str = "atomic.bench.export.v1";

/// Observed fields that are null because the thing did not happen (no
/// cancellation, no signal, no error, no fixture), not because it is unknown.
const ABSENT_WHEN_NOT_APPLICABLE: &[&str] = &[
    "cancellation",
    "interruption",
    "signal",
    "execution_error",
    "fixture",
];

/// Fields only stack-agent runs have: null for other agents is "not
/// applicable", not unknown.
const RUNTIME_FIELDS: &[&str] = &["agent_revision", "agent_runtime"];

pub fn export_line(run: &RunRecord) -> Value {
    let exec = run.execution.as_ref();
    let mut observed = Map::new();
    let mut put = |k: &str, v: Value| {
        observed.insert(k.to_string(), v);
    };
    put("agent_id", json!(run.agent.id));
    put("agent_version", json!(run.agent.version));
    // The exact stack-agent runtime (commit, source, executable digest) the
    // runtime manager handed this run; not applicable to other agents.
    put(
        "agent_revision",
        json!(run.agent.runtime.as_ref().and_then(|r| r.commit.clone())),
    );
    put("agent_runtime", json!(run.agent.runtime));
    put("execution_method", json!(run.agent.execution_method));
    put("model", json!(run.model));
    put("provider", json!(run.provider));
    put("endpoint", json!(run.endpoint));
    put("config", json!(run.config));
    put("credential_supplied", json!(run.credential_supplied));
    put("app_version", json!(run.app_version));
    put("workdir_source", json!(run.workdir.source));
    put("fixture", json!(run.workdir.fixture));
    put("created_at", json!(run.created_at));
    put("started_at", json!(run.started_at));
    put("finished_at", json!(run.finished_at));
    put("duration_ms", json!(run.duration_ms));
    put("execution_outcome", json!(exec.map(|e| e.outcome)));
    put("exit_code", json!(exec.and_then(|e| e.exit_code)));
    put("signal", json!(exec.and_then(|e| e.signal)));
    put("execution_duration_ms", json!(exec.map(|e| e.duration_ms)));
    put("execution_error", json!(exec.and_then(|e| e.error.clone())));
    put("stdout_bytes", json!(exec.map(|e| e.stdout.total_bytes)));
    put("stderr_bytes", json!(exec.map(|e| e.stderr.total_bytes)));
    put(
        "changes",
        json!(run.changes.as_ref().map(|c| json!({
            "added": c.added, "modified": c.modified, "deleted": c.deleted,
            "incomplete": c.incomplete,
        }))),
    );
    put("cancellation", json!(run.cancellation));
    put("interruption", json!(run.interruption));
    put("errors", json!(run.errors));

    let unknown: Vec<String> = observed
        .iter()
        .filter(|(k, v)| {
            v.is_null()
                && !ABSENT_WHEN_NOT_APPLICABLE.contains(&k.as_str())
                && !(RUNTIME_FIELDS.contains(&k.as_str()) && run.agent.id != "stack-agent")
        })
        .map(|(k, _)| format!("observed.{k}"))
        .chain(
            run.evaluation
                .verified
                .is_none()
                .then(|| "verified.outcome".to_string()),
        )
        .chain(run.evaluation.human.is_none().then(|| "human".to_string()))
        .collect();

    let verified = if run.evaluation.method == "command" {
        json!({
            "method": "command",
            "outcome": run.evaluation.verified,
            "checks": run.verification.iter().map(|c| json!({
                "name": c.name,
                "command": c.command_line,
                "outcome": c.outcome,
                "exit_code": c.exit_code,
                "duration_ms": c.duration_ms,
            })).collect::<Vec<_>>(),
        })
    } else {
        json!({ "method": "manual", "outcome": null, "checks": [] })
    };

    json!({
        "schema": EXPORT_SCHEMA,
        "run_id": run.id,
        "run_schema": run.schema,
        "task": {
            "id": run.task.id,
            "version": run.task.version,
            "digest": run.task.digest,
            "name": run.task.name,
            "evaluation_method": run.evaluation.method,
        },
        "final_status": run.status,
        "failure_kind": run.failure_kind,
        "terminal": run.status.is_terminal(),
        "observed": Value::Object(observed),
        "verified": verified,
        "human": run.evaluation.human,
        "agent_claims": agent_claims(run),
        "unknown": unknown,
        // ---- added within v1 (absent from lines written before) ----
        // The failure analysis: candidate causes with source, confidence and
        // evidence, unknowns, and (apart) suggestions, which are never causes.
        "analysis": run.analysis.clone().or_else(|| run.clone().with_analysis().analysis),
        "reproducibility": reproducibility(run),
        "suite": run.suite,
        "agent_trace": agent_trace(run),
        "evidence_gaps": evidence_gaps(run),
    })
}

fn reproducibility(run: &RunRecord) -> Value {
    let origin = run.task.snapshot.origin.as_ref();
    let mut limitations: Vec<String> = run
        .baseline
        .as_ref()
        .map(|b| b.limitations.clone())
        .unwrap_or_default();
    if let Some(o) = origin {
        limitations.extend(o.limitations.iter().cloned());
    }
    json!({
        "baseline": run.baseline,
        "fresh_workspace": run.baseline.as_ref().map(|b| b.fresh_workspace),
        "task_origin": origin,
        "task_expected": run.task.snapshot.expected,
        "limitations": limitations,
    })
}

/// The agent's trajectory without each event's raw data: kind, time,
/// provenance and a one-line summary.
fn agent_trace(run: &RunRecord) -> Value {
    let Some(t) = run.agent_trace.as_ref() else {
        return Value::Null;
    };
    let mut by: std::collections::BTreeMap<String, usize> = Default::default();
    for e in &t.events {
        let k = serde_json::to_value(e.provenance)
            .ok()
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_default();
        *by.entry(k).or_insert(0) += 1;
    }
    json!({
        "source": t.source,
        "available": t.available,
        "unavailable_reason": t.unavailable_reason,
        "events_by_provenance": by,
        "events": t.events.iter().map(|e| json!({
            "index": e.index, "at": e.at, "provenance": e.provenance,
            "kind": e.kind, "summary": e.summary,
        })).collect::<Vec<_>>(),
        "model_replies": t.replies.len(),
        "replies_unavailable_reason": t.replies_unavailable_reason,
    })
}

/// Evidence this run lacks, by name, so an analysis can tell "absent" from
/// "not applicable".
fn evidence_gaps(run: &RunRecord) -> Vec<&'static str> {
    let mut gaps = Vec::new();
    if run.baseline.is_none() {
        gaps.push("baseline");
    }
    match &run.agent_trace {
        None => gaps.push("agent_trace"),
        Some(t) if !t.available => gaps.push("agent_trace"),
        Some(t) if t.replies.is_empty() => gaps.push("model_replies"),
        _ => {}
    }
    if run
        .execution
        .as_ref()
        .is_some_and(|e| e.started_at.is_none())
    {
        gaps.push("process_timestamps");
    }
    gaps
}

/// The few self-reported fields worth keeping, labelled as the agent's claims.
fn agent_claims(run: &RunRecord) -> Value {
    let Some(report) = run.execution.as_ref().and_then(|e| e.agent_report.as_ref()) else {
        return Value::Null;
    };
    match run.agent.id.as_str() {
        "stack-agent" => json!({
            "terminal_state": report["terminal_state"],
            "verified": report["verified"],
            "goal_satisfied": report["goal_satisfied"],
            "outcome_reason": report["outcome_reason"],
            "paths_written": report["paths_written"],
            "usage": report["usage"],
            "model_replies": run.agent_trace.as_ref().map(|t| t.replies.clone()),
        }),
        "claude-code" => json!({
            "is_error": report["is_error"],
            "subtype": report["subtype"],
            "num_turns": report["num_turns"],
            "result": report["result"],
            "usage": report["usage"],
        }),
        _ => Value::Null,
    }
}

/// JSON Lines for `runs`, oldest first; runs still in progress are left out.
pub fn export_jsonl(runs: &[RunRecord]) -> String {
    let mut sorted: Vec<&RunRecord> = runs
        .iter()
        .filter(|r| {
            r.status.is_terminal()
                || (r.status == RunStatus::Evaluating && r.evaluation.awaiting_review)
        })
        .collect();
    sorted.sort_by(|a, b| a.created_at.cmp(&b.created_at));
    let mut out = String::new();
    for r in sorted {
        out.push_str(&export_line(r).to_string());
        out.push('\n');
    }
    out
}
