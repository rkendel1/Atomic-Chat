//! The agent-neutral task contract, `atomic.bench.task.v1`.
//!
//! A task says what to do (`goal`), where (`workdir`), what success means
//! (`success_criteria`) and how that is established (`evaluation`). Nothing in
//! it is specific to one agent: adapters turn it into each agent's own
//! invocation. `version` increases whenever the content changes, and `digest`
//! identifies the exact content a run was given.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const TASK_SCHEMA: &str = "atomic.bench.task.v1";

/// Upper bound on any task or verification timeout, in seconds (6 h).
pub const MAX_TIMEOUT_SECS: u64 = 6 * 60 * 60;
/// Agent timeout when a task sets none.
pub const DEFAULT_TIMEOUT_SECS: u64 = 15 * 60;
/// Verification command timeout when a command sets none.
pub const DEFAULT_CHECK_TIMEOUT_SECS: u64 = 10 * 60;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BenchTask {
    pub schema: String,
    /// Stable identifier: lowercase letters, digits, `-`, `_` and `.`.
    pub id: String,
    /// Starts at 1 and increases each time the content changes.
    pub version: u32,
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// The instruction every agent receives.
    pub goal: String,
    pub workdir: WorkdirRef,
    #[serde(default)]
    pub success_criteria: Vec<String>,
    pub evaluation: Evaluation,
    #[serde(default)]
    pub limits: Limits,
    /// Shipped with Atomic Chat; read-only, duplicate it to change it.
    #[serde(default)]
    pub builtin: bool,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WorkdirRef {
    /// A bundled fixture, copied into a fresh workspace for every run so a run
    /// never touches anything else.
    Fixture { fixture: String },
    /// A project on disk. The agent works in place and its changes are kept.
    Path { path: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "method", rename_all = "snake_case")]
pub enum Evaluation {
    /// Explicit commands, run after the agent in the run's working directory.
    /// Each passes when it exits 0. They come only from the task, never from
    /// an agent.
    Command {
        commands: Vec<VerificationCommand>,
        /// A bundled set of check files written into a fixture workspace just
        /// before verification, overwriting whatever the agent left there.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        held_out_checks: Option<String>,
    },
    /// A person marks the result successful, unsuccessful or inconclusive.
    Manual {
        #[serde(default)]
        instructions: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VerificationCommand {
    pub name: String,
    /// Run directly, without a shell.
    pub program: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
}

impl VerificationCommand {
    /// How the command reads to a person (display only; never re-parsed).
    pub fn display(&self) -> String {
        std::iter::once(self.program.as_str())
            .chain(self.args.iter().map(String::as_str))
            .map(|part| {
                if part.is_empty() || part.contains(char::is_whitespace) {
                    format!("\"{part}\"")
                } else {
                    part.to_string()
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Limits {
    /// Wall-clock limit for the agent; the run is stopped when it passes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
    /// Passed to agents that take a turn limit; ignored by the others.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_turns: Option<u32>,
}

impl Limits {
    pub fn effective_timeout_secs(&self) -> u64 {
        self.timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS)
    }
}

pub fn is_valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 80
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '_' | '.'))
        && !id.starts_with('.')
}

impl BenchTask {
    /// Checks everything a run relies on. Returns the first problem found.
    pub fn validate(&self) -> Result<(), String> {
        if self.schema != TASK_SCHEMA {
            return Err(format!(
                "unsupported task schema {:?}; this version reads {TASK_SCHEMA}",
                self.schema
            ));
        }
        if !is_valid_id(&self.id) {
            return Err(
                "task id must be 1-80 lowercase letters, digits, '-', '_' or '.'".to_string(),
            );
        }
        if self.version == 0 {
            return Err("task version starts at 1".to_string());
        }
        if self.name.trim().is_empty() {
            return Err("task name is required".to_string());
        }
        if self.goal.trim().is_empty() {
            return Err("task goal is required".to_string());
        }
        match &self.workdir {
            WorkdirRef::Fixture { fixture } => {
                if !super::suite::fixture_exists(fixture) {
                    return Err(format!("unknown fixture {fixture:?}"));
                }
            }
            WorkdirRef::Path { path } => {
                if path.trim().is_empty() {
                    return Err("choose a project directory".to_string());
                }
                if !std::path::Path::new(path).is_absolute() {
                    return Err("the project directory must be an absolute path".to_string());
                }
            }
        }
        match &self.evaluation {
            Evaluation::Command {
                commands,
                held_out_checks,
            } => {
                if commands.is_empty() {
                    return Err(
                        "command evaluation needs at least one verification command".to_string()
                    );
                }
                for c in commands {
                    if c.name.trim().is_empty() || c.program.trim().is_empty() {
                        return Err("each verification command needs a name and a program".into());
                    }
                    if c.timeout_secs
                        .is_some_and(|t| t == 0 || t > MAX_TIMEOUT_SECS)
                    {
                        return Err(format!(
                            "verification timeout must be 1-{MAX_TIMEOUT_SECS} seconds"
                        ));
                    }
                }
                if let Some(set) = held_out_checks {
                    if !matches!(self.workdir, WorkdirRef::Fixture { .. }) {
                        return Err("held-out checks apply only to fixture tasks".to_string());
                    }
                    if super::suite::held_out_files(set).is_none() {
                        return Err(format!("unknown held-out check set {set:?}"));
                    }
                }
            }
            Evaluation::Manual { .. } => {}
        }
        if self
            .limits
            .timeout_secs
            .is_some_and(|t| t == 0 || t > MAX_TIMEOUT_SECS)
        {
            return Err(format!("timeout must be 1-{MAX_TIMEOUT_SECS} seconds"));
        }
        if self.limits.max_turns == Some(0) {
            return Err("max turns must be at least 1".to_string());
        }
        Ok(())
    }

    /// SHA-256 of the content that decides what a run does: everything except
    /// the version, timestamps and the built-in flag.
    pub fn digest(&self) -> String {
        let content = serde_json::json!({
            "schema": self.schema,
            "id": self.id,
            "name": self.name,
            "description": self.description,
            "goal": self.goal,
            "workdir": self.workdir,
            "success_criteria": self.success_criteria,
            "evaluation": self.evaluation,
            "limits": self.limits,
        });
        // serde_json orders object keys, so this text is canonical.
        let text = serde_json::to_string(&content).unwrap_or_default();
        hex::encode(Sha256::digest(text.as_bytes()))
    }

    /// The instruction given to every agent: the goal, then the success
    /// criteria, so each agent is told the same thing.
    pub fn prompt(&self) -> String {
        let criteria: Vec<&str> = self
            .success_criteria
            .iter()
            .map(|c| c.trim())
            .filter(|c| !c.is_empty())
            .collect();
        if criteria.is_empty() {
            return self.goal.trim().to_string();
        }
        let mut out = self.goal.trim().to_string();
        out.push_str("\n\nSuccess criteria:");
        for c in criteria {
            out.push_str("\n- ");
            out.push_str(c);
        }
        out
    }
}
