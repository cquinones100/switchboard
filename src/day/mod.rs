//! Your day, as the sidebar daemon kept it: today's calendar and what
//! changed on it, what is new for you on Slack, the next meeting, and the
//! notes written for you (each Fathom meeting's, the morning's standup,
//! today's to-do list), with the notifications each gives. Ported from
//! lib/sidebar/'s calendar.js, calendar-feed.js, calendar-changes.js,
//! slack.js, slack-look.js, meetings.js, standup.js, todo.js and
//! new-notes.js, and the scripts the /today and /slack-updates skills save
//! with.
//!
//! The server keeps a `Day` and calls `Server::day_tick` on every tick:
//! anything that reaches out (the calendar feed, Slack and Fathom through
//! Claude's connectors, GitHub and Linear for the standup, `claude -p` for
//! a summary or a list, git for your commits) runs in a background task
//! that answers with `Event::Day`, and what mission control and the sidebar
//! read (the Calendar and Slack views' `ViewData`, the notifications) is
//! worked out on the tick from small files and kept, so nothing on the
//! drawing path waits on the network or a process.
//!
//! - `ical`, `rrule`: today's events from an iCal feed.
//! - `calendar`: the Calendar view, the feed's state, the day /today saved,
//!   the next meeting, RSVP.
//! - `changes`: what changed on today's calendar since you last looked.
//! - `slack`, `slack_look`: the Slack view, its saved look, and the look.
//! - `meetings`, `standup`, `todo`: the notes written for you.
//! - `new_notes`: the notifications for them until you read them.
//! - `serve`: the server's side: the tick, the events, what mission control
//!   calls.

pub mod calendar;
pub mod changes;
pub mod ical;
pub mod meetings;
pub mod new_notes;
pub mod rrule;
mod serve;
pub mod slack;
pub mod slack_look;
pub mod standup;
pub mod todo;

use std::future::Future;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::Value;

use crate::connector::ConnectorError;
use crate::{paths, util};

pub use serve::{Answered, Day, DayEvent};

/// Where the day's files are: the state directory, the notes directory
/// (`NOTES_DIR`, else `~/Notes`), your own config, and the Slack relay's
/// state (`SLACK_RELAY_STATE`, else `~/.local/state/slack-relay`), whose DM
/// the Slack view leaves out. Passed rather than read where used, so the
/// tests can give each its own.
#[derive(Clone, Debug)]
pub struct Dirs {
    pub state: PathBuf,
    pub notes: PathBuf,
    pub config: PathBuf,
    pub relay: PathBuf,
}

impl Dirs {
    /// As they are for this run.
    pub fn here() -> Dirs {
        let var = |name: &str| {
            std::env::var_os(name)
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
        };
        Dirs {
            state: paths::state_dir(),
            notes: var("NOTES_DIR").unwrap_or_else(|| paths::home_join("Notes")),
            config: paths::config_dir(),
            relay: var("SLACK_RELAY_STATE")
                .unwrap_or_else(|| paths::home_join(".local/state/slack-relay")),
        }
    }
}

/// One call to a Claude connector's tool. `Claude` makes it for real
/// (`crate::connector`); the tests answer as the connector would.
pub trait Connector: Sync {
    fn call(
        &self,
        tool: &str,
        input: Value,
        timeout: Option<Duration>,
    ) -> impl Future<Output = Result<String, ConnectorError>> + Send;
}

/// Claude's connectors, through a headless `claude -p`.
pub struct Claude;

impl Connector for Claude {
    fn call(
        &self,
        tool: &str,
        input: Value,
        timeout: Option<Duration>,
    ) -> impl Future<Output = Result<String, ConnectorError>> + Send {
        let tool = tool.to_string();
        async move { crate::connector::call_connector(&tool, &input, timeout).await }
    }
}

/// How a question to Claude is asked: its model, what made it (for the
/// request audit), what it should give (for the error when it doesn't), how
/// long it may take, and whether an answer is what was asked for.
#[derive(Clone, Copy, Debug)]
pub struct AskOpts {
    pub model: &'static str,
    pub source: &'static str,
    pub what: &'static str,
    pub timeout: Duration,
    pub looks: fn(&str) -> bool,
}

/// Asking Claude something with no tools: `ClaudeAsk` runs `claude -p`; the
/// tests answer for it.
pub trait Ask: Sync {
    fn ask(
        &self,
        prompt: String,
        opts: AskOpts,
    ) -> impl Future<Output = anyhow::Result<String>> + Send;
}

/// `claude -p`, as `todo::ask_claude` runs it.
pub struct ClaudeAsk;

impl Ask for ClaudeAsk {
    fn ask(
        &self,
        prompt: String,
        opts: AskOpts,
    ) -> impl Future<Output = anyhow::Result<String>> + Send {
        todo::ask_claude(prompt, opts)
    }
}

/// Writes a note that isn't there yet, its folder made first: never over
/// one that is (Node's `wx` flag), since a note is yours to edit once
/// written. `Ok(false)` when it was there.
pub fn write_new(file: &Path, text: &str) -> std::io::Result<bool> {
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir)?;
    }
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(file)
    {
        Ok(mut f) => {
            f.write_all(text.as_bytes())?;
            Ok(true)
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
        Err(e) => Err(e),
    }
}

/// A line in the state directory's `daemon.log`, as the sidebar daemon
/// logged, the file emptied once it passes a megabyte.
pub fn log(state: &Path, line: &str) {
    let file = state.join("daemon.log");
    if std::fs::metadata(&file).is_ok_and(|m| m.len() > 1024 * 1024) {
        let _ = std::fs::write(&file, "");
    }
    util::append_quietly(
        &file,
        format!("{} {line}\n", ical::iso(jiff::Timestamp::now())),
    );
}
