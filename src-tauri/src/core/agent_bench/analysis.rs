//! Failure analysis, `atomic.bench.analysis.v1`: why a run did not succeed,
//! as far as the recorded evidence shows.
//!
//! Causes are *candidates*, never facts. Each names its category, its source
//! (a deterministic rule, or a person), its confidence, and the evidence it
//! rests on. A run can have several contributing causes, or none that the
//! evidence supports, in which case it says so (`insufficient_evidence`) and
//! lists what is unknown.
//!
//! The rules read only the run record: the bench's own observations (status,
//! process outcome, time limit, cancellation, interruption, changes), the
//! verification results, a person's judgment, and the agent's evidence as its
//! adapter normalized it ([`super::evidence::Signal`], each with provenance).
//! No model judges anything here. A model may later *suggest* a diagnosis;
//! suggestions live in their own list and never change a cause.

use serde::{Deserialize, Serialize};

use super::adapters;
use super::evidence::{Provenance, Signal, SignalKind};
use super::run::{CheckOutcome, ExecutionOutcome, RunRecord, RunStatus, Verdict};

pub const ANALYSIS_SCHEMA: &str = "atomic.bench.analysis.v1";
/// Bumped whenever a rule changes, so analyses made by different rules are
/// never mistaken for each other.
pub const RULES_VERSION: &str = "atomic.bench.rules.1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    ProcessOrInfrastructure,
    TimeoutOrLimit,
    SafetyOrAuthority,
    DecisionFormat,
    GoalNotCompleted,
    IncorrectImplementation,
    ToolOrCapability,
    ContextOrDiscovery,
    VerificationFailure,
    InsufficientEvidence,
}

impl Category {
    pub const ALL: [Category; 10] = [
        Category::ProcessOrInfrastructure,
        Category::TimeoutOrLimit,
        Category::SafetyOrAuthority,
        Category::DecisionFormat,
        Category::GoalNotCompleted,
        Category::IncorrectImplementation,
        Category::ToolOrCapability,
        Category::ContextOrDiscovery,
        Category::InsufficientEvidence,
        Category::VerificationFailure,
    ];

    /// The order the primary cause is chosen in: what stopped the work before
    /// it could be judged first; then explanations; then "unknown"; the
    /// observed fact of failed checks last, since it explains nothing.
    fn priority(self) -> usize {
        Self::ALL
            .iter()
            .position(|c| *c == self)
            .unwrap_or(usize::MAX)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Confidence {
    High,
    Medium,
    Low,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CauseSource {
    Rule,
    Human,
}

/// What kind of evidence an item is, kept apart in every view.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceClass {
    /// Atomic Chat or the agent's host saw it happen.
    Observed,
    /// The agent's runtime decided it (it stopped, it escalated).
    AgentRuntime,
    /// The agent (its model) said it. Never evidence of success.
    AgentClaim,
    /// A verification command established it.
    Verified,
    /// A person concluded it.
    Human,
}

impl From<Provenance> for EvidenceClass {
    fn from(p: Provenance) -> Self {
        match p {
            Provenance::Observed => Self::Observed,
            Provenance::AgentClaim => Self::AgentClaim,
            Provenance::Runtime => Self::AgentRuntime,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EvidenceRef {
    pub class: EvidenceClass,
    /// Where it is in the record: `execution.exit_code`, `trace#10`, ...
    pub source: String,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Cause {
    pub category: Category,
    pub source: CauseSource,
    /// The rule that produced it (`source: rule`).
    #[serde(default)]
    pub rule: Option<String>,
    pub confidence: Confidence,
    /// The cause the summary leads with.
    #[serde(default)]
    pub primary: bool,
    pub explanation: String,
    #[serde(default)]
    pub evidence: Vec<EvidenceRef>,
    /// When a person recorded it (`source: human`).
    #[serde(default)]
    pub recorded_at: Option<String>,
}

/// A diagnosis someone or something *proposed*, e.g. a model. Kept apart from
/// the causes and never used by the rules.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Suggestion {
    /// Who proposed it: `model:<id>`, `tool:<name>`.
    pub source: String,
    #[serde(default)]
    pub category: Option<Category>,
    pub text: String,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FailureAnalysis {
    pub schema: String,
    pub rules_version: String,
    pub analyzed_at: String,
    /// The run's status when it was analyzed.
    pub status: RunStatus,
    /// What verification established, copied for the reader.
    #[serde(default)]
    pub verified: Option<Verdict>,
    /// One sentence built from the primary cause.
    pub summary: String,
    #[serde(default)]
    pub causes: Vec<Cause>,
    /// What the evidence cannot tell.
    #[serde(default)]
    pub unknowns: Vec<String>,
    #[serde(default)]
    pub suggestions: Vec<Suggestion>,
}

impl FailureAnalysis {
    pub fn primary(&self) -> Option<&Cause> {
        self.causes.iter().find(|c| c.primary)
    }
}

/// A person's assessment of a cause, recorded beside the rules' causes.
#[derive(Debug, Clone, Deserialize)]
pub struct HumanAssessment {
    pub category: Category,
    #[serde(default)]
    pub note: String,
    #[serde(default = "default_human_confidence")]
    pub confidence: Confidence,
}

fn default_human_confidence() -> Confidence {
    Confidence::High
}

struct Builder {
    causes: Vec<Cause>,
    unknowns: Vec<String>,
}

impl Builder {
    fn cause(
        &mut self,
        category: Category,
        rule: &str,
        confidence: Confidence,
        explanation: impl Into<String>,
        evidence: Vec<EvidenceRef>,
    ) {
        self.causes.push(Cause {
            category,
            source: CauseSource::Rule,
            rule: Some(rule.to_string()),
            confidence,
            primary: false,
            explanation: explanation.into(),
            evidence,
            recorded_at: None,
        });
    }
}

fn ev(class: EvidenceClass, source: &str, detail: impl Into<String>) -> EvidenceRef {
    EvidenceRef {
        class,
        source: source.to_string(),
        detail: detail.into(),
    }
}

fn from_signal(s: &Signal) -> EvidenceRef {
    ev(s.provenance.into(), &s.source, s.detail.clone())
}

/// The agent's normalized evidence for this run.
pub fn run_signals(run: &RunRecord) -> Vec<Signal> {
    adapters::signals(
        &run.agent.id,
        run.execution.as_ref().and_then(|e| e.agent_report.as_ref()),
        run.agent_trace.as_ref(),
    )
}

/// Analyzes a run with the rules, keeping any human causes and suggestions
/// an earlier analysis held.
pub fn analyze(run: &RunRecord) -> FailureAnalysis {
    let signals = run_signals(run);
    let mut b = Builder {
        causes: Vec::new(),
        unknowns: Vec::new(),
    };
    let exec = run.execution.as_ref();
    let has = |k: fn(&SignalKind) -> bool| signals.iter().filter(move |s| k(&s.kind));

    // ---- what stopped the run before anything could be judged ----------
    if let Some(i) = &run.interruption {
        b.cause(
            Category::ProcessOrInfrastructure,
            "R1-interrupted",
            Confidence::High,
            "Atomic Chat stopped while the run was in progress.",
            vec![ev(
                EvidenceClass::Observed,
                "interruption",
                format!(
                    "detected at {} while {:?}",
                    i.detected_at, i.previous_status
                ),
            )],
        );
    }
    if let Some(e) = exec {
        if e.outcome == ExecutionOutcome::FailedToStart {
            b.cause(
                Category::ProcessOrInfrastructure,
                "R2-failed-to-start",
                Confidence::High,
                "The agent could not be started, so it did no work.",
                vec![ev(
                    EvidenceClass::Observed,
                    "execution.error",
                    e.error.clone().unwrap_or_default(),
                )],
            );
        }
        if e.outcome == ExecutionOutcome::TimedOut {
            b.cause(
                Category::TimeoutOrLimit,
                "R3-time-limit",
                Confidence::High,
                "The task's time limit stopped the agent.",
                vec![ev(
                    EvidenceClass::Observed,
                    "execution.outcome",
                    format!("stopped after {} ms", e.duration_ms),
                )],
            );
        }
    }
    for s in has(|k| matches!(k, SignalKind::LimitReached(_))) {
        b.cause(
            Category::TimeoutOrLimit,
            "R4-agent-limit",
            Confidence::High,
            "One of the agent's own limits (turns, executions, time) stopped it.",
            vec![from_signal(s)],
        );
    }
    for s in has(|k| matches!(k, SignalKind::ModelCallFailed(_))) {
        let length = matches!(&s.kind, SignalKind::ModelCallFailed(k) if k == "length_limited");
        if length {
            b.cause(
                Category::DecisionFormat,
                "R5-reply-cut-off",
                Confidence::Medium,
                "A model reply stopped at its output-token cap, so it could not be used.",
                vec![from_signal(s)],
            );
        } else {
            b.cause(
                Category::ProcessOrInfrastructure,
                "R6-model-call-failed",
                Confidence::Medium,
                "A call to the model endpoint failed.",
                vec![from_signal(s)],
            );
        }
    }
    let invalid: Vec<EvidenceRef> = has(|k| matches!(k, SignalKind::InvalidDecision))
        .map(from_signal)
        .collect();
    if !invalid.is_empty() {
        b.cause(
            Category::DecisionFormat,
            "R7-invalid-decision",
            Confidence::High,
            "The model produced replies the agent could not use as decisions.",
            invalid,
        );
    }
    let authority: Vec<EvidenceRef> = has(|k| matches!(k, SignalKind::AuthorityRejection))
        .map(from_signal)
        .collect();
    if !authority.is_empty() {
        b.cause(
            Category::SafetyOrAuthority,
            "R8-authority",
            Confidence::High,
            "An action was refused, or evidence rejected, on authority grounds.",
            authority,
        );
    }

    // ---- how the agent ended ---------------------------------------------
    let verified = run.evaluation.verified;
    let failed_checks: Vec<EvidenceRef> = run
        .verification
        .iter()
        .filter(|c| matches!(c.outcome, CheckOutcome::Failed | CheckOutcome::TimedOut))
        .map(|c| {
            ev(
                EvidenceClass::Verified,
                &format!("verification[{}]", c.name),
                format!(
                    "{} → {} (exit {})",
                    c.command_line,
                    if c.outcome == CheckOutcome::TimedOut {
                        "timed out"
                    } else {
                        "failed"
                    },
                    c.exit_code.map_or("none".to_string(), |e| e.to_string())
                ),
            )
        })
        .collect();
    let changed = run
        .changes
        .as_ref()
        .map(|c| c.added.len() + c.modified.len() + c.deleted.len())
        .unwrap_or(0);
    let changes_ev = || {
        ev(
            EvidenceClass::Observed,
            "changes",
            format!("{changed} files added, modified or deleted"),
        )
    };
    let gave_up: Vec<&Signal> = has(|k| matches!(k, SignalKind::GaveUp)).collect();
    let claimed: Vec<&Signal> = has(|k| matches!(k, SignalKind::CompletionClaimed)).collect();
    let terminal: Vec<&Signal> = has(|k| matches!(k, SignalKind::TerminalState(_))).collect();
    if verified == Some(Verdict::Failure) {
        if !gave_up.is_empty() {
            let mut e: Vec<EvidenceRef> = terminal.iter().map(|s| from_signal(s)).collect();
            e.extend(gave_up.iter().map(|s| from_signal(s)));
            e.push(changes_ev());
            b.cause(
                Category::GoalNotCompleted,
                "R9-gave-up",
                Confidence::High,
                "The agent stopped without finishing the work, and the checks failed.",
                e,
            );
        } else if !claimed.is_empty() {
            let mut e: Vec<EvidenceRef> = claimed.iter().map(|s| from_signal(s)).collect();
            e.push(changes_ev());
            if changed > 0 {
                b.cause(
                    Category::IncorrectImplementation,
                    "R10-claimed-but-failed",
                    Confidence::High,
                    "The agent changed the project and claimed it was done, but the checks failed.",
                    e,
                );
            } else {
                b.cause(
                    Category::GoalNotCompleted,
                    "R11-claimed-without-change",
                    Confidence::High,
                    "The agent claimed it was done without changing anything, and the checks failed.",
                    e,
                );
            }
        }
        if changed > 0 && claimed.is_empty() {
            b.cause(
                Category::IncorrectImplementation,
                "R12-changed-but-failed",
                Confidence::Medium,
                "The project was changed, but the checks failed: the change is incorrect or incomplete.",
                vec![changes_ev()],
            );
        }
        b.cause(
            Category::VerificationFailure,
            "R13-checks-failed",
            Confidence::High,
            "One or more verification checks failed.",
            failed_checks.clone(),
        );
    }
    for s in has(|k| matches!(k, SignalKind::CapabilityFailed(_))) {
        b.cause(
            Category::ToolOrCapability,
            "R14-capability-failed",
            Confidence::Medium,
            "A capability the agent ran failed. Whether this caused the outcome is not established.",
            vec![from_signal(s)],
        );
    }
    for s in has(|k| matches!(k, SignalKind::RepeatedAction(_))) {
        let discovery = matches!(&s.kind, SignalKind::RepeatedAction(c)
            if ["project.search", "project.read", "project.list"].contains(&c.as_str()));
        if discovery {
            b.cause(
                Category::ContextOrDiscovery,
                "R21-repeated-discovery",
                Confidence::Medium,
                "The agent repeated the same search or read without new results until its runtime stopped it.",
                vec![from_signal(s)],
            );
        }
    }
    for s in has(|k| matches!(k, SignalKind::ContextLimit)) {
        b.cause(
            Category::ContextOrDiscovery,
            "R15-context-limit",
            Confidence::Medium,
            "The model's context reached its limit.",
            vec![from_signal(s)],
        );
    }
    if run.status != RunStatus::Succeeded {
        for s in has(|k| matches!(k, SignalKind::NoDiscovery)) {
            b.cause(
                Category::ContextOrDiscovery,
                "R16-no-discovery",
                Confidence::Low,
                "The agent stopped without looking at the project; it may have lacked the context to act.",
                vec![from_signal(s)],
            );
        }
    }

    // ---- what could not be established ------------------------------------
    if verified == Some(Verdict::Inconclusive) {
        let unavailable: Vec<EvidenceRef> = run
            .verification
            .iter()
            .filter(|c| c.outcome == CheckOutcome::FailedToStart)
            .map(|c| {
                ev(
                    EvidenceClass::Observed,
                    &format!("verification[{}]", c.name),
                    c.error.clone().unwrap_or_else(|| "could not start".into()),
                )
            })
            .collect();
        b.cause(
            Category::ProcessOrInfrastructure,
            "R17-check-could-not-run",
            Confidence::Medium,
            "A verification command could not run.",
            unavailable,
        );
        b.cause(
            Category::InsufficientEvidence,
            "R18-unverified",
            Confidence::High,
            "Verification did not establish an outcome.",
            vec![],
        );
    }
    if run.evaluation.awaiting_review {
        b.cause(
            Category::InsufficientEvidence,
            "R19-awaiting-review",
            Confidence::High,
            "The run waits for a person's review.",
            vec![],
        );
    }
    let failed = matches!(run.status, RunStatus::Failed | RunStatus::Inconclusive);
    let explained = b
        .causes
        .iter()
        .any(|c| c.category != Category::VerificationFailure);
    if failed && !explained {
        b.cause(
            Category::InsufficientEvidence,
            "R20-unexplained",
            Confidence::High,
            "The evidence does not show why the run failed.",
            vec![],
        );
    }

    // ---- unknowns ---------------------------------------------------------
    match run.agent_trace.as_ref() {
        None => b
            .unknowns
            .push("The agent's own trajectory was not captured for this run.".into()),
        Some(t) if !t.available => b.unknowns.push(format!(
            "The agent's trajectory is unavailable: {}",
            t.unavailable_reason
                .as_deref()
                .unwrap_or("no reason recorded")
        )),
        Some(t) => {
            if t.replies.is_empty() {
                b.unknowns.push(match &t.replies_unavailable_reason {
                    Some(r) => format!("What the model replied is not recorded: {r}"),
                    None => "What the model replied is not recorded.".into(),
                });
            }
        }
    }
    if gave_up
        .iter()
        .any(|s| s.provenance == Provenance::AgentClaim)
    {
        b.unknowns.push(
            "Why the model stopped is known only from its own stated reason, which is a claim; \
             its reasoning is not captured."
                .into(),
        );
    }
    if run.agent.version.is_none() && exec.is_some() {
        b.unknowns
            .push("The agent's version was not reported.".into());
    }
    if run.status == RunStatus::Cancelled {
        b.unknowns.push(
            "The run was cancelled before an outcome was established; how it would have ended is unknown."
                .into(),
        );
    }
    if run.evaluation.method == "manual" && run.evaluation.human.is_some() && failed {
        b.unknowns.push(
            "The reviewer's verdict says the run failed; it does not by itself say why.".into(),
        );
    }

    // ---- keep the human layer, pick the primary ------------------------------
    let previous = run.analysis.as_ref();
    let mut causes = b.causes;
    if let Some(p) = previous {
        causes.extend(
            p.causes
                .iter()
                .filter(|c| c.source == CauseSource::Human)
                .cloned(),
        );
    }
    choose_primary(&mut causes);
    let summary = summarize(run, &causes);
    FailureAnalysis {
        schema: ANALYSIS_SCHEMA.to_string(),
        rules_version: RULES_VERSION.to_string(),
        analyzed_at: super::run::now(),
        status: run.status,
        verified,
        summary,
        causes,
        unknowns: b.unknowns,
        suggestions: previous.map(|p| p.suggestions.clone()).unwrap_or_default(),
    }
}

/// A person's cause comes first; otherwise the most decisive rule, the more
/// confident one breaking a tie.
fn choose_primary(causes: &mut [Cause]) {
    for c in causes.iter_mut() {
        c.primary = false;
    }
    let pick = causes
        .iter()
        .enumerate()
        .min_by_key(|(_, c)| {
            (
                c.source != CauseSource::Human,
                c.confidence,
                c.category.priority(),
            )
        })
        .map(|(i, _)| i);
    if let Some(i) = pick {
        causes[i].primary = true;
    }
}

fn summarize(run: &RunRecord, causes: &[Cause]) -> String {
    let checks = run.verification.len();
    let passed = run
        .verification
        .iter()
        .filter(|c| c.outcome == CheckOutcome::Passed)
        .count();
    match run.status {
        RunStatus::Succeeded if run.evaluation.method == "manual" => {
            "Judged successful by a reviewer.".into()
        }
        RunStatus::Succeeded => format!("Verified success: {passed} of {checks} checks passed."),
        RunStatus::Cancelled => "Cancelled before an outcome was established.".into(),
        RunStatus::Queued | RunStatus::Running => "Still running; not analyzed yet.".into(),
        _ => match causes.iter().find(|c| c.primary) {
            Some(c) => format!(
                "{} ({}, {} confidence). {}",
                label(c.category),
                match c.source {
                    CauseSource::Rule => "rule",
                    CauseSource::Human => "reviewer",
                },
                match c.confidence {
                    Confidence::High => "high",
                    Confidence::Medium => "medium",
                    Confidence::Low => "low",
                },
                c.explanation
            ),
            None => "No cause could be established.".into(),
        },
    }
}

pub fn label(c: Category) -> &'static str {
    match c {
        Category::ProcessOrInfrastructure => "Process or infrastructure failure",
        Category::TimeoutOrLimit => "Timeout or resource limit",
        Category::SafetyOrAuthority => "Safety or authority rejection",
        Category::DecisionFormat => "Model output or decision-format failure",
        Category::GoalNotCompleted => "Goal not completed",
        Category::IncorrectImplementation => "Incorrect or incomplete implementation",
        Category::ToolOrCapability => "Tool or capability failure",
        Category::ContextOrDiscovery => "Context or discovery failure",
        Category::VerificationFailure => "Verification failure",
        Category::InsufficientEvidence => "Insufficient evidence to classify",
    }
}

/// Records a person's cause and re-chooses the primary. The rules' causes are
/// left as they are.
pub fn add_human_cause(analysis: &mut FailureAnalysis, a: HumanAssessment, run: &RunRecord) {
    analysis.causes.push(Cause {
        category: a.category,
        source: CauseSource::Human,
        rule: None,
        confidence: a.confidence,
        primary: false,
        explanation: if a.note.trim().is_empty() {
            "Recorded by a reviewer.".into()
        } else {
            a.note.trim().to_string()
        },
        evidence: vec![ev(EvidenceClass::Human, "assessment", a.note.trim())],
        recorded_at: Some(super::run::now()),
    });
    choose_primary(&mut analysis.causes);
    analysis.summary = summarize(run, &analysis.causes);
}

/// Stores a proposed diagnosis. It never becomes a cause.
pub fn add_suggestion(analysis: &mut FailureAnalysis, suggestion: Suggestion) {
    analysis.suggestions.push(suggestion);
}
