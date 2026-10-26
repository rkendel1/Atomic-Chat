//! Evidence an agent leaves about its own run, in agent-neutral form.
//!
//! Each adapter translates what its agent actually exposes (stack-agent: the
//! `work --json` report, `agent-state show --json` and `--print-reply`) into
//! these types. Every item keeps where it came from:
//!
//! - [`Provenance::Observed`]: the agent's host saw it happen (an execution
//!   started or failed, a model call returned). Facts about the process.
//! - [`Provenance::AgentClaim`]: the model said it (a decision, a reason, a
//!   completion). Never evidence of success.
//! - [`Provenance::Runtime`]: the agent's runtime decided it (it stopped at a
//!   limit, it escalated). A fact about the agent, not about the project.
//!
//! Nothing here is inferred: an item exists only when the agent recorded it.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Provenance {
    Observed,
    AgentClaim,
    Runtime,
}

/// One event of the agent's own trajectory, as the agent recorded it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TraceEvent {
    /// Position in the agent's trajectory (0, 1, 2, ...).
    pub index: u64,
    /// When the agent recorded it, RFC 3339, if it said.
    #[serde(default)]
    pub at: Option<String>,
    pub provenance: Provenance,
    /// The agent's own name for the event, e.g. `ExecutionFailed`.
    pub kind: String,
    /// A one-line description built only from the event's fields.
    pub summary: String,
    /// The event as the agent recorded it (credentials removed).
    #[serde(default)]
    pub data: Value,
}

/// A model reply the agent printed. The model's words: a claim.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelReply {
    /// 1-based order in which the agent printed it.
    pub position: u32,
    pub text: String,
    #[serde(default)]
    pub truncated: bool,
}

/// The agent's trajectory, or why there is none.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AgentTrace {
    /// The interface it was read from, e.g. `stack-agent agent-state show --json`.
    pub source: String,
    pub available: bool,
    #[serde(default)]
    pub unavailable_reason: Option<String>,
    #[serde(default)]
    pub events: Vec<TraceEvent>,
    #[serde(default)]
    pub replies: Vec<ModelReply>,
    /// Why the reply text is missing, when it is.
    #[serde(default)]
    pub replies_unavailable_reason: Option<String>,
}

/// A normalized fact or claim the failure analysis can use.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Signal {
    pub kind: SignalKind,
    pub provenance: Provenance,
    /// Where it came from: `report.terminal_state`, `trace#10`, ...
    pub source: String,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type", content = "value")]
pub enum SignalKind {
    /// How the agent's runtime ended the work (`completed`, `escalated`,
    /// `blocked`, `limit_reached`, `failed`, `cancelled`, ...).
    TerminalState(String),
    /// The agent stopped and said it could not continue (escalated, blocked).
    GaveUp,
    /// The agent claimed the work is done (and, for stack-agent, verified).
    CompletionClaimed,
    /// A turn, execution, time or other limit of the agent stopped it.
    LimitReached(String),
    /// A model call failed (`timeout`, `transport`, `length_limited`, ...).
    ModelCallFailed(String),
    /// A model reply was not a valid decision, or a decision was refused.
    InvalidDecision,
    /// A capability (tool) the agent ran failed, with its error code.
    CapabilityFailed(String),
    /// The agent refused or was refused an action on authority grounds
    /// (withheld file, path escape, replaced test, unauthorized execution).
    AuthorityRejection,
    /// The model's context hit its limit.
    ContextLimit,
    /// The agent executed no capability at all.
    NoActionTaken,
    /// The agent stopped without listing, searching or reading the project.
    NoDiscovery,
    /// The agent's runtime stopped it for repeating one capability with the
    /// same inputs and result (stack-agent's `stuck: ` loop guard).
    RepeatedAction(String),
}

impl SignalKind {
    pub fn name(&self) -> &'static str {
        match self {
            Self::TerminalState(_) => "terminal_state",
            Self::GaveUp => "gave_up",
            Self::CompletionClaimed => "completion_claimed",
            Self::LimitReached(_) => "limit_reached",
            Self::ModelCallFailed(_) => "model_call_failed",
            Self::InvalidDecision => "invalid_decision",
            Self::CapabilityFailed(_) => "capability_failed",
            Self::AuthorityRejection => "authority_rejection",
            Self::ContextLimit => "context_limit",
            Self::NoActionTaken => "no_action_taken",
            Self::NoDiscovery => "no_discovery",
            Self::RepeatedAction(_) => "repeated_action",
        }
    }
}

/// `Model reply:` blocks stack-agent writes to stderr with `--print-reply`.
/// Each text is cut at `limit` bytes (on a character boundary).
pub fn parse_print_replies(stderr: &str, limit: usize) -> Vec<ModelReply> {
    stderr
        .split("Model reply:\n")
        .skip(1)
        .enumerate()
        .map(|(i, chunk)| {
            // A reply ends where the next diagnostic line starts.
            let end = ["\nerror:", "\nnote:", "\nwarning:", "\nturn "]
                .iter()
                .filter_map(|m| chunk.find(m))
                .min()
                .unwrap_or(chunk.len());
            let text = chunk[..end].trim();
            let mut cut = text.len().min(limit);
            while !text.is_char_boundary(cut) {
                cut -= 1;
            }
            ModelReply {
                position: i as u32 + 1,
                text: text[..cut].to_string(),
                truncated: cut < text.len(),
            }
        })
        .collect()
}
