//! One call to a tool of Claude's connectors (Notion, Google Drive, Slack),
//! for parts that can't reach them otherwise: a headless Haiku with no
//! built-in tools but tool search, and only that one tool allowed, told to
//! load it and call it once with the given input. Tool search keeps every
//! other connector's tool definitions out of its context: with them all
//! loaded, a call read about 230,000 tokens a turn and cost about $0.05,
//! against well under a cent this way. The answer is the call's own result,
//! taken from the stream rather than from what Haiku says, so it is what the
//! connector gave. Its stdin is closed, so it doesn't wait for input, and it
//! keeps no session. Claude Code's own bin is added to the PATH, for a server
//! started without it. A result too large for the
//! conversation is saved to a file, which is read instead.
//!
//! A connector that isn't connected, or whose sign-in has lapsed, fails with
//! `not_connected` set and a message saying what to do, and each call records
//! how its service answered in the state directory:
//! `connectors.json` (`{ "Slack": { ok, at, message } }`) for the views, and
//! `connectors-down`, a `<service>\t<when>` line for each one down, for the
//! health light. So a feature that is on and
//! can't reach its connector says so where it shows, and in the health light,
//! rather than failing quietly.

use std::collections::HashSet;
use std::fmt;
use std::sync::LazyLock;
use std::time::Duration;

use regex::Regex;
use serde_json::{Map, Value, json};

use crate::{paths, requests, util};

/// Why a call failed; `not_connected` when the connector isn't connected (or
/// its sign-in lapsed) in Claude.
#[derive(Debug, Clone)]
pub struct ConnectorError {
    pub message: String,
    pub not_connected: bool,
}

impl fmt::Display for ConnectorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ConnectorError {}

fn failed(message: impl Into<String>) -> ConnectorError {
    ConnectorError {
        message: message.into(),
        not_connected: false,
    }
}

fn not_connected(service: &str) -> ConnectorError {
    ConnectorError {
        message: format!(
            "{service} isn't connected in Claude: run /mcp in Claude and sign in to {service}."
        ),
        not_connected: true,
    }
}

fn status_file() -> std::path::PathBuf {
    paths::state_dir().join("connectors.json")
}

static SERVICE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^mcp__claude_ai_(.+?)__").unwrap());

/// The service a tool belongs to, as Claude names its connector:
/// `mcp__claude_ai_Google_Calendar__list_events` is "Google Calendar".
pub fn service_of(tool: &str) -> String {
    SERVICE
        .captures(tool)
        .map(|c| c[1].to_string())
        .unwrap_or_else(|| "the connector".into())
        .replace('_', " ")
}

fn read_status() -> Map<String, Value> {
    util::read_json_object(status_file())
}

fn record(service: &str, ok: bool, message: Option<&str>) {
    let mut status = read_status();
    if ok
        && status
            .get(service)
            .and_then(|s| s.get("ok"))
            .and_then(Value::as_bool)
            == Some(true)
    {
        return;
    }
    let mut entry = Map::new();
    entry.insert("ok".into(), Value::Bool(ok));
    entry.insert("at".into(), Value::from(util::now_ms()));
    if let Some(m) = message.filter(|m| !m.is_empty()) {
        entry.insert("message".into(), Value::from(m));
    }
    status.insert(service.to_string(), Value::Object(entry));
    let dir = paths::state_dir();
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let _ = std::fs::write(
        status_file(),
        util::json_compact(&Value::Object(status.clone())),
    );
    let down: String = status
        .iter()
        .filter(|(_, s)| s.get("ok").and_then(Value::as_bool) != Some(true))
        .map(|(name, s)| {
            format!(
                "{name}\t{}\n",
                s.get("at")
                    .and_then(Value::as_i64)
                    .unwrap_or(0)
                    .div_euclid(1000)
            )
        })
        .collect();
    let _ = std::fs::write(dir.join("connectors-down"), down);
}

/// What a view shows when its service is down, or `None`.
pub fn connector_down(service: &str) -> Option<String> {
    let status = read_status();
    let s = status.get(service)?;
    if s.get("ok").and_then(Value::as_bool) == Some(true) {
        return None;
    }
    Some(
        s.get("message")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
    )
}

// A tool's own error that means its sign-in is missing or has lapsed.
static AUTH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(authenticat|unauthori[sz]ed|sign(ed)? ?in|log(ged)? ?in|re-?auth|expired token|invalid_auth|not_authed|401|403)").unwrap()
});
static SAVED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:has been saved to|Full output saved to:) (/\S+?\.(txt|json))\b").unwrap()
});

/// Calls `tool` once with `input` and gives its result's text. `timeout`
/// defaults to two minutes.
pub async fn call_connector(
    tool: &str,
    input: &Value,
    timeout: Option<Duration>,
) -> Result<String, ConnectorError> {
    let service = service_of(tool);
    match call(tool, input, timeout.unwrap_or(Duration::from_secs(120))).await {
        Ok(text) => {
            record(&service, true, None);
            Ok(text)
        }
        Err(err) => {
            if err.not_connected {
                record(&service, false, Some(&err.message));
            }
            Err(err)
        }
    }
}

/// Each line of Claude's stream-json output's message content parts.
fn content_parts(stdout: &str) -> Vec<Value> {
    stdout
        .split('\n')
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter_map(|v| {
            v.get("message")
                .and_then(|m| m.get("content"))
                .and_then(Value::as_array)
                .cloned()
        })
        .flatten()
        .collect()
}

async fn call(tool: &str, input: &Value, timeout: Duration) -> Result<String, ConnectorError> {
    let prompt = format!(
        "Load {tool} with ToolSearch (query \"select:{tool}\"), then call it once, with exactly this input: {}. Then reply \"done\".",
        util::json_compact(input)
    );
    let path = format!(
        "{}:{}",
        std::env::var("PATH").unwrap_or_default(),
        paths::home_join(".local/bin").display()
    );
    let allowed = format!("ToolSearch,{tool}");
    let mut cmd = tokio::process::Command::new("claude");
    cmd.args([
        "-p",
        "--model",
        "haiku",
        "--output-format",
        "stream-json",
        "--verbose",
        "--no-session-persistence",
        "--tools",
        "ToolSearch",
        "--allowedTools",
        &allowed,
        "--max-turns",
        "4",
        &prompt,
    ])
    .current_dir(std::env::temp_dir())
    .env("PATH", path)
    .env("ENABLE_TOOL_SEARCH", "true")
    .env("MAX_MCP_OUTPUT_TOKENS", "200000")
    .stdin(std::process::Stdio::null())
    .stdout(std::process::Stdio::piped())
    .stderr(std::process::Stdio::piped())
    .kill_on_drop(true);
    let child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(failed("Claude Code isn't installed"));
        }
        Err(e) => return Err(failed(format!("Claude didn't answer ({e})"))),
    };
    // Counted once Claude ran, and the connector once Claude called it, a run
    // that then failed or ran out of time included.
    let waited = tokio::time::timeout(timeout, child.wait_with_output()).await;
    requests::count("Anthropic", "claude -p", "connector");
    let (stdout, err) = match waited {
        Ok(Ok(out)) => {
            let text = String::from_utf8_lossy(&out.stdout).into_owned();
            let err = (!out.status.success()).then(|| {
                let stderr = String::from_utf8_lossy(&out.stderr);
                format!(
                    "Command failed: claude{}",
                    stderr
                        .lines()
                        .next()
                        .map(|l| format!(" ({l})"))
                        .unwrap_or_default()
                )
            });
            (text, err)
        }
        Ok(Err(e)) => (String::new(), Some(e.to_string())),
        Err(_) => (String::new(), Some("timed out".to_string())),
    };
    let parts = content_parts(&stdout);
    // Each call by its own ID, a retry counted as one more.
    let calls: HashSet<String> = parts
        .iter()
        .filter(|p| {
            p.get("type").and_then(Value::as_str) == Some("tool_use")
                && p.get("name").and_then(Value::as_str) == Some(tool)
        })
        .filter_map(|p| p.get("id").and_then(Value::as_str).map(str::to_string))
        .collect();
    if !calls.is_empty() {
        requests::count_n(&service_of(tool), "MCP", tool, calls.len());
    }
    if let Some(e) = err {
        return Err(failed(format!(
            "Claude didn't answer ({})",
            e.lines().next().unwrap_or("")
        )));
    }
    let use_ = parts.iter().find(|p| {
        p.get("type").and_then(Value::as_str) == Some("tool_use")
            && p.get("name").and_then(Value::as_str) == Some(tool)
    });
    let result = use_.and_then(|u| {
        let id = u.get("id");
        parts.iter().find(|p| {
            p.get("type").and_then(Value::as_str) == Some("tool_result")
                && p.get("tool_use_id") == id
        })
    });
    // Never called: Claude has no such tool, because the connector isn't
    // connected (or is turned off) in Claude.
    let Some(result) = result else {
        return Err(not_connected(&service_of(tool)));
    };
    let text = match result.get("content") {
        Some(Value::Array(items)) => items
            .iter()
            .map(|c| c.get("text").and_then(Value::as_str).unwrap_or(""))
            .collect::<String>(),
        Some(Value::Null) | None => String::new(),
        Some(other) => util::js_string(other),
    };
    // Claude Code says where it put a result too large to show, as text or as
    // the result's content blocks in JSON; the file is read and removed,
    // being this call's alone.
    if let Some(saved) = SAVED.captures(&text)
        && let Ok(raw) = std::fs::read_to_string(&saved[1])
    {
        let _ = std::fs::remove_file(&saved[1]);
        if &saved[2] == "json" {
            if let Ok(Value::Array(blocks)) = serde_json::from_str::<Value>(&raw) {
                return Ok(blocks
                    .iter()
                    .map(|c| c.get("text").and_then(Value::as_str).unwrap_or(""))
                    .collect());
            }
        } else {
            return Ok(raw);
        }
    }
    if result.get("is_error").is_some_and(util::truthy) {
        return Err(if AUTH.is_match(&text) {
            not_connected(&service_of(tool))
        } else {
            failed(
                text.split('\n')
                    .next()
                    .filter(|l| !l.is_empty())
                    .unwrap_or("the connector refused")
                    .to_string(),
            )
        });
    }
    Ok(text)
}

/// The input a connector tool is called with, from pairs: `input([("id", url)])`.
pub fn input<const N: usize>(pairs: [(&str, Value); N]) -> Value {
    let mut map = Map::new();
    for (k, v) in pairs {
        map.insert(k.to_string(), v);
    }
    json!(map)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tool_names_its_service() {
        assert_eq!(
            service_of("mcp__claude_ai_Google_Calendar__list_events"),
            "Google Calendar"
        );
        assert_eq!(service_of("Read"), "the connector");
    }

    #[test]
    fn the_result_is_taken_from_the_stream() {
        let stream = [
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t1","name":"mcp__claude_ai_Slack__search"}]}}"#,
            r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":[{"type":"text","text":"found it"}]}]}}"#,
        ]
        .join("\n");
        let parts = content_parts(&stream);
        assert_eq!(parts.len(), 2);
    }
}
