//! What is landing in each repository a tab is in, ported from
//! lib/sidebar/landing.js: queue-and-clean's "Land on the default branch",
//! which tab's worktree holds the landing lock and which wait for it, in
//! order, read from the files `land-lock` keeps in the main clone's git
//! directory (`land.lock/holder` and `land.queue`); and its merge queue, as
//! `queues` last heard it, joined to it on the server. Found from files
//! alone, with no git process: a worktree's `.git` file names its git
//! directory, whose `commondir` leads to the shared one, whose config names
//! origin and whose `refs/remotes/origin/HEAD` the default branch.
//!
//! Each branch landed on has a lock and queue of its own (src/cmd/
//! land_lock.rs): the default branch's `land.lock` and `land.queue`, and a
//! target branch's `land.lock.<branch>` and `land.queue.<branch>`, found by
//! their names, so a repository is read as one `RepoLanding` per branch.
//!
//! Mission control's Landing view (`view_items`), in place of the Merge
//! queues view, is there only while something lands, a section per
//! repository and way: "<repo> · straight to <branch>" for the default
//! branch and "<repo> → <branch>" for a target branch, each with the tab
//! landing and those waiting, and "<repo> · merge queue" with every PR in
//! it.
//!
//! A holder or waiter counts while its worktree is there and the Claude
//! session that wrote its entry runs, wherever it works, or any session
//! runs in the worktree, as `land-lock`'s `gone()` has it, so a landing that
//! ended without a word leaves the group before the script next reads the
//! queue. An entry from before entries named their session counts by its
//! worktree alone.
//!
//! An entry is filed under the tab whose pane took the lock or joined the
//! queue (its `pane`), else the tab whose Claude runs its `session`, so
//! tabs sharing a directory don't all show as landing. Only an entry from
//! before entries named them goes by its worktree, and only when one tab
//! is in it. The sessions come from Claude Code's session files and a
//! signal 0 to each one's process. A session in a worktree nested inside
//! another (the main clone's `.claude/worktrees`) is that one's, not the
//! outer one's.
//!
//! The server runs this off its task on every tick, since it reads a few
//! files per repository and every session file.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::facts::{Landing, Method};
use crate::items::{Item, State, Target};
use crate::mux::{PaneId, TabId};
use crate::util;

/// A worktree holding the landing lock or waiting for it, since when (ms),
/// the Claude session and pane that took it or joined, which an entry
/// from an older build doesn't name, and the tab `landing_of` found them
/// in, `None` before it looks or when no tab has them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub path: PathBuf,
    pub since: i64,
    pub session: Option<String>,
    pub pane: Option<String>,
    pub tab: Option<TabId>,
    /// What the landing has recorded of itself (`read_record`).
    pub record: Option<Value>,
}

/// A running Claude session: its id and its working directory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunningSession {
    pub session_id: String,
    pub cwd: PathBuf,
}

/// A repository a tab is in, and what lands there straight on one branch:
/// its default branch, or a target branch with a lock and queue of its own.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepoLanding {
    /// Its shared git directory, which the landing lock is in.
    pub common: PathBuf,
    /// `owner/name` on GitHub, from origin.
    pub repo: Option<String>,
    /// What the view calls it: `owner/name`, else its folder's name.
    pub name: String,
    /// The default branch, as `origin/HEAD` names it.
    pub branch: Option<String>,
    /// The branch this queue lands on when it isn't the default one
    /// (`land.queue.<branch>`), `None` for the default branch's.
    pub target: Option<String>,
    pub holder: Option<Entry>,
    pub waiting: Vec<Entry>,
    /// How it lands, `pr` or `direct`, as repos.json or the last answer
    /// worked out says (`repos::landing_by_key`), `None` when neither does.
    pub method: Option<String>,
}

/// What one read of the landing queues found: each tab's place, and each
/// repository the tabs are in, in the order their tabs first come.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Read {
    pub places: HashMap<TabId, Place>,
    pub repos: Vec<RepoLanding>,
}

/// A waiting tab's worktree and whether it is first or last in line, for
/// its menu's moves.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Waiter {
    pub root: PathBuf,
    pub first: bool,
    pub last: bool,
}

/// A tab's place in its repository's landing queue.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Place {
    pub landing: Landing,
    pub waiter: Option<Waiter>,
}

/// A directory's worktree (`root`) and its repository's common git
/// directory, with links resolved, or `None` outside a git repository.
pub fn git_of(dir: &Path) -> Option<(PathBuf, PathBuf)> {
    for at in dir.ancestors() {
        let dot_git = at.join(".git");
        let Ok(meta) = fs::metadata(&dot_git) else {
            continue;
        };
        let root = at.canonicalize().ok()?;
        if meta.is_dir() {
            return Some((root, dot_git.canonicalize().ok()?));
        }
        let text = fs::read_to_string(&dot_git).ok()?;
        let named = text.lines().find_map(|l| l.strip_prefix("gitdir:"))?.trim();
        let gitdir = at.join(named);
        // A submodule or a --separate-git-dir clone has no commondir: its
        // git directory is the shared one.
        let common = match fs::read_to_string(gitdir.join("commondir")) {
            Ok(text) => gitdir.join(text.trim()),
            Err(_) => gitdir,
        };
        return Some((root, common.canonicalize().ok()?));
    }
    None
}

/// A worktree's own git directory (a linked worktree's under the shared
/// one's `worktrees/`, the main clone's `.git`), from its `.git` alone.
pub fn own_git_dir(root: &Path) -> Option<PathBuf> {
    let dot_git = root.join(".git");
    let meta = fs::metadata(&dot_git).ok()?;
    if meta.is_dir() {
        return Some(dot_git);
    }
    let text = fs::read_to_string(&dot_git).ok()?;
    let named = text.lines().find_map(|l| l.strip_prefix("gitdir:"))?.trim();
    Some(root.join(named))
}

/// What a landing has done so far (`land-lock record`): its tests' counts,
/// its review's rounds, its leak check and the tail of its last output,
/// kept in the worktree's own git directory, where nothing commits it and
/// it goes when the worktree does.
pub const RECORD: &str = "landing.json";
/// The whole of that last output, which `o` in the Landing view pages.
pub const OUTPUT: &str = "landing-output.log";

/// The record of the landing in the worktree at `root`, when it has one.
pub fn read_record(root: &Path) -> Option<Value> {
    util::read_json(own_git_dir(root)?.join(RECORD)).filter(Value::is_object)
}

/// The running Claude sessions, from the session files in `dir`
/// (`~/.claude/sessions`) whose process is still there.
pub fn running_sessions(dir: &Path) -> Vec<RunningSession> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| util::read_json(e.path()))
        .filter(|s| {
            s.get("pid")
                .and_then(Value::as_i64)
                .is_some_and(util::is_alive)
        })
        .filter_map(|s| {
            Some(RunningSession {
                session_id: s
                    .get("sessionId")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                cwd: PathBuf::from(s.get("cwd").and_then(Value::as_str)?),
            })
        })
        .collect()
}

// Whether a landing is still there: its worktree is, and the session that
// wrote its entry runs, wherever it works, or a session runs in the
// worktree (not in a worktree nested inside it).
fn live_in(entry: &Entry, running: &[RunningSession]) -> bool {
    let root = entry.path.as_path();
    let its_session_runs = entry
        .session
        .as_deref()
        .is_some_and(|id| running.iter().any(|s| s.session_id == id));
    root.exists()
        && (its_session_runs
            || running.iter().any(|s| {
                s.cwd.starts_with(root) && git_of(&s.cwd).is_some_and(|(at, _)| at == root)
            }))
}

// An entry of the lock's files: its worktree, since when, and the session
// and pane that wrote it.
fn entry_of(value: &Value) -> Option<Entry> {
    let path = value.get("path").and_then(Value::as_str)?;
    let text = |key: &str| {
        value
            .get(key)
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    Some(Entry {
        path: PathBuf::from(path),
        since: value.get("since").and_then(Value::as_f64).unwrap_or(0.0) as i64,
        session: text("session"),
        pane: text("pane"),
        tab: None,
        record: None,
    })
}

// The branches other than the default one that have a landing lock or
// queue in a shared git directory, from their files' names, sorted.
fn targets_in(common: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(common) else {
        return Vec::new();
    };
    let mut targets: Vec<String> = entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let rest = name
                .strip_prefix("land.queue.")
                .or_else(|| name.strip_prefix("land.lock."))?;
            Some(crate::cmd::land_lock::branch_of_suffix(rest))
        })
        .collect();
    targets.sort();
    targets.dedup();
    targets
}

// A repository's landing onto one branch (`target`, `None` for the default
// branch), read from its shared git directory.
fn repo_landing(
    root: &Path,
    common: &Path,
    target: Option<&str>,
    running: &[RunningSession],
) -> RepoLanding {
    let named = |base: &str| match target {
        Some(t) => format!("{base}.{}", crate::cmd::land_lock::file_suffix(t)),
        None => base.to_string(),
    };
    let live = |e: &Entry| live_in(e, running);
    let recorded = |mut e: Entry| {
        e.record = read_record(&e.path);
        e
    };
    let holder = util::read_json(common.join(named("land.lock")).join("holder"))
        .as_ref()
        .and_then(entry_of)
        .filter(live)
        .map(recorded);
    let waiting = util::read_json(common.join(named("land.queue")))
        .and_then(|q| q.as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(entry_of)
        .filter(live)
        .map(recorded)
        .collect();
    let repo = super::repo::origin_of(root).map(|r| r.to_string());
    let folder = if common.file_name().is_some_and(|n| n == ".git") {
        common.parent().unwrap_or(common)
    } else {
        common
    };
    let branch = fs::read_to_string(common.join("refs/remotes/origin/HEAD"))
        .ok()
        .and_then(|t| {
            t.trim()
                .strip_prefix("ref: refs/remotes/origin/")
                .map(str::to_string)
        });
    let method = crate::repos::landing_by_key(
        &repo
            .clone()
            .unwrap_or_else(|| common.to_string_lossy().into_owned()),
    );
    RepoLanding {
        common: common.to_path_buf(),
        name: repo.clone().unwrap_or_else(|| {
            folder
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default()
        }),
        repo,
        branch,
        target: target.map(str::to_string),
        holder,
        waiting,
        method,
    }
}

/// A tab as the landing read sees it: where it works (`TabInfo.dirs`), its
/// panes, and the Claude sessions running in them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TabPlace {
    pub id: TabId,
    pub dirs: Vec<PathBuf>,
    pub panes: Vec<PaneId>,
    pub sessions: Vec<String>,
}

// The tab an entry belongs to: the one holding its pane, else the one whose
// Claude runs its session, else, for an entry from before entries named
// them, the one tab whose worktree is the entry's (none when several tabs
// share it, since any of them may have taken it).
fn tab_of(entry: &Entry, tabs: &[TabPlace], roots: &HashMap<TabId, PathBuf>) -> Option<TabId> {
    if let Some(pane) = entry.pane.as_deref().and_then(|p| p.parse::<PaneId>().ok())
        && let Some(t) = tabs.iter().find(|t| t.panes.contains(&pane))
    {
        return Some(t.id);
    }
    if let Some(session) = entry.session.as_deref()
        && let Some(t) = tabs
            .iter()
            .find(|t| t.sessions.iter().any(|s| s == session))
    {
        return Some(t.id);
    }
    if entry.pane.is_some() || entry.session.is_some() {
        return None;
    }
    let mut there = tabs
        .iter()
        .filter(|t| roots.get(&t.id) == Some(&entry.path));
    let one = there.next()?;
    there.next().is_none().then_some(one.id)
}

/// Each tab's place in a landing queue, given the running sessions, and
/// each repository the tabs are in (by the first of a tab's directories in
/// a git repository) with what lands there. An entry places the tab that
/// took it or joined (`tab_of`), so tabs sharing a directory don't all take
/// its place. Each repository's files are read once.
pub fn landing_of(tabs: &[TabPlace], running: &[RunningSession]) -> Read {
    let mut repos: Vec<RepoLanding> = Vec::new();
    let mut roots = HashMap::new();
    for tab in tabs {
        let Some((root, common)) = tab.dirs.iter().find_map(|d| git_of(d)) else {
            continue;
        };
        if !repos.iter().any(|r| r.common == common) {
            repos.push(repo_landing(&root, &common, None, running));
            for target in targets_in(&common) {
                repos.push(repo_landing(&root, &common, Some(&target), running));
            }
        }
        roots.insert(tab.id, root);
    }
    let mut places = HashMap::new();
    for r in &mut repos {
        if let Some(holder) = &mut r.holder {
            holder.tab = tab_of(holder, tabs, &roots);
            if let Some(tab) = holder.tab {
                places.entry(tab).or_insert(Place {
                    landing: Landing {
                        holding: true,
                        line: 1,
                        method: Method::Direct,
                    },
                    waiter: None,
                });
            }
        }
        let ahead = if r.holder.is_some() { 2 } else { 1 };
        let count = r.waiting.len();
        for (at, entry) in r.waiting.iter_mut().enumerate() {
            entry.tab = tab_of(entry, tabs, &roots);
            let Some(tab) = entry.tab else {
                continue;
            };
            places.entry(tab).or_insert(Place {
                landing: Landing {
                    holding: false,
                    line: at as u32 + ahead,
                    method: Method::Direct,
                },
                waiter: Some(Waiter {
                    root: entry.path.clone(),
                    first: at == 0,
                    last: at + 1 == count,
                }),
            });
        }
    }
    Read { places, repos }
}

/// A worktree's folder name, what the view calls a landing no tab has.
fn folder_of(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// "landing", or "3rd in line to land", for a tab's words.
pub fn landing_words(landing: &Landing) -> String {
    if landing.holding {
        return "landing".into();
    }
    let n = landing.line;
    let suffix = match (n % 10, n % 100) {
        (_, 11..=13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    };
    format!("{n}{suffix} in line to land")
}

/// A tab as the Landing view names it: its id, its place among the tabs,
/// its name, the step its landing is at, and whether it waits on you.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ViewTab {
    pub id: TabId,
    pub index: usize,
    pub name: String,
    pub phase: Option<String>,
    pub waiting: bool,
    /// Autopilot's "Land without asking" is on in it (`@autopilot_land`).
    pub autoland: bool,
}

/// A section's heading: its title, its count, and what UX 2.0 draws at
/// its right edge in place of the count.
fn header(id: &str, title: &str, count: usize, right: &str) -> Item {
    Item::new(format!("header:{id}"), title)
        .data(serde_json::json!({ "header": true, "count": count, "right": right }))
}

/// The Landing view's rows: for each repository with something landing, a
/// section for its landing queue, the landing one first, each named by the
/// tab `landing_of` found its entry in (the worktree's folder when none), and one for its merge queue, `queued` giving
/// a repository's merge queue rows (`queues::queue_items`) by its
/// `owner/name`. Two clones of one repository show its merge queue once,
/// with the first.
pub fn view_items(
    repos: &[RepoLanding],
    queued: &dyn Fn(&str) -> Vec<Item>,
    tabs: &[ViewTab],
) -> Vec<Item> {
    let mut items = Vec::new();
    let mut shown: Vec<String> = Vec::new();
    for r in repos {
        let queue = match &r.repo {
            Some(repo) if !shown.iter().any(|s| s.eq_ignore_ascii_case(repo)) => {
                shown.push(repo.clone());
                queued(repo)
            }
            _ => Vec::new(),
        };
        let direct: Vec<(&Entry, bool)> = r
            .holder
            .iter()
            .map(|e| (e, true))
            .chain(r.waiting.iter().map(|e| (e, false)))
            .collect();
        if !direct.is_empty() {
            let holder_tab = r
                .holder
                .as_ref()
                .and_then(|h| tabs.iter().find(|t| Some(t.id) == h.tab));
            let unasked = if holder_tab.is_some_and(|t| t.autoland) {
                " · lands without asking"
            } else {
                ""
            };
            let (way, id) = match &r.target {
                Some(target) => (
                    format!("{} → {target}{unasked}", r.name),
                    format!("{}:direct:{target}", r.common.display()),
                ),
                None => (
                    format!(
                        "{} · straight on {}{unasked}",
                        r.name,
                        r.branch.as_deref().unwrap_or("the default branch")
                    ),
                    format!("{}:direct", r.common.display()),
                ),
            };
            let right = match &r.holder {
                Some(_) => format!("{} landing · 1 holds the lock", direct.len()),
                None => format!("{} landing", direct.len()),
            };
            items.push(header(&id, &way, direct.len(), &right));
            let holder_name = r.holder.as_ref().map(|h| match holder_tab {
                Some(t) => t.name.clone(),
                None => folder_of(&h.path),
            });
            let ahead = usize::from(r.holder.is_some());
            for (i, (entry, holding)) in direct.iter().enumerate() {
                let tab = tabs.iter().find(|t| Some(t.id) == entry.tab);
                let name = match tab {
                    Some(t) => t.name.clone(),
                    None => folder_of(&entry.path),
                };
                let step = if *holding {
                    match tab.and_then(|t| t.phase.as_deref()) {
                        Some(phase) => format!("landing · {phase}"),
                        None => "landing".into(),
                    }
                } else {
                    "waiting".into()
                };
                let place = tab.map_or("no tab".to_string(), |t| format!("tab {}", t.index));
                let at = i.wrapping_sub(ahead);
                let mut item = Item::new(
                    format!("land:{}", entry.path.display()),
                    format!("{}. {name}", i + 1),
                )
                .detail(format!("{step} · {place}"))
                .state(if tab.is_some_and(|t| t.waiting) {
                    State::Attention
                } else if *holding {
                    State::Active
                } else {
                    State::Idle
                })
                .data(serde_json::json!({
                    "landing": {
                        "root": entry.path,
                        "tab": tab.map(|t| t.id.to_string()),
                        "holding": holding,
                        "first": !holding && at == 0,
                        "last": !holding && at + 1 == r.waiting.len(),
                        "place": (!holding).then(|| at + 1),
                        "phase": tab.and_then(|t| t.phase.clone()),
                        "target": r.target,
                        "record": entry.record,
                        "output": own_git_dir(&entry.path).map(|d| d.join(OUTPUT)),
                        "holder": (!holding).then(|| serde_json::json!({
                            "name": holder_name,
                            "phase": holder_tab.and_then(|t| t.phase.clone()),
                            "since": r.holder.as_ref().map(|h| h.since),
                        })),
                    },
                    "search": format!("{} {}", r.name, entry.path.display()),
                }));
                if let Some(t) = tab {
                    item = item.target(Target::Tab(t.id));
                }
                item.time = (entry.since > 0).then_some(entry.since);
                items.push(item);
            }
        }
        if !queue.is_empty() {
            let way = if r.method.as_deref() == Some("pr") {
                "through a PR · GitHub merge queue"
            } else {
                "merge queue"
            };
            items.push(header(
                &format!("{}:queue", r.common.display()),
                &format!("{} · {way}", r.name),
                queue.len(),
                &format!("{} in the merge queue", queue.len()),
            ));
            items.extend(queue);
        }
    }
    items
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tabs::git::tests::{Scratch, git, repo};

    fn at(id: u32, dirs: Vec<PathBuf>) -> TabPlace {
        TabPlace {
            id: TabId(id),
            dirs,
            panes: Vec::new(),
            sessions: Vec::new(),
        }
    }

    #[test]
    fn the_landing_one_first_then_the_queue_in_order_and_a_gone_waiter_leaves() {
        // lib/sidebar's "tabs landing on the default branch or waiting their
        // turn are a Landing group in the landing queue's order".
        let scratch = Scratch::new("landing");
        let main = scratch.0.join("repo");
        if !repo(&main) {
            eprintln!("skipped: no git");
            return;
        }
        let sessions = scratch.0.join("sessions");
        fs::create_dir_all(&sessions).unwrap();
        let pid = std::process::id();
        let [one, two, three] = ["one", "two", "three"].map(|name| {
            let worktree = scratch.0.join(name);
            assert!(git(
                &main,
                &[
                    "worktree",
                    "add",
                    "-q",
                    "-b",
                    name,
                    worktree.to_str().unwrap()
                ]
            ));
            fs::write(
                sessions.join(format!("landing-{name}.json")),
                serde_json::json!({ "sessionId": name, "cwd": worktree, "pid": pid }).to_string(),
            )
            .unwrap();
            worktree
        });
        let common = main.join(".git");
        fs::create_dir_all(common.join("land.lock")).unwrap();
        fs::write(
            common.join("land.lock/holder"),
            serde_json::json!({ "path": one, "since": 1 }).to_string(),
        )
        .unwrap();
        fs::write(
            common.join("land.queue"),
            serde_json::json!([{ "path": three, "since": 2 }, { "path": two, "since": 3 }])
                .to_string(),
        )
        .unwrap();
        fs::create_dir_all(two.join("sub")).unwrap();
        let tabs = vec![
            at(1, vec![scratch.0.clone()]),
            at(2, vec![two.join("sub")]),
            at(3, vec![one.clone()]),
            at(4, vec![three.clone()]),
        ];
        let running = running_sessions(&sessions);
        let read = landing_of(&tabs, &running);
        let places = &read.places;
        let line = |id: u32| places.get(&TabId(id)).map(|p| p.landing.line);
        assert_eq!(places.get(&TabId(1)), None, "a tab outside the repository");
        assert!(places[&TabId(3)].landing.holding);
        assert_eq!((line(3), line(4), line(2)), (Some(1), Some(2), Some(3)));
        assert_eq!(
            landing_words(&places[&TabId(2)].landing),
            "3rd in line to land",
            "a tab in a worktree's subdirectory included"
        );
        assert_eq!(landing_words(&places[&TabId(3)].landing), "landing");
        let first = places[&TabId(4)].waiter.as_ref().unwrap();
        assert!(
            first.first && !first.last,
            "the first in line moves only down"
        );
        assert_eq!(first.root, three);

        // Notes beside the landing tab and focused: the tab is found by the
        // first of its directories in a repository.
        let beside = landing_of(
            &[at(3, vec![scratch.0.join("nowhere"), one.clone()])],
            &running,
        )
        .places;
        assert!(beside[&TabId(3)].landing.holding);

        // A waiter whose Claude has gone leaves before the script reads the
        // queue again.
        fs::remove_file(sessions.join("landing-three.json")).unwrap();
        // The Landing view: a section for the repository, in the queue's
        // order, the tab each runs in, a worktree no tab has by its folder.
        let view_tabs = vec![
            ViewTab {
                id: TabId(3),
                index: 1,
                name: "lander".into(),
                phase: Some("pushing".into()),
                waiting: false,
                autoland: false,
            },
            ViewTab {
                id: TabId(2),
                index: 2,
                name: "waiter".into(),
                phase: None,
                waiting: true,
                autoland: false,
            },
        ];
        let none = |_: &str| Vec::new();
        let items = view_items(&read.repos, &none, &view_tabs);
        let rows: Vec<(&str, &str, State)> = items
            .iter()
            .map(|i| (i.title.as_str(), i.detail.as_str(), i.state))
            .collect();
        assert_eq!(
            rows,
            vec![
                ("repo · straight on the default branch", "", State::Idle),
                ("1. lander", "landing · pushing · tab 1", State::Active),
                ("2. three", "waiting · no tab", State::Idle),
                ("3. waiter", "waiting · tab 2", State::Attention),
            ]
        );
        assert_eq!(items[2].data["landing"]["first"], true);
        assert_eq!(items[3].data["landing"]["last"], true);
        assert_eq!(items[1].data["landing"]["holding"], true);

        let places = landing_of(&tabs, &running_sessions(&sessions)).places;
        let mut lines: Vec<(u32, u32)> =
            places.iter().map(|(t, p)| (t.0, p.landing.line)).collect();
        lines.sort();
        assert_eq!(lines, vec![(2, 2), (3, 1)]);
    }

    #[test]
    fn a_branch_named_lock_is_a_target_and_no_mutex_is() {
        let scratch = Scratch::new("landing-targets");
        let common = &scratch.0;
        for dir in [
            "land.lock",
            "land.mutex",
            "land.mutex.lock",
            "land.lock.team%2Fnext",
        ] {
            fs::create_dir_all(common.join(dir)).unwrap();
        }
        for file in [
            "land.queue",
            "land.queue.lock",
            "land.tmp.1.land.queue.lock",
        ] {
            fs::write(common.join(file), "[]").unwrap();
        }
        assert_eq!(targets_in(common), ["lock", "team/next"]);
    }

    #[test]
    fn a_target_branch_s_queue_is_a_section_of_its_own_and_places_its_tabs() {
        let scratch = Scratch::new("landing-target");
        let main = scratch.0.join("repo");
        if !repo(&main) {
            eprintln!("skipped: no git");
            return;
        }
        let sessions = scratch.0.join("sessions");
        fs::create_dir_all(&sessions).unwrap();
        let pid = std::process::id();
        let [one, two, three] = ["one", "two", "three"].map(|name| {
            let worktree = scratch.0.join(name);
            assert!(git(
                &main,
                &[
                    "worktree",
                    "add",
                    "-q",
                    "-b",
                    name,
                    worktree.to_str().unwrap()
                ]
            ));
            fs::write(
                sessions.join(format!("target-{name}.json")),
                serde_json::json!({ "sessionId": name, "cwd": worktree, "pid": pid }).to_string(),
            )
            .unwrap();
            worktree
        });
        let common = main.join(".git");
        // The default branch's lock held by one; team/next's by two,
        // three waiting for it. A queue's mutex is no branch.
        fs::create_dir_all(common.join("land.lock")).unwrap();
        fs::write(
            common.join("land.lock/holder"),
            serde_json::json!({ "path": one, "since": 1 }).to_string(),
        )
        .unwrap();
        fs::create_dir_all(common.join("land.lock.team%2Fnext")).unwrap();
        fs::write(
            common.join("land.lock.team%2Fnext/holder"),
            serde_json::json!({ "path": two, "since": 2 }).to_string(),
        )
        .unwrap();
        fs::write(
            common.join("land.queue.team%2Fnext"),
            serde_json::json!([{ "path": three, "since": 3 }]).to_string(),
        )
        .unwrap();
        fs::create_dir_all(common.join("land.mutex")).unwrap();
        fs::create_dir_all(common.join("land.mutex.team%2Fnext")).unwrap();
        let tabs = vec![
            at(1, vec![one.clone()]),
            at(2, vec![two.clone()]),
            at(3, vec![three.clone()]),
        ];
        let read = landing_of(&tabs, &running_sessions(&sessions));
        let targets: Vec<Option<&str>> = read.repos.iter().map(|r| r.target.as_deref()).collect();
        assert_eq!(targets, vec![None, Some("team/next")]);
        let line = |id: u32| {
            let p = &read.places[&TabId(id)].landing;
            (p.holding, p.line)
        };
        assert_eq!(
            (line(1), line(2), line(3)),
            ((true, 1), (true, 1), (false, 2)),
            "each branch's lock held at once, the waiter second in its own queue"
        );

        let view_tabs: Vec<ViewTab> = [(1, "first"), (2, "second"), (3, "third")]
            .map(|(id, name)| ViewTab {
                id: TabId(id),
                index: id as usize,
                name: name.into(),
                phase: None,
                waiting: false,
                autoland: false,
            })
            .into();
        let items = view_items(&read.repos, &|_| Vec::new(), &view_tabs);
        let titles: Vec<&str> = items.iter().map(|i| i.title.as_str()).collect();
        assert_eq!(
            titles,
            vec![
                "repo · straight on the default branch",
                "1. first",
                "repo → team/next",
                "1. second",
                "2. third",
            ]
        );
        assert_eq!(items[4].data["landing"]["target"], "team/next");
        assert_eq!(items[4].data["landing"]["place"], 1);
    }

    #[test]
    fn a_holder_counts_while_its_session_runs_wherever_it_works() {
        let scratch = Scratch::new("landing-session");
        let main = scratch.0.join("repo");
        if !repo(&main) {
            eprintln!("skipped: no git");
            return;
        }
        let one = scratch.0.join("one");
        assert!(git(
            &main,
            &["worktree", "add", "-q", "-b", "one", one.to_str().unwrap()]
        ));
        // The landing's Claude started in the main clone and went into the
        // worktree to land it, so its session file names the clone.
        let sessions = scratch.0.join("sessions");
        fs::create_dir_all(&sessions).unwrap();
        fs::write(
            sessions.join("lander.json"),
            serde_json::json!({ "sessionId": "lander", "cwd": main, "pid": std::process::id() })
                .to_string(),
        )
        .unwrap();
        let common = main.join(".git");
        fs::create_dir_all(common.join("land.lock")).unwrap();
        let held_by = |entry: serde_json::Value| {
            fs::write(common.join("land.lock/holder"), entry.to_string()).unwrap();
            let tabs = vec![TabPlace {
                panes: vec![PaneId(4)],
                ..at(1, vec![one.clone()])
            }];
            let read = landing_of(&tabs, &running_sessions(&sessions));
            (
                read.repos[0].holder.clone(),
                read.places.get(&TabId(1)).cloned(),
            )
        };

        let (holder, place) = held_by(serde_json::json!({
            "path": one, "since": 1, "session": "lander", "pane": "%4",
        }));
        let holder = holder.expect("its session runs, in the main clone");
        assert_eq!(
            (holder.session.as_deref(), holder.pane.as_deref()),
            (Some("lander"), Some("%4"))
        );
        assert!(place.is_some_and(|p| p.landing.holding));

        let (holder, place) = held_by(serde_json::json!({ "path": one, "since": 1 }));
        assert_eq!(
            (holder, place),
            (None, None),
            "an entry from before names no session, and none runs in its worktree"
        );

        let (holder, _) = held_by(serde_json::json!({
            "path": one, "since": 1, "session": "ended",
        }));
        assert_eq!(holder, None, "a session that isn't running keeps nothing");
    }

    #[test]
    fn tabs_sharing_a_directory_take_only_their_own_entry_s_place() {
        let scratch = Scratch::new("landing-shared");
        let main = scratch.0.join("repo");
        if !repo(&main) {
            eprintln!("skipped: no git");
            return;
        }
        // Two tabs in the main clone; the lock held from it.
        let sessions = scratch.0.join("sessions");
        fs::create_dir_all(&sessions).unwrap();
        fs::write(
            sessions.join("lander.json"),
            serde_json::json!({ "sessionId": "lander", "cwd": main, "pid": std::process::id() })
                .to_string(),
        )
        .unwrap();
        let common = main.join(".git");
        fs::create_dir_all(common.join("land.lock")).unwrap();
        let main = main.canonicalize().unwrap();
        let tabs = vec![
            TabPlace {
                panes: vec![PaneId(1)],
                sessions: vec!["other".into()],
                ..at(1, vec![main.clone()])
            },
            TabPlace {
                panes: vec![PaneId(2), PaneId(3)],
                sessions: vec!["lander".into()],
                ..at(2, vec![main.clone()])
            },
        ];
        let landing = |entry: serde_json::Value, tabs: &[TabPlace]| {
            fs::write(common.join("land.lock/holder"), entry.to_string()).unwrap();
            let read = landing_of(tabs, &running_sessions(&sessions));
            let mut holding: Vec<u32> = read
                .places
                .iter()
                .filter(|(_, p)| p.landing.holding)
                .map(|(t, _)| t.0)
                .collect();
            holding.sort();
            (holding, read)
        };

        let (holding, read) = landing(
            serde_json::json!({ "path": main, "since": 1, "session": "lander", "pane": "%3" }),
            &tabs,
        );
        assert_eq!(holding, [2], "the tab holding the entry's pane");
        let view_tabs: Vec<ViewTab> = [(1, "design"), (2, "merge")]
            .map(|(id, name)| ViewTab {
                id: TabId(id),
                index: id as usize,
                name: name.into(),
                phase: None,
                waiting: false,
                autoland: false,
            })
            .into();
        let items = view_items(&read.repos, &|_| Vec::new(), &view_tabs);
        assert_eq!(items[1].title, "1. merge", "the view names the entry's tab");

        let (holding, _) = landing(
            serde_json::json!({ "path": main, "since": 1, "session": "lander" }),
            &tabs,
        );
        assert_eq!(
            holding,
            [2],
            "the tab whose Claude runs the entry's session"
        );

        let (holding, read) = landing(serde_json::json!({ "path": main, "since": 1 }), &tabs);
        assert_eq!(
            holding,
            Vec::<u32>::new(),
            "an entry naming neither, with two tabs in its directory"
        );
        let items = view_items(&read.repos, &|_| Vec::new(), &view_tabs);
        assert_eq!(items[1].title, "1. repo", "named by its folder with no tab");

        let (holding, _) = landing(serde_json::json!({ "path": main, "since": 1 }), &tabs[..1]);
        assert_eq!(holding, [1], "an entry naming neither, with one tab there");
    }

    #[test]
    fn a_heading_says_how_its_repository_lands_with_its_counts_and_a_row_its_record() {
        let entry = |path: &str, tab: u32, record: Option<Value>| Entry {
            path: PathBuf::from(path),
            since: 1,
            session: None,
            pane: None,
            tab: Some(TabId(tab)),
            record,
        };
        let repo = |method: &str| RepoLanding {
            common: PathBuf::from(format!("/{method}/.git")),
            repo: Some(format!("me/{method}")),
            name: format!("me/{method}"),
            branch: Some("main".into()),
            target: None,
            holder: Some(entry(
                "/w/one",
                1,
                Some(serde_json::json!({ "tests": { "passed": 412 } })),
            )),
            waiting: vec![entry("/w/two", 2, None)],
            method: Some(method.into()),
        };
        let tabs: Vec<ViewTab> = [(1, "one", true), (2, "two", false)]
            .map(|(id, name, autoland)| ViewTab {
                id: TabId(id),
                index: id as usize,
                name: name.into(),
                phase: Some("testing".into()),
                waiting: false,
                autoland,
            })
            .into();
        let queued = |_: &str| vec![Item::new("https://pr/1", "1. #1 a PR")];
        let items = view_items(&[repo("direct")], &queued, &tabs);
        assert_eq!(
            items[0].title,
            "me/direct · straight on main · lands without asking"
        );
        assert_eq!(items[0].data["right"], "2 landing · 1 holds the lock");
        assert_eq!(items[1].data["landing"]["record"]["tests"]["passed"], 412);
        assert_eq!(items[2].data["landing"]["holder"]["name"], "one");
        assert_eq!(items[2].data["landing"]["holder"]["phase"], "testing");
        assert_eq!(items[3].title, "me/direct · merge queue");

        let items = view_items(&[repo("pr")], &queued, &tabs);
        let heading = items.iter().find(|i| i.id.ends_with(":queue")).unwrap();
        assert_eq!(heading.title, "me/pr · through a PR · GitHub merge queue");
        assert_eq!(heading.data["right"], "1 in the merge queue");
    }

    #[test]
    fn ordinals() {
        let words = |line| {
            landing_words(&Landing {
                holding: false,
                line,
                method: Method::Direct,
            })
        };
        assert_eq!(words(2), "2nd in line to land");
        assert_eq!(words(11), "11th in line to land");
        assert_eq!(words(21), "21st in line to land");
    }
}
