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
//!
//! A reconnect in `/mcp` is noticed sooner than that (`Watch`, which the
//! server runs from its tick): `/mcp` writes Claude Code's config
//! (`~/.claude.json`), so while a connector is down a change to that file
//! has each one down asked about once, at most every `CONFIG_CHECK_MS`; and
//! while a view that shows one missing is in front, it is asked about every
//! `SHOWN_CHECK_EVERY_MS`. One found connected is marked so at once.
//!
//! A run whose Haiku never called the tool is checked before the connector
//! is called down: in `claude -p` claude.ai's connectors connect in the
//! background, so the first turn could start before the tool was there, and
//! every such run once read as Slack not connected while it was. So the run
//! waits for its connectors (`MCP_CONNECTION_NONBLOCKING=false`), and a miss
//! asks `claude mcp get` (no model): connected, the call is made once more
//! after `RETRY_AFTER`, and a second miss is a failure of its own, never
//! recorded as down, which clears a down state from before; not connected
//! (or no answer), it is down as before.

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

fn not_ready(service: &str) -> ConnectorError {
    failed(format!(
        "{service}'s tool wasn't ready in Claude; it will be tried again"
    ))
}

/// How long a run whose tool was never called, its connector connected,
/// waits before the call is made again.
const RETRY_AFTER: Duration = Duration::from_secs(2);

/// How long a background call leaves a connector found not connected before
/// asking Claude Code again whether it is connected now.
pub const CHECK_EVERY_MS: i64 = 15 * 60 * 1000;

/// How often a connector down is asked about while a view that shows it
/// missing is in front.
pub const SHOWN_CHECK_EVERY_MS: i64 = 60 * 1000;

/// How soon after asking about the connectors down a change to Claude
/// Code's config has them asked about again: Claude Code writes it often,
/// and `/mcp` more than once as you reconnect.
pub const CONFIG_CHECK_MS: i64 = 10 * 1000;

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

/// The services down, each with when Claude Code was last asked whether it
/// is connected now (`None` when it hasn't been since it went down).
pub fn down() -> Vec<(String, Option<i64>)> {
    read_status()
        .into_iter()
        .filter(|(_, s)| s.get("ok").and_then(Value::as_bool) != Some(true))
        .map(|(name, s)| (name, s.get("checked").and_then(Value::as_i64)))
        .collect()
}

/// Claude Code's config, which `/mcp` writes as you reconnect a connector.
pub fn claude_config() -> std::path::PathBuf {
    paths::home_join(".claude.json")
}

/// When to ask again about the connectors down, as the module's comment
/// says, from what the server's tick sees.
#[derive(Debug, Default)]
pub struct Watch {
    /// Claude Code's config's modified time when last taken in.
    seen: Option<std::time::SystemTime>,
    /// Whether `seen` has been taken in once, the first sight being where
    /// changes count from rather than one.
    started: bool,
    /// When the watch last asked about them all for a change.
    asked: Option<i64>,
}

impl Watch {
    /// Takes in the config's first sight without asking anything, while the
    /// server can't ask (you are away), so a change made before it can
    /// counts on your return.
    pub fn first_sight(&mut self, config: Option<std::time::SystemTime>) {
        if !self.started {
            self.started = true;
            self.seen = config;
        }
    }

    /// The services to ask about now: every one down when the config has
    /// changed since last taken in (once `CONFIG_CHECK_MS` have passed since
    /// the last such ask, the change kept for then otherwise), else those of
    /// `shown` down and not asked about within `SHOWN_CHECK_EVERY_MS`.
    pub fn due(
        &mut self,
        down: &[(String, Option<i64>)],
        config: Option<std::time::SystemTime>,
        shown: &[&str],
        now: i64,
    ) -> Vec<String> {
        if !self.started || down.is_empty() {
            self.started = true;
            self.seen = config;
            if down.is_empty() {
                return Vec::new();
            }
        }
        let changed = config != self.seen;
        if changed && self.asked.is_none_or(|at| now - at >= CONFIG_CHECK_MS) {
            self.seen = config;
            self.asked = Some(now);
            return down.iter().map(|(s, _)| s.clone()).collect();
        }
        down.iter()
            .filter(|(s, checked)| {
                shown.contains(&s.as_str())
                    && checked.is_none_or(|at| now - at >= SHOWN_CHECK_EVERY_MS)
            })
            .map(|(s, _)| s.clone())
            .collect()
    }
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

/// Asks Claude Code whether a connector down is connected now, noting when
/// it was asked, and marks it connected when it is, which takes it out of
/// `connectors-down` and lets its background calls go out again.
pub async fn check_again(service: &str) -> Connection {
    let now = util::now_ms();
    let connection = check(service).await;
    took(service, now, connection);
    connection
}

/// What Claude Code said of a connector down, asked at `at`, kept: when it
/// was asked, and connected when it is. The server keeps its own asks'
/// answers on its task, one at a time, since each rewrites `connectors.json`.
pub fn took(service: &str, at: i64, connection: Connection) {
    mark_checked(service, at);
    if connection == Connection::Connected {
        record(service, true, None);
    }
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
            if check_again(&service).await != Connection::Connected {
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
    let timeout = timeout.unwrap_or(Duration::from_secs(120));
    let (result, said_connected) = confirmed(
        &service,
        || call(tool, input, timeout),
        || check(&service),
        RETRY_AFTER,
    )
    .await;
    match result {
        Ok(text) => {
            record(&service, true, None);
            Ok(text)
        }
        Err(err) => {
            if err.not_connected {
                record(&service, false, Some(&err.message));
            } else if said_connected {
                // A tool that wasn't ready, its connector connected: a down
                // state from before is cleared, not kept.
                record(&service, true, None);
            }
            Err(err)
        }
    }
}

/// A call's result, a run that never called the tool (`Ok(None)`) checked
/// as the module's comment says: made once more while Claude Code says the
/// connector is connected, down otherwise. Also whether Claude Code was asked
/// and said it is connected.
async fn confirmed<C, CF, K, KF>(
    service: &str,
    call: C,
    check: K,
    pause: Duration,
) -> (Result<String, ConnectorError>, bool)
where
    C: Fn() -> CF,
    CF: Future<Output = Result<Option<String>, ConnectorError>>,
    K: FnOnce() -> KF,
    KF: Future<Output = Connection>,
{
    match call().await {
        Ok(Some(text)) => return (Ok(text), false),
        Err(err) => return (Err(err), false),
        Ok(None) => {}
    }
    if check().await != Connection::Connected {
        return (Err(not_connected(service)), false);
    }
    tokio::time::sleep(pause).await;
    let again = call()
        .await
        .and_then(|text| text.ok_or_else(|| not_ready(service)));
    (again, true)
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

/// One run of the headless Haiku: the tool's result, or `None` when it never
/// called the tool.
async fn call(
    tool: &str,
    input: &Value,
    timeout: Duration,
) -> Result<Option<String>, ConnectorError> {
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
    // Waits for claude.ai's connectors before the first turn, which `-p`
    // otherwise starts while they are still connecting.
    .env("MCP_CONNECTION_NONBLOCKING", "false")
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
    // Never called: Claude had no such tool, because the connector isn't
    // connected (or is turned off) in Claude, or wasn't yet.
    let Some(result) = result else {
        return Ok(None);
    };
    let text = match result.get("content") {
        Some(Value::Array(items)) => items.iter().map(block_text).collect::<String>(),
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
                return Ok(Some(blocks.iter().map(block_text).collect()));
            }
        } else {
            return Ok(Some(raw));
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
    Ok(Some(text))
}

/// A result's content block as text: its text, or an image (as
/// `slack_read_file` gives a picture) as a `data:` URI on a line of its own,
/// for the caller to decode.
fn block_text(block: &Value) -> String {
    if block.get("type").and_then(Value::as_str) == Some("image")
        && let Some(source) = block.get("source")
        && let Some(data) = source.get("data").and_then(Value::as_str)
    {
        let media = source
            .get("media_type")
            .and_then(Value::as_str)
            .unwrap_or("application/octet-stream");
        return format!("\ndata:{media};base64,{data}\n");
    }
    block
        .get("text")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
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

    /// `confirmed` with runs that answer in turn (`None` a miss) and
    /// `claude mcp get` answering `connection`: its result and how many
    /// runs were made.
    async fn confirm(
        answers: &[Option<&str>],
        connection: Connection,
    ) -> (Result<String, ConnectorError>, bool, usize) {
        let runs = std::sync::atomic::AtomicUsize::new(0);
        let (result, said_connected) = confirmed(
            "Slack",
            || {
                let n = runs.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let answer = answers[n].map(str::to_string);
                async move { Ok(answer) }
            },
            || async move { connection },
            Duration::ZERO,
        )
        .await;
        (result, said_connected, runs.into_inner())
    }

    #[tokio::test]
    async fn a_tool_never_called_is_tried_again_while_its_connector_is_connected() {
        let (result, said_connected, runs) =
            confirm(&[None, Some("found it")], Connection::Connected).await;
        assert_eq!(result.unwrap(), "found it");
        assert!(said_connected);
        assert_eq!(runs, 2);

        let (result, said_connected, runs) = confirm(&[None, None], Connection::Connected).await;
        let err = result.unwrap_err();
        assert!(!err.not_connected, "a second miss isn't the connector down");
        assert!(said_connected, "so a down state from before is cleared");
        assert_eq!(
            err.message,
            "Slack's tool wasn't ready in Claude; it will be tried again"
        );
        assert_eq!(runs, 2);
    }

    #[tokio::test]
    async fn a_tool_never_called_is_down_when_claude_code_says_so() {
        for connection in [Connection::NotConnected, Connection::Unknown] {
            let (result, said_connected, runs) = confirm(&[None], connection).await;
            assert!(result.unwrap_err().not_connected);
            assert!(!said_connected);
            assert_eq!(runs, 1, "not tried again");
        }
        let (result, _, runs) = confirm(&[Some("found it")], Connection::NotConnected).await;
        assert_eq!(result.unwrap(), "found it", "a call made isn't checked");
        assert_eq!(runs, 1);
    }

    #[test]
    fn a_change_to_claude_codes_config_asks_about_the_connectors_down() {
        use std::time::{Duration, SystemTime};
        let at = |s: u64| Some(SystemTime::UNIX_EPOCH + Duration::from_secs(s));
        let now = 1_000_000_000;
        let down = vec![("Slack".to_string(), Some(now))];
        let mut watch = Watch::default();
        assert!(
            watch.due(&down, at(1), &[], now).is_empty(),
            "the first sight is where changes count from"
        );
        assert_eq!(watch.due(&down, at(2), &[], now + 1000), ["Slack"]);
        assert!(
            watch.due(&down, at(3), &[], now + 5000).is_empty(),
            "a second change within CONFIG_CHECK_MS waits"
        );
        assert_eq!(
            watch.due(&down, at(3), &[], now + 1000 + CONFIG_CHECK_MS),
            ["Slack"],
            "and is asked about once it has passed"
        );
        let mut calm = Watch::default();
        assert!(calm.due(&[], at(1), &[], now).is_empty());
        assert!(
            calm.due(&[], at(2), &[], now + 1000).is_empty(),
            "with nothing down, a change asks nothing"
        );
        assert!(
            calm.due(&down, at(2), &[], now + 2000).is_empty(),
            "nor once something goes down after it"
        );
    }

    #[test]
    fn a_change_made_before_the_watch_can_ask_counts_once_it_can() {
        use std::time::{Duration, SystemTime};
        let at = |s: u64| Some(SystemTime::UNIX_EPOCH + Duration::from_secs(s));
        let now = 1_000_000_000;
        let down = vec![("Fathom".to_string(), Some(now))];
        let mut watch = Watch::default();
        watch.first_sight(at(1));
        watch.first_sight(at(2));
        assert_eq!(watch.due(&down, at(2), &[], now), ["Fathom"]);
    }

    #[test]
    fn a_view_showing_a_connector_missing_asks_every_minute() {
        let now = 1_000_000_000;
        let mut watch = Watch::default();
        let asked_now = vec![("Slack".to_string(), Some(now))];
        assert!(
            watch
                .due(&asked_now, None, &["Slack"], now + 1000)
                .is_empty()
        );
        let asked_before = vec![("Slack".to_string(), Some(now - SHOWN_CHECK_EVERY_MS))];
        assert_eq!(watch.due(&asked_before, None, &["Slack"], now), ["Slack"]);
        assert!(
            watch.due(&asked_before, None, &[], now).is_empty(),
            "not while no view shows it"
        );
        let never = vec![("Fathom".to_string(), None)];
        assert_eq!(watch.due(&never, None, &["Fathom"], now), ["Fathom"]);
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
