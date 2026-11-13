//! The tabs that closed, for mission control's Reopen view, which reopens
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
//! `session`, `lived`), so a log it wrote reads here; what the Reopen view
//! added (the last prompt, what became of the work, how many panes, the
//! worktree a sealed review was of, and the name of its last screen's file,
//! src/usage/shots.rs) is read with a default where a line has none.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::mux::TabId;
use crate::util;

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
    /// Its Claude's last prompt.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub prompt: String,
    /// What became of its work: `landed <hash>`, `tests failed` or
    /// `changes asked`; empty with none of them.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub outcome: String,
    #[serde(default)]
    pub panes: usize,
    /// When it opened (`@tab_opened`), in ms; 0 when that isn't known,
    /// and then it is taken as when this server first saw it.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub opened: i64,
    /// The worktree of someone else's PR it reviewed in a sealed try
    /// (`@sealed_review`), started again beside it as it reopens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sealed: Option<PathBuf>,
    /// The sealed try fetched the PR's crates, as `/review-pr` had it do
    /// once it had read them (`@sealed_review_fetch`), so it does again.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub sealed_fetch: bool,
    /// Its last screen's file in `closed-screens/`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub screen: Option<String>,
    /// Its note (`@tab_note`), a file name in the notes directory, which
    /// its menu opens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
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
    /// Its Claude's last prompt, what became of its work, how many panes it
    /// has and the worktree it reviewed sealed, as `Closing` keeps them.
    pub prompt: String,
    pub outcome: String,
    pub panes: usize,
    pub opened: Option<i64>,
    pub sealed: Option<PathBuf>,
    pub sealed_fetch: bool,
    pub note: Option<String>,
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
        // The same conversation keeps its last prompt while it can't be
        // read (its Claude quit, the sidebar's read of it gone first).
        if now.prompt.is_empty()
            && let Some(was) = was
            && was.now.session == now.session
        {
            now.prompt = was.now.prompt.clone();
        }
        // Once its panes are gone as it closes, what became of its work
        // and its pane count are what they were last seen; while it has
        // panes, an outcome gone (its checks passing again) is gone.
        if let Some(was) = was {
            if now.outcome.is_empty() && now.panes == 0 {
                now.outcome = was.now.outcome.clone();
            }
            if now.panes == 0 {
                now.panes = was.now.panes;
            }
            if now.opened.is_none() {
                now.opened = was.now.opened;
            }
            if now.note.is_none() {
                now.note = was.now.note.clone();
            }
            // A sealed review stays one for its own conversation, not for
            // work the tab was handed since.
            if now.sealed.is_none() && was.now.session == now.session {
                now.sealed = was.now.sealed.clone();
                now.sealed_fetch = was.now.sealed_fetch;
            }
        }
        self.last.insert(now.tab, Seen { now, since });
    }

    /// A tab is being taken out: what it was, as last seen, goes in the
    /// log, with its last screen kept beside it when there is one.
    pub fn closed(&mut self, state: &Path, tab: TabId, at: i64, shot: Option<&super::shots::Shot>) {
        let Some(was) = self.last.remove(&tab) else {
            return;
        };
        let lived = at - was.since;
        let screen = shot.filter(|_| lived >= BRIEF_MS).map(|shot| {
            let name = format!("{at}-{}.json", tab.0);
            super::shots::write(state, &name, shot);
            name
        });
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
            lived,
            prompt: was.now.prompt,
            outcome: was.now.outcome,
            panes: was.now.panes,
            opened: was.now.opened.unwrap_or(at - lived),
            sealed: was.now.sealed,
            sealed_fetch: was.now.sealed_fetch,
            screen,
            note: was.now.note,
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
        let gone: Vec<String> = events.drain(..over).filter_map(|e| e.screen).collect();
        for name in gone {
            super::shots::remove(state, &name);
        }
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

    /// The closings the Reopen view lists, each with its key (its Claude
    /// session, else `dir:<path>`): the tabs closed most recently first,
    /// one per Claude conversation, leaving out one a tab has open again,
    /// so several tabs closed in the same repository are each listed; a tab
    /// with no conversation, one per directory no tab is open in. A tab that
    /// came and went within seconds is left out, and so is one removed from
    /// the view, up to the time it closed (`remove`), so it comes back only
    /// if it is closed again.
    pub fn listed(&mut self, state: &Path, open: &[OpenTab]) -> Vec<(String, Closing)> {
        let removed = removed(state);
        let mut listed: HashSet<String> = HashSet::new();
        let events = self.load(state).clone();
        events
            .into_iter()
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
                Some((key, e))
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

    /// Removes a closed tab or a past session from the view, up to `at`.
    /// Entries for closings no longer in the log go with it, once they are
    /// older than any session the view lists too.
    pub fn remove(&mut self, state: &Path, key: &str, at: i64) {
        let oldest = self.load(state).first().map_or(0, |e| e.at);
        let sessions_from = util::now_ms() - KEEP_REMOVED_MS;
        let mut saved: Map<String, Value> = util::read_json_object(removed_file(state));
        saved.insert(key.to_string(), Value::from(at));
        saved.retain(|_, v| {
            v.as_f64()
                .is_some_and(|t| t as i64 >= oldest.min(sessions_from))
        });
        let _ = util::write_atomic(
            removed_file(state),
            util::json_compact(&Value::Object(saved)),
        );
    }
}

/// How long a past session removed from the view is remembered: longer
/// than `wt-revive --sessions` lists one.
const KEEP_REMOVED_MS: i64 = 15 * 24 * 60 * 60 * 1000;

fn is_zero(n: &i64) -> bool {
    *n == 0
}

/// What was removed from the view, each key up to when.
pub fn removed(state: &Path) -> HashMap<String, i64> {
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
            prompt: String::new(),
            outcome: String::new(),
            panes: 1,
            opened: None,
            sealed: None,
            sealed_fetch: false,
            note: None,
        }
    }

    fn open(path: &str, session: Option<&str>) -> OpenTab {
        OpenTab {
            path: Some(path.into()),
            session: session.map(str::to_string),
        }
    }

    fn names(listed: Vec<(String, Closing)>) -> Vec<String> {
        listed.into_iter().map(|(_, c)| c.name).collect()
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
        closed.closed(&state, TabId(83), now + 1, None);
        closed.closed(&state, TabId(82), now + 2, None);
        closed.closed(&state, TabId(80), now + 3, None);
        closed.closed(&state, TabId(81), now + 4, None);
        let listed = closed.listed(&state, &[]);
        assert_eq!(
            listed
                .iter()
                .map(|(k, c)| (k.as_str(), c.name.as_str()))
                .collect::<Vec<_>>(),
            vec![
                ("dir:/work/scratch", "scratch"),
                ("1234-abcd", "ABC-1: Feat"),
                ("9999", "ABC-1 older"),
            ],
            "each conversation, two in one directory included, and not the tool's tab"
        );
        assert_eq!(
            resume_line(listed[1].1.session.as_deref()).as_deref(),
            Some("claude --resume 1234-abcd\r")
        );
        assert_eq!(resume_line(listed[0].1.session.as_deref()), None);
        assert_eq!(
            names(closed.listed(&state, &[open("/work/scratch", None)])),
            vec!["ABC-1: Feat", "ABC-1 older"],
            "not a shell's directory open again"
        );
        assert_eq!(
            names(closed.listed(&state, &[open("/work/abc-1", Some("9999"))])),
            vec!["scratch", "ABC-1: Feat"],
            "not a conversation open again, though a tab in its directory is no reason"
        );
        // Removing one leaves out older closings of the same conversation too.
        closed.remove(&state, "1234-abcd", now + 3);
        assert_eq!(
            names(closed.listed(&state, &[])),
            vec!["scratch", "ABC-1 older"]
        );
        // The log reads back in a new server.
        let mut fresh = Closed::default();
        assert_eq!(fresh.listed(&state, &[]).len(), 2);
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
        thread.prompt = "read the thread".into();
        thread.outcome = "changes asked".into();
        thread.panes = 2;
        thread.opened = Some(now - 3_600_000);
        closed.seen(thread.clone(), now - 60_000);
        closed.seen(
            TabNow {
                session: None,
                prompt: String::new(),
                outcome: String::new(),
                panes: 0,
                opened: None,
                ..thread.clone()
            },
            now - 30_000,
        );
        // Its prompt gone from the sidebar before its session is.
        closed.seen(
            TabNow {
                prompt: String::new(),
                panes: 0,
                opened: None,
                ..thread.clone()
            },
            now - 20_000,
        );
        closed.closed(&state, TabId(84), now, None);
        let mut failed = tab(86, "fixed", "/work/fixed", Some("8686"));
        failed.outcome = "tests failed".into();
        closed.seen(failed.clone(), now - 60_000);
        closed.seen(
            TabNow {
                outcome: String::new(),
                ..failed
            },
            now - 30_000,
        );
        closed.closed(&state, TabId(86), now, None);
        assert_eq!(
            closed.find(&state, "8686").map(|c| c.outcome),
            Some(String::new()),
            "its checks passing again, with its panes there, clear it"
        );
        let found = closed.find(&state, "5555").unwrap();
        assert_eq!(found.name, "Slack: a thread");
        assert_eq!(found.session.as_deref(), Some("5555"));
        assert_eq!(found.tab_name.as_deref(), Some("Slack: a thread"));
        assert_eq!(
            (found.prompt.as_str(), found.outcome.as_str(), found.panes),
            ("read the thread", "changes asked", 2),
            "what was known of it before its Claude quit and its panes went"
        );
        assert_eq!(
            found.opened,
            now - 3_600_000,
            "when it opened, not when first seen"
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
            sealed: Some("/cache/pr-review/acme-widgets/7".into()),
            ..tab(85, "#7 Review PR", "/work/widgets", Some("7777"))
        };
        closed.seen(review.clone(), now - 60_000);
        // Its directory can't be read as it closes (the worktree gone).
        closed.seen(
            TabNow {
                path: None,
                sealed: None,
                ..review
            },
            now,
        );
        closed.closed(&state, TabId(85), now, None);
        let found = closed.find(&state, "7777").unwrap();
        assert_eq!(found.path, Some("/work/widgets".into()));
        assert_eq!(
            found.tab_dir,
            Some("/cache/pr-review/acme-widgets/7".into())
        );
        assert_eq!(found.sealed, found.tab_dir, "the sealed review it was");
        // Handed to other work, its new conversation is no sealed review.
        let handed = TabNow {
            sealed: Some("/cache/pr-review/acme-widgets/8".into()),
            ..tab(87, "#8 Review PR", "/work/widgets", Some("8888"))
        };
        closed.seen(handed.clone(), now - 60_000);
        closed.seen(
            TabNow {
                sealed: None,
                session: Some("9191".into()),
                ..handed
            },
            now - 30_000,
        );
        closed.closed(&state, TabId(87), now, None);
        assert_eq!(closed.find(&state, "9191").and_then(|c| c.sealed), None);
        let _ = fs::remove_dir_all(&state);
    }

    #[test]
    fn a_log_the_node_version_wrote_reads_with_the_new_fields_empty() {
        let state = temp("node");
        fs::write(
            state.join("closed.jsonl"),
            "{\"at\":1700000000000,\"id\":\"@3\",\"index\":2,\"name\":\"api\",\"kind\":\"closed\",\"path\":\"/w/api\",\"session\":\"ab-1\",\"lived\":120000}\n",
        )
        .unwrap();
        let mut closed = Closed::default();
        let listed = closed.listed(&state, &[]);
        assert_eq!(listed.len(), 1);
        let (key, c) = &listed[0];
        assert_eq!(key, "ab-1");
        assert_eq!(
            (
                c.prompt.as_str(),
                c.outcome.as_str(),
                c.panes,
                &c.sealed,
                &c.screen
            ),
            ("", "", 0, &None, &None)
        );
        let _ = fs::remove_dir_all(&state);
    }

    #[test]
    fn a_closing_keeps_its_last_screen_until_it_leaves_the_log() {
        let state = temp("screens");
        let mut closed = Closed::default();
        let now = util::now_ms();
        let shot = super::super::shots::Shot {
            cols: 4,
            lines: vec![super::super::shots::ShotLine {
                runs: vec![(2, Some([1, 2, 3]))],
                text: "hi".into(),
            }],
        };
        for n in 0..=MAX as u32 {
            closed.seen(
                tab(n, &format!("t{n}"), &format!("/w/{n}"), None),
                now - 60_000,
            );
            closed.closed(&state, TabId(n), now + n as i64, Some(&shot));
        }
        let first = format!("{now}-0.json");
        let last = format!("{}-{MAX}.json", now + MAX as i64);
        assert_eq!(
            closed
                .find(&state, &format!("dir:/w/{MAX}"))
                .and_then(|c| c.screen),
            Some(last.clone())
        );
        assert_eq!(super::super::shots::read(&state, &last), Some(shot));
        assert_eq!(
            super::super::shots::read(&state, &first),
            None,
            "the closing that left the log took its screen with it"
        );
        let _ = fs::remove_dir_all(&state);
    }
}
