//! What the relay's tests share (test/slack-relay.test.js's setup): a
//! relay in a home of its own under the temporary directory, with stand-ins
//! for everything it reaches. Slack is a stand-in that keeps every call and
//! answers as Slack would (a test can answer a method its own way); the
//! tabs are tabs of the test's own, whose panes keep what is typed into
//! them the way `cat` echoed it in the tmux tests; Haiku answers as the
//! test says. Nothing here reaches Slack, a server, or your ~/.claude.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicU32, Ordering};

use anyhow::{Result, anyhow};
use futures_util::future::LocalBoxFuture;
use serde_json::{Value, json};

use super::config::Paths;
use super::mux::{Mux, PaneInfo, Window};
use super::slack::Transport;
use super::state::Dm;
use super::summaries::{Asked, Haiku};
use super::{Cx, Relay, presence};
use crate::presence::Presence as Reading;
use crate::util;

static NEXT: AtomicU32 = AtomicU32::new(0);

/// A call Slack's stand-in got.
#[derive(Clone, Debug)]
pub struct Call {
    pub method: String,
    pub params: Value,
}

type Answer = Box<dyn Fn(&Value) -> Result<Value>>;

/// Slack's stand-in.
#[derive(Default)]
pub struct FakeSlack {
    pub calls: RefCell<Vec<Call>>,
    pub uploads: RefCell<Vec<(String, Vec<u8>)>>,
    pub answers: RefCell<HashMap<String, Answer>>,
    pub downloads: RefCell<HashMap<String, Vec<u8>>>,
    /// The ts each post was given, in order.
    pub posted_ts: RefCell<Vec<String>>,
    next_ts: Cell<i64>,
}

impl FakeSlack {
    fn answer(&self, method: &str, params: &Value) -> Result<Value> {
        if let Some(answer) = self.answers.borrow().get(method) {
            return answer(params);
        }
        Ok(match method {
            "chat.postMessage" => {
                let n = self.next_ts.get();
                self.next_ts.set(n + 1);
                let ts = format!("{n}.000100");
                self.posted_ts.borrow_mut().push(ts.clone());
                json!({ "ok": true, "ts": ts })
            }
            "chat.update" => json!({ "ok": true, "ts": params["ts"] }),
            "reactions.add"
            | "reactions.remove"
            | "chat.delete"
            | "views.publish"
            | "views.update"
            | "views.push"
            | "chat.deleteScheduledMessage"
            | "files.completeUploadExternal"
            | "files.delete" => json!({ "ok": true }),
            "views.open" => json!({ "ok": true, "view": { "id": "V-opened" } }),
            "chat.scheduleMessage" => {
                let n = self
                    .calls
                    .borrow()
                    .iter()
                    .filter(|c| c.method == method)
                    .count();
                json!({ "ok": true, "scheduled_message_id": format!("Q{n}") })
            }
            "chat.scheduledMessages.list" => json!({ "ok": true, "scheduled_messages": [] }),
            "files.getUploadURLExternal" => {
                json!({ "ok": true, "upload_url": "https://files.example/upload", "file_id": "F1" })
            }
            "auth.test" => {
                json!({ "ok": true, "team_id": "T1", "user_id": "B1", "url": "https://example.slack.com/" })
            }
            "conversations.open" => json!({ "ok": true, "channel": { "id": "D1" } }),
            other => return Err(anyhow!("this test makes no {other} call")),
        })
    }
}

impl Transport for Rc<FakeSlack> {
    fn call<'a>(&'a self, method: &'a str, params: &'a Value) -> LocalBoxFuture<'a, Result<Value>> {
        self.calls.borrow_mut().push(Call {
            method: method.to_string(),
            params: params.clone(),
        });
        let answer = self.answer(method, params);
        Box::pin(async move { answer })
    }

    fn put<'a>(&'a self, url: &'a str, bytes: Vec<u8>) -> LocalBoxFuture<'a, Result<()>> {
        self.uploads.borrow_mut().push((url.to_string(), bytes));
        Box::pin(async { Ok(()) })
    }

    fn get<'a>(&'a self, url: &'a str) -> LocalBoxFuture<'a, Result<Vec<u8>>> {
        let got = self
            .downloads
            .borrow()
            .get(url)
            .cloned()
            .ok_or_else(|| anyhow!("this test downloads nothing from {url}"));
        Box::pin(async move { got })
    }
}

/// A pane of the test's own: what its screen shows, which typing adds to
/// as `cat` echoed it.
#[derive(Clone, Debug, Default)]
pub struct FakePane {
    pub info: PaneInfo,
    pub screen: String,
}

/// The tabs' stand-in.
#[derive(Default)]
pub struct FakeMux {
    pub windows: RefCell<Vec<(Window, Vec<FakePane>)>>,
    /// Every key, text and paste sent, as `<pane> <what>`.
    pub sent: RefCell<Vec<String>>,
    pub killed: RefCell<Vec<String>>,
    pub opened: RefCell<Vec<(PathBuf, Option<Vec<String>>)>>,
    next: Cell<u32>,
}

impl FakeMux {
    fn id(&self) -> u32 {
        let n = self.next.get() + 1;
        self.next.set(n);
        n
    }

    fn with_pane<T>(&self, pane: &str, f: impl FnOnce(&mut FakePane) -> T) -> Option<T> {
        let mut windows = self.windows.borrow_mut();
        // A tab named for a pane means its first pane, as the server's
        // active pane.
        for (w, panes) in windows.iter_mut() {
            if w.id == pane {
                return panes.first_mut().map(f);
            }
            if let Some(p) = panes.iter_mut().find(|p| p.info.id == pane) {
                return Some(f(p));
            }
        }
        None
    }
}

impl Mux for Rc<FakeMux> {
    fn windows(&self) -> Option<Vec<Window>> {
        Some(
            self.windows
                .borrow()
                .iter()
                .map(|(w, panes)| Window {
                    panes: panes.iter().map(|p| p.info.clone()).collect(),
                    ..w.clone()
                })
                .collect(),
        )
    }

    fn set_option(&self, target: &str, name: &str, value: Option<&str>) -> Result<()> {
        let mut windows = self.windows.borrow_mut();
        let Some((w, _)) = windows
            .iter_mut()
            .find(|(w, panes)| w.id == target || panes.iter().any(|p| p.info.id == target))
        else {
            return Err(anyhow!("no tab {target}"));
        };
        match name {
            "@claude_pane" => w.recorded = value.map(str::to_string),
            "@tab_name" => w.own_name = value.unwrap_or("").to_string(),
            "@slack_relay" => w.relay = value == Some("1"),
            "@loop_tab" => w.loop_tab = value == Some("1"),
            _ => {}
        }
        Ok(())
    }

    fn send_keys(&self, pane: &str, keys: &[&str]) -> Result<()> {
        for key in keys {
            self.sent.borrow_mut().push(format!("{pane} key {key}"));
            let echoed = match *key {
                "Enter" => "\n".to_string(),
                "Escape" => "^[".to_string(),
                "Up" => "^[[A".to_string(),
                "Down" => "^[[B".to_string(),
                other => format!("<{other}>"),
            };
            self.with_pane(pane, |p| p.screen.push_str(&echoed))
                .ok_or_else(|| anyhow!("no pane {pane}"))?;
        }
        Ok(())
    }

    fn type_text(&self, pane: &str, text: &str) -> Result<()> {
        self.sent.borrow_mut().push(format!("{pane} type {text}"));
        self.with_pane(pane, |p| p.screen.push_str(text))
            .ok_or_else(|| anyhow!("no pane {pane}"))
    }

    fn paste(&self, pane: &str, text: &str) -> Result<()> {
        self.sent.borrow_mut().push(format!("{pane} paste {text}"));
        self.with_pane(pane, |p| p.screen.push_str(text))
            .ok_or_else(|| anyhow!("no pane {pane}"))
    }

    fn capture(&self, pane: &str, _back: usize) -> Option<String> {
        self.with_pane(pane, |p| p.screen.clone())
    }

    fn new_tab(
        &self,
        dir: &Path,
        name: Option<&str>,
        command: Option<&[String]>,
    ) -> Result<(String, String)> {
        let (t, p) = (self.id(), self.id());
        let index = self
            .windows
            .borrow()
            .iter()
            .map(|(w, _)| w.index)
            .max()
            .unwrap_or(0)
            + 1;
        self.opened
            .borrow_mut()
            .push((dir.to_path_buf(), command.map(<[String]>::to_vec)));
        let window = Window {
            id: format!("@{t}"),
            index,
            name: name.unwrap_or("new").to_string(),
            ..Window::default()
        };
        let pane = FakePane {
            info: PaneInfo {
                id: format!("%{p}"),
                pid: (40_000 + p).to_string(),
                state: "idle".into(),
                cwd: Some(dir.to_path_buf()),
                ..PaneInfo::default()
            },
            screen: String::new(),
        };
        self.windows.borrow_mut().push((window, vec![pane]));
        Ok((format!("@{t}"), format!("%{p}")))
    }

    fn kill_tab(&self, tab: &str) -> Result<()> {
        self.killed.borrow_mut().push(tab.to_string());
        self.windows.borrow_mut().retain(|(w, _)| w.id != tab);
        Ok(())
    }

    fn rename_tab(&self, tab: &str, name: &str) -> Result<()> {
        self.set_option(tab, "@tab_name", (!name.is_empty()).then_some(name))
    }

    fn select_tab(&self, _tab: &str) -> Result<()> {
        Ok(())
    }
}

/// Haiku's stand-in: what it was given, and what it answers.
#[derive(Default)]
pub struct FakeHaiku {
    pub given: RefCell<Vec<(String, String)>>,
    /// The answer to every ask; none fails, as the tests' `claude` did.
    pub answer: RefCell<Option<String>>,
}

impl Haiku for Rc<FakeHaiku> {
    fn ask(&self, material: String, system_prompt: String) -> LocalBoxFuture<'static, Asked> {
        self.given.borrow_mut().push((material, system_prompt));
        let answer = self.answer.borrow().clone();
        Box::pin(async move {
            Asked {
                cost: answer.as_ref().map(|_| 0.0002),
                answer: answer
                    .ok_or_else(|| anyhow!("summary failed (exit 1): no real claude in tests")),
            }
        })
    }
}

/// A post as the tests read it back.
#[derive(Clone, Debug)]
pub struct Post {
    pub body: String,
    pub thread: Option<String>,
    pub broadcast: bool,
    pub ts: String,
    pub buttons: Vec<Value>,
}

/// A relay with stand-ins, in a home of its own.
pub struct Rig {
    pub cx: Cx,
    pub slack: Rc<FakeSlack>,
    pub mux: Rc<FakeMux>,
    pub haiku: Rc<FakeHaiku>,
    pub root: PathBuf,
    reading: Rc<Cell<Reading>>,
    runtime: tokio::runtime::Runtime,
    pub exited: Rc<Cell<bool>>,
}

impl Default for Rig {
    fn default() -> Rig {
        Rig::new()
    }
}

impl Rig {
    pub fn new() -> Rig {
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!("relay-rig-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let home = root.join("home");
        std::fs::create_dir_all(home.join(".claude/sessions")).unwrap();
        std::fs::create_dir_all(home.join(".claude/projects/-work")).unwrap();
        let paths = Paths::under(&home, &root.join("state"));
        let slack = Rc::new(FakeSlack::default());
        slack.next_ts.set(util::now_secs());
        let mux = Rc::new(FakeMux::default());
        let haiku = Rc::new(FakeHaiku::default());
        // Away from the computer, so what the relay says on its own goes out
        // at once; the tests about being at the computer say so.
        let reading = Rc::new(Cell::new(Reading {
            idle_ms: Some(presence::AWAY_AFTER_MS),
            locked: Some(false),
        }));
        let read = reading.clone();
        let presence = presence::Presence::reading_with(Box::new(move || {
            let r = read.get();
            Box::pin(async move { r })
        }));
        presence.reading.set(reading.get());
        let mut relay = Relay::new(
            paths,
            Box::new(slack.clone()),
            Box::new(mux.clone()),
            Box::new(haiku.clone()),
            presence,
            false,
        );
        relay.settings = Box::new(|_| None);
        relay.cwd = root.clone();
        *relay.state.borrow_mut() = super::State {
            dm: Some(Dm {
                channel: "D1".into(),
                user: "U1".into(),
                team: Some("T1".into()),
                app: true,
                ..Dm::default()
            }),
            ..super::State::default()
        };
        *relay.slack.channel.borrow_mut() = Some("D1".into());
        *relay.slack.owner.borrow_mut() = Some("U1".into());
        let exited = Rc::new(Cell::new(false));
        let noted = exited.clone();
        relay.life.borrow_mut().on_exit = Some(Box::new(move || noted.set(true)));
        super::state::owner_only(&relay.paths);
        Rig {
            cx: Rc::new(relay),
            slack,
            mux,
            haiku,
            root,
            reading,
            runtime: tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap(),
            exited,
        }
    }

    /// Runs a part of the relay, and the work it started in the background.
    pub fn block_on<F: std::future::Future>(&self, f: F) -> F::Output {
        let local = tokio::task::LocalSet::new();
        local.block_on(&self.runtime, async {
            let out = f.await;
            super::run::settle().await;
            out
        })
    }

    pub fn at_computer(&self) {
        self.set_reading(Some(0.0), Some(false));
    }

    pub fn set_reading(&self, idle_ms: Option<f64>, locked: Option<bool>) {
        let r = Reading { idle_ms, locked };
        self.reading.set(r);
        self.cx.presence.reading.set(r);
    }

    pub fn home(&self) -> &Path {
        &self.cx.paths.home
    }

    /// A tab running Claude: its id and its pane's.
    pub fn claude_tab(&self, name: &str) -> (String, String) {
        let (tab, pane) = self.mux.new_tab(&self.root, None, None).unwrap();
        let mut windows = self.mux.windows.borrow_mut();
        let (w, panes) = windows.iter_mut().find(|(w, _)| w.id == tab).unwrap();
        w.name = name.into();
        panes[0].info.claude = true;
        (tab, pane)
    }

    pub fn index_of(&self, tab: &str) -> i64 {
        self.mux
            .windows
            .borrow()
            .iter()
            .find(|(w, _)| w.id == tab)
            .map(|(w, _)| w.index)
            .unwrap_or(0)
    }

    /// Sets a tab's Claude pane's state and since when.
    pub fn set_state(&self, tab: &str, state: &str, since: &str) {
        let mut windows = self.mux.windows.borrow_mut();
        let (_, panes) = windows.iter_mut().find(|(w, _)| w.id == tab).unwrap();
        panes[0].info.state = state.into();
        panes[0].info.since = since.into();
    }

    pub fn set_screen(&self, tab: &str, text: &str) {
        let mut windows = self.mux.windows.borrow_mut();
        let (_, panes) = windows.iter_mut().find(|(w, _)| w.id == tab).unwrap();
        panes[0].screen = text.into();
    }

    pub fn set_focused(&self, tab: &str, focused: bool) {
        let mut windows = self.mux.windows.borrow_mut();
        let (w, _) = windows.iter_mut().find(|(w, _)| w.id == tab).unwrap();
        w.focused = focused;
    }

    pub fn set_loop(&self, tab: &str) {
        let mut windows = self.mux.windows.borrow_mut();
        let (w, _) = windows.iter_mut().find(|(w, _)| w.id == tab).unwrap();
        w.loop_tab = true;
    }

    pub fn screen(&self, tab: &str) -> String {
        self.mux.capture(tab, 0).unwrap_or_default()
    }

    pub fn pid_of(&self, tab: &str) -> String {
        self.mux
            .windows
            .borrow()
            .iter()
            .find(|(w, _)| w.id == tab)
            .map(|(_, p)| p[0].info.pid.clone())
            .unwrap_or_default()
    }

    /// Writes a transcript for a tab's Claude, and gives its path.
    pub fn transcript(&self, tab: &str, session: &str, entries: &[Value]) -> PathBuf {
        let file = self
            .home()
            .join(format!(".claude/projects/-work/{session}.jsonl"));
        let text: Vec<String> = entries.iter().map(|e| e.to_string()).collect();
        std::fs::write(&file, text.join("\n")).unwrap();
        let mut windows = self.mux.windows.borrow_mut();
        if let Some((_, panes)) = windows.iter_mut().find(|(w, _)| w.id == tab) {
            panes[0].info.transcript = Some(file.clone());
        }
        file
    }

    /// Writes the hooks' events file.
    pub fn events(&self, events: &[Value]) {
        let text: String = events.iter().map(|e| format!("{e}\n")).collect();
        std::fs::create_dir_all(&self.cx.paths.state_dir).unwrap();
        std::fs::write(self.cx.paths.events_file(), text).unwrap();
    }

    pub fn append_events(&self, events: &[Value]) {
        let text: String = events.iter().map(|e| format!("{e}\n")).collect();
        let before = util::read_text(self.cx.paths.events_file()).unwrap_or_default();
        std::fs::write(self.cx.paths.events_file(), before + &text).unwrap();
    }

    pub fn calls(&self, method: &str) -> Vec<Value> {
        self.slack
            .calls
            .borrow()
            .iter()
            .filter(|c| c.method == method)
            .map(|c| c.params.clone())
            .collect()
    }

    /// What was posted, in order.
    pub fn posts(&self) -> Vec<Post> {
        let calls = self.slack.calls.borrow();
        let posted = self.slack.posted_ts.borrow();
        let mut out = Vec::new();
        for (i, c) in calls
            .iter()
            .filter(|c| c.method == "chat.postMessage")
            .enumerate()
        {
            out.push(Post {
                body: c.params["text"].as_str().unwrap_or("").to_string(),
                thread: c.params["thread_ts"].as_str().map(str::to_string),
                broadcast: c.params["reply_broadcast"] == true,
                ts: posted.get(i).cloned().unwrap_or_default(),
                buttons: c.params["blocks"]
                    .as_array()
                    .and_then(|b| b.get(1))
                    .and_then(|b| b["elements"].as_array())
                    .cloned()
                    .unwrap_or_default(),
            });
        }
        out
    }

    pub fn reactions(&self) -> Vec<(String, String)> {
        self.calls("reactions.add")
            .iter()
            .map(|p| {
                (
                    p["timestamp"].as_str().unwrap_or("").to_string(),
                    p["name"].as_str().unwrap_or("").to_string(),
                )
            })
            .collect()
    }

    /// A ts a minute or so ago, as a message's or a thread's.
    pub fn ts_ago(&self, secs: i64, n: u32) -> String {
        format!("{}.{:06}", util::now_secs() - secs, n)
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}
