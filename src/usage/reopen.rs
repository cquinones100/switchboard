//! Mission control's Reopen view (board 18): the tabs closed and the past
//! Claude sessions no tab runs, one row a conversation, newest first under
//! "Today", "Earlier this week" and "Older". It took the place of the
//! Closed and Sessions views, with no tmux-version file behind it beyond
//! lib/sidebar/closed.js, which the closings come from (`closed`).
//!
//! A closed tab wins over its own session, which only lends it how many
//! prompts it had. A session comes from `wt-revive --sessions` (two weeks
//! of transcripts, src/cmd/wt_revive.rs), leaving out one a tab runs, one a
//! closed tab listed has, and one removed from the view since it was last
//! used. Each row carries what the view draws in its data's `reopen`: its
//! kind, directory, last prompt, what became of its work, its panes, its
//! sealed review, its last screen's file, when it was opened and closed
//! and its turns, so drawing reads nothing.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::closed::Closing;
use crate::cmd::wt_revive::SessionRow;
use crate::items::{Item, Target};
use crate::paths;

/// Which rows the view shows (its menu's "Show only tabs", "Show only
/// sessions" and "Show both").
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Kind {
    #[default]
    Both,
    Tabs,
    Sessions,
}

/// A row's kind, as its chip says it.
pub const TAB: &str = "tab";
pub const SESSION: &str = "session";

/// The headings, newest first.
const TODAY: &str = "Today";
const THIS_WEEK: &str = "Earlier this week";
const OLDER: &str = "Older";
const WEEK_MS: i64 = 7 * 24 * 60 * 60 * 1000;

/// The view's rows, under their headings: the closings as
/// `Closed::listed` gives them, the past sessions, and the sessions a tab
/// runs now (to leave out); `removed` is what was removed from the view,
/// each key up to when; `today` the local date of now, `YYYY-MM-DD`, and
/// `day_of` the local date of a time in milliseconds.
pub fn items(
    closings: &[(String, Closing)],
    sessions: &[SessionRow],
    open: &HashSet<String>,
    removed: &HashMap<String, i64>,
    now_ms: i64,
    day_of: &dyn Fn(i64) -> String,
) -> Vec<Item> {
    let turns: HashMap<&str, usize> = sessions.iter().map(|s| (s.id.as_str(), s.turns)).collect();
    let mut rows: Vec<Item> = closings
        .iter()
        .map(|(key, c)| tab_row(key, c, turns.get(key.as_str()).copied(), now_ms))
        .collect();
    let listed: HashSet<&str> = closings.iter().map(|(k, _)| k.as_str()).collect();
    rows.extend(
        sessions
            .iter()
            .filter(|s| !open.contains(&s.id) && !listed.contains(s.id.as_str()))
            .filter(|s| {
                !removed
                    .get(&s.id)
                    .is_some_and(|until| s.when * 1000 <= *until)
            })
            .map(|s| session_row(s, now_ms)),
    );
    rows.sort_by_key(|i| std::cmp::Reverse(i.time.unwrap_or(0)));
    let today = day_of(now_ms);
    let mut out = Vec::new();
    for heading in [TODAY, THIS_WEEK, OLDER] {
        let under: Vec<Item> = rows
            .iter()
            .filter(|i| {
                let at = i.time.unwrap_or(0);
                let is_today = day_of(at) == today;
                match heading {
                    TODAY => is_today,
                    THIS_WEEK => !is_today && now_ms - at < WEEK_MS,
                    _ => !is_today && now_ms - at >= WEEK_MS,
                }
            })
            .cloned()
            .collect();
        if under.is_empty() {
            continue;
        }
        out.push(heading_item(heading, under.len()));
        out.extend(under);
    }
    out
}

fn heading_item(title: &str, count: usize) -> Item {
    Item::new(format!("heading:{title}"), title).data(json!({ "header": true, "count": count }))
}

/// How long ago, as a plain row says it.
fn ago(ms: i64, now_ms: i64) -> String {
    super::sessions::ago(ms.div_euclid(1000), now_ms.div_euclid(1000))
}

/// A plain row's detail (UX 2.0 off): when, what reopening does, where,
/// and what became of the work.
fn detail(parts: &[String]) -> String {
    parts
        .iter()
        .filter(|p| !p.is_empty())
        .cloned()
        .collect::<Vec<_>>()
        .join(" · ")
}

fn tab_row(key: &str, c: &Closing, turns: Option<usize>, now_ms: i64) -> Item {
    let dir = c.path.clone().unwrap_or_default();
    let session = c.session.as_deref().filter(|s| *s == key);
    Item::new(key, c.name.clone())
        .detail(detail(&[
            format!("closed {} ago", ago(c.at, now_ms)),
            if session.is_some() {
                "resumes its Claude".into()
            } else {
                "a shell".into()
            },
            paths::tilde(&dir),
            outcome_of(c),
        ]))
        .time(c.at)
        .target(Target::Dir(dir.clone()))
        .data(json!({
            "search": c.prompt,
            "session": session,
            "tabName": c.tab_name,
            "closed": { "key": key, "at": c.at },
            "reopen": {
                "kind": TAB,
                "dir": paths::tilde(&dir),
                "prompt": c.prompt,
                "outcome": outcome_of(c),
                "panes": c.panes,
                "sealed": c.sealed.as_ref().map(paths::tilde),
                "screen": c.screen,
                "opened": if c.opened > 0 { c.opened } else { c.at - c.lived },
                "closed": c.at,
                "turns": turns,
                "session": session,
            },
        }))
}

/// What became of a closed tab's work, a sealed review saying so first.
fn outcome_of(c: &Closing) -> String {
    if c.sealed.is_some() {
        return "untrusted · resumes sealed".into();
    }
    c.outcome.clone()
}

/// A past session's row, its directory the one it resumes in (the folder
/// Claude finds it from), else where it ended.
fn session_row(s: &SessionRow, now_ms: i64) -> Item {
    let dir = PathBuf::from(if s.resume.is_empty() {
        &s.dir
    } else {
        &s.resume
    });
    let name = dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| dir.to_string_lossy().into_owned());
    Item::new(s.id.clone(), name)
        .detail(detail(&[
            format!("used {} ago", ago(s.when * 1000, now_ms)),
            "a past session".into(),
            paths::tilde(&dir),
        ]))
        .time(s.when * 1000)
        .target(Target::Dir(dir.clone()))
        .data(json!({
            "search": s.prompt,
            "session": s.id,
            "reopen": {
                "kind": SESSION,
                "dir": paths::tilde(&dir),
                "prompt": s.prompt,
                "outcome": "",
                "closed": s.when * 1000,
                "turns": s.turns,
                "session": s.id,
            },
        }))
}

/// A row's `reopen` data.
pub fn of(item: &Item) -> Option<&Value> {
    item.data.get("reopen").filter(|r| r.is_object())
}

/// Whether a row is a session's rather than a closed tab's.
pub fn is_session(item: &Item) -> bool {
    of(item).and_then(|r| r["kind"].as_str()) == Some(SESSION)
}

/// The rows `kind` keeps, a heading with nothing left under it left out
/// and the counts under the rest made again (a `/` filter having left
/// some out too).
pub fn keep(items: &mut Vec<Item>, kind: Kind) {
    items.retain(|i| {
        of(i).is_none()
            || match kind {
                Kind::Both => true,
                Kind::Tabs => !is_session(i),
                Kind::Sessions => is_session(i),
            }
    });
    let mut heading = None;
    for at in 0..items.len() {
        if of(&items[at]).is_none() {
            items[at].data["count"] = json!(0);
            heading = Some(at);
        } else if let Some(h) = heading {
            let n = items[h].data["count"].as_u64().unwrap_or(0);
            items[h].data["count"] = json!(n + 1);
        }
    }
    items.retain(|i| of(i).is_some() || i.data["count"].as_u64().unwrap_or(0) > 0);
}

/// What reopening a row brings back, as its pane says it.
pub fn brings_back(r: &Value) -> String {
    let dir = r["dir"].as_str().unwrap_or("");
    if r["kind"].as_str() == Some(SESSION) {
        return format!("Claude resumed in a new tab in {dir}");
    }
    let claude = if r["session"].is_string() {
        "its Claude resumed"
    } else {
        "a shell"
    };
    if r["sealed"].is_string() {
        return format!(
            "the review's tab in {dir} with {claude}, and the PR's code in a sealed try beside it"
        );
    }
    format!("a tab in {dir}, named as it was, with {claude}")
}

/// The directory a row reopens in.
pub fn dir_of(item: &Item) -> Option<&Path> {
    match &item.target {
        Target::Dir(d) => Some(d.as_path()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_700_000_000_000;
    const DAY: i64 = 86_400_000;

    fn day_of(ms: i64) -> String {
        (ms / DAY).to_string()
    }

    fn closing(key: &str, name: &str, at: i64, outcome: &str) -> (String, Closing) {
        (
            key.into(),
            Closing {
                at,
                name: name.into(),
                kind: "closed".into(),
                path: Some(format!("/w/{name}").into()),
                session: Some(key.into()),
                lived: 3_600_000,
                outcome: outcome.into(),
                panes: 2,
                ..Closing::default()
            },
        )
    }

    fn session(id: &str, dir: &str, when_ms: i64, turns: usize) -> SessionRow {
        SessionRow {
            id: id.into(),
            dir: dir.into(),
            when: when_ms / 1000,
            prompt: format!("prompt of {id}"),
            turns,
            resume: String::new(),
        }
    }

    fn titles(items: &[Item]) -> Vec<String> {
        items.iter().map(|i| i.title.clone()).collect()
    }

    #[test]
    fn a_closed_tab_and_its_own_session_are_one_row_grouped_by_day() {
        let closings = vec![
            closing("s1", "api", NOW - 60_000, "landed a3f1c2e"),
            closing("s2", "web", NOW - 3 * DAY, "tests failed"),
        ];
        let sessions = vec![
            session("s1", "/w/api", NOW - 120_000, 41),
            session("s3", "/w/cli", NOW - 600_000, 5),
            session("s4", "/w/open", NOW - 600_000, 2),
            session("s5", "/w/old", NOW - 10 * DAY, 3),
        ];
        let open: HashSet<String> = ["s4".to_string()].into();
        let items = items(&closings, &sessions, &open, &HashMap::new(), NOW, &day_of);
        assert_eq!(
            titles(&items),
            vec![
                "Today",
                "api",
                "cli",
                "Earlier this week",
                "web",
                "Older",
                "old"
            ]
        );
        assert_eq!(items[0].data["count"], 2);
        let api = of(&items[1]).unwrap();
        assert_eq!(
            (
                api["kind"].as_str(),
                api["turns"].as_u64(),
                api["outcome"].as_str()
            ),
            (Some(TAB), Some(41), Some("landed a3f1c2e")),
            "the tab wins and its session lends it its turns"
        );
        assert_eq!(api["opened"], NOW - 60_000 - 3_600_000);
        assert!(is_session(&items[2]));
        let mut moved = session("s8", "/w/top", NOW - 1000, 1);
        moved.resume = "/w/top/src".into();
        assert_eq!(
            brings_back(of(&session_row(&moved, NOW)).unwrap()),
            "Claude resumed in a new tab in /w/top/src",
            "the folder it resumes in, not where it ended"
        );
        assert!(
            crate::mission::views::search_text(&items[2]).contains("prompt of s3"),
            "a row's prompt is searched"
        );
        assert_eq!(
            brings_back(of(&items[2]).unwrap()),
            "Claude resumed in a new tab in /w/cli"
        );
        assert_eq!(
            brings_back(api),
            "a tab in /w/api, named as it was, with its Claude resumed"
        );
        // Removed since it was last used, a session stays out.
        let removed: HashMap<String, i64> = [("s3".to_string(), NOW - 600_000)].into();
        assert!(
            !titles(&super::items(
                &closings, &sessions, &open, &removed, NOW, &day_of
            ))
            .contains(&"cli".to_string())
        );
    }

    #[test]
    fn a_sealed_review_says_it_resumes_sealed() {
        let (key, mut c) = closing("s9", "review", NOW - 1000, "");
        c.sealed = Some("/cache/pr-review/a-b/7".into());
        let items = items(
            &[(key, c)],
            &[],
            &HashSet::new(),
            &HashMap::new(),
            NOW,
            &day_of,
        );
        let r = of(&items[1]).unwrap();
        assert_eq!(r["outcome"], "untrusted · resumes sealed");
        assert!(brings_back(r).contains("sealed try"));
    }

    #[test]
    fn tabs_only_and_sessions_only_keep_their_rows_and_headings() {
        let closings = vec![closing("s1", "api", NOW - 60_000, "")];
        let sessions = vec![
            session("s3", "/w/cli", NOW - 600_000, 5),
            session("s5", "/w/old", NOW - 10 * DAY, 3),
        ];
        let all = items(
            &closings,
            &sessions,
            &HashSet::new(),
            &HashMap::new(),
            NOW,
            &day_of,
        );
        let mut tabs = all.clone();
        keep(&mut tabs, Kind::Tabs);
        assert_eq!(titles(&tabs), vec!["Today", "api"]);
        assert_eq!(tabs[0].data["count"], 1);
        let mut sessions_only = all.clone();
        keep(&mut sessions_only, Kind::Sessions);
        assert_eq!(titles(&sessions_only), vec!["Today", "cli", "Older", "old"]);
        let mut both = all.clone();
        keep(&mut both, Kind::Both);
        assert_eq!(both, all);
        // A filter leaving one of Today's two, its count says one.
        let mut filtered: Vec<Item> = all.iter().filter(|i| i.title != "cli").cloned().collect();
        keep(&mut filtered, Kind::Both);
        assert_eq!(titles(&filtered), vec!["Today", "api", "Older", "old"]);
        assert_eq!(filtered[0].data["count"], 1);
    }
}
