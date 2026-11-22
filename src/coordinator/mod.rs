//! The coordinator: one interactive Claude Code session the server keeps
//! running in a pane that belongs to no tab, so it shows in no tab bar,
//! sidebar group or mission control card, and keeps the work moving while
//! you work in the tabs: it closes a tab once its PR has merged, opens the
//! next ready issues of its note one at a time, keeps that note, and files
//! issues after a leak-check. Its instructions are the `coordinate` skill
//! (assets/skills/coordinate), its permissions a settings file of its own
//! passed with `--settings` as it starts, never a line in yours. New, with
//! no tmux-version file behind it.
//!
//! It is woken only by events, each typed into its session as a line: a
//! tab's PR merged, a tab finished or started waiting on you, its note
//! changed outside its own edits; a message from another tab reaches it as
//! any tab's Claude, by its address in `switchboard tabs`, and you write to
//! it in its pane (M-C). The events wait while it works, while it waits on
//! you and for a while after you typed in it, and go in together once it
//! sleeps, so with nothing happening it makes no model call.
//!
//! Its pane is filed under `TAB`, a tab the server never holds, so every
//! part that walks the sessions' tabs passes it over; the server draws it
//! beside the tab in front only while M-C shows it, and hands it over
//! through an upgrade as `Handover::coordinator`. Its Claude session's ID is
//! kept in `state.json` here, so a server starting again resumes it.
//!
//! This module is its plain data: the events' lines, the waiting ones, the
//! command it starts with and its permissions; src/coordinator/serve.rs puts
//! it on the server.

pub mod serve;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::mux::server::ClientId;
use crate::mux::{PaneId, TabId};

/// The setting that turns it on, off by default.
pub const SETTING: &str = "coordinator";
/// The model its session runs on.
pub const MODEL: &str = "coordinator_model";
/// The model of the subagent it hands shaping to.
pub const SHAPE_MODEL: &str = "coordinator_shape_model";
/// Its note's title.
pub const NOTE: &str = "coordinator_note";
/// The tab its pane is filed under, which the server never holds.
pub const TAB: TabId = TabId(u32::MAX);
/// The prompt a session started fresh is given: its skill.
pub const FIRST_PROMPT: &str = "/coordinate";
/// How long after it last started before it is started again, once it ended.
pub const RESTART_AFTER: std::time::Duration = std::time::Duration::from_secs(60);
/// A resumed session that ends this soon couldn't be resumed: the next
/// start is a fresh one.
pub const FAILED_WITHIN: std::time::Duration = std::time::Duration::from_secs(15);
/// How long after you last typed in it events still wait, so none is typed
/// into a message you are writing.
pub const HELD_AFTER_TYPING: std::time::Duration = std::time::Duration::from_secs(60);
/// The longest the tab it opened last holds the next open back while it
/// hasn't made its worktree: a `wt` that failed leaves the tab at a shell
/// in the folder it opened in for good.
pub const OPEN_WAIT: std::time::Duration = std::time::Duration::from_secs(10 * 60);
/// The columns the side panel needs to take it; narrower, it goes over the
/// tab's right half.
pub const LEAST_COLUMNS: u16 = 50;

/// Something that wakes it, typed into its session as one line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Wake {
    /// A tab's PR merged.
    Merged {
        tab: usize,
        name: String,
        number: u64,
        url: String,
    },
    /// A tab's Claude finished its turn.
    Finished { tab: usize, name: String },
    /// A tab's Claude started waiting on you.
    Waiting { tab: usize, name: String },
    /// Its note changed outside its own edits.
    NoteChanged { note: String },
}

impl Wake {
    /// The line typed for it.
    pub fn line(&self) -> String {
        let line = match self {
            Wake::Merged {
                tab,
                name,
                number,
                url,
            } => format!("[switchboard] Tab {tab} ({name}): PR #{number} merged, {url}"),
            Wake::Finished { tab, name } => format!("[switchboard] Tab {tab} ({name}) finished"),
            Wake::Waiting { tab, name } => {
                format!("[switchboard] Tab {tab} ({name}) is waiting on you")
            }
            Wake::NoteChanged { note } => {
                format!("[switchboard] Your note \"{note}\" changed outside your edits")
            }
        };
        line.replace(['\n', '\r'], " ")
    }
}

/// Where a client shows it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Shown {
    /// Whether it has the keys.
    pub focused: bool,
    /// Where it was last drawn, its box's inside, for a click.
    pub at: Option<ratatui::layout::Rect>,
}

/// The coordinator on the server.
#[derive(Debug, Default)]
pub struct Coordinator {
    /// Its pane, while it runs.
    pub pane: Option<PaneId>,
    /// When it last started, for `RESTART_AFTER` and `FAILED_WITHIN`.
    pub started: Option<Instant>,
    /// Whether the session it last started was a resumed one.
    pub resumed: bool,
    /// Whether its session has started (its SessionStart hook came), before
    /// which nothing is typed.
    pub ready: bool,
    /// The lines waiting to be typed, in order.
    pub pending: Vec<String>,
    /// The clients showing it.
    pub shown: HashMap<ClientId, Shown>,
    /// When you last typed in it.
    pub typed: Option<Instant>,
    /// Its note's modification time as last seen, its own edits included.
    pub note_seen: Option<SystemTime>,
    /// Whether it is editing its note now (an Edit or Write of it begun and
    /// not yet ended), while which a change is its own.
    pub editing_note: bool,
    /// The tab it last opened for an issue, which must have its worktree
    /// before it opens the next (`coordinator-open`), the folder it
    /// opened in, and when, so a `wt` that failed holds the next at most
    /// `OPEN_WAIT`.
    pub opened: Option<(TabId, PathBuf, Instant)>,
    /// What is kept in `state.json`.
    pub saved: Saved,
    /// The note's title its running session was given its permissions for:
    /// another chosen in Settings starts it again, resumed, with the new.
    pub note: String,
}

/// What an upgrade hands over beside its pane (`Handover::coordinator_state`):
/// the events still waiting and the tab it opened last, with its folder.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Handed {
    #[serde(default)]
    pub pending: Vec<String>,
    /// Whether its session had started, before which nothing is typed.
    #[serde(default)]
    pub ready: bool,
    #[serde(default)]
    pub opened: Option<(TabId, PathBuf)>,
}

impl Coordinator {
    /// What an upgrade hands over.
    pub fn handed(&self) -> Handed {
        Handed {
            pending: self.pending.clone(),
            ready: self.ready,
            opened: self
                .opened
                .as_ref()
                .map(|(tab, dir, _)| (*tab, dir.clone())),
        }
    }
}

/// What is kept between servers: its Claude session's ID.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Saved {
    #[serde(default)]
    pub session: Option<String>,
    /// The PRs whose merge it was told of, by their addresses, the latest
    /// last, at most `TOLD`.
    #[serde(default)]
    pub told: Vec<String>,
    /// The events not yet typed in, so a restart doesn't lose one told of.
    #[serde(default)]
    pub pending: Vec<String>,
}

/// The most merges `Saved::told` keeps.
pub const TOLD: usize = 500;

impl Saved {
    /// A merge told of, kept.
    pub fn tell(&mut self, url: &str) {
        self.told.retain(|u| u != url);
        self.told.push(url.to_string());
        let over = self.told.len().saturating_sub(TOLD);
        self.told.drain(..over);
    }
}

impl Coordinator {
    /// What `state.json` holds, read in: its session, the merges told of,
    /// and the events it still had waiting, after any waiting now.
    pub fn load(&mut self) {
        let mut saved = read_saved();
        for line in std::mem::take(&mut saved.pending) {
            if !self.pending.contains(&line) {
                self.pending.push(line);
            }
        }
        self.saved = saved;
    }

    /// An event, waiting until it is typed; one already waiting isn't
    /// added twice.
    pub fn push(&mut self, wake: &Wake) {
        let line = wake.line();
        if !self.pending.contains(&line) {
            self.pending.push(line);
        }
    }

    /// The waiting lines as one message, taking them, when they may be
    /// typed now: its session started and not working or waiting on you,
    /// and you haven't typed in it for `HELD_AFTER_TYPING`.
    pub fn take_due(&mut self, asleep: bool, now: Instant) -> Option<String> {
        let typing = self
            .typed
            .is_some_and(|t| now.duration_since(t) < HELD_AFTER_TYPING);
        if self.pending.is_empty() || !self.ready || !asleep || typing {
            return None;
        }
        Some(std::mem::take(&mut self.pending).join("\n"))
    }
}

/// Its folder, where it starts and keeps its settings and state.
pub fn dir() -> PathBuf {
    crate::paths::state_dir().join("coordinator")
}

/// The folder it writes what it posts to GitHub in, the one folder
/// `coordinator-post` takes a body from and the one it may write in
/// without asking, apart from its note: never its own folder, which holds
/// its settings and is where Claude Code looks for a project's.
pub fn posts_dir() -> PathBuf {
    dir().join("posts")
}

/// The file its session's ID is kept in.
pub fn state_file() -> PathBuf {
    dir().join("state.json")
}

/// The settings file passed to its Claude with `--settings`.
pub fn settings_file() -> PathBuf {
    dir().join("settings.json")
}

/// Its note's file, from its title, in your notes folder.
pub fn note_file(notes: &Path, title: &str) -> PathBuf {
    notes.join(format!("{title}.md"))
}

/// What is kept, as the file holds it; nothing when it can't be read.
pub fn read_saved() -> Saved {
    std::fs::read_to_string(state_file())
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

/// The model's name as `claude --model` takes it, from the setting's value.
pub fn model_of(value: &str, fallback: &str) -> String {
    match value {
        "haiku" | "sonnet" | "opus" => value.to_string(),
        _ => fallback.to_string(),
    }
}

/// The program its pane runs: Claude, through your interactive shell so
/// your PATH finds it, on `model`, with its permissions in `settings`,
/// and either the session `resume` names or a
/// fresh one given its skill. The pane ends with Claude.
pub fn command(shell: &str, model: &str, settings: &Path, resume: Option<&str>) -> Vec<String> {
    let mut command = vec![
        shell.to_string(),
        "-i".into(),
        "-c".into(),
        "claude \"$@\"".into(),
        "claude".into(),
        "--model".into(),
        model.to_string(),
        "--settings".into(),
        settings.to_string_lossy().into_owned(),
    ];
    match resume {
        Some(id) => command.extend(["--resume".into(), id.to_string()]),
        None => command.push(FIRST_PROMPT.into()),
    }
    command
}

/// The permissions its session alone gets, beside yours: closing a tab
/// whose PR merged and opening an issue's tab (both through commands that
/// refuse anything else), naming a tab, listing the tabs, reading the
/// settings it follows and the shape skill's file, the leak-check,
/// filing and commenting on issues through `coordinator-post` (which posts
/// only to a repository of yours, a body from `posts_dir`, after a clean
/// leak-check), reading issues, reading and editing its note, and writing
/// in `posts_dir`. No other note is readable without asking, so text it
/// read on GitHub can't have it post one. Everything else asks.
pub fn permissions(note: &Path, posts: &Path) -> Value {
    // An absolute path in a rule starts with `//`.
    let abs = |p: &Path| format!("/{}", p.to_string_lossy());
    let note = abs(note);
    json!({
        "permissions": {
            "allow": [
                "Bash(switchboard tabs)",
                "Bash(switchboard coordinator-close *)",
                "Bash(switchboard coordinator-open *)",
                "Bash(switchboard set -t * @tab_name *)",
                "Bash(switchboard skill-path shape)",
                "Bash(switchboard setting coordinator_note)",
                "Bash(switchboard setting coordinator_shape_model)",
                "Bash(switchboard leak-check --file *)",
                "Bash(switchboard coordinator-post *)",
                "Bash(gh issue view *)",
                format!("Read({note})"),
                format!("Edit({note})"),
                format!("Write({note})"),
                format!("Write({}/**)", abs(posts)),
                // The shape skill's files, which its shaping subagent reads.
                format!("Read({}/**)", abs(&crate::paths::plugin_dir().join("skills"))),
            ],
            // What Switchboard's own rules allow every pane, which reach
            // this session too when they are in your settings (Settings'
            // claude_plugin off), and are no part of its work.
            "deny": [
                "Bash(switchboard respawn *)",
                "Bash(switchboard item-done *)",
                "Bash(switchboard land-lock *)",
                "Bash(switchboard apply-actions *)",
                "Bash(switchboard new-note *)",
                "Bash(switchboard autopilot-duplicate *)",
                // Settings are yours to change: these are the ones
                // Switchboard's instructions let a pane change unasked.
                "Bash(switchboard setting relay_*)",
                "Bash(switchboard slack-relay *)",
            ],
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn each_event_is_one_line() {
        let merged = Wake::Merged {
            tab: 3,
            name: "Fix\nit".into(),
            number: 12,
            url: "https://github.com/o/r/pull/12".into(),
        };
        assert_eq!(
            merged.line(),
            "[switchboard] Tab 3 (Fix it): PR #12 merged, https://github.com/o/r/pull/12"
        );
        assert_eq!(
            Wake::Waiting {
                tab: 2,
                name: "x".into()
            }
            .line(),
            "[switchboard] Tab 2 (x) is waiting on you"
        );
    }

    #[test]
    fn events_wait_while_it_works_and_go_in_together() {
        let mut c = Coordinator {
            ready: true,
            ..Coordinator::default()
        };
        let now = Instant::now();
        c.push(&Wake::Finished {
            tab: 1,
            name: "a".into(),
        });
        c.push(&Wake::Finished {
            tab: 1,
            name: "a".into(),
        });
        c.push(&Wake::Waiting {
            tab: 2,
            name: "b".into(),
        });
        assert_eq!(c.take_due(false, now), None, "it works");
        assert_eq!(
            c.take_due(true, now).as_deref(),
            Some("[switchboard] Tab 1 (a) finished\n[switchboard] Tab 2 (b) is waiting on you")
        );
        assert!(c.pending.is_empty());
    }

    #[test]
    fn nothing_is_typed_before_it_starts_or_while_you_type() {
        let now = Instant::now();
        let mut c = Coordinator::default();
        c.push(&Wake::NoteChanged { note: "N".into() });
        assert_eq!(c.take_due(true, now), None, "not started");
        c.ready = true;
        c.typed = Some(now);
        assert_eq!(c.take_due(true, now), None, "you are typing");
        assert!(
            c.take_due(true, now + HELD_AFTER_TYPING + Duration::from_secs(1))
                .is_some()
        );
    }

    #[test]
    fn a_fresh_session_gets_its_skill_and_a_kept_one_is_resumed() {
        let fresh = command("/bin/zsh", "sonnet", Path::new("/s/settings.json"), None);
        assert_eq!(
            &fresh[..5],
            ["/bin/zsh", "-i", "-c", "claude \"$@\"", "claude"]
        );
        assert!(fresh.windows(2).any(|w| w == ["--model", "sonnet"]));
        assert!(
            fresh
                .windows(2)
                .any(|w| w == ["--settings", "/s/settings.json"])
        );
        assert_eq!(fresh.last().map(String::as_str), Some(FIRST_PROMPT));
        assert!(
            !fresh.contains(&"--add-dir".to_string()),
            "no other folder is opened to it"
        );
        let resumed = command("/bin/zsh", "opus", Path::new("/s"), Some("abc"));
        assert!(resumed.ends_with(&["--resume".into(), "abc".into()]));
        assert!(!resumed.contains(&FIRST_PROMPT.to_string()));
    }

    #[test]
    fn a_merge_told_of_is_kept_once_and_the_oldest_go() {
        let mut saved = Saved::default();
        saved.tell("a");
        saved.tell("b");
        saved.tell("a");
        assert_eq!(saved.told, ["b", "a"]);
        for n in 0..TOLD {
            saved.tell(&n.to_string());
        }
        assert_eq!(saved.told.len(), TOLD);
        assert!(!saved.told.contains(&"a".to_string()));
    }

    #[test]
    fn a_model_it_doesnt_know_falls_back() {
        assert_eq!(model_of("opus", "sonnet"), "opus");
        assert_eq!(model_of("gpt", "sonnet"), "sonnet");
    }

    #[test]
    fn its_permissions_name_its_note_by_its_absolute_path() {
        let p = permissions(
            Path::new("/h/Notes/Coordination.md"),
            Path::new("/s/coordinator/posts"),
        );
        let allow: Vec<&str> = p["permissions"]["allow"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(Value::as_str)
            .collect();
        assert!(allow.contains(&"Edit(//h/Notes/Coordination.md)"));
        assert!(allow.contains(&"Read(//h/Notes/Coordination.md)"));
        assert!(allow.contains(&"Write(//s/coordinator/posts/**)"));
        assert!(
            !allow
                .iter()
                .any(|r| r.contains("Notes/**") || r.ends_with("coordinator/**)")),
            "no other note, nor its own folder: {allow:?}"
        );
        assert!(allow.contains(&"Bash(switchboard coordinator-post *)"));
        assert!(!allow.iter().any(|r| r.starts_with("Bash(gh issue create")));
        assert!(!allow.iter().any(|r| r.contains("kill-tab")));
        assert!(
            p["permissions"]["deny"]
                .as_array()
                .unwrap()
                .contains(&Value::from("Bash(switchboard respawn *)"))
        );
    }
}
