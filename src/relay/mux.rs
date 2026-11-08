//! The tabs, as the relay reaches them: through the server, where the Node
//! relay made a tmux call for each (`list-panes -a -F ...`, `capture-pane`,
//! `send-keys`, `paste-buffer`, `new-window`, `kill-window`,
//! `set-option`). Each is one command on the server's socket, which the
//! server answers at once, so the relay asks as it needs, as it did of tmux.
//!
//! `Mux` is what the relay asks; `Server` asks the running server. The
//! tests give the relay tabs of their own instead (src/relay/testing.rs),
//! and run `Server` against a server of their own.
//!
//! What tmux's format strings gave is read from `list`'s JSON: a tab's
//! label (`@tab_label`), its options (`@slack_relay`, `@loop_tab`,
//! `@claude_pane`), each pane's process, Claude's state on it, and the
//! Claude session the server found in front on it with its transcript, so
//! the relay no longer walks `ps` for them.

use std::io::Write;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use serde_json::Value;

use crate::mux::proto::{self, ClientMsg, ServerMsg};

/// One pane of a tab.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PaneInfo {
    /// `%<n>`.
    pub id: String,
    /// The pane's process, which names the tab's session in the relay's
    /// threads and buttons, since tab ids are used again.
    pub pid: String,
    /// working, waiting, done, or idle when Claude has said nothing.
    pub state: String,
    /// When that state began, in seconds, or empty.
    pub since: String,
    /// Whether Claude runs in front on it, its session file written yet or
    /// not: a Claude asking whether to trust its folder has none, and still
    /// has to be alerted on and answered.
    pub claude: bool,
    pub transcript: Option<PathBuf>,
    pub cwd: Option<PathBuf>,
    /// The program in front (tmux's `pane_current_command`), for saying
    /// what a tab runs that isn't Claude.
    pub command: String,
    /// The tab's active pane.
    pub active: bool,
}

/// One tab, with its panes in order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Window {
    /// `@<n>`.
    pub id: String,
    /// Its number in its session, as the tab bar shows it.
    pub index: i64,
    /// Its whole label, as the tab bar shows it before cutting it short.
    pub name: String,
    /// The name given to it, not the one it would have anyway.
    pub own_name: String,
    /// The tab in front of an attached session.
    pub focused: bool,
    /// The relay's own tab.
    pub relay: bool,
    /// A tab looping a skill (`@loop_tab`), whose every tick ends a turn.
    pub loop_tab: bool,
    /// The pane the relay recorded as its Claude (`@claude_pane`).
    pub recorded: Option<String>,
    pub panes: Vec<PaneInfo>,
}

/// What the relay asks of the tabs.
pub trait Mux {
    /// Every tab, or `None` when the server can't be reached.
    fn windows(&self) -> Option<Vec<Window>>;
    /// Sets (or with `None`, unsets) an option on a tab or pane.
    fn set_option(&self, target: &str, name: &str, value: Option<&str>) -> Result<()>;
    /// Keys by name (`Enter`, `Escape`, `Down`).
    fn send_keys(&self, pane: &str, keys: &[&str]) -> Result<()>;
    /// Text typed as it is.
    fn type_text(&self, pane: &str, text: &str) -> Result<()>;
    /// Text pasted, bracketed when the program asks for it, so its line
    /// breaks stay line breaks.
    fn paste(&self, pane: &str, text: &str) -> Result<()>;
    /// The screen and `back` lines of history, or `None` for a pane gone.
    fn capture(&self, pane: &str, back: usize) -> Option<String>;
    /// A new tab in the background: its id and its pane's.
    fn new_tab(
        &self,
        dir: &Path,
        name: Option<&str>,
        command: Option<&[String]>,
    ) -> Result<(String, String)>;
    fn kill_tab(&self, tab: &str) -> Result<()>;
    /// Gives a tab a name of its own; empty for its own name again.
    fn rename_tab(&self, tab: &str, name: &str) -> Result<()>;
    fn select_tab(&self, tab: &str) -> Result<()>;
}

/// The running server, on its socket.
pub struct Server {
    pub socket: PathBuf,
}

impl Server {
    /// The server this process runs under (`SWITCHBOARD`), else the one in
    /// the state directory.
    pub fn current() -> Server {
        Server {
            socket: crate::mux::socket::socket_path(),
        }
    }

    /// Runs a command in the server: what it printed, or why it refused.
    pub fn ask(&self, args: &[&str]) -> Result<String> {
        let mut stream =
            UnixStream::connect(&self.socket).context("no Switchboard server is running")?;
        stream.set_read_timeout(Some(Duration::from_secs(10)))?;
        let msg = ClientMsg::Command {
            args: args.iter().map(|a| a.to_string()).collect(),
            cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/")),
            pane: None,
            stdin: None,
        };
        stream.write_all(&proto::encode_client(&msg))?;
        loop {
            match proto::read_server(&mut stream)? {
                Some(ServerMsg::Reply {
                    code,
                    stdout,
                    stderr,
                }) => {
                    if code != 0 {
                        let why = String::from_utf8_lossy(&stderr);
                        bail!(
                            "{}",
                            why.trim().lines().next().unwrap_or("the server refused")
                        );
                    }
                    return Ok(String::from_utf8_lossy(&stdout).into_owned());
                }
                Some(ServerMsg::Exit { message }) => {
                    return Err(anyhow!(
                        "{}",
                        message.unwrap_or_else(|| "the server ended the command".into())
                    ));
                }
                Some(ServerMsg::Output(_)) => {}
                None => bail!("the server closed the connection without an answer"),
            }
        }
    }
}

/// How long since a key was last pressed in the workspace, in ms, as the
/// running server says (`list`'s `idle_ms`), or `None` when it can't be
/// asked.
pub fn server_idle_ms() -> Option<f64> {
    let list: Value = serde_json::from_str(&Server::current().ask(&["list"]).ok()?).ok()?;
    list.get("idle_ms").and_then(Value::as_f64)
}

impl Mux for Server {
    fn windows(&self) -> Option<Vec<Window>> {
        let list: Value = serde_json::from_str(&self.ask(&["list"]).ok()?).ok()?;
        Some(windows_of(&list))
    }

    fn set_option(&self, target: &str, name: &str, value: Option<&str>) -> Result<()> {
        match value {
            Some(v) => self.ask(&["set", "-t", target, name, v]),
            None => self.ask(&["set", "-u", "-t", target, name]),
        }
        .map(|_| ())
    }

    fn send_keys(&self, pane: &str, keys: &[&str]) -> Result<()> {
        let mut args = vec!["send-keys", "-t", pane];
        args.extend_from_slice(keys);
        self.ask(&args).map(|_| ())
    }

    fn type_text(&self, pane: &str, text: &str) -> Result<()> {
        self.ask(&["send-keys", "-l", "-t", pane, "--", text])
            .map(|_| ())
    }

    fn paste(&self, pane: &str, text: &str) -> Result<()> {
        self.ask(&["send-keys", "-p", "-t", pane, "--", text])
            .map(|_| ())
    }

    fn capture(&self, pane: &str, back: usize) -> Option<String> {
        self.ask(&["capture-pane", "-t", pane, "-S", &format!("-{back}")])
            .ok()
    }

    fn new_tab(
        &self,
        dir: &Path,
        name: Option<&str>,
        command: Option<&[String]>,
    ) -> Result<(String, String)> {
        let dir = dir.to_string_lossy();
        let mut args = vec!["new-tab", "-d", "-c", dir.as_ref()];
        if let Some(name) = name {
            args.extend(["-n", name]);
        }
        if let Some(command) = command {
            args.push("--");
            args.extend(command.iter().map(String::as_str));
        }
        let out = self.ask(&args)?;
        let mut ids = out.split_whitespace();
        match (ids.next(), ids.next()) {
            (Some(tab), Some(pane)) => Ok((tab.to_string(), pane.to_string())),
            _ => bail!("the server didn't say which tab it opened"),
        }
    }

    fn kill_tab(&self, tab: &str) -> Result<()> {
        self.ask(&["kill-tab", "-t", tab]).map(|_| ())
    }

    fn rename_tab(&self, tab: &str, name: &str) -> Result<()> {
        // The name M-p's "Rename the tab" gives is the tab's own; the one a
        // skill sets is its @tab_name. The relay's takes the place of both.
        self.ask(&["rename-tab", "-t", tab, ""])?;
        if name.is_empty() {
            self.set_option(tab, "@tab_name", None)
        } else {
            self.set_option(tab, "@tab_name", Some(name))
        }
    }

    fn select_tab(&self, tab: &str) -> Result<()> {
        self.ask(&["select-tab", "-t", tab]).map(|_| ())
    }
}

/// The tabs in `list`'s JSON.
pub fn windows_of(list: &Value) -> Vec<Window> {
    let attached: Vec<&str> = list
        .get("clients")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|c| c.get("session").and_then(Value::as_str))
        .collect();
    let text = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let mut out = Vec::new();
    for session in list
        .get("sessions")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let name = text(session, "name");
        let shown = attached.contains(&name.as_str());
        for tab in session
            .get("tabs")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let opt = |k: &str| {
                tab.pointer(&format!("/options/{k}"))
                    .and_then(Value::as_str)
                    .map(str::to_string)
            };
            // The Notifications window is no tab.
            if opt("@notices_window").as_deref() == Some("1") {
                continue;
            }
            let label = text(tab, "label");
            let panes = tab
                .get("panes")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .map(|p| {
                    let agent = p.get("agent");
                    let state = agent
                        .and_then(|a| a.get("state"))
                        .and_then(Value::as_str)
                        .unwrap_or("idle");
                    let since = agent
                        .and_then(|a| a.get("since"))
                        .map(crate::util::js_string)
                        .filter(|s| s != "null")
                        .unwrap_or_default();
                    let claude = p.get("claude").filter(|c| c.is_object());
                    PaneInfo {
                        id: text(p, "id"),
                        pid: p.get("pid").map(crate::util::js_string).unwrap_or_default(),
                        state: state.to_string(),
                        since,
                        claude: claude.is_some()
                            || p.get("runs_claude") == Some(&Value::Bool(true)),
                        transcript: claude
                            .and_then(|c| c.get("transcript"))
                            .and_then(Value::as_str)
                            .map(PathBuf::from),
                        cwd: p.get("cwd").and_then(Value::as_str).map(PathBuf::from),
                        command: text(p, "command"),
                        active: p.get("active") == Some(&Value::Bool(true)),
                    }
                })
                .collect();
            out.push(Window {
                id: text(tab, "id"),
                index: tab.get("index").and_then(Value::as_i64).unwrap_or(0),
                name: if label.is_empty() {
                    text(tab, "name")
                } else {
                    label
                },
                own_name: tab
                    .get("own")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .or_else(|| opt("@tab_name"))
                    .unwrap_or_default(),
                focused: shown && tab.get("current") == Some(&Value::Bool(true)),
                relay: opt("@slack_relay").as_deref() == Some("1"),
                loop_tab: opt("@loop_tab").as_deref() == Some("1"),
                recorded: opt("@claude_pane"),
                panes,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn tabs_are_read_from_the_servers_list() {
        let list = json!({
            "sessions": [{
                "name": "main",
                "tabs": [{
                    "id": "@1", "index": 1, "name": "work", "label": "#12 ABC-1: Fix it", "current": true,
                    "options": { "@loop_tab": "1", "@claude_pane": "%2", "@tab_name": "mine" },
                    "panes": [
                        { "id": "%1", "pid": 100, "cwd": "/w", "agent": null, "claude": null,
                          "command": "zsh", "active": true },
                        { "id": "%2", "pid": 200, "agent": { "state": "waiting", "since": 1700 },
                          "claude": { "session": "s1", "transcript": "/t/s1.jsonl" } },
                        // Claude asking whether to trust its folder, before
                        // its session file is written.
                        { "id": "%3", "pid": 300, "agent": null, "claude": null,
                          "runs_claude": true, "command": "claude" }
                    ]
                }, {
                    "id": "@2", "index": 2, "name": "relay", "label": "", "current": false,
                    "options": { "@slack_relay": "1" }, "panes": []
                }]
            }],
            "clients": [{ "session": "main" }]
        });
        let tabs = windows_of(&list);
        assert_eq!(tabs.len(), 2);
        let t = &tabs[0];
        assert_eq!(
            (t.name.as_str(), t.own_name.as_str()),
            ("#12 ABC-1: Fix it", "mine")
        );
        assert!(t.focused && t.loop_tab && !t.relay);
        assert_eq!(t.recorded.as_deref(), Some("%2"));
        assert_eq!(t.panes[0].state, "idle");
        assert!(!t.panes[0].claude);
        assert_eq!(
            (t.panes[0].command.as_str(), t.panes[0].active),
            ("zsh", true)
        );
        assert!(t.panes[1].claude && !t.panes[1].active);
        assert!(t.panes[2].claude && t.panes[2].transcript.is_none());
        assert_eq!(t.panes[1].pid, "200");
        assert_eq!(
            (t.panes[1].state.as_str(), t.panes[1].since.as_str()),
            ("waiting", "1700")
        );
        assert_eq!(
            t.panes[1].transcript.as_deref(),
            Some(Path::new("/t/s1.jsonl"))
        );
        assert_eq!(tabs[1].name, "relay", "its name when it has no label");
        assert!(tabs[1].relay && !tabs[1].focused);
    }
}
