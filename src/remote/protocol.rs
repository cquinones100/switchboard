//! The protocol a remote front-end speaks to the workspace: the server's
//! commands that the phone app (src/phone) and the Slack relay (src/relay)
//! both ask over its socket. List the tabs (`list`), read a tab's screen
//! (`capture-pane`), answer its prompt or a dialog before its session
//! starts (`answer`), send its Claude a message (`answer -m`), clear a
//! draft left in its prompt box (`clear-draft`), stop a turn (`stop`),
//! mark it seen (`seen`), set its reminder (`remind`), pin it (`pin`),
//! close it (`kill-tab`), read its PR, ticket or note (`phone-tab`), run a
//! mission control view's menu option (`phone-item`, `phone-view`), and
//! follow every Claude tab as it changes (`watch`). The server answers
//! each in src/remote/serve.rs with the same refusals whoever asks; what
//! the relay does beyond the phone (a dialog answered, a message after a
//! draft, Esc on a waiting Claude) it asks for with a flag of its own.
//!
//! `Remote` is what a front-end asks; `Server` asks the running server.
//! The relay's tests give it tabs of their own instead
//! (src/relay/testing.rs), and run `Server` against a server of their own.
//! Moved here from src/relay/mux.rs, which had taken the place of the Node
//! relay's tmux calls (`list-panes -a -F ...`, `capture-pane`,
//! `send-keys`, `new-window`, `kill-window`, `set-option`).
//!
//! What tmux's format strings gave is read from `list`'s JSON: a tab's
//! label (`@tab_label`), its options (`@slack_relay`, `@loop_tab`,
//! `@claude_pane`), each pane's process, Claude's state on it, and the
//! Claude session the server found in front on it with its transcript.

use std::io::Write;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use futures_util::future::LocalBoxFuture;
use serde_json::Value;

use crate::mux::proto::{self, ClientMsg, ServerMsg};

/// Why the server didn't do what it was asked: it refused, in its words
/// (shown by their first line, the phone showing them whole), or it
/// couldn't be reached. The phone answers the two differently.
#[derive(Debug)]
pub enum Refused {
    No(String),
    Down(String),
}

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refused::No(why) => f.write_str(why.lines().next().unwrap_or(why)),
            Refused::Down(why) => f.write_str(why),
        }
    }
}

impl std::error::Error for Refused {}

/// What may be done beyond the plain answer or message: who asked, for
/// the server's log (`-s`); the prompt it is meant for (`-h`, the prompt's
/// id), refused when another shows; in place of a reply, a step over a
/// prompt of several questions (`-g`: `back`, `forward` or a step's
/// name); a dialog before the session starts
/// answered (`-d`), which the phone leaves for the computer; a message
/// typed after a draft left in the prompt box (`-a`), which the relay asks
/// about first and the phone leaves for the computer.
#[derive(Clone, Copy, Debug, Default)]
pub struct How<'a> {
    pub from: Option<&'a str>,
    pub expect: Option<&'a str>,
    pub step: Option<&'a str>,
    pub dialog: bool,
    pub after_draft: bool,
}

/// A tab's direct landing, from `list`'s `landing` and its pane's phase.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LandingAt {
    pub holding: bool,
    /// Its place in the queue, 1 holding the lock.
    pub line: u32,
    /// Since when it holds the lock or waits, in ms.
    pub since: i64,
    pub branch: String,
    /// Waiting first in line while its change is reviewed again.
    pub reviewing: bool,
    /// What the landing recorded of itself (`land-lock record`).
    pub record: Value,
    /// The step queue-and-clean last named (`land-lock phase`).
    pub phase: String,
}

/// `list`'s `landing` for a tab, or `None` when it isn't landing.
pub fn landing_of(value: Option<&Value>) -> Option<LandingAt> {
    let v = value.filter(|v| v.is_object())?;
    Some(LandingAt {
        holding: v["holding"].as_bool() == Some(true),
        line: v["line"].as_u64().unwrap_or(1) as u32,
        since: v["since"].as_i64().unwrap_or(0),
        branch: v["branch"].as_str().unwrap_or("").to_string(),
        reviewing: v["reviewing"].as_bool() == Some(true),
        record: v["record"].clone(),
        phase: String::new(),
    })
}

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
    /// Its Claude's address for another session's messages (`uds:<socket>`,
    /// as `switchboard tabs` shows it), or empty.
    pub address: String,
    /// The step queue-and-clean last named on it (`land-lock phase`, the
    /// pane's agent record's `phase`).
    pub phase: String,
}

/// A tab's stage as the sidebar groups it (`list`'s `stage`): its key and
/// title, the stage's place among the sidebar's stages and the tab's within
/// it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TabStage {
    pub key: String,
    pub title: String,
    pub rank: u64,
    pub place: u64,
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
    /// A tab looping a skill (`@loop_tab`), or a routine's (`@routine`),
    /// whose every tick or run ends a turn.
    pub loop_tab: bool,
    /// A tab autopilot opened (`@autopilot`), not since made yours.
    pub autopilot: bool,
    /// The pane the relay recorded as its Claude (`@claude_pane`).
    pub recorded: Option<String>,
    /// Its stage in the sidebar, or `None` from a server too old to say.
    pub stage: Option<TabStage>,
    /// A reminder set on it (`@remind_at`, in seconds).
    pub remind_at: Option<i64>,
    /// Its direct landing (`list`'s `landing`), its phase left empty.
    pub landing: Option<LandingAt>,
    /// Its PR, as the server last heard of it.
    pub pr: Option<crate::facts::PrFacts>,
    pub panes: Vec<PaneInfo>,
}

/// What a remote front-end asks of the workspace. A pane is named `%<n>`,
/// a tab `@<n>`; every answer, message and stop is refused for a pane not
/// running Claude.
pub trait Remote {
    /// Every tab, or `None` when the server can't be reached.
    fn windows(&self) -> Option<Vec<Window>>;
    /// The screen and `back` lines of history, or `None` for a pane gone.
    fn capture(&self, pane: &str, back: usize) -> Option<String>;
    /// The screen as `capture` gives it, with what is drawn dimmed left
    /// blank (`capture-pane -D`), line for line.
    fn capture_typed(&self, pane: &str) -> Option<String>;
    /// `reply` pressed in the prompt the pane's Claude shows (an option's
    /// number or the start of its label, `submit`, or your own words where
    /// it takes them), or, with `how.dialog`, in a dialog before its
    /// session starts: what was done, in a line.
    fn answer(&self, pane: &str, reply: &str, how: How) -> Result<String>;
    /// `text` typed to the pane's Claude as a message, pasted and then
    /// sent with Enter: what was done, in a line.
    fn message(&self, pane: &str, text: &str, how: How) -> Result<String>;
    /// What is typed and unsent in the pane's prompt box cleared.
    fn clear_draft(&self, pane: &str) -> Result<()>;
    /// Esc pressed in the pane's Claude, which stops its turn; with
    /// `force`, whenever it is working or waiting, whatever it shows, where
    /// Esc answers no to a prompt.
    fn stop(&self, pane: &str, from: Option<&str>, force: bool) -> Result<String>;
    /// Marks a tab's finished Claude seen, as looking at it does.
    fn mark_seen(&self, tab: &str, from: Option<&str>) -> Result<()>;
    /// The reminder of the pane's tab set for `when` with `note`, or with
    /// `None` cleared.
    fn remind(
        &self,
        pane: &str,
        when: Option<&str>,
        note: &str,
        from: Option<&str>,
    ) -> Result<String>;
    /// The pane's tab pinned, or unpinned.
    fn pin(&self, pane: &str, pin: bool, from: Option<&str>) -> Result<String>;
    /// The tab of a pane running Claude closed for `from`.
    fn close(&self, pane: &str, from: &str) -> Result<String>;
    /// The pane's tab's PR, ticket or note (`kind`) as JSON, read only.
    fn tab_view(&self, pane: &str, kind: &str) -> Result<String>;
    /// The option labeled `label` of mission control's menu for the item
    /// `id` of `view`, or with no `id`, of the view's own menu.
    fn view_option(&self, view: &str, id: Option<&str>, label: &str) -> Result<String>;
    /// `line` typed into a new tab's shell and run with Enter, so the
    /// shell stays once the program it starts exits.
    fn run_in_shell(&self, pane: &str, line: &str) -> Result<()>;
    /// Sets (or with `None`, unsets) an option on a tab or pane.
    fn set_option(&self, target: &str, name: &str, value: Option<&str>) -> Result<()>;
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
    /// Starts autopilot with the Autopilot form's fields, as JSON: what
    /// the server says of it, or why it didn't start.
    /// Waited for off the caller's thread, since the server answers once
    /// it has checked them, which can reach GitHub.
    /// With `change`, autopilot's settings changed while it works, as the
    /// settings box's Save changes them.
    fn start_autopilot(
        &self,
        fields: String,
        change: bool,
    ) -> LocalBoxFuture<'static, Result<String>>;
    /// Autopilot's progress (`autopilot-board`), or `None` when the
    /// server can't be asked or is too old to say.
    fn autopilot_board(&self) -> Option<Value>;
    /// Autopilot paused, or resumed: what the server says of it.
    fn pause_autopilot(&self, pause: bool) -> Result<String>;
}

/// How long `autopilot-start` is waited for: longer than the server lets
/// its check take before it refuses to start (`autopilot::START_LIMIT`),
/// so an answer is never given up on while the server may still start it.
const START_WAIT: Duration = Duration::from_secs(300);
const _: () = assert!(START_WAIT.as_secs() > crate::autopilot::START_LIMIT.as_secs());

/// The running server, on its socket.
pub struct Server {
    pub socket: PathBuf,
}

/// `-s <from>` when someone is named.
fn from_args(from: Option<&str>) -> Vec<&str> {
    from.map(|f| vec!["-s", f]).unwrap_or_default()
}

impl Server {
    /// The server this process runs under (`SWITCHBOARD`), else the one in
    /// the state directory.
    pub fn current() -> Server {
        Server {
            socket: crate::mux::socket::socket_path(),
        }
    }

    /// Runs a command in the server: what it printed, or why it refused
    /// (`Refused`).
    pub fn ask(&self, args: &[&str]) -> Result<String> {
        self.ask_waiting(args, Duration::from_secs(10))
    }

    /// `ask`, waiting at most `wait` for the answer.
    pub fn ask_waiting(&self, args: &[&str], wait: Duration) -> Result<String> {
        self.ask_for(args, Some(wait))
    }

    /// `ask`, waiting for the answer however long the server takes (a
    /// view's menu option works off the server's task), its last line
    /// break taken off: what a front-end asks to be done, which the phone
    /// never gave up on.
    fn said(&self, args: &[&str]) -> Result<String> {
        Ok(self.ask_for(args, None)?.trim_end().to_string())
    }

    fn ask_for(&self, args: &[&str], wait: Option<Duration>) -> Result<String> {
        let down = |why: &str| anyhow::Error::new(Refused::Down(why.to_string()));
        let mut stream = UnixStream::connect(&self.socket)
            .map_err(|_| down("no Switchboard server is running"))?;
        stream.set_read_timeout(wait)?;
        let msg = ClientMsg::Command {
            args: args.iter().map(|a| a.to_string()).collect(),
            cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/")),
            pane: None,
            stdin: None,
            by_hand: false,
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
                        let why = String::from_utf8_lossy(&stderr).trim().to_string();
                        let why = if why.is_empty() {
                            "the server refused".into()
                        } else {
                            why
                        };
                        return Err(Refused::No(why).into());
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

    /// Keys by name (`Enter`, `Escape`, `Down`).
    pub fn send_keys(&self, pane: &str, keys: &[&str]) -> Result<()> {
        let mut args = vec!["send-keys", "-t", pane];
        args.extend_from_slice(keys);
        self.ask(&args).map(|_| ())
    }

    /// Text typed as it is.
    pub fn type_text(&self, pane: &str, text: &str) -> Result<()> {
        self.ask(&["send-keys", "-l", "-t", pane, "--", text])
            .map(|_| ())
    }

    /// Text pasted, bracketed when the program asks for it, so its line
    /// breaks stay line breaks.
    pub fn paste(&self, pane: &str, text: &str) -> Result<()> {
        self.ask(&["send-keys", "-p", "-t", pane, "--", text])
            .map(|_| ())
    }
}

/// How long since a key was last pressed in the workspace, in ms, as the
/// running server says (`list`'s `idle_ms`), and whether one has been
/// since it started (`typed`; a server a version behind doesn't say, and
/// counted its start as one), or `None` when it can't be asked.
pub fn server_idle_ms() -> Option<(f64, bool)> {
    let list: Value = serde_json::from_str(&Server::current().ask(&["list"]).ok()?).ok()?;
    let typed = list.get("typed").and_then(Value::as_bool).unwrap_or(true);
    Some((list.get("idle_ms").and_then(Value::as_f64)?, typed))
}

/// `watch` followed on the running server: each line of JSON it sends
/// handed to `each`, until it hangs up or refuses, which is the error.
pub async fn watch(mut each: impl FnMut(Value)) -> Result<()> {
    use tokio::io::AsyncWriteExt;
    let mut stream = tokio::net::UnixStream::connect(crate::mux::socket::socket_path()).await?;
    let msg = ClientMsg::Command {
        args: vec!["watch".into()],
        cwd: crate::paths::home(),
        pane: None,
        stdin: None,
        by_hand: false,
    };
    stream.write_all(&proto::encode_client(&msg)).await?;
    let mut held = String::new();
    loop {
        match proto::read_server_async(&mut stream).await? {
            Some(ServerMsg::Output(bytes)) => {
                held.push_str(&String::from_utf8_lossy(&bytes));
                while let Some(at) = held.find('\n') {
                    let line: String = held.drain(..=at).collect();
                    if let Ok(now) = serde_json::from_str::<Value>(&line) {
                        each(now);
                    }
                }
            }
            Some(ServerMsg::Reply { stderr, .. }) => {
                return Err(anyhow!("{}", String::from_utf8_lossy(&stderr).trim()));
            }
            Some(ServerMsg::Exit { .. }) | None => return Err(anyhow!("it hung up")),
        }
    }
}

impl Remote for Server {
    fn windows(&self) -> Option<Vec<Window>> {
        let list: Value = serde_json::from_str(&self.ask(&["list"]).ok()?).ok()?;
        Some(windows_of(&list))
    }

    fn capture(&self, pane: &str, back: usize) -> Option<String> {
        self.ask(&["capture-pane", "-t", pane, "-S", &format!("-{back}")])
            .ok()
    }

    fn capture_typed(&self, pane: &str) -> Option<String> {
        self.ask(&["capture-pane", "-D", "-t", pane]).ok()
    }

    fn answer(&self, pane: &str, reply: &str, how: How) -> Result<String> {
        let mut args = vec!["answer", "-t", pane];
        args.extend(from_args(how.from));
        if let Some(id) = how.expect {
            args.extend(["-h", id]);
        }
        if let Some(step) = how.step {
            args.extend(["-g", step]);
        }
        if how.dialog {
            args.push("-d");
        }
        args.extend(["--", reply]);
        self.said(&args)
    }

    fn message(&self, pane: &str, text: &str, how: How) -> Result<String> {
        let mut args = vec!["answer", "-m", "-t", pane];
        args.extend(from_args(how.from));
        if how.after_draft {
            args.push("-a");
        }
        args.extend(["--", text]);
        self.said(&args)
    }

    fn clear_draft(&self, pane: &str) -> Result<()> {
        self.said(&["clear-draft", "-t", pane]).map(|_| ())
    }

    fn stop(&self, pane: &str, from: Option<&str>, force: bool) -> Result<String> {
        let mut args = vec!["stop", "-t", pane];
        args.extend(from_args(from));
        if force {
            args.push("-f");
        }
        self.said(&args)
    }

    fn mark_seen(&self, tab: &str, from: Option<&str>) -> Result<()> {
        let mut args = vec!["seen", "-t", tab];
        args.extend(from_args(from));
        self.said(&args).map(|_| ())
    }

    fn remind(
        &self,
        pane: &str,
        when: Option<&str>,
        note: &str,
        from: Option<&str>,
    ) -> Result<String> {
        let mut args = vec!["remind", "-t", pane];
        args.extend(from_args(from));
        match when {
            None => args.push("-c"),
            Some(when) => args.extend(["-n", note, "--", when]),
        }
        self.said(&args)
    }

    fn pin(&self, pane: &str, pin: bool, from: Option<&str>) -> Result<String> {
        let mut args = vec!["pin", "-t", pane];
        args.extend(from_args(from));
        if !pin {
            args.push("-u");
        }
        self.said(&args)
    }

    fn close(&self, pane: &str, from: &str) -> Result<String> {
        self.said(&["kill-tab", "-t", pane, "-s", from])
    }

    fn tab_view(&self, pane: &str, kind: &str) -> Result<String> {
        self.said(&["phone-tab", "-t", pane, kind])
    }

    fn view_option(&self, view: &str, id: Option<&str>, label: &str) -> Result<String> {
        match id {
            Some(id) => self.said(&["phone-item", view, id, label]),
            None => self.said(&["phone-view", view, label]),
        }
    }

    fn run_in_shell(&self, pane: &str, line: &str) -> Result<()> {
        self.type_text(pane, line)?;
        self.send_keys(pane, &["Enter"])
    }

    fn set_option(&self, target: &str, name: &str, value: Option<&str>) -> Result<()> {
        match value {
            Some(v) => self.ask(&["set", "-t", target, name, v]),
            None => self.ask(&["set", "-u", "-t", target, name]),
        }
        .map(|_| ())
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

    fn start_autopilot(
        &self,
        fields: String,
        change: bool,
    ) -> LocalBoxFuture<'static, Result<String>> {
        let server = Server {
            socket: self.socket.clone(),
        };
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                let mut args = vec!["autopilot-start"];
                if change {
                    args.push("--change");
                }
                args.push(&fields);
                server.ask_waiting(&args, START_WAIT)
            })
            .await?
        })
    }

    fn autopilot_board(&self) -> Option<Value> {
        serde_json::from_str(&self.ask(&["autopilot-board"]).ok()?).ok()
    }

    fn pause_autopilot(&self, pause: bool) -> Result<String> {
        self.ask(&["autopilot-pause", if pause { "on" } else { "off" }])
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
                        address: claude
                            .and_then(|c| c.pointer("/peer/socket"))
                            .and_then(Value::as_str)
                            .map(|s| format!("uds:{s}"))
                            .unwrap_or_default(),
                        phase: agent
                            .and_then(|a| a.get("phase"))
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string(),
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
                loop_tab: opt("@loop_tab").as_deref() == Some("1")
                    || opt(crate::routines::TAB_MARK).is_some_and(|id| !id.is_empty()),
                autopilot: opt("@autopilot")
                    .and_then(|v| v.trim().parse::<f64>().ok())
                    .is_some_and(|n| n != 0.0)
                    && opt("@autopilot_yours").is_none(),
                recorded: opt("@claude_pane"),
                stage: tab
                    .get("stage")
                    .filter(|s| s.is_object())
                    .map(|s| TabStage {
                        key: text(s, "key"),
                        title: text(s, "title"),
                        rank: s.get("rank").and_then(Value::as_u64).unwrap_or(0),
                        place: s.get("place").and_then(Value::as_u64).unwrap_or(0),
                    }),
                remind_at: opt("@remind_at")
                    .and_then(|v| v.trim().parse::<i64>().ok())
                    .filter(|n| *n != 0),
                landing: landing_of(tab.get("landing")),
                pr: tab
                    .pointer("/facts/pr")
                    .and_then(|p| serde_json::from_value(p.clone()).ok()),
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
                    "options": { "@loop_tab": "1", "@claude_pane": "%2", "@tab_name": "mine", "@remind_at": "1800" },
                    "stage": { "key": "later", "title": "Reminders", "rank": 13, "place": 2 },
                    "landing": { "holding": true, "line": 1, "since": 5000, "branch": "main",
                                 "reviewing": false, "record": { "leak": { "found": 0 } } },
                    "facts": { "pr": { "number": 57, "repo": "o/r", "url": "", "state": "OPEN",
                               "merge": null, "checks": { "pass": 9, "fail": 0, "pending": 0 },
                               "review_decision": "APPROVED", "additions": 0, "deletions": 0 } },
                    "panes": [
                        { "id": "%1", "pid": 100, "cwd": "/w", "agent": null, "claude": null,
                          "command": "zsh", "active": true },
                        { "id": "%2", "pid": 200, "agent": { "state": "waiting", "since": 1700, "phase": "pushing" },
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
        assert_eq!(t.panes[1].phase, "pushing");
        let landing = t.landing.as_ref().unwrap();
        assert_eq!(
            (
                landing.holding,
                landing.line,
                landing.since,
                landing.branch.as_str()
            ),
            (true, 1, 5000, "main")
        );
        assert_eq!(t.pr.as_ref().map(|p| p.number), Some(57));
        assert!(tabs[1].landing.is_none() && tabs[1].pr.is_none());
        assert_eq!(
            (t.panes[1].state.as_str(), t.panes[1].since.as_str()),
            ("waiting", "1700")
        );
        assert_eq!(
            t.panes[1].transcript.as_deref(),
            Some(Path::new("/t/s1.jsonl"))
        );
        assert_eq!(
            t.stage,
            Some(TabStage {
                key: "later".into(),
                title: "Reminders".into(),
                rank: 13,
                place: 2
            })
        );
        assert_eq!(t.remind_at, Some(1800));
        assert_eq!(tabs[1].stage, None, "none from a server too old to say");
        assert_eq!(tabs[1].name, "relay", "its name when it has no label");
        assert!(tabs[1].relay && !tabs[1].focused);
    }
}
