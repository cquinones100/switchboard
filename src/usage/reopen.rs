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
/// A worktree no tab has open, and the line its merged ones fold under.
pub const WORKTREE: &str = "worktree";
pub const MERGED: &str = "merged";

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
                "note": c.note,
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
                "ended": s.dir,
                "prompt": s.prompt,
                "outcome": "",
                "closed": s.when * 1000,
                "turns": s.turns,
                "session": s.id,
            },
        }))
}

/// The section after the days: the worktrees no tab has open, from the
/// Work map's gathering (`mission::views::work_map`'s rows with no tab),
/// one line each, most recently used first, those whose PR merged behind
/// one fold line (`MERGED`) with their rows after it, which `fold_merged`
/// leaves out while it is folded.
pub fn worktrees(map: &[Item]) -> Vec<Item> {
    let mut rows: Vec<Item> = map
        .iter()
        .filter(|i| !crate::mission::views::is_header(i) && matches!(i.target, Target::Dir(_)))
        .map(worktree_row)
        .collect();
    if rows.is_empty() {
        return rows;
    }
    rows.sort_by_key(|i| std::cmp::Reverse(i.time.unwrap_or(0)));
    let (merged, open): (Vec<Item>, Vec<Item>) = rows.into_iter().partition(is_merged);
    let mut out = vec![heading_item(WORKTREES, open.len())];
    out.extend(open);
    if !merged.is_empty() {
        let n = merged.len();
        let s = if n == 1 { "" } else { "s" };
        let trees: Vec<Value> = merged
            .iter()
            .filter_map(|i| dir_of(i).map(|d| json!(d)))
            .collect();
        out.push(
            Item::new(FOLD_ID, format!("{n} merged worktree{s}"))
                .detail("their PRs merged; no tab has them open")
                .data(json!({
                    "search": "merged",
                    "reopen": { "kind": MERGED, "count": n, "trees": trees },
                })),
        );
        out.extend(merged);
    }
    out
}

/// The section's heading and its fold line's id.
const WORKTREES: &str = "Worktrees with no tab";
pub const FOLD_ID: &str = "merged-worktrees";

fn is_merged(item: &Item) -> bool {
    of(item).is_some_and(|r| r["merged"].as_bool() == Some(true))
}

/// A Work map row as Reopen's: its name, its repository, a chip for where
/// its PR stands and one for its commits (once the branch was counted, as
/// the chosen row's is), and when it was last used.
fn worktree_row(map: &Item) -> Item {
    let d = &map.data;
    let s = |key: &str| d[key].as_str().unwrap_or("").to_string();
    let tree = dir_of(map).map(Path::to_path_buf).unwrap_or_default();
    let name = Some(s("worktree"))
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| map.title.clone());
    let repo = Path::new(&s("repo"))
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let merged = d["merged"].as_bool() == Some(true);
    let pr = match d["pr"].as_str() {
        Some(n) => {
            let n = n.trim_start_matches('#');
            if !s("checks").is_empty() && s("checksTone") == "failed" {
                (format!("PR {n} · {}", s("checks")), "failed")
            } else {
                let standing = s("standing").to_lowercase();
                let tone = d["standingTone"].as_str().unwrap_or("dim");
                let tone = if tone == "text" { "dim" } else { tone };
                (format!("PR {n} · {standing}"), tone)
            }
        }
        None if s("standing") == "no PR" => ("no PR".into(), "dim"),
        None => (String::new(), "dim"),
    };
    let counts = &d["counts"];
    let plural = |n: u64| if n == 1 { "" } else { "s" };
    let commits = if counts.is_object() {
        match (counts["toPush"].as_u64(), counts["ahead"].as_u64()) {
            (Some(n), _) if n > 0 => (format!("{n} commit{} to push", plural(n)), "needs"),
            (_, Some(n)) if n > 0 => (format!("{n} commit{} ahead", plural(n)), "dim"),
            // A count git couldn't make says nothing.
            (Some(0), _) | (_, Some(0)) => ("nothing to push".into(), "dim"),
            _ => (String::new(), "dim"),
        }
    } else {
        (String::new(), "dim")
    };
    let when = d["when"].as_i64().unwrap_or(0) * 1000;
    let prompt = s("prompt");
    let place = paths::tilde(&tree);
    Item::new(format!("worktree:{}", tree.display()), name)
        .detail(detail(&[
            repo.clone(),
            pr.0.clone(),
            commits.0.clone(),
            Some(s("used"))
                .filter(|u| !u.is_empty())
                .map(|u| format!("used {u} ago"))
                .unwrap_or_default(),
            place.clone(),
        ]))
        .time(when)
        .target(Target::Dir(tree.clone()))
        .data(json!({
            "search": format!("{} {} {prompt}", s("branch"), repo),
            "reopen": {
                "kind": WORKTREE,
                "dir": place,
                "repo": repo,
                "branch": d["branch"],
                "prompt": prompt,
                "pr": pr.0,
                "prTone": pr.1,
                "commits": commits.0,
                "commitsTone": commits.1,
                "ahead": counts["ahead"],
                "default": counts["default"],
                "merged": merged,
                "closed": when,
            },
        }))
}

/// Leaves out the merged worktrees while their fold line is folded.
pub fn fold_merged(items: &mut Vec<Item>, shown: bool) {
    if !shown {
        items.retain(|i| !is_merged(i));
    }
    for fold in items.iter_mut().filter(|i| i.id == FOLD_ID) {
        fold.data["reopen"]["shown"] = json!(shown);
    }
}

/// Marks each past session that ended in a worktree with its PR's address
/// when one is known (`pr_of` for a worktree no tab has open, else the
/// address `tabbed` gives a worktree a tab has open), and, when no tab has
/// that worktree open, with the worktree, which its menu offers to remove.
pub fn mark_sessions(
    items: &mut [Item],
    tabbed: &[(PathBuf, Option<String>)],
    pr_of: &dyn Fn(&Path) -> Option<String>,
) {
    let untabbed: Vec<PathBuf> = items
        .iter()
        .filter(|i| of(i).is_some_and(|r| r["kind"] == WORKTREE))
        .filter_map(|i| dir_of(i).map(Path::to_path_buf))
        .collect();
    let trees: Vec<PathBuf> = untabbed
        .iter()
        .cloned()
        .chain(tabbed.iter().map(|(t, _)| t.clone()))
        .collect();
    // Where the session ended, not the folder it resumes from (a session
    // moved into a worktree resumes from the main clone).
    for item in items.iter_mut().filter(|i| is_session(i)) {
        let Some(dir) = item.data["reopen"]["ended"]
            .as_str()
            .filter(|d| !d.is_empty())
            .map(PathBuf::from)
            .or_else(|| dir_of(item).map(Path::to_path_buf))
        else {
            continue;
        };
        let Some(tree) = trees
            .iter()
            .filter(|t| dir.starts_with(t))
            .max_by_key(|t| t.as_os_str().len())
        else {
            continue;
        };
        // Its diff in the worktree it ended in, which git still lists.
        item.data["reopen"]["diffDir"] = json!(tree);
        let url = if untabbed.contains(tree) {
            item.data["reopen"]["worktree"] = json!(tree);
            pr_of(tree)
        } else {
            tabbed
                .iter()
                .find(|(t, _)| t == tree)
                .and_then(|(_, url)| url.clone())
        };
        if let Some(url) = url {
            item.data["reopen"]["prUrl"] = json!(url);
        }
    }
}

/// A row's `reopen` data.
pub fn of(item: &Item) -> Option<&Value> {
    item.data.get("reopen").filter(|r| r.is_object())
}

/// A row's kind: `TAB`, `SESSION`, `WORKTREE` or `MERGED`.
pub fn kind_of(item: &Item) -> &str {
    of(item).and_then(|r| r["kind"].as_str()).unwrap_or("")
}

/// Whether a row is a session's rather than a closed tab's.
pub fn is_session(item: &Item) -> bool {
    kind_of(item) == SESSION
}

/// The rows `kind` keeps, a heading with nothing left under it left out
/// and the counts under the rest made again (a `/` filter having left
/// some out too).
pub fn keep(items: &mut Vec<Item>, kind: Kind) {
    items.retain(|i| {
        of(i).is_none()
            || match kind {
                Kind::Both => true,
                Kind::Tabs => kind_of(i) == TAB,
                Kind::Sessions => is_session(i),
            }
    });
    let mut heading = None;
    for at in 0..items.len() {
        if of(&items[at]).is_none() {
            items[at].data["count"] = json!(0);
            heading = Some(at);
        } else if kind_of(&items[at]) == MERGED {
            continue;
        } else if let Some(h) = heading {
            let n = items[h].data["count"].as_u64().unwrap_or(0);
            items[h].data["count"] = json!(n + 1);
        }
    }
    // A heading over nothing goes (the worktrees' over a fold line has
    // the fold line under it).
    let over_rows: Vec<bool> = (0..items.len())
        .map(|at| items.get(at + 1).is_some_and(|next| of(next).is_some()))
        .collect();
    let mut at = 0;
    items.retain(|i| {
        at += 1;
        of(i).is_some() || over_rows[at - 1]
    });
}

/// What reopening a row brings back, as its pane says it.
pub fn brings_back(r: &Value) -> String {
    let dir = r["dir"].as_str().unwrap_or("");
    if r["kind"].as_str() == Some(SESSION) {
        return format!("Claude resumed in a new tab in {dir}");
    }
    if r["kind"].as_str() == Some(WORKTREE) {
        return format!("a tab in {dir} with its last Claude session resumed, or a new one");
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

    /// A Work map row of a worktree no tab has open.
    fn map_row(tree: &str, pr: Option<&str>, standing: &str, merged: bool, when: i64) -> Item {
        Item::new(tree, "title")
            .target(Target::Dir(tree.into()))
            .data(json!({
                "worktree": Path::new(tree).file_name().unwrap().to_string_lossy(),
                "repo": "/code/web-app",
                "branch": "feat",
                "pr": pr,
                "standing": standing,
                "standingTone": "done",
                "checks": "",
                "checksTone": "faint",
                "merged": merged,
                "when": when,
                "used": "3d",
                "prompt": "draw it",
                "counts": if merged { Value::Null } else { json!({ "default": "main", "ahead": 4, "toPush": 2 }) },
            }))
    }

    #[test]
    fn worktrees_with_no_tab_follow_the_days_the_merged_ones_folded() {
        let map = vec![
            Item::new("repo:/code/web-app", "web-app").data(json!({ "header": true })),
            map_row("/w/rate-limits", Some("#12345"), "Approved", false, 200),
            map_row("/w/condensed", None, "no PR", false, 300),
            map_row("/w/retry", Some("#12345"), "Merged", true, 100),
            map_row("/w/mute", Some("#81"), "Merged", true, 50),
        ];
        let mut items = worktrees(&map);
        assert_eq!(
            titles(&items),
            vec![
                "Worktrees with no tab",
                "condensed",
                "rate-limits",
                "2 merged worktrees",
                "retry",
                "mute"
            ],
            "most recently used first, the merged ones behind their fold line"
        );
        let rate = of(&items[2]).unwrap();
        assert_eq!(
            (rate["pr"].as_str(), rate["prTone"].as_str()),
            (Some("PR 12345 · approved"), Some("done"))
        );
        assert_eq!(rate["commits"], "2 commits to push");
        assert_eq!(of(&items[1]).unwrap()["pr"], "no PR");
        assert_eq!(of(&items[3]).unwrap()["count"], 2);
        assert_eq!(
            brings_back(rate),
            "a tab in /w/rate-limits with its last Claude session resumed, or a new one"
        );
        let mut folded = items.clone();
        fold_merged(&mut folded, false);
        keep(&mut folded, Kind::Both);
        assert_eq!(
            titles(&folded),
            vec![
                "Worktrees with no tab",
                "condensed",
                "rate-limits",
                "2 merged worktrees"
            ]
        );
        assert_eq!(folded[0].data["count"], 2, "the fold line isn't counted");
        fold_merged(&mut items, true);
        keep(&mut items, Kind::Both);
        assert_eq!(items.len(), 6);
        assert_eq!(of(&items[3]).unwrap()["shown"], true);
        keep(&mut items, Kind::Sessions);
        assert!(items.is_empty(), "only sessions leaves no worktree");
    }

    #[test]
    fn a_session_in_a_worktree_no_tab_has_open_can_remove_it() {
        let mut items = items(
            &[],
            &[
                session("s1", "/w/rate-limits/src", NOW - 1000, 3),
                session("s2", "/w/other", NOW - 2000, 1),
            ],
            &HashSet::new(),
            &HashMap::new(),
            NOW,
            &day_of,
        );
        items.extend(worktrees(&[map_row(
            "/w/rate-limits",
            None,
            "no PR",
            false,
            1,
        )]));
        let tabbed = vec![(
            PathBuf::from("/w/other"),
            Some("https://github.com/o/r/pull/2".to_string()),
        )];
        mark_sessions(&mut items, &tabbed, &|_| {
            Some("https://github.com/o/r/pull/1".into())
        });
        let s1 = items.iter().find(|i| i.id == "s1").unwrap();
        assert_eq!(of(s1).unwrap()["worktree"], "/w/rate-limits");
        assert_eq!(of(s1).unwrap()["prUrl"], "https://github.com/o/r/pull/1");
        let s2 = items.iter().find(|i| i.id == "s2").unwrap();
        assert!(
            of(s2).unwrap()["worktree"].is_null(),
            "a tab has its worktree open"
        );
        assert_eq!(of(s2).unwrap()["prUrl"], "https://github.com/o/r/pull/2");
        // One moved into the worktree resumes from the main clone, and is
        // matched by where it ended.
        let mut moved = session("s3", "/w/rate-limits", NOW - 3000, 2);
        moved.resume = "/code/web-app".into();
        let mut rows = vec![session_row(&moved, NOW)];
        rows.extend(worktrees(&[map_row(
            "/w/rate-limits",
            None,
            "no PR",
            false,
            1,
        )]));
        mark_sessions(&mut rows, &[], &|_| None);
        assert_eq!(dir_of(&rows[0]), Some(Path::new("/code/web-app")));
        assert_eq!(of(&rows[0]).unwrap()["worktree"], "/w/rate-limits");
    }
}
