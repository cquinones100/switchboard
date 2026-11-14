//! Where the relay keeps its files, and the times and limits it keeps to
//! (lib/slack-relay/config.js). Its state stays where the Node relay kept it,
//! `~/.local/state/slack-relay`, so a relay that comes over carries on with
//! the threads it had, and `switchboard hook` appends to the events file
//! there. Every path is under a home given to it, so the tests run in a home
//! of their own.

use std::path::{Path, PathBuf};

/// Whose relay this is: `{ "owner": "<your Slack member ID>" }`, outside
/// this repository, since it names a person.
pub const CONFIG_NAME: &str = ".config/switchboard/slack-relay.json";
/// The relay's own directory, under the home.
pub const STATE_NAME: &str = ".local/state/slack-relay";

/// The log is moved aside past this size, three old ones kept.
pub const MAX_LOG_BYTES: u64 = 1_000_000;
/// The tabs are looked at this often, and whenever a hook writes an event.
pub const TAB_POLL_MS: u64 = 1_000;
/// Threads you may reply in are watched for a day, up to this many: a reply
/// in one is handled as part of that conversation, and caught up after a
/// restart.
pub const THREAD_WINDOW_MS: i64 = 24 * 60 * 60_000;
pub const MAX_THREADS: usize = 30;
/// The lines of a tab's screen a post shows.
pub const SCREEN_LINES: usize = 30;
/// A tab that reports no state (a Claude started before the hooks) counts
/// as done once its screen has stopped changing for this long.
pub const QUIET_SCREEN_MS: i64 = 20_000;
pub const FOLLOW_UP_LIMIT_MS: i64 = 60 * 60_000;
/// How long a tab that isn't busy may take to pick up a typed message
/// before the relay says it hasn't.
pub const PICKUP_WARN_MS: i64 = 30_000;
/// Claude Code writes the transcript behind the session, by tens of seconds
/// for one running in the background, so a turn reported ended is read
/// again until the transcript has caught up, for up to this long, before it
/// is posted as it is (with its final message from the Stop hook, when the
/// hook gave one).
pub const REPLY_WAIT_MS: i64 = 60_000;
/// Slack shows about 4,000 characters of a message.
pub const MAX_POST_CHARS: usize = 3_500;

/// The relay's files, under one home.
#[derive(Clone, Debug)]
pub struct Paths {
    pub home: PathBuf,
    /// `~/.local/state/slack-relay`.
    pub state_dir: PathBuf,
    /// `~/.claude`.
    pub claude_dir: PathBuf,
    /// Switchboard's plugin for Claude Code, `~/.local/share/switchboard/claude/plugin`.
    pub plugin_dir: PathBuf,
    /// Where the statusline writes each session's context use
    /// (`context/<session>`): Switchboard's state directory, then where
    /// claude/statusline-command.sh wrote it for the sidebar daemon.
    pub context_dirs: Vec<PathBuf>,
}

impl Paths {
    /// The relay's files under `home`, the statusline's in `switchboard_state`.
    pub fn under(home: &Path, switchboard_state: &Path) -> Paths {
        Paths {
            home: home.to_path_buf(),
            state_dir: home.join(STATE_NAME),
            claude_dir: home.join(".claude"),
            plugin_dir: home.join(crate::paths::PLUGIN),
            context_dirs: vec![
                switchboard_state.join("context"),
                home.join(".local/state/tmux-sidebar/context"),
            ],
        }
    }

    /// The relay's files where this run's home and state directory are.
    pub fn current() -> Paths {
        Paths::under(&crate::paths::home(), &crate::paths::state_dir())
    }

    pub fn config_file(&self) -> PathBuf {
        self.home.join(CONFIG_NAME)
    }

    pub fn state_file(&self) -> PathBuf {
        self.state_dir.join("state.json")
    }

    pub fn log_file(&self) -> PathBuf {
        self.state_dir.join("relay.log")
    }

    /// Turn ends and waits reported by Claude Code's hooks, which write only
    /// while it exists (see `followups::new_events`).
    pub fn events_file(&self) -> PathBuf {
        self.state_dir.join("events.jsonl")
    }

    /// Only one relay listens to the DM at a time.
    pub fn lock_file(&self) -> PathBuf {
        self.state_dir.join("relay.lock")
    }

    pub fn outbox_dir(&self) -> PathBuf {
        self.state_dir.join("outbox")
    }

    /// Images and videos you sent, downloaded for the tab.
    pub fn files_dir(&self) -> PathBuf {
        self.state_dir.join("files")
    }

    /// What Haiku has cost today.
    pub fn usage_file(&self) -> PathBuf {
        self.state_dir.join("haiku.json")
    }

    pub fn sessions_dir(&self) -> PathBuf {
        self.claude_dir.join("sessions")
    }

    pub fn projects_dir(&self) -> PathBuf {
        self.claude_dir.join("projects")
    }

    /// A path with the home shown as `~`, as places.js's `shortPath`.
    pub fn short(&self, path: &Path) -> String {
        let text = path.to_string_lossy();
        let home = self.home.to_string_lossy();
        match text.strip_prefix(home.as_ref()) {
            Some(rest) if !home.is_empty() && home != "/" => format!("~{rest}"),
            _ => text.into_owned(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_are_under_the_home_given() {
        let p = Paths::under(Path::new("/h"), Path::new("/s"));
        assert_eq!(
            p.events_file(),
            PathBuf::from("/h/.local/state/slack-relay/events.jsonl")
        );
        assert_eq!(
            p.config_file(),
            PathBuf::from("/h/.config/switchboard/slack-relay.json")
        );
        assert_eq!(p.short(Path::new("/h/work")), "~/work");
        assert_eq!(p.short(Path::new("/other")), "/other");
    }
}
