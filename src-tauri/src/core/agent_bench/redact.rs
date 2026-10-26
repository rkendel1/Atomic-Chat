//! Keeps credentials out of everything the ledger stores or logs.
//!
//! Two layers: every value of a secret handed to an agent is replaced
//! wherever it appears, and well-known credential shapes (`sk-…` keys, bearer
//! tokens, `*_API_KEY=` assignments) are replaced even when their value was
//! never known to Atomic Chat, e.g. a key an agent read from its own config.

use std::sync::OnceLock;

use regex::Regex;
use serde_json::Value;

pub const REDACTED: &str = "[REDACTED]";

/// Values shorter than this are not treated as secrets: replacing a short
/// placeholder such as `atomic` would mangle ordinary text and protect nothing.
const MIN_SECRET_LEN: usize = 8;

fn patterns() -> &'static [Regex] {
    static PATTERNS: OnceLock<Vec<Regex>> = OnceLock::new();
    PATTERNS.get_or_init(|| {
        [
            // Assignments to credential-named variables or fields, quoted or not.
            r#"(?i)((?:[A-Z0-9_]*(?:API_KEY|AUTH_TOKEN|ACCESS_TOKEN|SECRET|PASSWORD))["']?\s*[:=]\s*["']?)[^\s"',}]+"#,
            r"(?i)(authorization:\s*bearer\s+)[A-Za-z0-9._~+/=-]+",
            r"(?i)(\bbearer\s+)[A-Za-z0-9._~+/=-]{16,}",
            r"()\bsk-(?:ant-|proj-)?[A-Za-z0-9_-]{16,}",
        ]
        .iter()
        .map(|p| Regex::new(p).expect("valid redaction pattern"))
        .collect()
    })
}

#[derive(Debug, Clone, Default)]
pub struct Redactor {
    secrets: Vec<String>,
}

impl Redactor {
    pub fn new<I, S>(secrets: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let mut secrets: Vec<String> = secrets
            .into_iter()
            .map(Into::into)
            .filter(|s| s.len() >= MIN_SECRET_LEN)
            .collect();
        // Longest first, so a secret containing another is replaced whole.
        secrets.sort_by_key(|s| std::cmp::Reverse(s.len()));
        secrets.dedup();
        Self { secrets }
    }

    pub fn text(&self, input: &str) -> String {
        let mut out = input.to_string();
        for s in &self.secrets {
            if out.contains(s.as_str()) {
                out = out.replace(s.as_str(), REDACTED);
            }
        }
        for re in patterns() {
            out = re
                .replace_all(&out, |c: &regex::Captures| format!("{}{REDACTED}", &c[1]))
                .into_owned();
        }
        out
    }

    /// Redacts every string in a JSON value, and drops the value of any
    /// object field whose name says it holds a credential.
    pub fn json(&self, value: &Value) -> Value {
        match value {
            Value::String(s) => Value::String(self.text(s)),
            Value::Array(items) => Value::Array(items.iter().map(|v| self.json(v)).collect()),
            Value::Object(map) => Value::Object(
                map.iter()
                    .map(|(k, v)| {
                        if is_credential_name(k) && !v.is_null() && !v.is_boolean() {
                            (k.clone(), Value::String(REDACTED.to_string()))
                        } else {
                            (k.clone(), self.json(v))
                        }
                    })
                    .collect(),
            ),
            other => other.clone(),
        }
    }
}

/// True for names of variables or fields that hold credentials.
pub fn is_credential_name(name: &str) -> bool {
    let upper = name.to_ascii_uppercase().replace('-', "_");
    // Token *counts* (`input_tokens`, `max_tokens`) are not credentials.
    upper == "TOKEN"
        || upper.ends_with("_TOKEN")
        || upper.ends_with("KEY") && (upper.contains("API") || upper.contains("SECRET"))
        || ["SECRET", "PASSWORD", "CREDENTIAL", "AUTHORIZATION"]
            .iter()
            .any(|needle| upper.contains(needle))
}
