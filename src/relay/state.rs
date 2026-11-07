//! What the relay keeps between runs, in `state.json` (lib/slack-relay/
//! state.js): the DM, how far it has read, the threads you may reply in,
//! which tab each alert was about, each tab's home thread, the turns it is
//! following, and the rest each part describes. Written whole after each
//! change, readable by its owner alone.
//!
//! Every field is read on its own, so one an older relay wrote in another
//! shape starts afresh rather than losing the rest, and fields the relay no
//! longer keeps are dropped when it is next written.

use std::path::Path;

use indexmap::IndexMap;
use serde::de::{DeserializeOwned, Deserializer};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::config::{MAX_THREADS, Paths, THREAD_WINDOW_MS};
use crate::util;

fn lenient<'de, D, T>(d: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: DeserializeOwned + Default,
{
    let v = Value::deserialize(d)?;
    Ok(serde_json::from_value(v).unwrap_or_default())
}

/// The DM with the owner: its channel, the owner, the workspace, the app's
/// bot user, the workspace's address for links, and the app's ID once an
/// event has given it. `app` marks state from the relay as an app (not the
/// self-DM it once used).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Dm {
    #[serde(default, deserialize_with = "lenient")]
    pub channel: String,
    #[serde(default, deserialize_with = "lenient")]
    pub user: String,
    #[serde(default, deserialize_with = "lenient")]
    pub team: Option<String>,
    #[serde(default, deserialize_with = "lenient")]
    pub bot: Option<String>,
    #[serde(default, deserialize_with = "lenient")]
    pub url: Option<String>,
    #[serde(default, deserialize_with = "lenient")]
    pub app: bool,
    #[serde(
        default,
        deserialize_with = "lenient",
        skip_serializing_if = "Option::is_none"
    )]
    pub app_id: Option<String>,
}

/// Which tab a message was about: a reply under it goes to that tab.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Target {
    #[serde(default, deserialize_with = "lenient")]
    pub window: String,
    #[serde(default, deserialize_with = "lenient")]
    pub index: i64,
    #[serde(default, deserialize_with = "lenient")]
    pub name: String,
    #[serde(
        default,
        deserialize_with = "lenient",
        skip_serializing_if = "Option::is_none"
    )]
    pub thread: Option<String>,
}

/// A turn the relay typed into a tab and is waiting to report: where to
/// report it, what was typed, and how far it has got.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Follow {
    #[serde(default, deserialize_with = "lenient")]
    pub thread: Option<String>,
    #[serde(default, deserialize_with = "lenient")]
    pub index: i64,
    #[serde(default, deserialize_with = "lenient")]
    pub name: String,
    #[serde(default, deserialize_with = "lenient")]
    pub text: String,
    #[serde(default, deserialize_with = "lenient")]
    pub request: Option<String>,
    #[serde(default, deserialize_with = "lenient")]
    pub broadcast: bool,
    #[serde(default, deserialize_with = "lenient")]
    pub sent_at: i64,
    #[serde(default, deserialize_with = "lenient")]
    pub screen: String,
    #[serde(default, deserialize_with = "lenient")]
    pub changed_at: i64,
    #[serde(default, deserialize_with = "lenient")]
    pub taken: bool,
    #[serde(default, deserialize_with = "lenient")]
    pub warned: bool,
    /// The "Working on it" note, deleted once the answer is posted.
    #[serde(
        default,
        deserialize_with = "lenient",
        skip_serializing_if = "Option::is_none"
    )]
    pub working: Option<String>,
    /// The hook said the turn ended, remembered since each event is read
    /// once and the reply may not be in the transcript yet.
    #[serde(
        default,
        deserialize_with = "lenient",
        skip_serializing_if = "Option::is_none"
    )]
    pub ended: Option<Ended>,
    #[serde(
        default,
        deserialize_with = "lenient",
        skip_serializing_if = "Option::is_none"
    )]
    pub done_at: Option<i64>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Ended {
    #[serde(default, deserialize_with = "lenient")]
    pub event: String,
    #[serde(default, deserialize_with = "lenient")]
    pub transcript: String,
    #[serde(default, deserialize_with = "lenient")]
    pub at: i64,
    #[serde(
        default,
        deserialize_with = "lenient",
        rename = "final",
        skip_serializing_if = "Option::is_none"
    )]
    pub last: Option<String>,
}

/// A tab's home thread: its first message, the tab's process when it was
/// made (tab ids are reused), its label and what its header shows.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Home {
    #[serde(default, deserialize_with = "lenient")]
    pub ts: String,
    #[serde(default, deserialize_with = "lenient")]
    pub pid: String,
    #[serde(default, deserialize_with = "lenient")]
    pub label: String,
    #[serde(default, deserialize_with = "lenient")]
    pub shown: String,
}

/// The messages showing a tab's prompt as buttons (post.js's `offerChoice`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Offered {
    #[serde(default, deserialize_with = "lenient")]
    pub kind: String,
    #[serde(default, deserialize_with = "lenient")]
    pub messages: Vec<OfferedMessage>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct OfferedMessage {
    #[serde(default, deserialize_with = "lenient")]
    pub ts: String,
    #[serde(default, deserialize_with = "lenient")]
    pub text: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ClosedHome {
    #[serde(default, deserialize_with = "lenient")]
    pub label: String,
    #[serde(default, deserialize_with = "lenient")]
    pub at: i64,
}

/// A tab's line on the Home tab, kept so a restart doesn't ask Haiku again.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Doing {
    #[serde(default, deserialize_with = "lenient")]
    pub pid: String,
    #[serde(default, deserialize_with = "lenient")]
    pub state: String,
    #[serde(default, deserialize_with = "lenient")]
    pub last_at: Option<i64>,
    #[serde(default, deserialize_with = "lenient")]
    pub line: Option<String>,
    #[serde(default, deserialize_with = "lenient")]
    pub at: i64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct OfflineNote {
    #[serde(default, deserialize_with = "lenient")]
    pub id: String,
    #[serde(default, deserialize_with = "lenient")]
    pub at: i64,
}

/// Everything kept between runs.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct State {
    /// Each watched thread, by its first message, and the last reply handled.
    #[serde(default, deserialize_with = "lenient")]
    pub threads: IndexMap<String, String>,
    #[serde(default, deserialize_with = "lenient")]
    pub alerts: IndexMap<String, Target>,
    /// What was last reported of each tab: its state's `since`, a dialog's
    /// key, or a turn posted from its event.
    #[serde(default, deserialize_with = "lenient")]
    pub reported: IndexMap<String, String>,
    #[serde(default, deserialize_with = "lenient")]
    pub following: IndexMap<String, Follow>,
    #[serde(default, deserialize_with = "lenient")]
    pub homes: IndexMap<String, Home>,
    /// The newest message in each thread, for links that open at it.
    #[serde(default, deserialize_with = "lenient")]
    pub latest: IndexMap<String, String>,
    #[serde(
        default,
        deserialize_with = "lenient",
        skip_serializing_if = "Option::is_none"
    )]
    pub dm: Option<Dm>,
    /// The newest message at the top level that was handled.
    #[serde(
        default,
        deserialize_with = "lenient",
        skip_serializing_if = "Option::is_none"
    )]
    pub cursor: Option<String>,
    #[serde(
        default,
        deserialize_with = "lenient",
        skip_serializing_if = "Option::is_none"
    )]
    pub welcome: Option<String>,
    #[serde(default, deserialize_with = "lenient")]
    pub events_offset: u64,
    #[serde(default, deserialize_with = "lenient")]
    pub last_activity: i64,
    #[serde(
        default,
        deserialize_with = "lenient",
        skip_serializing_if = "IndexMap::is_empty"
    )]
    pub choices: IndexMap<String, Offered>,
    #[serde(
        default,
        deserialize_with = "lenient",
        skip_serializing_if = "IndexMap::is_empty"
    )]
    pub turn_posted: IndexMap<String, i64>,
    #[serde(
        default,
        deserialize_with = "lenient",
        skip_serializing_if = "IndexMap::is_empty"
    )]
    pub stuck: IndexMap<String, i64>,
    #[serde(
        default,
        deserialize_with = "lenient",
        skip_serializing_if = "IndexMap::is_empty"
    )]
    pub context_warned: IndexMap<String, i64>,
    #[serde(
        default,
        deserialize_with = "lenient",
        skip_serializing_if = "IndexMap::is_empty"
    )]
    pub closed_homes: IndexMap<String, ClosedHome>,
    #[serde(
        default,
        deserialize_with = "lenient",
        skip_serializing_if = "IndexMap::is_empty"
    )]
    pub doing: IndexMap<String, Doing>,
    /// The Home tab as last published, but its check-in time.
    #[serde(
        default,
        deserialize_with = "lenient",
        skip_serializing_if = "Option::is_none"
    )]
    pub home: Option<String>,
    #[serde(
        default,
        deserialize_with = "lenient",
        skip_serializing_if = "Option::is_none"
    )]
    pub offline_note: Option<OfflineNote>,
    /// The dashboard message an older relay pinned in the DM, deleted once.
    #[serde(
        default,
        deserialize_with = "lenient",
        skip_serializing_if = "Option::is_none"
    )]
    pub dashboard: Option<Value>,
}

impl State {
    /// The DM, as the parts that post need it; empty before it is known.
    pub fn dm(&self) -> Dm {
        self.dm.clone().unwrap_or_default()
    }

    /// Watches a thread for replies (state.js's `watchThread`): the newest
    /// `MAX_THREADS` from the last day, and every tab's home thread for as
    /// long as the tab is open. Alerts live as long as their thread.
    pub fn watch_thread(&mut self, ts: &str, now_ms: i64) {
        if !self.threads.contains_key(ts) {
            self.threads.insert(ts.to_string(), ts.to_string());
        }
        let cutoff = now_ms as f64 / 1000.0 - THREAD_WINDOW_MS as f64 / 1000.0;
        let homes: Vec<String> = self.homes.values().map(|h| h.ts.clone()).collect();
        let mut recent: Vec<&String> = self
            .threads
            .keys()
            .filter(|t| number(t) >= cutoff && !homes.contains(t))
            .collect();
        recent.sort_by(|a, b| number(b).total_cmp(&number(a)));
        let mut kept: Vec<String> = recent.into_iter().take(MAX_THREADS).cloned().collect();
        kept.extend(
            homes
                .iter()
                .filter(|t| self.threads.contains_key(*t))
                .cloned(),
        );
        self.threads = kept
            .iter()
            .filter_map(|t| self.threads.get(t).map(|v| (t.clone(), v.clone())))
            .collect();
        self.latest.retain(|t, _| kept.contains(t));
        self.alerts.retain(|t, a| {
            kept.contains(t) || a.thread.as_ref().is_some_and(|th| kept.contains(th))
        });
    }

    /// The newest message in a thread, yours or the relay's, so a link to
    /// the thread can open at its bottom (state.js's `newestIn`).
    pub fn newest_in(&mut self, thread: &str, ts: &str) {
        let known = self.latest.get(thread).map(|t| number(t)).unwrap_or(0.0);
        if number(ts) > known {
            self.latest.insert(thread.to_string(), ts.to_string());
        }
    }
}

/// A Slack ts as a number, as `Number(ts)`; 0 for anything else.
pub fn number(ts: &str) -> f64 {
    ts.parse().unwrap_or(0.0)
}

/// The state as last saved, or a fresh one.
pub fn load(paths: &Paths) -> State {
    util::read_text(paths.state_file())
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

/// Writes the state whole, owner-only, renamed into place.
pub fn save(paths: &Paths, state: &State) -> std::io::Result<()> {
    std::fs::create_dir_all(&paths.state_dir)?;
    let file = paths.state_file();
    let tmp = file.with_extension("json.tmp");
    util::write_private(&tmp, util::json_pretty(state), false)?;
    std::fs::rename(&tmp, &file)
}

/// The relay's directory and files readable by their owner alone
/// (lifecycle.js's `ownerOnly`).
pub fn owner_only(paths: &Paths) {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    let _ = std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&paths.state_dir);
    let _ = std::fs::set_permissions(&paths.state_dir, std::fs::Permissions::from_mode(0o700));
    for file in [paths.state_file(), paths.log_file(), paths.events_file()] {
        if Path::new(&file).exists() {
            let _ = std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn a_home_thread_stays_watched_past_the_usual_limits() {
        let now = 1_791_500_000_000i64;
        let mut state = State::default();
        let old = "1791000000.000100";
        state.homes.insert(
            "@1".into(),
            Home {
                ts: old.into(),
                pid: "1".into(),
                ..Home::default()
            },
        );
        state.threads.insert(old.into(), old.into());
        for i in 0..40 {
            state.watch_thread(&format!("{}.000{:03}", now / 1000 - i, i), now);
        }
        assert!(state.threads.contains_key(old), "the home thread is kept");
        assert_eq!(state.threads.len(), MAX_THREADS + 1);
        // An old thread that isn't a home goes.
        state.watch_thread("1700000000.000100", now);
        assert!(!state.threads.contains_key("1700000000.000100"));
    }

    #[test]
    fn state_and_its_directory_are_owner_only_and_read_leniently() {
        let dir = std::env::temp_dir().join(format!("relay-state-{}", std::process::id()));
        let paths = Paths::under(&dir, &dir.join("s"));
        std::fs::create_dir_all(&paths.state_dir).unwrap();
        std::fs::set_permissions(&paths.state_dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut state = State::default();
        state.threads.insert("1.0".into(), "1.0".into());
        save(&paths, &state).unwrap();
        owner_only(&paths);
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&paths.state_dir), 0o700);
        assert_eq!(mode(&paths.state_file()), 0o600);
        assert_eq!(load(&paths), state);
        // A field in another shape starts afresh; the rest is kept, and what
        // an older relay kept is dropped.
        std::fs::write(
            paths.state_file(),
            r#"{"threads":{"1.0":"1.0"},"alerts":[1,2],"transcripts":{"x":1},"cursor":"5.0"}"#,
        )
        .unwrap();
        let read = load(&paths);
        assert_eq!(read.threads.len(), 1);
        assert!(read.alerts.is_empty());
        assert_eq!(read.cursor.as_deref(), Some("5.0"));
        assert!(!util::json_compact(&read).contains("transcripts"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
