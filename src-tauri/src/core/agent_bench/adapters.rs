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

pub fn plan(task: &BenchTask, cfg: &RunConfig, program: &Path) -> Result<Launch, String> {
    match cfg.agent_id.as_str() {
        "stack-agent" => plan_stack_agent(task, cfg, program),
        "claude-code" => plan_claude_code(task, cfg, program),
        other => Err(format!("{other} has no Agent Bench adapter")),
    }
}

fn plan_stack_agent(task: &BenchTask, cfg: &RunConfig, program: &Path) -> Result<Launch, String> {
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
        "--deadline-secs".to_string(),
        deadline.to_string(),
    ];
    let mut config = BTreeMap::new();
    config.insert("kind".to_string(), "change".to_string());
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
