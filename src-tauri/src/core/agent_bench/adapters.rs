//! Small, explicit adapters from a bench task to one agent's own
//! non-interactive command line. Each adapter uses only that agent's
//! documented interface; no agent is forced through an invented common CLI.
//!
//! | Agent       | Invocation                                                   | Report on stdout |
//! | ----------- | ------------------------------------------------------------ | ---------------- |
//! | stack-agent | `stack-agent work <prompt> --kind change --json --deadline-secs N [--max-turns N --max-executions N]`, model from `CHIP_*` | `work --json` report |
//! | claude-code | `claude -p <prompt> --output-format json --permission-mode acceptEdits [--model M]`, endpoint from `ANTHROPIC_*` | result JSON |

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::evidence::{AgentTrace, Provenance, Signal, SignalKind, TraceEvent};
use super::task::BenchTask;

/// What the UI sends to start a run. The key is used for this run only and
/// is never stored.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct RunConfig {
    pub agent_id: String,
    #[serde(default)]
    pub model: Option<String>,
    /// The Local API Server URL in the form this agent expects (with `/v1`
    /// for stack-agent, bare host:port for Claude Code), or none to use the
    /// agent's own configuration where it has one.
    #[serde(default)]
    pub api_url: Option<String>,
    #[serde(default)]
    pub api_key: Option<String>,
    /// Absolute path found by detection or set on the Launch page.
    #[serde(default)]
    pub executable: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AdapterInfo {
    pub id: &'static str,
    pub name: &'static str,
    pub bin: &'static str,
    pub execution_method: &'static str,
    pub requires_model: bool,
    pub requires_endpoint: bool,
    pub notes: &'static str,
}

pub const ADAPTERS: &[AdapterInfo] = &[
    AdapterInfo {
        id: "stack-agent",
        name: "stack-agent",
        bin: "stack-agent",
        execution_method: "cli:stack-agent work --json",
        requires_model: true,
        requires_endpoint: true,
        notes: "Runs `stack-agent work` with --kind change. It needs pax on PATH to run the \
                project's tests, and a model on the Local API Server.",
    },
    AdapterInfo {
        id: "claude-code",
        name: "Claude Code",
        bin: "claude",
        execution_method: "cli:claude -p --output-format json",
        requires_model: false,
        requires_endpoint: false,
        notes: "Runs `claude -p` with --permission-mode acceptEdits: it may edit files; shell \
                commands that would need approval are refused. With no endpoint it uses Claude \
                Code's own configuration. Values in ~/.claude/settings.json `env` take precedence \
                over the ones passed here.",
    },
];

pub fn adapter(id: &str) -> Option<&'static AdapterInfo> {
    ADAPTERS.iter().find(|a| a.id == id)
}

/// Everything needed to start the agent, plus what the record may show.
#[derive(Debug, Clone)]
pub struct Launch {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub env_remove: Vec<String>,
    /// Values to scrub from everything recorded.
    pub secrets: Vec<String>,
    pub model: Option<String>,
    pub provider: Option<String>,
    pub endpoint: Option<String>,
    pub config: BTreeMap<String, String>,
    /// The process limit. Longer than the task's limit when the agent enforces
    /// that limit itself, so it can stop cleanly and report first.
    pub timeout: Duration,
    /// Address the run needs reachable before it starts, if any.
    pub preflight_url: Option<String>,
    /// stack-agent's per-run FeltDB database (`--state-db`), read back with
    /// `agent-state show` after the run for the agent's own trajectory.
    pub state_db: Option<PathBuf>,
}

/// `provider` of a run whose model was served by Atomic Chat's Local API Server.
pub const LOCAL_API_PROVIDER: &str = "atomic-chat-local-api";

/// Grace past the task limit for an agent that enforces the limit itself.
const SELF_DEADLINE_GRACE: Duration = Duration::from_secs(60);
/// stack-agent refuses turn and execution limits above this.
const STACK_AGENT_LIMIT_CEILING: u32 = 50;

/// Settings stack-agent would otherwise read from Atomic Chat's own
/// environment; removed so every run is configured only by what it records.
const STACK_AGENT_INHERITED: &[&str] = &[
    "CHIP_PROVIDER_PROFILE",
    "CHIP_STRUCTURED_OUTPUT",
    "CHIP_TEMPERATURE",
    "CHIP_MAX_OUTPUT_TOKENS",
    "CHIP_TOKEN_LIMIT_FIELD",
    "CHIP_CONTEXT_BUDGET_BYTES",
    "CHIP_EXECUTOR",
    "CHIP_DECISION_PROMPT",
    "OPENAI_API_KEY",
    "ANTHROPIC_API_KEY",
];

fn non_empty(v: &Option<String>) -> Option<&str> {
    v.as_deref().map(str::trim).filter(|s| !s.is_empty())
}

/// `evidence_dir` is the run's own directory in the ledger, outside the
/// project, for anything the agent records about itself.
pub fn plan(
    task: &BenchTask,
    cfg: &RunConfig,
    program: &Path,
    evidence_dir: Option<&Path>,
) -> Result<Launch, String> {
    match cfg.agent_id.as_str() {
        "stack-agent" => plan_stack_agent(task, cfg, program, evidence_dir),
        "claude-code" => plan_claude_code(task, cfg, program),
        other => Err(format!("{other} has no Agent Bench adapter")),
    }
}

fn plan_stack_agent(
    task: &BenchTask,
    cfg: &RunConfig,
    program: &Path,
    evidence_dir: Option<&Path>,
) -> Result<Launch, String> {
    let model = non_empty(&cfg.model).ok_or(
        "stack-agent needs a model: load one in Atomic Chat (or pick one in the run settings) \
         and run again.",
    )?;
    let api_url = non_empty(&cfg.api_url).ok_or(
        "stack-agent needs the Local API Server: start it from Server → Overview and run again.",
    )?;
    let deadline = task.limits.effective_timeout_secs();
    let mut args = vec![
        "work".to_string(),
        task.prompt(),
        "--kind".to_string(),
        "change".to_string(),
        "--json".to_string(),
        // The model's replies go to stderr: the only record of what it said.
        "--print-reply".to_string(),
        "--deadline-secs".to_string(),
        deadline.to_string(),
    ];
    let mut config = BTreeMap::new();
    config.insert("kind".to_string(), "change".to_string());
    config.insert("print_reply".to_string(), "true".to_string());
    // A database of the run's own, so its trajectory can be read back; without
    // one stack-agent records into a per-project default the bench cannot name.
    let state_db = evidence_dir.map(|d| d.join("stack-agent-state.feltdb"));
    if let Some(db) = &state_db {
        args.extend(["--state-db".to_string(), db.to_string_lossy().into_owned()]);
        config.insert("state_db".to_string(), "per-run".to_string());
    }
    config.insert("deadline_secs".to_string(), deadline.to_string());
    if let Some(turns) = task.limits.max_turns {
        let turns = turns.min(STACK_AGENT_LIMIT_CEILING);
        let executions = (turns * 2).min(STACK_AGENT_LIMIT_CEILING);
        args.extend([
            "--max-turns".to_string(),
            turns.to_string(),
            "--max-executions".to_string(),
            executions.to_string(),
        ]);
        config.insert("max_turns".to_string(), turns.to_string());
        config.insert("max_executions".to_string(), executions.to_string());
    }
    let env =
        crate::core::system::commands::stack_agent_env_vars(api_url, model, cfg.api_key.as_deref());
    let endpoint = env
        .iter()
        .find(|(k, _)| k == "CHIP_ENDPOINT")
        .map(|(_, v)| v.clone());
    for (k, v) in &env {
        if k.starts_with("CHIP_") && !crate::core::agent_bench::redact::is_credential_name(k) {
            config.insert(k.clone(), v.clone());
        }
    }
    config.insert("inherited_chip_settings".to_string(), "removed".to_string());
    Ok(Launch {
        program: program.to_path_buf(),
        args,
        env,
        env_remove: STACK_AGENT_INHERITED
            .iter()
            .map(|s| s.to_string())
            .collect(),
        secrets: cfg.api_key.iter().cloned().collect(),
        model: Some(model.to_string()),
        // Where inference is served, as for every adapter; the wire protocol
        // is CHIP_PROVIDER in the config.
        provider: Some(LOCAL_API_PROVIDER.to_string()),
        endpoint: endpoint.clone(),
        config,
        timeout: Duration::from_secs(deadline) + SELF_DEADLINE_GRACE,
        preflight_url: endpoint,
        state_db,
    })
}

fn plan_claude_code(task: &BenchTask, cfg: &RunConfig, program: &Path) -> Result<Launch, String> {
    let mut args = vec![
        "-p".to_string(),
        task.prompt(),
        "--output-format".to_string(),
        "json".to_string(),
        "--permission-mode".to_string(),
        "acceptEdits".to_string(),
    ];
    let mut config = BTreeMap::new();
    config.insert("permission_mode".to_string(), "acceptEdits".to_string());
    config.insert("output_format".to_string(), "json".to_string());
    if task.limits.max_turns.is_some() {
        config.insert(
            "max_turns".to_string(),
            "not supported by this adapter; the time limit applies".to_string(),
        );
    }
    let model = non_empty(&cfg.model).map(str::to_string);
    if let Some(m) = &model {
        args.extend(["--model".to_string(), m.clone()]);
    }
    let mut env = Vec::new();
    let mut env_remove = Vec::new();
    let (provider, endpoint) = match non_empty(&cfg.api_url) {
        Some(url) => {
            let token = cfg
                .api_key
                .as_deref()
                .filter(|k| !k.is_empty())
                .unwrap_or("atomic");
            env.push(("ANTHROPIC_BASE_URL".to_string(), url.to_string()));
            env.push(("ANTHROPIC_AUTH_TOKEN".to_string(), token.to_string()));
            if let Some(m) = &model {
                for var in [
                    "ANTHROPIC_MODEL",
                    "ANTHROPIC_DEFAULT_OPUS_MODEL",
                    "ANTHROPIC_DEFAULT_SONNET_MODEL",
                    "ANTHROPIC_DEFAULT_HAIKU_MODEL",
                ] {
                    env.push((var.to_string(), m.clone()));
                }
            }
            // A real Anthropic key would otherwise be sent to the local server.
            env_remove.push("ANTHROPIC_API_KEY".to_string());
            config.insert("ANTHROPIC_BASE_URL".to_string(), url.to_string());
            (Some(LOCAL_API_PROVIDER.to_string()), Some(url.to_string()))
        }
        None => {
            config.insert(
                "endpoint".to_string(),
                "Claude Code's own configuration".to_string(),
            );
            (Some("claude-code-configuration".to_string()), None)
        }
    };
    Ok(Launch {
        program: program.to_path_buf(),
        args,
        env,
        env_remove,
        secrets: cfg.api_key.iter().cloned().collect(),
        model,
        provider,
        preflight_url: endpoint.clone(),
        endpoint,
        config,
        timeout: Duration::from_secs(task.limits.effective_timeout_secs()),
        state_db: None,
    })
}

/// The agent's structured report from its stdout, when it printed one.
pub fn parse_report(stdout: &str) -> Option<Value> {
    let trimmed = stdout.trim();
    if let Ok(v) = serde_json::from_str::<Value>(trimmed) {
        return v.is_object().then_some(v);
    }
    // Tolerate lines before the report: take the last line that is an object.
    trimmed
        .lines()
        .rev()
        .find_map(|l| serde_json::from_str::<Value>(l.trim()).ok())
        .filter(Value::is_object)
}

/// References into the agent's own records, from its report.
pub fn report_artifacts(agent_id: &str, report: &Value) -> Vec<super::run::Artifact> {
    let mut out = Vec::new();
    match agent_id {
        "stack-agent" => {
            let state = &report["agent_state"];
            let id = state["task_id"]
                .as_str()
                .or_else(|| state["work_id"].as_str());
            if let Some(id) = id {
                out.push(super::run::Artifact {
                    kind: "stack-agent-state".to_string(),
                    reference: format!("stack-agent agent-state show --task-id {id}"),
                    description: "stack-agent's own durable record of this work".to_string(),
                });
            }
        }
        "claude-code" => {
            if let Some(id) = report["session_id"].as_str() {
                out.push(super::run::Artifact {
                    kind: "claude-code-session".to_string(),
                    reference: id.to_string(),
                    description: "Claude Code session id".to_string(),
                });
            }
        }
        _ => {}
    }
    out
}

/// Finds the agent's executable: the path the UI resolved, then the
/// login-shell PATH, then the known install locations off PATH.
pub fn resolve_executable(bin: &str, given: Option<&str>) -> Result<PathBuf, String> {
    if let Some(p) = given.map(str::trim).filter(|p| !p.is_empty()) {
        let path = PathBuf::from(p);
        return if path.is_file() {
            Ok(path)
        } else {
            Err(format!(
                "{p} does not exist; fix the path on the Launch page"
            ))
        };
    }
    if let Some(found) = search_path(bin, &child_path()) {
        return Ok(found);
    }
    crate::core::system::commands::resolve_off_path(bin).ok_or_else(|| {
        format!(
            "{bin} is not installed or not on PATH. Install it from the Launch page and run again."
        )
    })
}

/// PATH for processes the bench starts: the user's login-shell PATH, so a GUI
/// launch finds the same tools (`cargo`, `pax`) a terminal does.
pub fn child_path() -> String {
    #[cfg(not(windows))]
    if let Some(p) = crate::core::system::commands::login_shell_path() {
        return p;
    }
    std::env::var("PATH").unwrap_or_default()
}

pub fn search_path(bin: &str, path: &str) -> Option<PathBuf> {
    let names: Vec<String> = if cfg!(windows) {
        vec![
            format!("{bin}.exe"),
            format!("{bin}.cmd"),
            format!("{bin}.bat"),
            bin.to_string(),
        ]
    } else {
        vec![bin.to_string()]
    };
    std::env::split_paths(path).find_map(|dir| {
        names
            .iter()
            .map(|n| dir.join(n))
            .find(|candidate| candidate.is_file())
    })
}

// ---- the agent's own evidence --------------------------------------------------

/// How to read the agent's trajectory after its run, or why it cannot be read.
/// stack-agent: `agent-state show --state-db FILE --task-id ID --json`, its
/// documented inspection command, with the task id its report names.
pub fn trace_command(
    agent_id: &str,
    launch: &Launch,
    report: Option<&Value>,
) -> Result<Vec<String>, String> {
    match agent_id {
        "stack-agent" => {
            let db = launch
                .state_db
                .as_ref()
                .ok_or("the run had no per-run state database")?;
            let report =
                report.ok_or("stack-agent printed no report, so its task id is unknown")?;
            let state = &report["agent_state"];
            let id = state["task"]["task_id"]
                .as_str()
                .or_else(|| state["task_id"].as_str())
                .or_else(|| state["work_id"].as_str())
                .ok_or("stack-agent's report names no task id")?;
            if state["recorded"] == false {
                return Err(format!(
                    "stack-agent did not record this work: {}",
                    state["reason"].as_str().unwrap_or("no reason given")
                ));
            }
            Ok(vec![
                "agent-state".into(),
                "show".into(),
                "--state-db".into(),
                db.to_string_lossy().into_owned(),
                "--task-id".into(),
                id.to_string(),
                "--json".into(),
            ])
        }
        "claude-code" => Err(
            "Claude Code's `--output-format json` reports only its final result; this adapter \
             does not capture its tool calls"
                .into(),
        ),
        other => Err(format!("{other} exposes no trajectory to Agent Bench")),
    }
}

pub const STACK_AGENT_TRACE_SOURCE: &str = "stack-agent agent-state show --json";

/// stack-agent's events from `agent-state show --json`, in order, with the
/// provenance stack-agent gave each one.
pub fn stack_agent_trace(show: &Value, redactor: &super::redact::Redactor) -> Vec<TraceEvent> {
    let mut events: Vec<TraceEvent> = show["events"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|e| {
                    let event = redactor.json(&e["event"]);
                    let kind = event["kind"].as_str().unwrap_or("unknown").to_string();
                    TraceEvent {
                        index: e["index"].as_u64().unwrap_or(0),
                        at: e["recorded_at_ms"]
                            .as_i64()
                            .and_then(chrono::DateTime::from_timestamp_millis)
                            .map(|t| t.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)),
                        provenance: match e["provenance"].as_str() {
                            Some("host_observed") => Provenance::Observed,
                            Some("model_inferred") => Provenance::AgentClaim,
                            _ => Provenance::Runtime,
                        },
                        summary: summarize_stack_agent_event(&kind, &event),
                        kind,
                        data: event,
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    events.sort_by_key(|e| (e.data["attempt"].as_u64().unwrap_or(0), e.index));
    events
}

/// One line per event, from its own fields only.
fn summarize_stack_agent_event(kind: &str, e: &Value) -> String {
    let s = |k: &str| e[k].as_str().unwrap_or("").to_string();
    let n = |k: &str| {
        e[k].as_u64()
            .map(|v| v.to_string())
            .unwrap_or_else(|| "?".into())
    };
    match kind {
        "WorkStarted" => format!(
            "work started (at most {} turns, {} executions)",
            n("max_turns"),
            n("max_executions")
        ),
        "CapabilitiesAvailable" => format!("{} capabilities available", n("count")),
        "CapabilitiesUnavailable" => format!("capabilities unavailable: {}", s("reason")),
        "DecisionStarted" => format!("turn {} started", n("turn")),
        "ModelEscalation" => format!("turn {}: asking the model ({})", n("turn"), s("reason")),
        "LocalDecision" => format!("turn {}: local decision {}", n("turn"), s("decision")),
        "ModelCalled" => format!(
            "turn {}: model call {} ({} prompt / {} completion tokens)",
            n("turn"),
            if e["succeeded"] == false {
                "failed"
            } else {
                "returned"
            },
            n("prompt_tokens"),
            n("completion_tokens")
        ),
        "ContextLimit" => format!("turn {}: context limit reached", n("turn")),
        "DecisionMade" => format!("turn {}: model decided: {}", n("turn"), s("decision")),
        "DecisionRejected" => format!("turn {}: decision rejected: {}", n("turn"), s("reason")),
        "CapabilityRequested" => format!("turn {}: model requested {}", n("turn"), s("capability")),
        "ExecutionRequested" => format!("execution of {} requested", s("capability")),
        "ExecutionStarted" => "execution started".to_string(),
        "ExecutionCompleted" => "execution completed".to_string(),
        "ExecutionFailed" => format!(
            "execution failed: {}",
            failure_code(e).unwrap_or_else(|| s("reason"))
        ),
        "ObservationRecorded" => format!("observation recorded ({})", s("observation")),
        "EvidenceRecorded" => format!("evidence recorded from {}", s("capability")),
        "GoalEvaluated" => format!(
            "goal evaluated: {}",
            if e["satisfied"] == true {
                "satisfied".to_string()
            } else {
                format!("not satisfied ({} remaining)", n("remaining"))
            }
        ),
        "WorkCompleted" => "work completed".to_string(),
        "WorkEscalated" => format!("work escalated: {}", s("reason")),
        "WorkBlocked" => format!("work blocked: {}", s("reason")),
        "WorkLimitReached" => format!("work stopped at the {} limit", s("limit")),
        "WorkFailed" => format!("work failed: {}", s("reason")),
        "WorkCancelled" => format!("work cancelled ({}): {}", s("cause"), s("reason")),
        other => other.to_string(),
    }
}

/// The error code of a failed stack-agent execution
/// (`{"capability":…,"error":"not_a_repository"}` in its reason).
fn failure_code(e: &Value) -> Option<String> {
    let reason = e["reason"].as_str()?;
    let v: Value = serde_json::from_str(reason).ok()?;
    let cap = v["capability"].as_str().unwrap_or("?");
    v["error"].as_str().map(|err| format!("{cap}: {err}"))
}

/// How stack-agent's loop guard begins the reason of a work it blocked.
const STACK_AGENT_STUCK_PREFIX: &str = "stuck: ";

/// stack-agent's words for a refusal on authority grounds (chip-project's
/// path policy), as they appear in a rejected decision or failed execution.
const STACK_AGENT_AUTHORITY_PHRASES: &[&str] = &[
    "is protected and is never written",
    "is reserved",
    "resolves outside the project root",
];

/// Facts and claims from the agent's report and trajectory, each with where it
/// came from. Only what the agent recorded; nothing is inferred from prose.
pub fn signals(agent_id: &str, report: Option<&Value>, trace: Option<&AgentTrace>) -> Vec<Signal> {
    let mut out = Vec::new();
    let mut push = |kind: SignalKind, provenance: Provenance, source: &str, detail: String| {
        out.push(Signal {
            kind,
            provenance,
            source: source.to_string(),
            detail,
        })
    };
    match agent_id {
        "stack-agent" => {
            if let Some(r) = report {
                let terminal = r["terminal_state"].as_str().unwrap_or("");
                if !terminal.is_empty() {
                    push(
                        SignalKind::TerminalState(terminal.into()),
                        Provenance::Runtime,
                        "report.terminal_state",
                        format!("stack-agent ended the work as {terminal}"),
                    );
                }
                let reason = r["outcome_reason"].as_str().unwrap_or("").to_string();
                // stack-agent's loop guard (chip-core `stuck::REASON_PREFIX`) blocks a
                // work that repeats itself; that reason is the runtime's, not the model's.
                let stuck = reason.strip_prefix(STACK_AGENT_STUCK_PREFIX);
                if let Some(rest) = stuck {
                    let capability = rest.split_whitespace().next().unwrap_or("").to_string();
                    push(
                        SignalKind::RepeatedAction(capability),
                        Provenance::Runtime,
                        "report.outcome_reason",
                        reason.clone(),
                    );
                }
                match terminal {
                    "escalated" | "blocked" => push(
                        SignalKind::GaveUp,
                        if stuck.is_some() {
                            Provenance::Runtime
                        } else {
                            Provenance::AgentClaim
                        },
                        "report.outcome_reason",
                        reason.clone(),
                    ),
                    "completed" => push(
                        SignalKind::CompletionClaimed,
                        Provenance::AgentClaim,
                        "report.terminal_state",
                        format!(
                            "stack-agent reported the work completed (verified: {})",
                            r["verified"]
                        ),
                    ),
                    "limit_reached" => push(
                        SignalKind::LimitReached(reason.clone()),
                        Provenance::Runtime,
                        "report.terminal_state",
                        reason.clone(),
                    ),
                    _ => {}
                }
                for f in r["model_failures"].as_array().into_iter().flatten() {
                    let kind = f["kind"].as_str().unwrap_or("unknown").to_string();
                    push(
                        SignalKind::ModelCallFailed(kind.clone()),
                        Provenance::Observed,
                        "report.model_failures",
                        format!("model call failed: {kind} (status {})", f["status"]),
                    );
                }
                let invalid = r["invalid_decisions"].as_u64().unwrap_or(0)
                    + r["malformed_replies"]
                        .as_array()
                        .map_or(0, |a| a.len() as u64)
                    + r["malformed_replies"].as_u64().unwrap_or(0);
                if invalid > 0 {
                    push(
                        SignalKind::InvalidDecision,
                        Provenance::Observed,
                        "report.invalid_decisions",
                        format!("{invalid} model replies were not valid decisions"),
                    );
                }
                let audit = &r["audit"];
                let violations: u64 = [
                    "unauthorized_executions",
                    "unauthorized_completions",
                    "path_escape",
                    "out_of_root_write",
                ]
                .iter()
                .map(|k| audit[*k].as_u64().unwrap_or(0))
                .sum();
                let replaced = r["evidence"]["test_files_replaced"]
                    .as_array()
                    .map_or(0, |a| a.len());
                if violations > 0 || replaced > 0 || !r["evidence"]["invalidated"].is_null() {
                    push(
                        SignalKind::AuthorityRejection,
                        Provenance::Observed,
                        "report.audit/evidence",
                        format!(
                            "{violations} audit violations, {replaced} replaced test files, evidence invalidated: {}",
                            r["evidence"]["invalidated"]
                        ),
                    );
                }
                if audit["context_limit_rejections"].as_u64().unwrap_or(0) > 0 {
                    push(
                        SignalKind::ContextLimit,
                        Provenance::Observed,
                        "report.audit.context_limit_rejections",
                        "a model request was over the context limit".into(),
                    );
                }
                let executions: u64 = r["executions_by_capability"]
                    .as_object()
                    .map_or(0, |m| m.values().filter_map(Value::as_u64).sum());
                if r["executions_by_capability"].is_object() && executions == 0 {
                    push(
                        SignalKind::NoActionTaken,
                        Provenance::Observed,
                        "report.executions_by_capability",
                        "stack-agent executed no capability".into(),
                    );
                }
                let discovery: u64 = ["lists", "searches", "reads"]
                    .iter()
                    .map(|k| r[*k].as_u64().unwrap_or(0))
                    .sum();
                if matches!(terminal, "escalated" | "blocked") && discovery == 0 {
                    push(
                        SignalKind::NoDiscovery,
                        Provenance::Observed,
                        "report.lists/searches/reads",
                        "it stopped without listing, searching or reading any project file".into(),
                    );
                }
            }
            for e in trace.map(|t| t.events.as_slice()).unwrap_or(&[]) {
                let source = format!("trace#{}", e.index);
                match e.kind.as_str() {
                    "ExecutionFailed" => {
                        let code = failure_code(&e.data)
                            .unwrap_or_else(|| e.data["reason"].as_str().unwrap_or("").to_string());
                        if STACK_AGENT_AUTHORITY_PHRASES
                            .iter()
                            .any(|p| code.contains(p))
                        {
                            push(
                                SignalKind::AuthorityRejection,
                                Provenance::Observed,
                                &source,
                                code,
                            );
                        } else {
                            push(
                                SignalKind::CapabilityFailed(code.clone()),
                                Provenance::Observed,
                                &source,
                                code,
                            );
                        }
                    }
                    "DecisionRejected" => {
                        let reason = e.data["reason"].as_str().unwrap_or("").to_string();
                        let kind = if STACK_AGENT_AUTHORITY_PHRASES
                            .iter()
                            .any(|p| reason.contains(p))
                        {
                            SignalKind::AuthorityRejection
                        } else {
                            SignalKind::InvalidDecision
                        };
                        push(kind, Provenance::Runtime, &source, reason);
                    }
                    "ContextLimit" => push(
                        SignalKind::ContextLimit,
                        Provenance::Runtime,
                        &source,
                        e.summary.clone(),
                    ),
                    "WorkLimitReached" => push(
                        SignalKind::LimitReached(e.data["limit"].as_str().unwrap_or("").into()),
                        Provenance::Runtime,
                        &source,
                        e.summary.clone(),
                    ),
                    "ModelCalled" if e.data["succeeded"] == false => push(
                        SignalKind::ModelCallFailed("call_failed".into()),
                        Provenance::Observed,
                        &source,
                        e.summary.clone(),
                    ),
                    _ => {}
                }
            }
        }
        "claude-code" => {
            if let Some(r) = report {
                if r["is_error"] == true {
                    push(
                        SignalKind::TerminalState(r["subtype"].as_str().unwrap_or("error").into()),
                        Provenance::Runtime,
                        "report.is_error",
                        format!("Claude Code ended with an error ({})", r["subtype"]),
                    );
                } else if r["subtype"] == "success" {
                    push(
                        SignalKind::CompletionClaimed,
                        Provenance::AgentClaim,
                        "report.result",
                        "Claude Code reported a successful result".into(),
                    );
                }
                if r["subtype"]
                    .as_str()
                    .is_some_and(|s| s.contains("max_turns"))
                {
                    push(
                        SignalKind::LimitReached("max_turns".into()),
                        Provenance::Runtime,
                        "report.subtype",
                        "Claude Code stopped at its turn limit".into(),
                    );
                }
            }
        }
        _ => {}
    }
    out
}
