//! The Slack relay (bin/slack-relay and lib/slack-relay/): a Slack app of
//! its own that talks to its owner in the app's DM, for when you are away
//! from the computer. Each tab running Claude has a thread there; what you
//! reply in it is typed into that tab's Claude, and the relay posts back
//! when the tab is done or waiting on you, says when a tab may be stuck,
//! commits or pushes, keeps the app's Home tab listing every tab, and lets
//! you open a tab's screen from it to message, answer, stop, rename or close
//! it.
//!
//! It runs as a program of its own, `switchboard slack-relay`, in a tab of
//! its own, and reaches the tabs through the server, as the Node version
//! reached them through tmux: `list` for the tabs and their Claude panes,
//! `capture-pane` for a screen, `send-keys` to type, `new-tab`, `kill-tab`
//! and `set` for the rest (`mux`). It hears what the tabs' Claude sessions
//! do from the events `switchboard hook` appends to its events file while
//! it runs (src/agent/hook.rs), and from their transcripts.
//!
//! Who it answers to is the point of the whole thing, and `slack` holds the
//! rules: only what the owner named in `~/.config/switchboard/slack-relay.json`
//! typed, in the app's DM with them, in that workspace, is acted on; buttons
//! and forms only from the owner, on the relay's own messages and screens;
//! and every API call naming a conversation is refused unless it is the DM.
//! The owner is read from the file, never from a message.
//!
//! The parts, one per file of the original:
//! - `config`: where its files are, and the times and limits it keeps to.
//! - `log`: its log, which outlives its tab and explains a crash.
//! - `state`: what it keeps between runs (threads, alerts, homes, ...).
//! - `slack`: the Slack app: the Web API and its rules, who may be heard.
//! - `http`: Slack over HTTPS and Socket Mode's WebSocket.
//! - `mux`: the tabs, through the server.
//! - `tabs`: each tab's Claude, its transcript, its screen and prompts.
//! - `post`: posting, editing and uploading in the DM.
//! - `presence`: whether you're at the computer, holding notices while you are.
//! - `read`: catching up on what was sent while the relay couldn't hear.
//! - `commands`: what you send, and typing into a tab.
//! - `summaries`: `progress`, written by a tool-less Haiku.
//! - `followups`, `alerts`, `stuck`, `milestones`: what it reports by itself.
//! - `homes`: each tab's thread and its header.
//! - `dashboard`: the app's Home tab.
//! - `screens`, `skills`, `places`: a tab's screen and its forms.
//! - `buttons`, `presses`, `quickreplies`: buttons, and acting on a press.
//! - `files`, `slices`: images and videos, both ways.
//! - `cleanup`: deleting closed tabs' threads.
//! - `offline`: the note Slack sends when the computer has gone quiet.
//! - `outbox`: `slack-relay notify`, notices other programs post.
//! - `run`, `lifecycle`, `cli`: starting, keeping it running, stopping.
//! - `serve`: the server's side: M-S and stopping it from the palette.
//!
//! Everything runs on one thread, as Node did: the parts are async functions
//! that share the relay's state through `Relay`, each borrowing it only
//! between its awaits, so a slow call to Slack or Haiku never holds up the
//! tabs being looked at every second.

pub mod alerts;
pub mod buttons;
pub mod cleanup;
pub mod cli;
pub mod commands;
pub mod config;
pub mod dashboard;
pub mod files;
pub mod followups;
pub mod homes;
pub mod http;
pub mod lifecycle;
pub mod log;
pub mod milestones;
pub mod mux;
pub mod offline;
pub mod outbox;
pub mod places;
pub mod post;
pub mod presence;
pub mod presses;
pub mod quickreplies;
pub mod read;
pub mod run;
pub mod screens;
pub mod serve;
pub mod skills;
pub mod slack;
pub mod slices;
pub mod state;
pub mod stuck;
pub mod summaries;
pub mod tabs;

#[cfg(test)]
mod scenarios;
#[cfg(test)]
pub mod testing;

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

use futures_util::future::{LocalBoxFuture, Shared};
use serde_json::Value;

pub use config::Paths;
pub use state::State;

/// The relay: its files, its state, and what it reaches Slack, the tabs and
/// Haiku through. Shared by every part as `Cx`, on one thread.
pub struct Relay {
    pub paths: Paths,
    pub state: RefCell<State>,
    pub slack: slack::Slack,
    pub mux: Box<dyn mux::Mux>,
    pub haiku: Box<dyn summaries::Haiku>,
    pub presence: presence::Presence,
    /// A setting's value (lib/settings.js's `setting`).
    pub settings: Settings,
    pub log: log::Log,
    pub life: RefCell<lifecycle::Life>,
    /// What is kept in memory only, for the life of the run.
    pub mem: RefCell<Memory>,
    /// The tabs whose home thread is being made, so however many messages
    /// ask for one at once, one is made (homes.js's `homing`).
    pub homing: RefCell<HashMap<String, Homing>>,
    /// When the relay started, in ms: a message sent more than a minute
    /// before was sent while it wasn't running.
    pub started_at: i64,
    /// Where `new:` without a place starts its Claude: the relay's own
    /// directory, as `process.cwd()` was.
    pub cwd: PathBuf,
}

pub type Cx = Rc<Relay>;

/// How the relay reads a setting; the tests give it settings of their own.
pub type Settings = Box<dyn Fn(&str) -> Option<Value>>;

/// A tab's home thread being started, which every ask for it waits on.
/// It gives the thread, or why Slack refused to start it.
pub type Homing = Shared<LocalBoxFuture<'static, Result<String, String>>>;

/// What the parts keep in memory between ticks, each as the module that
/// owns it describes.
#[derive(Default)]
pub struct Memory {
    pub tabs: tabs::Memory,
    pub post: post::Memory,
    pub alerts: alerts::Memory,
    pub milestones: milestones::Memory,
    pub screens: screens::Memory,
    pub picks: buttons::Picks,
    pub drafts: commands::Drafts,
    /// The welcome message's words, for editing it when the app's ID comes.
    pub welcome: String,
}

impl Relay {
    /// Writes the state file (state.js's `saveState`).
    pub fn save(&self) {
        let state = self.state.borrow();
        if let Err(e) = state::save(&self.paths, &state) {
            self.log(&format!("couldn't save the state: {e}"));
        }
    }

    /// A line in the log (util.js's `log`).
    pub fn log(&self, line: &str) {
        self.log.line(&self.paths, line);
    }

    /// A setting that is off only when set to false, as `setting(key) ===
    /// false` read.
    pub fn setting_off(&self, key: &str) -> bool {
        (self.settings)(key) == Some(Value::Bool(false))
    }
}
