//! Agent Workshop tests: failure analysis, run timeline, regression tasks,
//! baselines, regression suites and the export's added sections. Agent
//! evidence below has the shape of real stack-agent 0.1.0 output (the
//! `work --json` report and `agent-state show --json`), recorded against a
//! local model; no model or agent runs here.

use std::collections::HashMap;
use std::path::Path;

use serde_json::{json, Value};

use super::analysis::{self, Category, CauseSource, Confidence, HumanAssessment, Suggestion};
use super::baseline;
use super::evidence::{self, AgentTrace, Provenance};
use super::executor;
use super::export;
use super::regression::{self, RegressionRequest};
use super::regression_suite::{
    self, Change, ResultClass, SuiteRun, SuiteRunEntry, SUITE_RUN_SCHEMA,
};
use super::run::*;
use super::store::Store;
use super::suite;
use super::task::*;
use super::timeline::{self, EntryClass};

fn builtin(id: &str) -> BenchTask {
    suite::builtin_tasks()
        .into_iter()
        .find(|t| t.id == id)
        .unwrap()
}

fn check(name: &str, outcome: CheckOutcome, exit: Option<i32>) -> CheckResult {
    CheckResult {
        name: name.into(),
        command_line: format!("cargo test --offline --test {name}"),
        outcome,
        exit_code: exit,
        duration_ms: 100,
        stdout: Default::default(),
        stderr: Default::default(),
        error: (outcome == CheckOutcome::FailedToStart).then(|| "could not start cargo".into()),
        started_at: Some("2026-10-26T18:20:00.000Z".into()),
    }
}

fn exec(outcome: ExecutionOutcome, exit: Option<i32>, report: Option<Value>) -> ExecutionRecord {
    ExecutionRecord {
        outcome,
        command_line: "stack-agent work …".into(),
        exit_code: exit,
        signal: None,
        duration_ms: 19055,
        stdout: Default::default(),
        stderr: Default::default(),
        agent_report: report,
        error: None,
        started_at: Some("2026-10-26T18:19:20.100Z".into()),
        ended_at: Some("2026-10-26T18:19:39.200Z".into()),
    }
}

/// The report stack-agent printed for the real 1.5B run: it escalated on turn
/// 1 without executing anything.
fn escalated_report() -> Value {
    json!({
        "terminal_state": "escalated",
        "outcome_reason": "The issue is likely related to the rounding logic in the library code.",
        "model_failures": [], "invalid_decisions": 0, "malformed_replies": 0,
        "executions_by_capability": {}, "lists": 0, "searches": 0, "reads": 0,
        "writes": 0, "paths_written": [], "verified": false,
        "evidence": {"invalidated": null, "test_files_replaced": []},
        "audit": {"clean": true, "unauthorized_executions": 0, "path_escape": 0},
        "agent_state": {"recorded": true, "task": {"task_id": "work_2b603a0519730e90"}}
    })
}

/// The trajectory of the real 3B run: a git capability failed, then it escalated.
fn git_failure_trace() -> AgentTrace {
    let show = json!({"events": [
        {"attempt": 2, "index": 0, "recorded_at_ms": 1793038760698i64, "provenance": "runtime_decided",
         "event": {"kind": "WorkStarted", "max_turns": 16, "max_executions": 32}},
        {"attempt": 2, "index": 5, "recorded_at_ms": 1793038779623i64, "provenance": "host_observed",
         "event": {"kind": "ModelCalled", "turn": 1, "succeeded": true, "prompt_tokens": 1325, "completion_tokens": 24}},
        {"attempt": 2, "index": 6, "recorded_at_ms": 1793038779700i64, "provenance": "model_inferred",
         "event": {"kind": "DecisionMade", "turn": 1, "decision": "request project.git.status"}},
        {"attempt": 2, "index": 10, "recorded_at_ms": 1793038779750i64, "provenance": "host_observed",
         "event": {"kind": "ExecutionFailed", "execution_id": "x-exec-1",
                   "reason": "{\"capability\":\"project.git.status\",\"error\":\"not_a_repository\"}"}},
        {"attempt": 2, "index": 17, "recorded_at_ms": 1793038790000i64, "provenance": "model_inferred",
         "event": {"kind": "DecisionMade", "turn": 2, "decision": "escalate"}},
        {"attempt": 2, "index": 18, "recorded_at_ms": 1793038790001i64, "provenance": "runtime_decided",
         "event": {"kind": "WorkEscalated", "reason": "the project is not a repository"}}
    ]});
    AgentTrace {
        source: "stack-agent agent-state show --json".into(),
        available: true,
        unavailable_reason: None,
        events: super::adapters::stack_agent_trace(&show, &super::redact::Redactor::default()),
        replies: vec![],
        replies_unavailable_reason: None,
    }
}

fn finished(store: &Store, task: &BenchTask, configure: impl FnOnce(&mut RunRecord)) -> RunRecord {
    let mut run = executor::new_record(store, task, "stack-agent", "9.9.9");
    run.transition(RunStatus::Running).unwrap();
    run.model = Some("qwen2.5-coder-1.5b-instruct-q4_k_m".into());
    run.provider = Some("atomic-chat-local-api".into());
    run.agent.version = Some("stack-agent 0.1.0".into());
    run.changes = Some(ChangeSummary::default());
    configure(&mut run);
    if run.status == RunStatus::Running {
        run.transition(RunStatus::Evaluating).unwrap();
        let verdict = executor::verdict_of(&run.verification);
        run.evaluation.verified = Some(verdict);
        run.transition(match verdict {
            Verdict::Success => RunStatus::Succeeded,
            Verdict::Failure => RunStatus::Failed,
            Verdict::Inconclusive => RunStatus::Inconclusive,
        })
        .unwrap();
    }
    run
}

fn categories(run: &RunRecord) -> Vec<(Category, Confidence, bool)> {
    run.analysis
        .as_ref()
        .unwrap()
        .causes
        .iter()
        .map(|c| (c.category, c.confidence, c.primary))
        .collect()
}

fn primary(run: &RunRecord) -> Category {
    run.analysis.as_ref().unwrap().primary().unwrap().category
}

// ---- failure analysis ----------------------------------------------------------

#[test]
fn a_real_escalation_is_classified_from_verified_evidence() {
    let data = tempfile::tempdir().unwrap();
    let store = Store::new(data.path());
    let run = finished(&store, &builtin("ledger-fix-discount"), |r| {
        r.execution = Some(exec(
            ExecutionOutcome::ExitedNonZero,
            Some(1),
            Some(escalated_report()),
        ));
        r.verification = vec![
            check("pricing", CheckOutcome::Failed, Some(101)),
            check("all", CheckOutcome::Failed, Some(101)),
        ];
    });
    let a = run.analysis.as_ref().unwrap();
    assert_eq!(a.schema, analysis::ANALYSIS_SCHEMA);
    assert_eq!(a.rules_version, analysis::RULES_VERSION);
    assert_eq!(primary(&run), Category::GoalNotCompleted);
    let cats = categories(&run);
    assert!(cats.contains(&(Category::GoalNotCompleted, Confidence::High, true)));
    assert!(cats.contains(&(Category::VerificationFailure, Confidence::High, false)));
    assert!(cats.contains(&(Category::ContextOrDiscovery, Confidence::Low, false)));
    // Every cause names its source and evidence; the give-up reason is a claim.
    for c in &a.causes {
        assert_eq!(c.source, CauseSource::Rule);
        assert!(c.rule.is_some());
    }
    let goal = a
        .causes
        .iter()
        .find(|c| c.category == Category::GoalNotCompleted)
        .unwrap();
    assert!(goal
        .evidence
        .iter()
        .any(|e| e.class == analysis::EvidenceClass::AgentClaim && e.detail.contains("rounding")));
    let checks = a
        .causes
        .iter()
        .find(|c| c.category == Category::VerificationFailure)
        .unwrap();
    assert!(checks
        .evidence
        .iter()
        .all(|e| e.class == analysis::EvidenceClass::Verified));
    assert_eq!(checks.evidence.len(), 2);
    assert!(a.summary.starts_with("Goal not completed"));
    assert!(a
        .unknowns
        .iter()
        .any(|u| u.contains("trajectory was not captured")));
    assert!(a.unknowns.iter().any(|u| u.contains("stated reason")));
}

#[test]
fn a_failed_capability_is_a_contributing_cause_not_the_verdict() {
    let data = tempfile::tempdir().unwrap();
    let store = Store::new(data.path());
    let run = finished(&store, &builtin("ledger-fix-discount"), |r| {
        r.execution = Some(exec(
            ExecutionOutcome::ExitedNonZero,
            Some(1),
            Some(escalated_report()),
        ));
        r.agent_trace = Some(git_failure_trace());
        r.verification = vec![check("pricing", CheckOutcome::Failed, Some(101))];
    });
    assert_eq!(primary(&run), Category::GoalNotCompleted);
    let tool = run
        .analysis
        .as_ref()
        .unwrap()
        .causes
        .iter()
        .find(|c| c.category == Category::ToolOrCapability)
        .expect("the failed git capability is a cause");
    assert_eq!(tool.confidence, Confidence::Medium);
    assert!(!tool.primary);
    assert_eq!(tool.evidence[0].source, "trace#10");
    assert!(tool.evidence[0].detail.contains("not_a_repository"));
}

#[test]
fn a_loop_guard_stop_is_the_runtimes_not_the_models() {
    // The real 3B run of ledger-explain-rounding: ten turns, then blocked.
    let data = tempfile::tempdir().unwrap();
    let store = Store::new(data.path());
    let mut report = escalated_report();
    report["terminal_state"] = json!("blocked");
    report["outcome_reason"] = json!(
        "stuck: project.search was requested 3 times in a row with the same inputs and returned the same observation each time. Nothing new was learned."
    );
    report["executions_by_capability"] =
        json!({"project.search": 6, "project.list": 1, "project.read": 1});
    report["searches"] = json!(6);
    report["reads"] = json!(1);
    let run = finished(&store, &builtin("ledger-explain-rounding"), |r| {
        r.execution = Some(exec(ExecutionOutcome::ExitedNonZero, Some(1), Some(report)));
        r.verification = vec![check("bench_answer", CheckOutcome::Failed, Some(101))];
    });
    let a = run.analysis.as_ref().unwrap();
    assert_eq!(primary(&run), Category::GoalNotCompleted);
    let repeated = a
        .causes
        .iter()
        .find(|c| c.rule.as_deref() == Some("R21-repeated-discovery"))
        .expect("the repeated search is a discovery cause");
    assert_eq!(repeated.category, Category::ContextOrDiscovery);
    assert_eq!(
        repeated.evidence[0].class,
        analysis::EvidenceClass::AgentRuntime
    );
    let goal = a
        .causes
        .iter()
        .find(|c| c.category == Category::GoalNotCompleted)
        .unwrap();
    assert!(goal
        .evidence
        .iter()
        .all(|e| e.class != analysis::EvidenceClass::AgentClaim));
    assert!(
        !a.unknowns.iter().any(|u| u.contains("stated reason")),
        "no model claim here"
    );
    assert!(!a
        .causes
        .iter()
        .any(|c| c.rule.as_deref() == Some("R16-no-discovery")));
}

#[test]
fn writes_followed_by_giving_up_are_an_incomplete_implementation() {
    // The real 3B run of ledger-add-truncate: six whole-file writes, then escalated.
    let data = tempfile::tempdir().unwrap();
    let store = Store::new(data.path());
    let mut report = escalated_report();
    report["executions_by_capability"] = json!({"project.write": 6});
    report["writes"] = json!(6);
    let run = finished(&store, &builtin("ledger-add-truncate"), |r| {
        r.execution = Some(exec(ExecutionOutcome::ExitedNonZero, Some(1), Some(report)));
        r.changes = Some(ChangeSummary {
            modified: vec!["src/text.rs".into()],
            ..Default::default()
        });
        r.verification = vec![
            check("bench_truncate", CheckOutcome::Failed, Some(101)),
            check("all", CheckOutcome::Failed, Some(101)),
        ];
    });
    assert_eq!(primary(&run), Category::GoalNotCompleted);
    assert!(categories(&run).contains(&(
        Category::IncorrectImplementation,
        Confidence::Medium,
        false
    )));
    assert!(categories(&run).contains(&(Category::ContextOrDiscovery, Confidence::Low, false)));
}

#[test]
fn claimed_completion_with_failing_checks_is_an_incorrect_implementation() {
    let data = tempfile::tempdir().unwrap();
    let store = Store::new(data.path());
    let mut report = escalated_report();
    report["terminal_state"] = json!("completed");
    report["verified"] = json!(true);
    let changed = finished(&store, &builtin("ledger-fix-discount"), |r| {
        r.execution = Some(exec(
            ExecutionOutcome::Exited,
            Some(0),
            Some(report.clone()),
        ));
        r.changes = Some(ChangeSummary {
            modified: vec!["src/pricing.rs".into()],
            ..Default::default()
        });
        r.verification = vec![check("pricing", CheckOutcome::Failed, Some(101))];
    });
    assert_eq!(primary(&changed), Category::IncorrectImplementation);
    let unchanged = finished(&store, &builtin("ledger-fix-discount"), |r| {
        r.execution = Some(exec(
            ExecutionOutcome::Exited,
            Some(0),
            Some(report.clone()),
        ));
        r.verification = vec![check("pricing", CheckOutcome::Failed, Some(101))];
    });
    assert_eq!(primary(&unchanged), Category::GoalNotCompleted);
    assert_eq!(
        unchanged.status,
        RunStatus::Failed,
        "exit 0 never decides success"
    );
}

#[test]
fn stopping_causes_take_precedence() {
    let data = tempfile::tempdir().unwrap();
    let store = Store::new(data.path());
    let task = builtin("ledger-fix-discount");
    let mut report = escalated_report();
    report["terminal_state"] = json!("failed");
    report["invalid_decisions"] = json!(3);
    let invalid = finished(&store, &task, |r| {
        r.execution = Some(exec(ExecutionOutcome::ExitedNonZero, Some(4), Some(report)));
        r.verification = vec![check("pricing", CheckOutcome::Failed, Some(101))];
    });
    assert_eq!(primary(&invalid), Category::DecisionFormat);

    let timed_out = finished(&store, &task, |r| {
        r.execution = Some(exec(ExecutionOutcome::TimedOut, None, None));
        r.verification = vec![check("pricing", CheckOutcome::Failed, Some(101))];
    });
    assert_eq!(primary(&timed_out), Category::TimeoutOrLimit);

    let mut limit = escalated_report();
    limit["terminal_state"] = json!("limit_reached");
    let at_limit = finished(&store, &task, |r| {
        r.execution = Some(exec(ExecutionOutcome::ExitedNonZero, Some(1), Some(limit)));
        r.verification = vec![check("pricing", CheckOutcome::Failed, Some(101))];
    });
    assert_eq!(primary(&at_limit), Category::TimeoutOrLimit);

    let mut refused = escalated_report();
    refused["evidence"]["test_files_replaced"] = json!(["tests/pricing.rs"]);
    let authority = finished(&store, &task, |r| {
        r.execution = Some(exec(
            ExecutionOutcome::ExitedNonZero,
            Some(1),
            Some(refused),
        ));
        r.verification = vec![check("pricing", CheckOutcome::Failed, Some(101))];
    });
    assert_eq!(primary(&authority), Category::SafetyOrAuthority);

    let mut not_started = executor::new_record(&store, &task, "stack-agent", "1");
    not_started.transition(RunStatus::Running).unwrap();
    not_started.execution = Some(ExecutionRecord {
        error: Some("stack-agent is not installed".into()),
        ..exec(ExecutionOutcome::FailedToStart, None, None)
    });
    not_started.failure_kind = Some(FailureKind::Execution);
    not_started.transition(RunStatus::Failed).unwrap();
    assert_eq!(primary(&not_started), Category::ProcessOrInfrastructure);
}

#[test]
fn missing_evidence_is_classified_as_insufficient_and_listed_as_unknown() {
    let data = tempfile::tempdir().unwrap();
    let store = Store::new(data.path());
    let task = builtin("ledger-fix-discount");
    // An agent that left no report or trajectory, and failed checks.
    let mut run = executor::new_record(&store, &task, "claude-code", "1");
    run.transition(RunStatus::Running).unwrap();
    run.execution = Some(exec(ExecutionOutcome::ExitedNonZero, Some(1), None));
    run.verification = vec![check("pricing", CheckOutcome::Failed, Some(101))];
    run.transition(RunStatus::Evaluating).unwrap();
    run.evaluation.verified = Some(Verdict::Failure);
    run.transition(RunStatus::Failed).unwrap();
    assert_eq!(primary(&run), Category::InsufficientEvidence);
    assert!(categories(&run).contains(&(Category::VerificationFailure, Confidence::High, false)));
    assert!(run
        .analysis
        .as_ref()
        .unwrap()
        .unknowns
        .iter()
        .any(|u| u.contains("trajectory")));

    // Verification that could not run: nothing established.
    let unverified = finished(&store, &task, |r| {
        r.execution = Some(exec(ExecutionOutcome::Exited, Some(0), None));
        r.verification = vec![check("pricing", CheckOutcome::FailedToStart, None)];
    });
    assert_eq!(unverified.status, RunStatus::Inconclusive);
    let cats = categories(&unverified);
    assert!(cats.iter().any(|c| c.0 == Category::InsufficientEvidence));
    assert!(cats
        .iter()
        .any(|c| c.0 == Category::ProcessOrInfrastructure));

    // Success has no cause.
    let ok = finished(&store, &task, |r| {
        r.execution = Some(exec(ExecutionOutcome::Exited, Some(0), None));
        r.verification = vec![check("pricing", CheckOutcome::Passed, Some(0))];
    });
    assert!(ok.analysis.as_ref().unwrap().causes.is_empty());
    assert_eq!(
        ok.analysis.as_ref().unwrap().summary,
        "Verified success: 1 of 1 checks passed."
    );

    // Cancelled: no cause, and the outcome is an unknown.
    let mut cancelled = executor::new_record(&store, &task, "stack-agent", "1");
    cancelled.transition(RunStatus::Cancelled).unwrap();
    let a = cancelled.analysis.as_ref().unwrap();
    assert!(a.causes.is_empty());
    assert!(a.unknowns.iter().any(|u| u.contains("cancelled")));
}

#[test]
fn interrupted_runs_are_analyzed_on_recovery() {
    let data = tempfile::tempdir().unwrap();
    let store = Store::new(data.path());
    let mut run = executor::new_record(&store, &builtin("ledger-fix-discount"), "stack-agent", "1");
    run.transition(RunStatus::Running).unwrap();
    store.save_run(&run).unwrap();
    Store::new(data.path()).recover_stale_runs().unwrap();
    let r = store.get_run(&run.id).unwrap();
    assert_eq!(r.status, RunStatus::Inconclusive);
    assert_eq!(primary(&r), Category::ProcessOrInfrastructure);
    assert_eq!(
        r.analysis.as_ref().unwrap().causes[0].rule.as_deref(),
        Some("R1-interrupted")
    );
}

#[test]
fn suggestions_and_human_causes_never_change_the_rules_causes() {
    let data = tempfile::tempdir().unwrap();
    let store = Store::new(data.path());
    let mut run = finished(&store, &builtin("ledger-fix-discount"), |r| {
        r.execution = Some(exec(
            ExecutionOutcome::ExitedNonZero,
            Some(1),
            Some(escalated_report()),
        ));
        r.verification = vec![check("pricing", CheckOutcome::Failed, Some(101))];
    });
    let rules_before: Vec<_> = run.analysis.as_ref().unwrap().causes.clone();

    // A model's suggestion is stored apart and changes no cause.
    let mut a = run.analysis.clone().unwrap();
    analysis::add_suggestion(
        &mut a,
        Suggestion {
            source: "model:some-judge".into(),
            category: Some(Category::DecisionFormat),
            text: "Probably a prompt-format problem.".into(),
            created_at: now(),
        },
    );
    assert_eq!(a.causes, rules_before);
    run.analysis = Some(a);
    run.reanalyze();
    let a = run.analysis.as_ref().unwrap();
    assert_eq!(a.suggestions.len(), 1, "suggestions survive re-analysis");
    assert!(a
        .causes
        .iter()
        .all(|c| c.category != Category::DecisionFormat));

    // A person's cause is recorded beside the rules' and leads the summary.
    let mut a = run.analysis.clone().unwrap();
    analysis::add_human_cause(
        &mut a,
        HumanAssessment {
            category: Category::ContextOrDiscovery,
            note: "The prompt never mentions src/money.rs.".into(),
            confidence: Confidence::Medium,
        },
        &run,
    );
    let human = a
        .causes
        .iter()
        .find(|c| c.source == CauseSource::Human)
        .unwrap();
    assert!(human.primary);
    assert!(a.summary.contains("reviewer"));
    let rules_after: Vec<_> = a
        .causes
        .iter()
        .filter(|c| c.source == CauseSource::Rule)
        .cloned()
        .map(|mut c| {
            c.primary = false;
            c
        })
        .collect();
    let rules_before: Vec<_> = rules_before
        .into_iter()
        .map(|mut c| {
            c.primary = false;
            c
        })
        .collect();
    assert_eq!(rules_after, rules_before);
    run.analysis = Some(a);
    run.reanalyze();
    assert!(run
        .analysis
        .as_ref()
        .unwrap()
        .causes
        .iter()
        .any(|c| c.source == CauseSource::Human));
}

// ---- evidence parsing ------------------------------------------------------------

#[test]
fn print_reply_blocks_are_parsed_and_capped() {
    let stderr = "Model reply:\n{\n  \"decision\": \"escalate\"\n}\nModel reply:\n{\"decision\":\"request_capability\"}\nerror: something\n";
    let replies = evidence::parse_print_replies(stderr, 1024);
    assert_eq!(replies.len(), 2);
    assert!(replies[0].text.contains("escalate"));
    assert_eq!(replies[1].text, "{\"decision\":\"request_capability\"}");
    let capped = evidence::parse_print_replies("Model reply:\nééééé\n", 3);
    assert_eq!(capped[0].text, "é");
    assert!(capped[0].truncated);
}

#[test]
fn trace_events_keep_the_agents_provenance() {
    let t = git_failure_trace();
    let by = |kind: &str| t.events.iter().find(|e| e.kind == kind).unwrap();
    assert_eq!(by("WorkStarted").provenance, Provenance::Runtime);
    assert_eq!(by("ModelCalled").provenance, Provenance::Observed);
    assert_eq!(by("DecisionMade").provenance, Provenance::AgentClaim);
    assert_eq!(
        by("ExecutionFailed").summary,
        "execution failed: project.git.status: not_a_repository"
    );
    assert!(by("WorkStarted").at.as_deref().unwrap().ends_with('Z'));
}

// ---- timeline -----------------------------------------------------------------------

#[test]
fn the_timeline_orders_evidence_and_marks_what_is_missing() {
    let data = tempfile::tempdir().unwrap();
    let store = Store::new(data.path());
    let run = finished(&store, &builtin("ledger-fix-discount"), |r| {
        r.execution = Some(exec(
            ExecutionOutcome::ExitedNonZero,
            Some(1),
            Some(escalated_report()),
        ));
        r.agent_trace = Some(git_failure_trace());
        r.verification = vec![check("pricing", CheckOutcome::Failed, Some(101))];
    });
    let t = timeline::build(&run);
    let classes: Vec<EntryClass> = t.iter().map(|e| e.class).collect();
    for c in [
        EntryClass::Observed,
        EntryClass::AgentRuntime,
        EntryClass::AgentClaim,
        EntryClass::Verified,
    ] {
        assert!(classes.contains(&c), "{c:?} missing");
    }
    let timed: Vec<&str> = t.iter().filter_map(|e| e.at.as_deref()).collect();
    let mut sorted = timed.clone();
    sorted.sort();
    assert_eq!(timed, sorted, "chronological");
    let claim = t
        .iter()
        .find(|e| e.class == EntryClass::AgentClaim)
        .unwrap();
    assert!(claim.label.contains("model decided"));

    // A record from before traces and timestamps: gaps are entries, not guesses.
    let mut old = run.clone();
    old.agent_trace = None;
    if let Some(e) = old.execution.as_mut() {
        e.started_at = None;
        e.ended_at = None;
    }
    let t = timeline::build(&old);
    assert!(t
        .iter()
        .any(|e| e.class == EntryClass::Missing && e.reference == "agent_trace"));
    assert!(t
        .iter()
        .any(|e| e.class == EntryClass::Missing && e.reference == "execution.started_at"));
    assert!(!t.iter().any(|e| e.class == EntryClass::AgentClaim));

    // An unavailable trajectory says why.
    let mut unavailable = run.clone();
    unavailable.agent_trace = Some(AgentTrace {
        source: "x".into(),
        unavailable_reason: Some("stack-agent did not record this work".into()),
        ..Default::default()
    });
    let t = timeline::build(&unavailable);
    let gap = t.iter().find(|e| e.reference == "agent_trace").unwrap();
    assert_eq!(gap.class, EntryClass::Missing);
    assert_eq!(
        gap.detail.as_deref(),
        Some("stack-agent did not record this work")
    );
}

// ---- baselines -----------------------------------------------------------------------

fn git_available() -> bool {
    super::adapters::search_path("git", &std::env::var("PATH").unwrap_or_default()).is_some()
}

#[test]
fn fixture_baselines_are_fresh_and_identical_every_time() {
    let dir = tempfile::tempdir().unwrap();
    let path = std::env::var("PATH").unwrap_or_default();
    let w = WorkdirRef::Fixture {
        fixture: "ledger".into(),
    };
    let a = baseline::prepare(&w, &dir.path().join("a"), &path).unwrap();
    let b = baseline::prepare(&w, &dir.path().join("b"), &path).unwrap();
    assert!(a.fresh_workspace);
    assert_eq!(a.identity, b.identity);
    assert!(a.identity.starts_with("fixture:ledger@"));
    assert!(
        baseline::prepare(&w, &dir.path().join("a"), &path).is_err(),
        "never reused"
    );
    if git_available() {
        assert!(a.git_commit.is_some());
        assert_eq!(
            a.git_commit, b.git_commit,
            "the baseline commit is deterministic"
        );
        assert!(dir.path().join("a/.git").is_dir());
    } else {
        assert!(!a.limitations.is_empty());
    }
    let other = baseline::prepare(
        &WorkdirRef::Fixture {
            fixture: "ledger-discount-bug".into(),
        },
        &dir.path().join("c"),
        &path,
    )
    .unwrap();
    assert_ne!(other.identity, a.identity);
}

#[cfg(unix)]
fn init_repo(dir: &Path, files: &[(&str, &str)]) -> String {
    use std::process::Command;
    for (p, c) in files {
        let path = dir.join(p);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, c).unwrap();
    }
    let git = |args: &[&str]| {
        let out = Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t.invalid")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t.invalid")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };
    git(&["init", "-q"]);
    git(&["add", "-A"]);
    git(&["commit", "-q", "-m", "start"]);
    git(&["rev-parse", "HEAD"])
}

#[cfg(unix)]
#[test]
fn git_snapshots_copy_one_commit_without_credentials() {
    if !git_available() {
        return;
    }
    let repo = tempfile::tempdir().unwrap();
    let commit = init_repo(
        repo.path(),
        &[
            ("src/lib.rs", "pub fn one() -> u8 { 1 }\n"),
            (".env", "API_KEY=sk-should-never-be-copied\n"),
            ("config/.env.local", "X=1\n"),
            ("deploy/id_rsa", "PRIVATE\n"),
            ("certs/server.pem", "PRIVATE\n"),
        ],
    );
    // Uncommitted work in the repository is not part of the snapshot.
    std::fs::write(repo.path().join("untracked.txt"), "not committed").unwrap();
    let ws = tempfile::tempdir().unwrap();
    let dest = ws.path().join("ws");
    let path = std::env::var("PATH").unwrap_or_default();
    let b = baseline::prepare(
        &WorkdirRef::GitSnapshot {
            repo: repo.path().to_string_lossy().into(),
            commit: commit.clone(),
        },
        &dest,
        &path,
    )
    .unwrap();
    assert_eq!(b.identity, format!("git:{commit}"));
    assert!(dest.join("src/lib.rs").is_file());
    for p in [
        ".env",
        "config/.env.local",
        "deploy/id_rsa",
        "certs/server.pem",
        "untracked.txt",
    ] {
        assert!(!dest.join(p).exists(), "{p} was copied");
    }
    assert_eq!(b.excluded_paths.len(), 4);
    assert!(b.fresh_workspace);
    // The repository itself is untouched.
    assert!(repo.path().join("untracked.txt").exists());
}

#[cfg(unix)]
#[test]
fn path_baselines_identify_the_start_and_say_they_cannot_restore_it() {
    if !git_available() {
        return;
    }
    let repo = tempfile::tempdir().unwrap();
    let commit = init_repo(repo.path(), &[("a.txt", "a")]);
    let path = std::env::var("PATH").unwrap_or_default();
    let w = WorkdirRef::Path {
        path: repo.path().to_string_lossy().into(),
    };
    let clean = baseline::prepare(&w, Path::new("/unused"), &path).unwrap();
    assert_eq!(clean.identity, format!("git:{commit}"));
    assert_eq!(clean.git_dirty, Some(false));
    assert!(!clean.fresh_workspace);
    std::fs::write(repo.path().join("a.txt"), "changed").unwrap();
    let dirty = baseline::prepare(&w, Path::new("/unused"), &path).unwrap();
    assert_eq!(dirty.git_dirty, Some(true));
    assert!(dirty.identity.starts_with("path:"));
    assert!(dirty.limitations.iter().any(|l| l.contains("dirty")));
}

// ---- regression tasks ------------------------------------------------------------------

#[test]
fn a_regression_task_copies_its_source_without_changing_it() {
    let data = tempfile::tempdir().unwrap();
    let store = Store::new(data.path());
    let source_task = builtin("ledger-fix-discount");
    let run = finished(&store, &source_task, |r| {
        r.execution = Some(exec(
            ExecutionOutcome::ExitedNonZero,
            Some(1),
            Some(escalated_report()),
        ));
        r.verification = vec![check("pricing", CheckOutcome::Failed, Some(101))];
    });
    store.save_run(&run).unwrap();
    let run_file = data
        .path()
        .join(format!("agent_bench/runs/{}.json", run.id));
    let before = std::fs::read(&run_file).unwrap();

    let req = RegressionRequest {
        id: "fix-discount-escalates".into(),
        name: None,
        failure_description: "Escalates on turn 1 without reading src/pricing.rs.".into(),
        proposed_improvement: Some("Read the failing test before deciding.".into()),
        expected_outcome: None,
    };
    let task = store
        .create_task(regression::build(&store.get_run(&run.id).unwrap(), &req).unwrap())
        .unwrap();
    assert_eq!(task.version, 1);
    assert_eq!(task.goal, source_task.goal);
    assert_eq!(
        task.evaluation, source_task.evaluation,
        "same checks, same held-out files"
    );
    assert_eq!(task.workdir, source_task.workdir);
    assert_eq!(task.expected.as_ref().unwrap().outcome, "success");
    let o = task.origin.as_ref().unwrap();
    assert_eq!(o.source_run_id, run.id);
    assert_eq!(o.source_task_version, source_task.version);
    assert_eq!(o.source_task_digest, source_task.digest());
    assert_eq!(o.source_status, "failed");
    assert_eq!(o.source_checks[0].outcome, "failed");
    assert_eq!(
        o.source_primary_cause.as_deref(),
        Some("goal_not_completed (rule)")
    );
    assert!(o
        .limitations
        .iter()
        .any(|l| l.contains("not deterministic")));

    // Source run and source task untouched; the new task is stored and versioned.
    assert_eq!(std::fs::read(&run_file).unwrap(), before);
    assert_eq!(
        store.get_task(&source_task.id).unwrap().digest(),
        source_task.digest()
    );
    assert_eq!(
        store.get_task_version("fix-discount-escalates", 1).unwrap(),
        task
    );
    assert!(
        store
            .create_task(regression::build(&run, &req).unwrap())
            .is_err(),
        "ids are unique"
    );

    // Refusals.
    let mut bad = req.clone();
    bad.failure_description = "  ".into();
    assert!(regression::build(&run, &bad).is_err());
    let mut live = run.clone();
    live.status = RunStatus::Running;
    assert!(regression::build(&live, &req).is_err());
}

#[test]
fn a_regression_from_a_project_snapshots_a_clean_commit_only() {
    let data = tempfile::tempdir().unwrap();
    let store = Store::new(data.path());
    let mut task = builtin("ledger-fix-discount");
    task.id = "proj".into();
    task.builtin = false;
    task.workdir = WorkdirRef::Path {
        path: "/work/proj".into(),
    };
    task.evaluation = Evaluation::Command {
        commands: vec![VerificationCommand {
            name: "t".into(),
            program: "true".into(),
            args: vec![],
            timeout_secs: None,
        }],
        held_out_checks: None,
    };
    let mk = |dirty: bool| {
        let mut run = finished(&store, &task, |r| {
            r.execution = Some(exec(ExecutionOutcome::ExitedNonZero, Some(1), None));
            r.verification = vec![check("t", CheckOutcome::Failed, Some(1))];
        });
        run.baseline = Some(baseline::Baseline {
            kind: "path".into(),
            identity: "git:abcdef1".into(),
            content_digest: "d".into(),
            file_count: 1,
            git_commit: Some("abcdef1234567".into()),
            git_dirty: Some(dirty),
            source_commit: None,
            fresh_workspace: false,
            excluded_paths: vec![],
            limitations: vec![],
        });
        run
    };
    let req = RegressionRequest {
        id: "r".into(),
        name: Some("R".into()),
        failure_description: "fails".into(),
        proposed_improvement: None,
        expected_outcome: Some("success".into()),
    };
    let clean = regression::build(&mk(false), &req).unwrap();
    assert_eq!(
        clean.workdir,
        WorkdirRef::GitSnapshot {
            repo: "/work/proj".into(),
            commit: "abcdef1234567".into()
        }
    );
    let dirty = regression::build(&mk(true), &req).unwrap();
    assert_eq!(dirty.workdir, task.workdir);
    assert!(dirty
        .origin
        .unwrap()
        .limitations
        .iter()
        .any(|l| l.contains("uncommitted")));
}

#[test]
fn task_versions_are_archived_and_old_digests_are_unchanged() {
    // The digest the landed v1 recorded for this bundled task (run_20261026T171945).
    assert_eq!(
        builtin("ledger-fix-discount").digest(),
        "bfb3be2628384eab492ab340ba9b0c47b0901d348e2e2d78e2d07ff2a5b0abb8"
    );
    let data = tempfile::tempdir().unwrap();
    let store = Store::new(data.path());
    let mut t = builtin("ledger-add-truncate");
    t.id = "mine".into();
    t.builtin = false;
    let v1 = store.save_task(t.clone()).unwrap();
    t.goal.push_str(" Also keep slugify.");
    let v2 = store.save_task(t).unwrap();
    assert_eq!(v2.version, 2);
    assert_eq!(store.get_task_version("mine", 1).unwrap().goal, v1.goal);
    assert_eq!(store.get_task_version("mine", 2).unwrap().goal, v2.goal);
    store.delete_task("mine").unwrap();
    assert!(
        store.get_task_version("mine", 1).is_ok(),
        "a pinned version outlives the task"
    );
    assert!(store.get_task_version("ledger-add-truncate", 9).is_err());
}

// ---- suites -------------------------------------------------------------------------

fn suite_run(id: &str, agent: &str, entries: &[(&str, Option<&str>)]) -> SuiteRun {
    SuiteRun {
        schema: SUITE_RUN_SCHEMA.into(),
        id: id.into(),
        suite_id: "s".into(),
        suite_version: 1,
        label: String::new(),
        agent_id: agent.into(),
        model: None,
        entries: entries
            .iter()
            .map(|(task, run)| SuiteRunEntry {
                task_id: (*task).into(),
                task_version: 1,
                expected: "success".into(),
                run_id: run.map(str::to_string),
                not_started_reason: run.is_none().then(|| "stack-agent is not installed".into()),
            })
            .collect(),
        created_at: now(),
    }
}

#[test]
fn suite_aggregation_shows_its_denominator_and_exclusions() {
    let data = tempfile::tempdir().unwrap();
    let store = Store::new(data.path());
    let task = builtin("ledger-fix-discount");
    let mut runs = HashMap::new();
    let mut add = |id: &str, r: RunRecord| {
        let mut r = r;
        r.id = id.into();
        runs.insert(id.to_string(), r);
    };
    let pass = |r: &mut RunRecord| {
        r.execution = Some(exec(ExecutionOutcome::Exited, Some(0), None));
        r.verification = vec![check("p", CheckOutcome::Passed, Some(0))];
    };
    let fail = |r: &mut RunRecord| {
        r.execution = Some(exec(ExecutionOutcome::ExitedNonZero, Some(1), None));
        r.verification = vec![check("p", CheckOutcome::Failed, Some(101))];
    };
    add("ok", finished(&store, &task, pass));
    add("bad", finished(&store, &task, fail));
    add(
        "unverified",
        finished(&store, &task, |r| {
            r.execution = Some(exec(ExecutionOutcome::Exited, Some(0), None));
            r.verification = vec![check("p", CheckOutcome::FailedToStart, None)];
        }),
    );
    let mut interrupted = executor::new_record(&store, &task, "stack-agent", "1");
    interrupted.transition(RunStatus::Running).unwrap();
    interrupted.recover_if_stale();
    add("interrupted", interrupted);
    let mut cancelled = executor::new_record(&store, &task, "stack-agent", "1");
    cancelled.transition(RunStatus::Cancelled).unwrap();
    add("cancelled", cancelled);
    let mut running = executor::new_record(&store, &task, "stack-agent", "1");
    running.transition(RunStatus::Running).unwrap();
    add("running", running);

    let sr = suite_run(
        "suiterun_x",
        "stack-agent",
        &[
            ("a", Some("ok")),
            ("b", Some("bad")),
            ("c", Some("unverified")),
            ("d", Some("interrupted")),
            ("e", Some("cancelled")),
            ("f", Some("running")),
            ("g", None),
            ("h", Some("run_gone")),
        ],
    );
    let s = regression_suite::summarize(&sr, &runs);
    assert_eq!(s.total, 8);
    assert_eq!(s.verified_successes, 1);
    assert_eq!(s.verified_failures, 1);
    assert_eq!(s.denominator, 2);
    assert_eq!(s.verified_success_rate, Some(0.5));
    assert_eq!(
        s.exclusions.len(),
        6,
        "nothing unverified is counted either way"
    );
    let reason = |t: &str| {
        s.exclusions
            .iter()
            .find(|e| e.task_id == t)
            .unwrap()
            .reason
            .clone()
    };
    assert_eq!(reason("c"), "verification established no outcome");
    assert_eq!(reason("d"), "Atomic Chat stopped during the run");
    assert_eq!(reason("e"), "cancelled");
    assert_eq!(reason("f"), "still running");
    assert_eq!(reason("g"), "stack-agent is not installed");
    assert!(reason("h").contains("not in the ledger"));
    assert_eq!(s.met_expectation, 1);
    assert_eq!(s.counts["interrupted"], 1);

    let empty = regression_suite::summarize(
        &suite_run("suiterun_y", "stack-agent", &[("g", None)]),
        &runs,
    );
    assert_eq!(empty.denominator, 0);
    assert_eq!(
        empty.verified_success_rate, None,
        "no rate without a denominator"
    );
}

const REVISION: &str = "d66cf80fd946e8df7f502bf2f30cecab98f1c38f";

/// The provenance the runtime manager records for a managed stack-agent.
fn managed_runtime(commit: &str) -> crate::core::stack_agent_runtime::RuntimeProvenance {
    crate::core::stack_agent_runtime::RuntimeProvenance {
        source: "managed".into(),
        selection: "follow_main".into(),
        path: format!("/data/stack-agent-runtime/runtimes/x86_64-linux/{commit}/bin/stack-agent"),
        commit: Some(commit.into()),
        version: Some("0.1.0".into()),
        platform: "x86_64-linux".into(),
        binary_sha256: Some("f".repeat(64)),
        repository: Some("https://github.com/rkendel1/stack-agent".into()),
        toolchain: None,
        prepared_at: None,
        latest_known_main: Some(commit.into()),
        latest_checked_at: None,
        matches_latest: Some(true),
    }
}

#[test]
fn suite_comparison_finds_changes_and_flags_incomparable_conditions() {
    let data = tempfile::tempdir().unwrap();
    let store = Store::new(data.path());
    let task = builtin("ledger-fix-discount");
    let mut runs = HashMap::new();
    let mk = |id: &str, ok: bool, model: &str, version: &str| {
        let mut r = finished(&store, &task, |r| {
            r.execution = Some(exec(ExecutionOutcome::Exited, Some(0), None));
            r.verification = vec![check(
                "p",
                if ok {
                    CheckOutcome::Passed
                } else {
                    CheckOutcome::Failed
                },
                Some(if ok { 0 } else { 101 }),
            )];
        });
        r.id = id.into();
        r.model = Some(model.into());
        r.agent.version = Some(version.into());
        r.agent.runtime = Some(managed_runtime(REVISION));
        r
    };
    for (id, ok, model, v) in [
        ("b1", false, "m", "0.1.0"),
        ("b2", true, "m", "0.1.0"),
        ("b3", false, "m", "0.1.0"),
        ("c1", true, "m", "0.1.0"),
        ("c2", false, "m", "0.1.0"),
    ] {
        runs.insert(id.to_string(), mk(id, ok, model, v));
    }
    let mut interrupted = executor::new_record(&store, &task, "stack-agent", "9.9.9");
    interrupted.transition(RunStatus::Running).unwrap();
    interrupted.recover_if_stale();
    interrupted.model = Some("m".into());
    interrupted.provider = Some("atomic-chat-local-api".into());
    interrupted.agent.version = Some("0.1.0".into());
    interrupted.agent.runtime = Some(managed_runtime(REVISION));
    runs.insert("c3".into(), {
        interrupted.id = "c3".into();
        interrupted
    });
    let base = suite_run(
        "suiterun_b",
        "stack-agent",
        &[("t1", Some("b1")), ("t2", Some("b2")), ("t3", Some("b3"))],
    );
    let cand = suite_run(
        "suiterun_c",
        "stack-agent",
        &[("t1", Some("c1")), ("t2", Some("c2")), ("t3", Some("c3"))],
    );
    let cmp = regression_suite::compare(&base, &cand, &runs);
    let change = |t: &str| cmp.tasks.iter().find(|x| x.task_id == t).unwrap().change;
    assert_eq!(change("t1"), Change::Improvement);
    assert_eq!(change("t2"), Change::Regression);
    assert_eq!(
        change("t3"),
        Change::NotComparable,
        "interrupted is neither"
    );
    assert_eq!(
        (cmp.improvements, cmp.regressions, cmp.not_comparable),
        (1, 1, 1)
    );
    assert!(cmp.directly_comparable, "{:?}", cmp.warnings);
    // A condition nobody recorded cannot be confirmed equal, so it is flagged.
    let mut unknown = runs.clone();
    for r in unknown.values_mut() {
        r.provider = None;
    }
    let w = regression_suite::compare(&base, &cand, &unknown).warnings;
    assert!(w
        .iter()
        .any(|w| w.field == "provider" && w.baseline == "unknown"));

    // A different stack-agent revision is flagged, and so is one nobody
    // recorded (a run from before runtime management).
    let mut moved = runs.clone();
    moved.get_mut("c2").unwrap().agent.runtime = Some(managed_runtime(&"e".repeat(40)));
    let w = regression_suite::compare(&base, &cand, &moved).warnings;
    assert!(w.iter().any(|w| w.field == "agent_revision"
        && w.baseline == REVISION
        && w.candidate.starts_with("mixed")));
    let mut legacy = runs.clone();
    for r in legacy.values_mut() {
        r.agent.runtime = None;
    }
    let w = regression_suite::compare(&base, &cand, &legacy).warnings;
    assert!(w
        .iter()
        .any(|w| w.field == "agent_revision" && w.baseline == "unknown"));

    // A different model and agent version are flagged, not hidden.
    runs.insert("c1".into(), mk("c1", true, "other-model", "0.2.0"));
    let cmp = regression_suite::compare(&base, &cand, &runs);
    let fields: Vec<&str> = cmp.warnings.iter().map(|w| w.field.as_str()).collect();
    assert!(fields.contains(&"model"));
    assert!(fields.contains(&"agent_version"));
    assert!(!cmp.directly_comparable);
    // A configuration warning names only the settings that differ.
    let mut cfg = runs["c2"].clone();
    cfg.config.insert("max_turns".into(), "4".into());
    cfg.config.insert("kind".into(), "change".into());
    let mut b2 = runs["b2"].clone();
    b2.config.insert("kind".into(), "change".into());
    let mut r2 = runs.clone();
    r2.insert("c2".into(), cfg);
    r2.insert("b2".into(), b2);
    for id in ["b1", "b3"] {
        r2.get_mut(id)
            .unwrap()
            .config
            .insert("kind".into(), "change".into());
    }
    for id in ["c1", "c3"] {
        let c = r2.get_mut(id).unwrap();
        c.config.insert("kind".into(), "change".into());
        c.config.insert("max_turns".into(), "4".into());
    }
    let w = regression_suite::compare(&base, &cand, &r2).warnings;
    let config = w.iter().find(|w| w.field == "config").unwrap();
    assert_eq!(config.baseline, "(not set)");
    assert_eq!(config.candidate, "max_turns=4");

    // Different task versions or baselines are never compared as a change.
    let mut v2 = cand.clone();
    v2.entries[0].task_version = 2;
    assert_eq!(
        regression_suite::compare(&base, &v2, &runs).tasks[0].change,
        Change::NotComparable
    );
    let mut b = runs["b1"].clone();
    b.baseline = Some(baseline::Baseline {
        kind: "fixture".into(),
        identity: "fixture:a@1".into(),
        content_digest: "1".into(),
        file_count: 1,
        git_commit: None,
        git_dirty: None,
        source_commit: None,
        fresh_workspace: true,
        excluded_paths: vec![],
        limitations: vec![],
    });
    let mut c = runs["c1"].clone();
    c.baseline = Some(baseline::Baseline {
        identity: "fixture:a@2".into(),
        ..b.baseline.clone().unwrap()
    });
    runs.insert("b1".into(), b);
    runs.insert("c1".into(), c);
    let cmp = regression_suite::compare(&base, &cand, &runs);
    assert_eq!(cmp.tasks[0].change, Change::NotComparable);
    assert!(cmp.tasks[0]
        .reason
        .as_deref()
        .unwrap()
        .contains("baselines"));
}

#[test]
fn the_failure_summary_counts_each_failed_run_once_by_primary_cause() {
    let data = tempfile::tempdir().unwrap();
    let store = Store::new(data.path());
    let task = builtin("ledger-fix-discount");
    let escalated = |r: &mut RunRecord| {
        r.execution = Some(exec(
            ExecutionOutcome::ExitedNonZero,
            Some(1),
            Some(escalated_report()),
        ));
        r.verification = vec![check("p", CheckOutcome::Failed, Some(101))];
    };
    let runs = vec![
        finished(&store, &task, escalated),
        finished(&store, &task, escalated),
        finished(&store, &task, |r| {
            r.execution = Some(exec(ExecutionOutcome::Exited, Some(0), None));
            r.verification = vec![check("p", CheckOutcome::Passed, Some(0))];
        }),
        {
            let mut c = executor::new_record(&store, &task, "stack-agent", "1");
            c.transition(RunStatus::Cancelled).unwrap();
            c
        },
    ];
    let s = regression_suite::failure_summary(&runs, None);
    assert_eq!(
        (s.runs_considered, s.succeeded, s.not_succeeded, s.excluded),
        (4, 1, 2, 1)
    );
    let top = &s.categories[0];
    assert_eq!(top.category, Category::GoalNotCompleted);
    assert_eq!(top.primary_count, 2);
    assert_eq!(top.examples.len(), 2);
    assert!(top.examples.iter().all(|e| e.primary));
    let total_primary: usize = s.categories.iter().map(|c| c.primary_count).sum();
    assert_eq!(total_primary, 2, "one primary per failed run");
    assert_eq!(
        regression_suite::failure_summary(&runs, Some("claude-code")).runs_considered,
        0
    );
    assert_eq!(ResultClass::of(&runs[2]), ResultClass::VerifiedSuccess);
}

// ---- export compatibility -----------------------------------------------------------------

/// A run record as Agent Bench v1 (before the workshop) wrote it: no baseline,
/// trajectory, analysis, suite or process timestamps.
const LANDED_V1_RECORD: &str = r#"{
  "schema": "atomic.bench.run.v1",
  "id": "run_20261026T181920_4fa05d74",
  "task": {"id": "ledger-fix-discount", "version": 1,
    "digest": "bfb3be2628384eab492ab340ba9b0c47b0901d348e2e2d78e2d07ff2a5b0abb8",
    "name": "Fix: failing discount test",
    "snapshot": {"schema": "atomic.bench.task.v1", "id": "ledger-fix-discount", "version": 1,
      "name": "Fix: failing discount test", "description": "", "goal": "Fix it.",
      "workdir": {"kind": "fixture", "fixture": "ledger-discount-bug"}, "success_criteria": [],
      "evaluation": {"method": "command", "commands": [{"name": "Pricing tests pass", "program": "cargo", "args": ["test"]}],
        "held_out_checks": "ledger-pricing"},
      "limits": {"timeout_secs": 900, "max_turns": 16}, "builtin": true, "created_at": "", "updated_at": ""}},
  "agent": {"id": "stack-agent", "name": "stack-agent", "execution_method": "cli:stack-agent work --json",
    "executable": "/home/u/.local/bin/stack-agent", "version": "stack-agent 0.1.0"},
  "model": "qwen2.5-coder-1.5b-instruct-q4_k_m", "provider": "atomic-chat-local-api",
  "endpoint": "http://127.0.0.1:1337/v1/chat/completions",
  "config": {"CHIP_MODEL": "qwen2.5-coder-1.5b-instruct-q4_k_m", "kind": "change"},
  "credential_supplied": true, "app_version": "2.2.26",
  "workdir": {"source": "fixture", "path": "/data/agent_bench/workspaces/run_20261026T181920_4fa05d74", "fixture": "ledger-discount-bug"},
  "status": "failed", "failure_kind": "verification",
  "created_at": "2026-10-26T18:19:20.000Z", "started_at": "2026-10-26T18:19:20.001Z",
  "finished_at": "2026-10-26T18:19:40.256Z", "duration_ms": 20255,
  "execution": {"outcome": "exited_non_zero", "command_line": "stack-agent work …", "exit_code": 1,
    "duration_ms": 19055, "stdout": {"text": "", "total_bytes": 7000, "truncated": false},
    "stderr": {"text": "", "total_bytes": 0, "truncated": false},
    "agent_report": {"terminal_state": "escalated", "outcome_reason": "needs review",
      "executions_by_capability": {}, "lists": 0, "searches": 0, "reads": 0, "invalid_decisions": 0,
      "malformed_replies": 0, "model_failures": [], "audit": {}, "evidence": {"invalidated": null, "test_files_replaced": []}}},
  "verification": [{"name": "Pricing tests pass", "command_line": "cargo test", "outcome": "failed",
    "exit_code": 101, "duration_ms": 653, "stdout": {"text": "", "total_bytes": 1, "truncated": false},
    "stderr": {"text": "", "total_bytes": 1, "truncated": false}}],
  "changes": {"added": [], "modified": [], "deleted": [], "incomplete": false},
  "evaluation": {"method": "command", "verified": "failure", "human": null, "awaiting_review": false},
  "artifacts": [], "errors": [], "cancellation": null, "interruption": null,
  "history": [{"status": "queued", "at": "2026-10-26T18:19:20.000Z"}, {"status": "failed", "at": "2026-10-26T18:19:40.256Z"}]
}"#;

#[test]
fn landed_v1_records_load_and_export_with_the_added_sections() {
    let data = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(data.path().join("agent_bench/runs")).unwrap();
    std::fs::write(
        data.path()
            .join("agent_bench/runs/run_20261026T181920_4fa05d74.json"),
        LANDED_V1_RECORD,
    )
    .unwrap();
    let store = Store::new(data.path());
    let run = store.get_run("run_20261026T181920_4fa05d74").unwrap();
    assert!(run.baseline.is_none() && run.agent_trace.is_none() && run.suite.is_none());
    // Analyzed on read from what the old record holds.
    assert_eq!(primary(&run), Category::GoalNotCompleted);
    let text = export::export_jsonl(&store.list_runs().unwrap());
    let line: Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
    // Every v1 section is unchanged in name and place.
    assert_eq!(line["schema"], export::EXPORT_SCHEMA);
    for key in [
        "observed",
        "verified",
        "human",
        "agent_claims",
        "unknown",
        "task",
        "final_status",
    ] {
        assert!(line.get(key).is_some(), "{key}");
    }
    assert_eq!(
        line["task"]["digest"],
        "bfb3be2628384eab492ab340ba9b0c47b0901d348e2e2d78e2d07ff2a5b0abb8"
    );
    // The added sections.
    assert_eq!(line["analysis"]["schema"], analysis::ANALYSIS_SCHEMA);
    assert_eq!(line["analysis"]["causes"][0]["source"], "rule");
    assert!(line["reproducibility"]["baseline"].is_null());
    assert!(line["suite"].is_null());
    assert!(line["agent_trace"].is_null());
    let gaps: Vec<&str> = line["evidence_gaps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(gaps, vec!["baseline", "agent_trace", "process_timestamps"]);
    // The record on disk was not rewritten by reading it.
    let on_disk = std::fs::read_to_string(
        data.path()
            .join("agent_bench/runs/run_20261026T181920_4fa05d74.json"),
    )
    .unwrap();
    assert_eq!(on_disk, LANDED_V1_RECORD);
}

// ---- the whole loop with fake agents --------------------------------------------------------

#[cfg(unix)]
mod workflow {
    use super::*;
    use crate::core::agent_bench::adapters::{self, RunConfig};
    use crate::core::agent_bench::executor::Context;
    use crate::core::agent_bench::regression_suite::Suite;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::Arc;
    use tokio_util::sync::CancellationToken;

    const KEY: &str = "sk-workshop-0123456789abcdefghij";

    /// A fake stack-agent with the real interface: `work … --json
    /// --print-reply --state-db DB`, and `agent-state show --state-db DB
    /// --task-id ID --json`. `fix` decides whether it repairs the bug.
    fn fake_agent(dir: &Path, name: &str, version: &str, fix: bool) -> std::path::PathBuf {
        let fixed = include_str!("fixtures/ledger/src/pricing.rs").replace('\'', "'\\''");
        let (work, terminal, reply) = if fix {
            (
                format!("printf '%s' '{fixed}' > src/pricing.rs"),
                "completed",
                "{\"decision\":\"complete\",\"summary\":\"rounded half to even\"}",
            )
        } else {
            (
                String::new(),
                "escalated",
                "{\"decision\":\"escalate\",\"reason\":\"unsure\"}",
            )
        };
        let events = if fix {
            r#"[{"attempt":1,"index":0,"recorded_at_ms":1793038760698,"provenance":"runtime_decided","event":{"kind":"WorkStarted","max_turns":16,"max_executions":32}},{"attempt":1,"index":1,"recorded_at_ms":1793038760800,"provenance":"model_inferred","event":{"kind":"DecisionMade","turn":1,"decision":"request project.write"}},{"attempt":1,"index":2,"recorded_at_ms":1793038760900,"provenance":"host_observed","event":{"kind":"ExecutionCompleted","execution_id":"e1"}},{"attempt":1,"index":3,"recorded_at_ms":1793038761000,"provenance":"runtime_decided","event":{"kind":"WorkCompleted"}}]"#
        } else {
            r#"[{"attempt":1,"index":0,"recorded_at_ms":1793038760698,"provenance":"runtime_decided","event":{"kind":"WorkStarted","max_turns":16,"max_executions":32}},{"attempt":1,"index":1,"recorded_at_ms":1793038760800,"provenance":"model_inferred","event":{"kind":"DecisionMade","turn":1,"decision":"escalate"}},{"attempt":1,"index":2,"recorded_at_ms":1793038760900,"provenance":"runtime_decided","event":{"kind":"WorkEscalated","reason":"unsure"}}]"#
        };
        let path = dir.join(name);
        let script = format!(
            r#"#!/bin/sh
if [ "$1" = "--version" ]; then echo "stack-agent {version}"; exit 0; fi
if [ "$1" = "agent-state" ]; then
  case "$*" in *"--state-db"*"--task-id work_fake"*"--json"*) ;; *) echo "error: bad show" >&2; exit 2;; esac
  echo '{{"events":{events}}}'; exit 0
fi
[ "$1" = "work" ] || exit 2
case "$*" in *"--print-reply"*"--state-db"*) ;; *) echo "error: expected --print-reply and --state-db" >&2; exit 2;; esac
printf 'Model reply:\n%s\n' '{reply}' >&2
echo "note: key was $CHIP_API_KEY" >&2
{work}
echo '{{"terminal_state":"{terminal}","outcome_reason":"unsure","executions_by_capability":{{}},"lists":0,"searches":0,"reads":0,"invalid_decisions":0,"malformed_replies":0,"model_failures":[],"audit":{{}},"evidence":{{"invalidated":null,"test_files_replaced":[]}},"agent_state":{{"recorded":true,"task":{{"task_id":"work_fake"}}}}}}'
[ "{terminal}" = "completed" ] && exit 0 || exit 1
"#
        );
        std::fs::write(&path, script).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    async fn run_task(
        store: &Store,
        task: &BenchTask,
        agent: &Path,
        url: &str,
        suite: Option<SuiteRef>,
    ) -> RunRecord {
        let config = RunConfig {
            agent_id: "stack-agent".into(),
            model: Some("local-model".into()),
            api_url: Some(url.into()),
            api_key: Some(KEY.into()),
            executable: Some(agent.to_string_lossy().into()),
        };
        let mut record = executor::new_record(store, task, "stack-agent", "9.9.9");
        record.suite = suite;
        let evidence = store.evidence_dir(&record.id);
        std::fs::create_dir_all(&evidence).unwrap();
        let ctx = Context {
            store: store.clone(),
            notify: Arc::new(|_: &RunRecord| {}),
            cancel: CancellationToken::new(),
            slot: Arc::new(tokio::sync::Semaphore::new(1)),
            launch: adapters::plan(task, &config, agent, Some(&evidence)),
            path_env: std::env::var("PATH").unwrap_or_default(),
        };
        executor::execute(ctx, record).await
    }

    /// Run → observe → diagnose → reproduce → improve → re-run → verify.
    #[tokio::test]
    async fn the_improvement_loop_end_to_end() {
        if adapters::search_path("cargo", &std::env::var("PATH").unwrap_or_default()).is_none() {
            eprintln!("cargo not on PATH; skipping");
            return;
        }
        let data = tempfile::tempdir().unwrap();
        let agents = tempfile::tempdir().unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/v1", listener.local_addr().unwrap());
        let store = Store::new(data.path());
        let failing = fake_agent(agents.path(), "agent-old", "0.1.0", false);
        let fixed = fake_agent(agents.path(), "agent-new", "0.1.1", true);

        // Run and observe.
        let task = builtin("ledger-fix-discount");
        let first = run_task(&store, &task, &failing, &url, None).await;
        assert_eq!(first.status, RunStatus::Failed);
        assert_eq!(
            first.execution.as_ref().unwrap().error,
            None,
            "a model reply on stderr is never reported as the agent's error"
        );
        let trace = first.agent_trace.as_ref().unwrap();
        assert!(trace.available, "{:?}", trace.unavailable_reason);
        assert_eq!(trace.events.len(), 3);
        assert_eq!(trace.replies.len(), 1);
        assert!(first.baseline.as_ref().unwrap().fresh_workspace);
        let t = timeline::build(&first);
        assert!(t
            .iter()
            .any(|e| e.class == EntryClass::AgentClaim && e.label.contains("escalate")));

        // Diagnose.
        assert_eq!(primary(&first), Category::GoalNotCompleted);

        // Reproduce: a regression task, without touching the original.
        let source_bytes = std::fs::read(
            data.path()
                .join(format!("agent_bench/runs/{}.json", first.id)),
        )
        .unwrap();
        let reg = store
            .create_task(
                regression::build(
                    &first,
                    &RegressionRequest {
                        id: "fix-discount-gives-up".into(),
                        name: None,
                        failure_description: "Gives up on turn 1.".into(),
                        proposed_improvement: Some("Read the failing test first.".into()),
                        expected_outcome: None,
                    },
                )
                .unwrap(),
            )
            .unwrap();
        assert_eq!(
            std::fs::read(
                data.path()
                    .join(format!("agent_bench/runs/{}.json", first.id))
            )
            .unwrap(),
            source_bytes
        );

        // Promote it into a suite and record a baseline suite run with the old agent.
        let mut suite = Suite {
            schema: regression_suite::SUITE_SCHEMA.into(),
            id: "stack-agent-regressions".into(),
            name: "stack-agent regressions".into(),
            description: String::new(),
            version: 1,
            entries: vec![regression_suite::SuiteEntry::from_task(&reg)],
            created_at: now(),
            updated_at: now(),
        };
        store.save_suite(&suite).unwrap();
        suite = store.get_suite(&suite.id).unwrap();
        assert_eq!(
            suite.entries[0].source_run_id.as_deref(),
            Some(first.id.as_str())
        );
        assert_eq!(suite.entries[0].expected, "success");

        let mut suite_runs = Vec::new();
        for (label, agent) in [("baseline", &failing), ("candidate", &fixed)] {
            let id = crate::core::agent_bench::store::new_suite_run_id();
            let pinned = store
                .get_task_version(&suite.entries[0].task_id, suite.entries[0].task_version)
                .unwrap();
            let r = run_task(
                &store,
                &pinned,
                agent,
                &url,
                Some(SuiteRef {
                    suite_id: suite.id.clone(),
                    suite_version: suite.version,
                    suite_run_id: id.clone(),
                }),
            )
            .await;
            let sr = SuiteRun {
                schema: SUITE_RUN_SCHEMA.into(),
                id,
                suite_id: suite.id.clone(),
                suite_version: suite.version,
                label: label.into(),
                agent_id: "stack-agent".into(),
                model: Some("local-model".into()),
                entries: vec![SuiteRunEntry {
                    task_id: reg.id.clone(),
                    task_version: reg.version,
                    expected: "success".into(),
                    run_id: Some(r.id.clone()),
                    not_started_reason: None,
                }],
                created_at: now(),
            };
            store.save_suite_run(&sr).unwrap();
            suite_runs.push((sr, r));
        }
        let (base_sr, base_run) = &suite_runs[0];
        let (cand_sr, cand_run) = &suite_runs[1];

        // Re-run from a clean baseline: same identity, new workspace.
        assert_eq!(
            base_run.baseline.as_ref().unwrap().identity,
            cand_run.baseline.as_ref().unwrap().identity
        );
        assert_ne!(base_run.workdir.path, cand_run.workdir.path);
        assert_eq!(
            cand_run.status,
            RunStatus::Succeeded,
            "{:#?}",
            cand_run.verification
        );

        // Verify: the comparison shows the improvement and the changed agent version.
        let runs: HashMap<String, RunRecord> = store
            .list_runs()
            .unwrap()
            .into_iter()
            .map(|r| (r.id.clone(), r))
            .collect();
        let cmp = regression_suite::compare(base_sr, cand_sr, &runs);
        assert_eq!(cmp.improvements, 1);
        assert_eq!(cmp.regressions, 0);
        assert_eq!(cmp.baseline.verified_success_rate, Some(0.0));
        assert_eq!(cmp.candidate.verified_success_rate, Some(1.0));
        assert!(
            cmp.warnings.iter().any(|w| w.field == "agent_version"),
            "{:?}",
            cmp.warnings
        );
        assert_eq!(store.list_suite_runs(Some(&suite.id)).unwrap().len(), 2);

        // Export: one line per run, with the added sections and no credential.
        let text = export::export_jsonl(&store.list_runs().unwrap());
        assert!(!text.contains(KEY), "credential in the export");
        for f in std::fs::read_dir(data.path().join("agent_bench/runs")).unwrap() {
            assert!(!std::fs::read_to_string(f.unwrap().path())
                .unwrap()
                .contains(KEY));
        }
        let lines: Vec<Value> = text
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(lines.len(), 3);
        let cand_line = lines
            .iter()
            .find(|l| l["run_id"] == cand_run.id.as_str())
            .unwrap();
        assert_eq!(
            cand_line["reproducibility"]["task_origin"]["source_run_id"],
            first.id.as_str()
        );
        assert_eq!(cand_line["suite"]["suite_id"], "stack-agent-regressions");
        assert_eq!(cand_line["agent_trace"]["available"], true);
        assert_eq!(cand_line["verified"]["outcome"], "success");
        assert!(cand_line["analysis"]["causes"]
            .as_array()
            .unwrap()
            .is_empty());
        let first_line = lines
            .iter()
            .find(|l| l["run_id"] == first.id.as_str())
            .unwrap();
        assert_eq!(
            first_line["analysis"]["causes"][0]["category"],
            "goal_not_completed"
        );
        assert_eq!(
            first_line["agent_claims"]["model_replies"][0]["text"],
            "{\"decision\":\"escalate\",\"reason\":\"unsure\"}"
        );

        // Everything is still there after a restart.
        let reopened = Store::new(data.path());
        assert!(reopened.recover_stale_runs().unwrap().is_empty());
        assert_eq!(reopened.list_runs().unwrap().len(), 3);
        assert_eq!(reopened.get_task("fix-discount-gives-up").unwrap(), reg);
        assert_eq!(reopened.list_suites().unwrap().len(), 1);
    }
}

/// The whole loop against a real agent and model, opt-in:
///
/// ```sh
/// ATOMIC_BENCH_DATA_DIR=/tmp/ledger ATOMIC_BENCH_OUT=/tmp/out \
/// ATOMIC_BENCH_API_URL=http://127.0.0.1:1337/v1 ATOMIC_BENCH_MODEL=<model> \
/// [ATOMIC_BENCH_ALT_API_URL=… ATOMIC_BENCH_ALT_MODEL=<another model>] \
/// [ATOMIC_BENCH_TASK=ledger-fix-discount] [ATOMIC_BENCH_EXECUTABLE=/path/to/stack-agent] \
/// cargo test --lib agent_bench::tests_workshop::live_workshop_loop -- --ignored --nocapture
/// ```
///
/// Run a task, analyze it, make a regression task, put it in a suite, run the
/// suite twice with the same configuration and once with the alternative
/// model, compare, and export. Writes inspections, comparisons and the JSONL
/// export to `ATOMIC_BENCH_OUT`.
#[tokio::test]
#[ignore = "runs a real agent and model; see the doc comment"]
async fn live_workshop_loop() {
    use super::adapters::{self, RunConfig};
    use super::executor::Context;
    use super::regression_suite::{Suite, SuiteEntry, SUITE_SCHEMA};
    use std::sync::Arc;
    use tokio_util::sync::CancellationToken;

    let var = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    let root =
        std::path::PathBuf::from(var("ATOMIC_BENCH_DATA_DIR").expect("ATOMIC_BENCH_DATA_DIR"));
    let out = std::path::PathBuf::from(var("ATOMIC_BENCH_OUT").expect("ATOMIC_BENCH_OUT"));
    std::fs::create_dir_all(&out).unwrap();
    let store = Store::new(&root);
    let path_env = adapters::child_path();
    let exe =
        adapters::resolve_executable("stack-agent", var("ATOMIC_BENCH_EXECUTABLE").as_deref())
            .unwrap();
    let config = |model: String, url: String| RunConfig {
        agent_id: "stack-agent".into(),
        model: Some(model),
        api_url: Some(url),
        api_key: var("ATOMIC_BENCH_API_KEY"),
        executable: Some(exe.to_string_lossy().into()),
    };
    let main = config(
        var("ATOMIC_BENCH_MODEL").unwrap(),
        var("ATOMIC_BENCH_API_URL").unwrap(),
    );
    let alt = var("ATOMIC_BENCH_ALT_MODEL").map(|m| {
        config(
            m,
            var("ATOMIC_BENCH_ALT_API_URL")
                .or_else(|| var("ATOMIC_BENCH_API_URL"))
                .unwrap(),
        )
    });
    let go = |task: BenchTask, cfg: RunConfig, suite: Option<SuiteRef>| {
        let store = store.clone();
        let path_env = path_env.clone();
        let exe = exe.clone();
        async move {
            let mut record = executor::new_record(&store, &task, "stack-agent", "live");
            record.suite = suite;
            let evidence = store.evidence_dir(&record.id);
            std::fs::create_dir_all(&evidence).unwrap();
            let ctx = Context {
                store: store.clone(),
                notify: Arc::new(|r: &RunRecord| eprintln!("  {} {:?}", r.id, r.status)),
                cancel: CancellationToken::new(),
                slot: Arc::new(tokio::sync::Semaphore::new(1)),
                launch: adapters::plan(&task, &cfg, &exe, Some(&evidence)),
                path_env,
            };
            executor::execute(ctx, record).await
        }
    };
    let dump = |name: &str, v: &Value| {
        std::fs::write(out.join(name), serde_json::to_string_pretty(v).unwrap()).unwrap();
    };

    // 1. Run a real task and inspect it.
    let task = store
        .get_task(&var("ATOMIC_BENCH_TASK").unwrap_or_else(|| "ledger-fix-discount".into()))
        .unwrap();
    let first = go(task.clone(), main.clone(), None).await;
    eprintln!(
        "first run: {:?} — {}",
        first.status,
        first
            .analysis
            .as_ref()
            .map(|a| a.summary.as_str())
            .unwrap_or("")
    );
    let inspect = |r: &RunRecord| {
        serde_json::json!({
            "run": r, "prompt": r.task.snapshot.prompt(),
            "timeline": timeline::build(r), "signals": analysis::run_signals(r),
        })
    };
    dump(&format!("inspection-{}.json", first.id), &inspect(&first));

    // 2. Reproduce it as a regression task (the original stays as it is).
    let reg_id = format!("{}-regression-{}", task.id, &first.id[first.id.len() - 8..]);
    let reg = store
        .create_task(
            regression::build(
                &first,
                &RegressionRequest {
                    id: reg_id,
                    name: None,
                    failure_description: format!(
                        "Real-model run {} ended {:?}; see its analysis.",
                        first.id, first.status
                    ),
                    proposed_improvement: None,
                    expected_outcome: Some("success".into()),
                },
            )
            .unwrap(),
        )
        .unwrap();

    // 3. Promote it into a suite.
    let suite_id = "live-regressions";
    let mut suite = store.get_suite(suite_id).unwrap_or(Suite {
        schema: SUITE_SCHEMA.into(),
        id: suite_id.into(),
        name: "Live regressions".into(),
        description: String::new(),
        version: 0,
        entries: vec![],
        created_at: now(),
        updated_at: now(),
    });
    suite.entries.retain(|e| e.task_id != reg.id);
    suite.entries.push(SuiteEntry::from_task(&reg));
    suite.version += 1;
    suite.updated_at = now();
    store.save_suite(&suite).unwrap();

    // 4. Run the suite: baseline, re-run with the same configuration, and the alternative.
    let mut views = Vec::new();
    let mut runs_cfg: Vec<(&str, RunConfig)> =
        vec![("baseline", main.clone()), ("re-run", main.clone())];
    if let Some(a) = alt.clone() {
        runs_cfg.push(("alternative model", a));
    }
    for (label, cfg) in runs_cfg {
        let id = super::store::new_suite_run_id();
        let mut sr = SuiteRun {
            schema: SUITE_RUN_SCHEMA.into(),
            id: id.clone(),
            suite_id: suite.id.clone(),
            suite_version: suite.version,
            label: label.into(),
            agent_id: "stack-agent".into(),
            model: cfg.model.clone(),
            entries: vec![],
            created_at: now(),
        };
        for e in &suite.entries {
            let pinned = store.get_task_version(&e.task_id, e.task_version).unwrap();
            let r = go(
                pinned,
                cfg.clone(),
                Some(SuiteRef {
                    suite_id: suite.id.clone(),
                    suite_version: suite.version,
                    suite_run_id: id.clone(),
                }),
            )
            .await;
            dump(&format!("inspection-{}.json", r.id), &inspect(&r));
            sr.entries.push(SuiteRunEntry {
                task_id: e.task_id.clone(),
                task_version: e.task_version,
                expected: e.expected.clone(),
                run_id: Some(r.id),
                not_started_reason: None,
            });
        }
        store.save_suite_run(&sr).unwrap();
        views.push(sr);
    }

    // 5. Compare and export.
    let runs: HashMap<String, RunRecord> = store
        .list_runs()
        .unwrap()
        .into_iter()
        .map(|r| (r.id.clone(), r))
        .collect();
    for (i, cand) in views.iter().enumerate().skip(1) {
        let cmp = regression_suite::compare(&views[0], cand, &runs);
        eprintln!(
            "{} vs {}: {} improvements, {} regressions, {} not comparable; warnings {:?}; rates {:?} vs {:?}",
            views[0].label, cand.label, cmp.improvements, cmp.regressions, cmp.not_comparable,
            cmp.warnings.iter().map(|w| &w.field).collect::<Vec<_>>(),
            cmp.baseline.verified_success_rate, cmp.candidate.verified_success_rate
        );
        dump(
            &format!("comparison-{i}.json"),
            &serde_json::to_value(&cmp).unwrap(),
        );
    }
    dump(
        "summary.json",
        &serde_json::to_value(regression_suite::failure_summary(
            &store.list_runs().unwrap(),
            None,
        ))
        .unwrap(),
    );
    std::fs::write(
        out.join("export.jsonl"),
        export::export_jsonl(&store.list_runs().unwrap()),
    )
    .unwrap();
}
