//! The health light (tmux/health.sh): nothing on the status line while all
//! is well, and a red mark naming what quietly stopped when something that
//! should be running has, so it is seen before what it stopped is missed.
//!
//! What it watches, of what health.sh did, is what still applies:
//!
//! - connectors: a Claude connector a feature needs (Slack, Google Calendar,
//!   Notion, Fathom...) wasn't connected when it last tried, in the last
//!   `CONNECTOR_LIMIT` (src/connector.rs writes `connectors-down`, a
//!   `<service>\t<when>` line each, in the state directory).
//! - relay: the Slack relay crashed. Its events file is still there, which
//!   it removes only when stopped on purpose, but the process its lock file
//!   names has gone (lib/slack-relay/config.js's `EVENTS_FILE` and
//!   `LOCK_FILE`).
//! - saves: the layout changed and the write asked for never reached
//!   `layout.json` for over `SAVE_LIMIT` (src/mux/persist.rs), where health.sh
//!   watched tmux-continuum's last save. A save here is written only when
//!   something changed, so an old file alone is no sign of trouble; a write
//!   asked for and not landed is.
//! - GitHub: under 20% of its hour's limit is left, so its lookups are
//!   slowed down (`sources::schedule::Budget`), until the limit resets.
//!
//! The sidebar daemon going down can't happen apart from the server now, and
//! whether setup is out of date is setup's to say. It is read on the server's
//! tick with file checks and a `kill(pid, 0)`, as health.sh was one tmux call
//! and file checks.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use crate::{paths, util};

/// A connector down longer ago than this is no longer shown.
pub const CONNECTOR_LIMIT: i64 = 7200;
/// A save this late counts as stopped.
pub const SAVE_LIMIT: Duration = Duration::from_secs(300);

/// The relay's state directory (lib/slack-relay/config.js's `STATE_DIR`).
pub fn relay_dir() -> PathBuf {
    paths::home_join(".local/state/slack-relay")
}

/// The services in a `connectors-down` file down within `CONNECTOR_LIMIT`
/// of `now` (seconds), each as "<service> not connected".
pub fn connectors_down(text: &str, now: i64) -> Vec<String> {
    text.lines()
        .filter_map(|line| {
            let (service, at) = line.split_once('\t').unwrap_or((line, "0"));
            let at: i64 = at.trim().parse().unwrap_or(0);
            (!service.is_empty() && now - at < CONNECTOR_LIMIT)
                .then(|| format!("{service} not connected"))
        })
        .collect()
}

/// Whether the relay crashed: its events file is there, and the pid its lock
/// file starts with (`<pid> <who>`) isn't running, or there is no lock.
pub fn relay_down(dir: &Path, alive: impl Fn(i64) -> bool) -> bool {
    if !dir.join("events.jsonl").is_file() {
        return false;
    }
    let pid = util::read_text(dir.join("relay.lock"))
        .and_then(|t| t.split_whitespace().next().and_then(|p| p.parse().ok()))
        .unwrap_or(0);
    pid <= 0 || !alive(pid)
}

/// How long the layout has gone unsaved: a write was asked for at `asked`
/// and the file is older than that (or missing), for longer than
/// `SAVE_LIMIT`. Measured from the file's time, as health.sh gave the save's
/// age.
pub fn unsaved_for(
    asked: Option<SystemTime>,
    file: Option<SystemTime>,
    now: SystemTime,
) -> Option<Duration> {
    let asked = asked?;
    if file.is_some_and(|f| f >= asked) {
        return None;
    }
    let since = file.unwrap_or(asked);
    let age = now.duration_since(since).ok()?;
    (now.duration_since(asked).ok()? > SAVE_LIMIT).then_some(age)
}

/// The words for an unsaved layout.
pub fn unsaved_words(age: Duration) -> String {
    format!("saves {}m old", age.as_secs() / 60)
}

/// What the light says while GitHub's lookups are slowed down for its hour's
/// limit (`sources::schedule::Budget`), from the share of it left.
pub fn github_words(left: Option<f64>) -> Option<String> {
    let left = left.filter(|l| *l < crate::sources::schedule::LOW)?;
    Some(format!(
        "GitHub: {}% of the hour left",
        (left * 100.0).floor()
    ))
}

/// Everything wrong now, in health.sh's order (saves, relay, connectors).
/// `asked` is when the layout's last write was asked for, `None` with no
/// panes to save.
pub fn check(state_dir: &Path, relay: &Path, asked: Option<SystemTime>) -> Vec<String> {
    let mut problems = Vec::new();
    let file = std::fs::metadata(state_dir.join(crate::mux::persist::FILE))
        .and_then(|m| m.modified())
        .ok();
    if let Some(age) = unsaved_for(asked, file, SystemTime::now()) {
        problems.push(unsaved_words(age));
    }
    if relay_down(relay, util::is_alive) {
        problems.push("relay down".into());
    }
    if let Some(text) = util::read_text(state_dir.join("connectors-down")) {
        problems.extend(connectors_down(&text, util::now_secs()));
    }
    problems
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("sb-health-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_connector_down_shows_for_two_hours() {
        let text = "Slack\t1000\nNotion\t9000\n\nFathom\n";
        assert_eq!(connectors_down(text, 9100), ["Notion not connected"]);
        assert_eq!(
            connectors_down(text, 1100),
            [
                "Slack not connected",
                "Notion not connected",
                "Fathom not connected"
            ]
        );
    }

    #[test]
    fn the_relay_is_down_when_its_events_outlive_its_process() {
        let dir = temp("relay");
        assert!(!relay_down(&dir, |_| false), "never started");
        std::fs::write(dir.join("events.jsonl"), "").unwrap();
        assert!(relay_down(&dir, |_| true), "no lock");
        std::fs::write(dir.join("relay.lock"), "4242 owner").unwrap();
        assert!(!relay_down(&dir, |pid| pid == 4242));
        assert!(relay_down(&dir, |pid| pid == 1));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_save_that_never_landed_shows_after_five_minutes() {
        let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let min = |m: u64| Duration::from_secs(m * 60);
        // Nothing asked, or the write landed.
        assert_eq!(unsaved_for(None, Some(t0), t0 + min(60)), None);
        assert_eq!(unsaved_for(Some(t0), Some(t0), t0 + min(60)), None);
        // Asked and not landed, not yet long.
        assert_eq!(unsaved_for(Some(t0), Some(t0 - min(2)), t0 + min(4)), None);
        // Long since: as old as the file.
        assert_eq!(
            unsaved_for(Some(t0), Some(t0 - min(2)), t0 + min(6)),
            Some(min(8))
        );
        assert_eq!(unsaved_words(min(8)), "saves 8m old");
        // No file at all.
        assert_eq!(unsaved_for(Some(t0), None, t0 + min(6)), Some(min(6)));
    }

    #[test]
    fn github_shows_only_while_its_lookups_are_slowed() {
        assert_eq!(github_words(None), None);
        assert_eq!(github_words(Some(0.5)), None);
        assert_eq!(
            github_words(Some(0.123)).as_deref(),
            Some("GitHub: 12% of the hour left")
        );
    }

    #[test]
    fn check_reads_the_state_directory() {
        let state = temp("state");
        let relay = temp("relay-check");
        assert!(check(&state, &relay, None).is_empty());
        std::fs::write(
            state.join("connectors-down"),
            format!("Slack\t{}\n", util::now_secs()),
        )
        .unwrap();
        std::fs::write(relay.join("events.jsonl"), "").unwrap();
        assert_eq!(
            check(&state, &relay, None),
            ["relay down", "Slack not connected"]
        );
        let _ = std::fs::remove_dir_all(&state);
        let _ = std::fs::remove_dir_all(&relay);
    }
}
