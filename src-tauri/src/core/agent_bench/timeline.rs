//! The run inspector's chronological timeline, built only from what was
//! recorded: the bench's own status changes and timings, the agent's
//! trajectory (with the provenance the agent gave each event), verification
//! commands, and a person's review. Gaps are entries too: where evidence is
//! missing the timeline says so instead of filling it in.

use serde::{Deserialize, Serialize};

use super::analysis::{CauseSource, EvidenceClass};
use super::evidence::Provenance;
use super::run::{CheckOutcome, ExecutionOutcome, RunRecord};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryClass {
    Observed,
    AgentRuntime,
    AgentClaim,
    Verified,
    Human,
    /// Evidence that does not exist for this run.
    Missing,
}

impl From<EvidenceClass> for EntryClass {
    fn from(c: EvidenceClass) -> Self {
        match c {
            EvidenceClass::Observed => Self::Observed,
            EvidenceClass::AgentRuntime => Self::AgentRuntime,
            EvidenceClass::AgentClaim => Self::AgentClaim,
            EvidenceClass::Verified => Self::Verified,
            EvidenceClass::Human => Self::Human,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TimelineEntry {
    /// RFC 3339, when recorded; entries without a time keep their place.
    #[serde(default)]
    pub at: Option<String>,
    pub class: EntryClass,
    /// `bench`, `agent`, `verification` or `review`.
    pub origin: String,
    pub label: String,
    #[serde(default)]
    pub detail: Option<String>,
    /// Where in the record it comes from.
    pub reference: String,
}

fn entry(
    at: Option<&str>,
    class: EntryClass,
    origin: &str,
    label: impl Into<String>,
    detail: Option<String>,
    reference: impl Into<String>,
) -> TimelineEntry {
    TimelineEntry {
        at: at.map(str::to_string),
        class,
        origin: origin.into(),
        label: label.into(),
        detail,
        reference: reference.into(),
    }
}

pub fn build(run: &RunRecord) -> Vec<TimelineEntry> {
    let mut out: Vec<TimelineEntry> = Vec::new();
    for (i, h) in run.history.iter().enumerate() {
        out.push(entry(
            Some(&h.at),
            EntryClass::Observed,
            "bench",
            format!(
                "Run {}",
                serde_json::to_value(h.status)
                    .ok()
                    .and_then(|v| v.as_str().map(str::to_string))
                    .unwrap_or_default()
            ),
            None,
            format!("history[{i}]"),
        ));
    }
    if let Some(b) = &run.baseline {
        out.push(entry(
            run.started_at.as_deref(),
            EntryClass::Observed,
            "bench",
            format!(
                "Baseline prepared ({}{})",
                b.kind,
                if b.fresh_workspace {
                    ", fresh workspace"
                } else {
                    ", in place"
                }
            ),
            Some(b.identity.clone()),
            "baseline",
        ));
    }
    if let Some(e) = &run.execution {
        if e.outcome == ExecutionOutcome::FailedToStart {
            out.push(entry(
                run.finished_at.as_deref(),
                EntryClass::Observed,
                "bench",
                "Agent did not start",
                e.error.clone(),
                "execution",
            ));
        } else {
            out.push(entry(
                e.started_at.as_deref(),
                EntryClass::Observed,
                "bench",
                "Agent process started",
                Some(e.command_line.chars().take(300).collect()),
                "execution.command_line",
            ));
            let how = match e.outcome {
                ExecutionOutcome::Exited => "exited (0)".to_string(),
                ExecutionOutcome::ExitedNonZero => format!(
                    "exited with {}",
                    e.exit_code.map_or("no code".into(), |c| c.to_string())
                ),
                ExecutionOutcome::TimedOut => "was stopped at the time limit".to_string(),
                ExecutionOutcome::Cancelled => "was cancelled".to_string(),
                ExecutionOutcome::FailedToStart => "did not start".to_string(),
            };
            out.push(entry(
                e.ended_at.as_deref(),
                EntryClass::Observed,
                "bench",
                format!("Agent process {how} after {} ms", e.duration_ms),
                e.error.clone(),
                "execution.outcome",
            ));
            if e.started_at.is_none() {
                out.push(entry(
                    None,
                    EntryClass::Missing,
                    "bench",
                    "This record predates process timestamps; the agent's start and end are placed by status changes only",
                    None,
                    "execution.started_at",
                ));
            }
        }
    }
    match &run.agent_trace {
        Some(t) if t.available => {
            for ev in &t.events {
                out.push(entry(
                    ev.at.as_deref(),
                    match ev.provenance {
                        Provenance::Observed => EntryClass::Observed,
                        Provenance::AgentClaim => EntryClass::AgentClaim,
                        Provenance::Runtime => EntryClass::AgentRuntime,
                    },
                    "agent",
                    ev.summary.clone(),
                    None,
                    format!("trace#{}", ev.index),
                ));
            }
        }
        Some(t) => out.push(entry(
            run.execution.as_ref().and_then(|e| e.ended_at.as_deref()),
            EntryClass::Missing,
            "agent",
            "The agent's own trajectory is unavailable",
            t.unavailable_reason.clone(),
            "agent_trace",
        )),
        None if run
            .execution
            .as_ref()
            .is_some_and(|e| e.outcome != ExecutionOutcome::FailedToStart) =>
        {
            out.push(entry(
                run.execution.as_ref().and_then(|e| e.ended_at.as_deref()),
                EntryClass::Missing,
                "agent",
                "The agent's own trajectory was not captured for this run",
                None,
                "agent_trace",
            ))
        }
        None => {}
    }
    for (i, c) in run.verification.iter().enumerate() {
        out.push(entry(
            c.started_at.as_deref(),
            if c.outcome == CheckOutcome::FailedToStart || c.outcome == CheckOutcome::Skipped {
                EntryClass::Missing
            } else {
                EntryClass::Verified
            },
            "verification",
            format!(
                "Check \"{}\": {}",
                c.name,
                serde_json::to_value(c.outcome)
                    .ok()
                    .and_then(|v| v.as_str().map(str::to_string))
                    .unwrap_or_default()
            ),
            Some(format!(
                "{} (exit {}, {} ms)",
                c.command_line,
                c.exit_code.map_or("none".to_string(), |e| e.to_string()),
                c.duration_ms
            )),
            format!("verification[{i}]"),
        ));
    }
    if let Some(h) = &run.evaluation.human {
        out.push(entry(
            Some(&h.judged_at),
            EntryClass::Human,
            "review",
            format!(
                "Reviewer judged: {}",
                serde_json::to_value(h.verdict)
                    .ok()
                    .and_then(|v| v.as_str().map(str::to_string))
                    .unwrap_or_default()
            ),
            (!h.note.is_empty()).then(|| h.note.clone()),
            "evaluation.human",
        ));
    }
    if let Some(a) = &run.analysis {
        for (i, c) in a.causes.iter().enumerate() {
            if c.source == CauseSource::Human {
                out.push(entry(
                    c.recorded_at.as_deref(),
                    EntryClass::Human,
                    "review",
                    format!(
                        "Reviewer recorded cause: {}",
                        super::analysis::label(c.category)
                    ),
                    Some(c.explanation.clone()),
                    format!("analysis.causes[{i}]"),
                ));
            }
        }
    }
    // Chronological by recorded time; untimed entries keep their position
    // relative to the entry before them.
    let mut last = String::new();
    let mut keyed: Vec<(String, usize, TimelineEntry)> = out
        .into_iter()
        .enumerate()
        .map(|(i, e)| {
            if let Some(at) = &e.at {
                last = at.clone();
            }
            (e.at.clone().unwrap_or_else(|| last.clone()), i, e)
        })
        .collect();
    keyed.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    keyed.into_iter().map(|(_, _, e)| e).collect()
}
