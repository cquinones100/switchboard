//! The tabs that closed, for mission control's Closed view, which reopens
//! one where it was with its Claude session resumed. Ported from
//! lib/sidebar/closed.js.
//!
//! The daemon compared the tabs on each refresh with how they were, since
//! tmux didn't tell it when a window closed. The server knows: each tab's
//! name, directory and Claude session are kept as last seen (`seen`, on
//! every tick, and once more as it closes), and a tab taken out records them
//! (`closed`). The newest `MAX` are kept in memory and in `closed.jsonl` in
//! the state directory, so a restart doesn't lose them. Each line is the
//! Node version's (`at`, `id`, `index`, `name`, `tabName`, `kind`, `path`,
//! `session`, `lived`), so a log it wrote reads here.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::items::{Item, Target};
use crate::mux::TabId;
use crate::{paths, util};

/// Closings kept.
pub const MAX: usize = 200;
/// A tab that came and went within this long (a tool's) isn't listed.
pub const BRIEF_MS: i64 = 10_000;
/// The log is written whole again once it grows past this, so it stays near
/// `MAX` lines.
const REWRITE_OVER: u64 = 64 * 1024;

fn file(state: &Path) -> PathBuf {
    state.join("closed.jsonl")
}

fn removed_file(state: &Path) -> PathBuf {
    state.join("closed-removed.json")
}

/// One closing, as a line of the log.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Closing {
    pub at: i64,
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub index: usize,
    #[serde(default)]
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tab_name: Option<String>,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub path: Option<PathBuf>,
    /// The directory it was about while its panes worked in `path`
    /// (`@tab_dir`, the review tab's PR worktree), given back as it reopens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tab_dir: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    #[serde(default)]
    pub lived: i64,
}

/// A tab as it is now, to be kept as last seen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TabNow {
    pub tab: TabId,
    pub index: usize,
    /// Its label.
    pub name: String,
    /// The name given to it, to take back when it reopens.
    pub tab_name: Option<String>,
    pub path: Option<PathBuf>,
    /// Its `@tab_dir`, when it has one.
    pub tab_dir: Option<PathBuf>,
    /// Its Claude's session, when one runs in it.
    pub session: Option<String>,
}

#[derive(Clone, Debug)]
struct Seen {
    now: TabNow,
    since: i64,
}

/// An open tab, for leaving out what is open again.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OpenTab {
    pub path: Option<PathBuf>,
    pub session: Option<String>,
}

/// The log and each open tab as last seen.
#[derive(Debug, Default)]
pub struct Closed {
    events: Option<Vec<Closing>>,
    last: HashMap<TabId, Seen>,
}

fn safe_session(s: &Option<String>) -> Option<&str> {
    s.as_deref()
        .filter(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))
}

impl Closed {
    fn load(&mut self, state: &Path) -> &mut Vec<Closing> {
        self.events.get_or_insert_with(|| {
            let text = util::read_text(file(state)).unwrap_or_default();
            let mut list: Vec<Closing> = text
                .lines()
                .filter(|l| !l.trim().is_empty())
                .filter_map(|l| serde_json::from_str(l).ok())
                .collect();
            let skip = list.len().saturating_sub(MAX);
            list.drain(..skip);
            list
        })
    }

    /// Keeps a tab as it is now. A Claude that has quit in the same
    /// directory, the pane back at its shell, is still the one to resume.
    pub fn seen(&mut self, now: TabNow, at: i64) {
        let was = self.last.get(&now.tab);
        let since = was.map_or(at, |w| w.since);
        let mut now = now;
        // A directory that can no longer be read (a review worktree removed
        // before its tab closed) keeps the one last known.
        if now.path.is_none()
            && let Some(was) = was
        {
            now.path = was.now.path.clone();
        }
        if now.session.is_none()
            && let Some(was) = was
            && was.now.path == now.path
        {
            now.session = was.now.session.clone();
        }
        self.last.insert(now.tab, Seen { now, since });
    }

    /// A tab is being taken out: what it was, as last seen, goes in the log.
    pub fn closed(&mut self, state: &Path, tab: TabId, at: i64) {
        let Some(was) = self.last.remove(&tab) else {
            return;
        };
        let event = Closing {
            at,
            id: tab.to_string(),
            index: was.now.index,
            name: if was.now.name.is_empty() {
                tab.to_string()
            } else {
                was.now.name
            },
            tab_name: was.now.tab_name,
            tab_dir: was.now.tab_dir,
            kind: "closed".into(),
            path: was.now.path,
            session: was.now.session,
            lived: at - was.since,
        };
        self.push(state, event);
    }

    /// Forgets the tabs that are gone without closing (a restart's).
    pub fn keep_only(&mut self, open: &HashSet<TabId>) {
        self.last.retain(|t, _| open.contains(t));
    }

    fn push(&mut self, state: &Path, event: Closing) {
        let line = util::json_compact(&event);
        let events = self.load(state);
        events.push(event);
        let over = events.len().saturating_sub(MAX);
        events.drain(..over);
        let path = file(state);
        let _ = fs::create_dir_all(state);
        let big = fs::metadata(&path).is_ok_and(|m| m.len() > REWRITE_OVER);
        if big {
            let whole: String = events
                .iter()
                .map(|e| format!("{}\n", util::json_compact(e)))
                .collect();
            let _ = util::write_private(&path, whole, false);
        } else {
            let _ = util::write_private(&path, format!("{line}\n"), true);
        }
    }

    /// The view's rows: the tabs closed most recently first, one per Claude
    /// conversation, leaving out one a tab has open again, so several tabs
    /// closed in the same repository are each listed; a tab with no
    /// conversation, one per directory no tab is open in. A tab that came
    /// and went within seconds is left out, and so is one removed from the
    /// view, up to the time it closed (`remove`), so it comes back only if it
    /// is closed again.
    pub fn items(&mut self, state: &Path, open: &[OpenTab], now: i64) -> Vec<Item> {
        let removed = removed(state);
        let mut listed: HashSet<String> = HashSet::new();
        let events = self.load(state).clone();
        events
            .iter()
            .rev()
            .filter(|e| e.kind == "closed" && e.lived >= BRIEF_MS)
            .filter_map(|e| {
                let path = e.path.as_ref()?;
                let session = safe_session(&e.session);
                let is_open = match session {
                    Some(s) => open.iter().any(|t| t.session.as_deref() == Some(s)),
                    None => open.iter().any(|t| t.path.as_ref() == Some(path)),
                };
                let key = match session {
                    Some(s) => s.to_string(),
                    None => format!("dir:{}", path.display()),
                };
                if is_open
                    || removed.get(&key).is_some_and(|until| e.at <= *until)
                    || !listed.insert(key.clone())
                {
                    return None;
                }
                let detail = [
                    format!(
                        "closed {} ago",
                        super::sessions::ago(e.at.div_euclid(1000), now.div_euclid(1000))
                    ),
                    if session.is_some() {
                        "resumes its Claude".into()
                    } else {
                        "a shell".into()
                    },
                    paths::tilde(path),
                ]
                .join(" · ");
                Some(
                    Item::new(key.clone(), e.name.clone())
                        .detail(detail)
                        .time(e.at)
                        .target(Target::Dir(path.clone()))
                        .data(json!({
                            "session": session,
                            "tabName": e.tab_name,
                            "closed": { "key": key, "at": e.at },
                        })),
                )
            })
            .collect()
    }

    /// The closing an item of the view stands for, by its key.
    pub fn find(&mut self, state: &Path, key: &str) -> Option<Closing> {
        self.load(state)
            .iter()
            .rev()
            .find(|e| match safe_session(&e.session) {
                Some(s) => s == key,
                None => e
                    .path
                    .as_ref()
                    .is_some_and(|p| format!("dir:{}", p.display()) == key),
            })
            .cloned()
    }

    /// Removes a closed tab from the view, up to `at`. Entries for closings
    /// no longer in the log go with it.
    pub fn remove(&mut self, state: &Path, key: &str, at: i64) {
        let oldest = self.load(state).first().map_or(0, |e| e.at);
        let mut saved: Map<String, Value> = util::read_json_object(removed_file(state));
        saved.insert(key.to_string(), Value::from(at));
        saved.retain(|_, v| v.as_f64().is_some_and(|t| t as i64 >= oldest));
        let _ = util::write_atomic(
            removed_file(state),
            util::json_compact(&Value::Object(saved)),
        );
    }
}

fn removed(state: &Path) -> HashMap<String, i64> {
    util::read_json_object(removed_file(state))
        .into_iter()
        .filter_map(|(k, v)| Some((k, v.as_f64()? as i64)))
        .collect()
}

/// What to type into a reopened tab's shell: `claude --resume <session>`,
/// so the shell and its PATH are there once Claude ends, as the daemon's
/// `zsh -i -c 'claude --resume …; exec zsh -i'` did.
pub fn resume_line(session: Option<&str>) -> Option<String> {
    let s = session?;
    (!s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))
        .then(|| format!("claude --resume {s}\r"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "sb-closed-{name}-{}-{}",
            std::process::id(),
            util::now_ms()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn tab(n: u32, name: &str, path: &str, session: Option<&str>) -> TabNow {
        TabNow {
            tab: TabId(n),
            index: n as usize,
            name: name.into(),
            tab_name: None,
            tab_dir: None,
            path: Some(path.into()),
            session: session.map(str::to_string),
        }
    }

    fn open(path: &str, session: Option<&str>) -> OpenTab {
        OpenTab {
            path: Some(path.into()),
            session: session.map(str::to_string),
        }
    }

    #[test]
    fn closed_tabs_are_listed_newest_first_one_per_conversation() {
        let state = temp("list");
        let mut closed = Closed::default();
        let now = util::now_ms();
        let minute_ago = now - 60_000;
        let claude = tab(80, "ABC-1: Feat", "/work/abc-1", Some("1234-abcd"));
        let shell = tab(81, "scratch", "/work/scratch", None);
        let again = tab(82, "ABC-1 older", "/work/abc-1", Some("9999"));
        let tool = tab(83, "lazygit", "/work/tool", None);
        for t in [&claude, &shell, &again] {
            closed.seen(t.clone(), minute_ago);
        }
        closed.seen(tool.clone(), now);
        closed.closed(&state, TabId(83), now + 1);
        closed.closed(&state, TabId(82), now + 2);
        closed.closed(&state, TabId(80), now + 3);
        closed.closed(&state, TabId(81), now + 4);
        let rows = |items: Vec<Item>| -> Vec<(String, String)> {
            items.into_iter().map(|i| (i.title, i.detail)).collect()
        };
        let items = closed.items(&state, &[], now + 5);
        assert_eq!(
            rows(items.clone()),
            vec![
                (
                    "scratch".into(),
                    "closed <1m ago · a shell · /work/scratch".into()
                ),
                (
                    "ABC-1: Feat".into(),
                    "closed <1m ago · resumes its Claude · /work/abc-1".into()
                ),
                (
                    "ABC-1 older".into(),
                    "closed <1m ago · resumes its Claude · /work/abc-1".into()
                ),
            ],
            "each conversation, two in one directory included, and not the tool's tab"
        );
        assert_eq!(items[1].target, Target::Dir("/work/abc-1".into()));
        assert_eq!(items[1].data["session"], "1234-abcd");
        assert_eq!(
            resume_line(items[1].data["session"].as_str()).as_deref(),
            Some("claude --resume 1234-abcd\r")
        );
        assert_eq!(resume_line(items[0].data["session"].as_str()), None);
        assert_eq!(
            closed
                .items(&state, &[open("/work/scratch", None)], now)
                .into_iter()
                .map(|i| i.title)
                .collect::<Vec<_>>(),
            vec!["ABC-1: Feat", "ABC-1 older"],
            "not a shell's directory open again"
        );
        assert_eq!(
            closed
                .items(&state, &[open("/work/abc-1", Some("9999"))], now)
                .into_iter()
                .map(|i| i.title)
                .collect::<Vec<_>>(),
            vec!["scratch", "ABC-1: Feat"],
            "not a conversation open again, though a tab in its directory is no reason"
        );
        // Removing one leaves out older closings of the same conversation too.
        closed.remove(&state, &items[1].id, now + 3);
        assert_eq!(
            closed
                .items(&state, &[], now)
                .into_iter()
                .map(|i| i.title)
                .collect::<Vec<_>>(),
            vec!["scratch", "ABC-1 older"]
        );
        // The log reads back in a new server.
        let mut fresh = Closed::default();
        assert_eq!(fresh.items(&state, &[], now).len(), 2);
        assert_eq!(
            fresh.find(&state, "9999").map(|e| e.name),
            Some("ABC-1 older".to_string())
        );
        let _ = fs::remove_dir_all(&state);
    }

    #[test]
    fn a_claude_that_quit_in_the_same_directory_is_still_the_one_to_resume() {
        let state = temp("quit");
        let mut closed = Closed::default();
        let now = util::now_ms();
        let mut thread = tab(84, "Slack: a thread", "/work/thread", Some("5555"));
        thread.tab_name = Some("Slack: a thread".into());
        closed.seen(thread.clone(), now - 60_000);
        closed.seen(
            TabNow {
                session: None,
                ..thread.clone()
            },
            now - 30_000,
        );
        closed.closed(&state, TabId(84), now);
        let items = closed.items(&state, &[], now);
        assert_eq!(items[0].title, "Slack: a thread");
        assert_eq!(items[0].data["session"], "5555");
        assert_eq!(
            items[0].data["tabName"], "Slack: a thread",
            "the name it was given, to take back"
        );
        let _ = fs::remove_dir_all(&state);
    }

    #[test]
    fn a_review_tab_keeps_its_worktree_and_the_last_directory_it_had() {
        let state = temp("review");
        let mut closed = Closed::default();
        let now = util::now_ms();
        let review = TabNow {
            tab_dir: Some("/cache/pr-review/acme-widgets/7".into()),
            ..tab(85, "#7 Review PR", "/work/widgets", Some("7777"))
        };
        closed.seen(review.clone(), now - 60_000);
        // Its directory can't be read as it closes (the worktree gone).
        closed.seen(
            TabNow {
                path: None,
                ..review
            },
            now,
        );
        closed.closed(&state, TabId(85), now);
        let id = closed.items(&state, &[], now)[0].id.clone();
        let found = closed.find(&state, &id).unwrap();
        assert_eq!(found.path, Some("/work/widgets".into()));
        assert_eq!(
            found.tab_dir,
            Some("/cache/pr-review/acme-widgets/7".into())
        );
        let _ = fs::remove_dir_all(&state);
    }

    #[test]
    fn a_log_the_node_version_wrote_reads() {
        let state = temp("node");
        fs::write(
            state.join("closed.jsonl"),
            "{\"at\":1700000000000,\"id\":\"@3\",\"index\":2,\"name\":\"api\",\"kind\":\"closed\",\"path\":\"/w/api\",\"session\":\"ab-1\",\"lived\":120000}\n",
        )
        .unwrap();
        let mut closed = Closed::default();
        let items = closed.items(&state, &[], 1_700_000_100_000);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, "ab-1");
        assert_eq!(
            items[0].detail,
            "closed 1m ago · resumes its Claude · /w/api"
        );
        let _ = fs::remove_dir_all(&state);
    }
}
