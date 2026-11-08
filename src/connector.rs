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
//!
//! A call nobody waits on (the Slack look, the meeting notes:
//! `call_in_background`) isn't made while its connector was last found not
//! connected: Claude Code is asked instead whether it is connected now
//! (`claude mcp get "claude.ai <service>"`, which checks that one connector
//! with no model, in about a second), at most every `CHECK_EVERY_MS`, and
//! the call goes out once it says so. Before, every look that came round
//! (once a minute for Slack) was a Haiku run to find the connector still
//! wasn't there. A call you start (a search, a link opened, an answer to an
//! invitation) is always made, so it is how you try again at once after
//! connecting one with `/mcp`.

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

/// How long a background call leaves a connector found not connected before
/// asking Claude Code again whether it is connected now.
pub const CHECK_EVERY_MS: i64 = 15 * 60 * 1000;

/// What `claude mcp get` says of a connector.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Connection {
    Connected,
    NotConnected,
    /// No answer to go by: Claude Code missing, slow or saying something else.
    Unknown,
}

/// `claude mcp get "claude.ai <service>"`'s output (stdout and stderr) read:
/// `Status: ✔ Connected` is connected; another status (failed, needs
/// authentication) or no server by that name is not.
pub fn read_connection(output: &str) -> Connection {
    let status = output
        .lines()
        .map(str::trim)
        .find(|l| l.starts_with("Status:"));
    match status {
        Some(s) if s.contains('✔') => Connection::Connected,
        Some(_) => Connection::NotConnected,
        None if output.contains("No MCP server named") => Connection::NotConnected,
        None => Connection::Unknown,
    }
}

/// What a background call does, from its service's entry in
/// `connectors.json`: call it (not down), ask whether it is connected now
/// (down, not asked within `CHECK_EVERY_MS`), or leave it with what it last
/// said.
#[derive(Debug, PartialEq, Eq)]
pub enum Gate {
    Call,
    Check(String),
    Skip(String),
}

pub fn gate(entry: Option<&Value>, now_ms: i64) -> Gate {
    let Some(entry) = entry else {
        return Gate::Call;
    };
    if entry.get("ok").and_then(Value::as_bool) == Some(true) {
        return Gate::Call;
    }
    let message = entry
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    match entry.get("checked").and_then(Value::as_i64) {
        Some(at) if now_ms - at < CHECK_EVERY_MS => Gate::Skip(message),
        _ => Gate::Check(message),
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
    let _ = util::write_atomic(
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
    let _ = util::write_atomic(dir.join("connectors-down"), down);
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

/// When Claude Code was last asked whether a connector that is down is
/// connected now, kept in its `connectors.json` entry.
fn mark_checked(service: &str, at: i64) {
    let mut status = read_status();
    let Some(Value::Object(entry)) = status.get_mut(service) else {
        return;
    };
    entry.insert("checked".into(), Value::from(at));
    let _ = util::write_atomic(status_file(), util::json_compact(&Value::Object(status)));
}

/// Asks Claude Code whether a connector is connected, with no model:
/// `claude mcp get "claude.ai <service>"`, which checks that connector alone.
pub async fn check(service: &str) -> Connection {
    let path = format!(
        "{}:{}",
        std::env::var("PATH").unwrap_or_default(),
        paths::home_join(".local/bin").display()
    );
    let mut cmd = tokio::process::Command::new("claude");
    cmd.args(["mcp", "get", &format!("claude.ai {service}")])
        .current_dir(std::env::temp_dir())
        .env("PATH", path)
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    crate::setup::plugin::headless(&mut cmd);
    let Ok(Ok(out)) = tokio::time::timeout(Duration::from_secs(20), cmd.output()).await else {
        return Connection::Unknown;
    };
    // It reaches the connector's server to see that it answers.
    requests::count(service, "claude mcp get", "connector check");
    read_connection(&format!(
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    ))
}

/// A call nobody waits on, as the module's comment says: made while its
/// connector isn't known to be down, else only once Claude Code says it is
/// connected now, asked at most every `CHECK_EVERY_MS`.
pub async fn call_in_background(
    tool: &str,
    input: &Value,
    timeout: Option<Duration>,
) -> Result<String, ConnectorError> {
    let service = service_of(tool);
    let now = util::now_ms();
    let down = |message: String| ConnectorError {
        message,
        not_connected: true,
    };
    match gate(read_status().get(&service), now) {
        Gate::Call => {}
        Gate::Skip(message) => return Err(down(message)),
        Gate::Check(message) => {
            let connection = check(&service).await;
            mark_checked(&service, now);
            if connection != Connection::Connected {
                return Err(down(message));
            }
        }
    }
    call_connector(tool, input, timeout).await
}

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
    crate::setup::plugin::headless(&mut cmd);
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(failed("Claude Code isn't installed"));
        }
        Err(e) => return Err(failed(format!("Claude didn't answer ({e})"))),
    };
    // What Claude printed is gathered as it comes, so a run that runs out of
    // time still has what it printed before, as Node's execFile handed it on.
    let printed = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let gather = |pipe: Option<Box<dyn tokio::io::AsyncRead + Send + Unpin>>,
                  into: std::sync::Arc<std::sync::Mutex<Vec<u8>>>| {
        tokio::spawn(async move {
            use tokio::io::AsyncReadExt;
            let Some(mut pipe) = pipe else {
                return;
            };
            let mut chunk = [0u8; 8192];
            while let Ok(n) = pipe.read(&mut chunk).await {
                if n == 0 {
                    break;
                }
                into.lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .extend_from_slice(&chunk[..n]);
            }
        })
    };
    let stderr_text = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let out_reader = gather(
        child.stdout.take().map(|p| Box::new(p) as Box<_>),
        printed.clone(),
    );
    let err_reader = gather(
        child.stderr.take().map(|p| Box::new(p) as Box<_>),
        stderr_text.clone(),
    );
    let waited = tokio::time::timeout(timeout, async {
        let status = child.wait().await;
        let _ = out_reader.await;
        let _ = err_reader.await;
        status
    })
    .await;
    if waited.is_err() {
        let _ = child.start_kill();
    }
    // Counted once Claude ran, and the connector once Claude called it, a run
    // that then failed or ran out of time included.
    requests::count("Anthropic", "claude -p", "connector");
    let take = |buf: &std::sync::Mutex<Vec<u8>>| {
        String::from_utf8_lossy(&buf.lock().unwrap_or_else(|e| e.into_inner())).into_owned()
    };
    let stdout = take(&printed);
    let err = match waited {
        Ok(Ok(status)) => (!status.success()).then(|| {
            format!(
                "Command failed: claude{}",
                take(&stderr_text)
                    .lines()
                    .next()
                    .map(|l| format!(" ({l})"))
                    .unwrap_or_default()
            )
        }),
        Ok(Err(e)) => Some(e.to_string()),
        Err(_) => Some("timed out".to_string()),
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
    fn claude_mcp_get_says_whether_a_connector_is_connected() {
        // As Claude Code 2.1.319 prints them.
        let connected = "claude.ai Gmail:\n  Scope: claude.ai config\n  Status: ✔ Connected\n";
        assert_eq!(read_connection(connected), Connection::Connected);
        let missing =
            "No MCP server named \"claude.ai Slack\". Configured servers: claude.ai Gmail\n";
        assert_eq!(read_connection(missing), Connection::NotConnected);
        let lapsed = "claude.ai Slack:\n  Status: ⚠ Needs authentication\n";
        assert_eq!(read_connection(lapsed), Connection::NotConnected);
        assert_eq!(read_connection("command not found"), Connection::Unknown);
    }

    #[test]
    fn a_background_call_waits_while_its_connector_is_down() {
        let now = 1_000_000_000;
        assert_eq!(gate(None, now), Gate::Call, "nothing known: call it");
        let up = json!({"ok": true, "at": now});
        assert_eq!(gate(Some(&up), now), Gate::Call);
        let down = json!({"ok": false, "at": now - 60_000, "message": "Slack isn't connected"});
        assert_eq!(
            gate(Some(&down), now),
            Gate::Check("Slack isn't connected".into()),
            "found down by a call: ask Claude Code before another"
        );
        let asked = json!({"ok": false, "message": "m", "checked": now - 60_000});
        assert_eq!(gate(Some(&asked), now), Gate::Skip("m".into()));
        let asked_long_ago = json!({"ok": false, "message": "m", "checked": now - CHECK_EVERY_MS});
        assert_eq!(gate(Some(&asked_long_ago), now), Gate::Check("m".into()));
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
