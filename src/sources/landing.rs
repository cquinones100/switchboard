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
//! worktree alone. The sessions come from Claude Code's session files and a
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
use crate::mux::TabId;
use crate::util;

/// A worktree holding the landing lock or waiting for it, since when (ms),
/// and the Claude session and pane that took it or joined, which an entry
/// from an older build doesn't name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub path: PathBuf,
    pub since: i64,
    pub session: Option<String>,
    pub pane: Option<String>,
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
}

/// What one read of the landing queues found: each tab's place, each
/// repository the tabs are in, in the order their tabs first come, and the
/// worktree each tab works in.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Read {
    pub places: HashMap<TabId, Place>,
    pub repos: Vec<RepoLanding>,
    pub roots: HashMap<TabId, PathBuf>,
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
            // A queue's mutex; no branch's name ends in `.lock`.
            (!rest.ends_with(".lock")).then(|| crate::cmd::land_lock::branch_of_suffix(rest))
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
    let holder = util::read_json(common.join(named("land.lock")).join("holder"))
        .as_ref()
        .and_then(entry_of)
        .filter(live);
    let waiting = util::read_json(common.join(named("land.queue")))
        .and_then(|q| q.as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(entry_of)
        .filter(live)
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
    }
}

/// Each tab's place in its repository's landing queue, from the first of
/// its directories in a git repository, given the running sessions, and
/// each repository the tabs are in with what lands there.
/// Each repository's files are read once.
pub fn landing_of(tabs: &[(TabId, Vec<PathBuf>)], running: &[RunningSession]) -> Read {
    let mut repos: Vec<RepoLanding> = Vec::new();
    let mut places = HashMap::new();
    let mut roots = HashMap::new();
    for (tab, dirs) in tabs {
        let Some((root, common)) = dirs.iter().find_map(|d| git_of(d)) else {
            continue;
        };
        roots.insert(*tab, root.clone());
        if !repos.iter().any(|r| r.common == common) {
            repos.push(repo_landing(&root, &common, None, running));
            for target in targets_in(&common) {
                repos.push(repo_landing(&root, &common, Some(&target), running));
            }
        }
        // The queue the tab's worktree is in, whichever branch it lands on,
        // else the default branch's.
        let mine = repos.iter().filter(|r| r.common == common);
        let at = mine
            .clone()
            .find(|r| {
                r.holder.as_ref().is_some_and(|e| e.path == root)
                    || r.waiting.iter().any(|e| e.path == root)
            })
            .or_else(|| mine.clone().next())
            .expect("the repository's queues were read above");
        let holder = at.holder.as_ref().map(|e| e.path.clone());
        let queue: Vec<PathBuf> = at.waiting.iter().map(|e| e.path.clone()).collect();
        if holder.as_deref() == Some(root.as_path()) {
            places.insert(
                *tab,
                Place {
                    landing: Landing {
                        holding: true,
                        line: 1,
                        method: Method::Direct,
                    },
                    waiter: None,
                },
            );
        } else if let Some(at) = queue.iter().position(|p| *p == root) {
            let line = at as u32 + if holder.is_some() { 2 } else { 1 };
            places.insert(
                *tab,
                Place {
                    landing: Landing {
                        holding: false,
                        line,
                        method: Method::Direct,
                    },
                    waiter: Some(Waiter {
                        root,
                        first: at == 0,
                        last: at + 1 == queue.len(),
                    }),
                },
            );
        }
    }
    Read {
        places,
        repos,
        roots,
    }
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
}

fn header(id: &str, title: &str, count: usize) -> Item {
    Item::new(format!("header:{id}"), title)
        .data(serde_json::json!({ "header": true, "count": count }))
}

/// The Landing view's rows: for each repository with something landing, a
/// section for its landing queue, the landing one first (the worktree's
/// folder when no tab has it), and one for its merge queue, `queued` giving
/// a repository's merge queue rows (`queues::queue_items`) by its
/// `owner/name`. Two clones of one repository show its merge queue once,
/// with the first.
pub fn view_items(
    repos: &[RepoLanding],
    queued: &dyn Fn(&str) -> Vec<Item>,
    tabs: &[ViewTab],
    roots: &HashMap<TabId, PathBuf>,
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
            let (way, id) = match &r.target {
                Some(target) => (
                    format!("{} → {target}", r.name),
                    format!("{}:direct:{target}", r.common.display()),
                ),
                None => (
                    format!(
                        "{} · straight to {}",
                        r.name,
                        r.branch.as_deref().unwrap_or("the default branch")
                    ),
                    format!("{}:direct", r.common.display()),
                ),
            };
            items.push(header(&id, &way, direct.len()));
            let ahead = usize::from(r.holder.is_some());
            for (i, (entry, holding)) in direct.iter().enumerate() {
                let tab = tabs.iter().find(|t| roots.get(&t.id) == Some(&entry.path));
                let name = match tab {
                    Some(t) => t.name.clone(),
                    None => entry
                        .path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default(),
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
            items.push(header(
                &format!("{}:queue", r.common.display()),
                &format!("{} · merge queue", r.name),
                queue.len(),
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
            (TabId(1), vec![scratch.0.clone()]),
            (TabId(2), vec![two.join("sub")]),
            (TabId(3), vec![one.clone()]),
            (TabId(4), vec![three.clone()]),
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
            &[(TabId(3), vec![scratch.0.join("nowhere"), one.clone()])],
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
            },
            ViewTab {
                id: TabId(2),
                index: 2,
                name: "waiter".into(),
                phase: None,
                waiting: true,
            },
        ];
        let roots: HashMap<TabId, PathBuf> = [(TabId(3), one.clone()), (TabId(2), two.clone())]
            .into_iter()
            .collect();
        let none = |_: &str| Vec::new();
        let items = view_items(&read.repos, &none, &view_tabs, &roots);
        let rows: Vec<(&str, &str, State)> = items
            .iter()
            .map(|i| (i.title.as_str(), i.detail.as_str(), i.state))
            .collect();
        assert_eq!(
            rows,
            vec![
                ("repo · straight to the default branch", "", State::Idle),
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
        fs::create_dir_all(common.join("land.queue.team%2Fnext.lock")).unwrap();
        let tabs = vec![
            (TabId(1), vec![one.clone()]),
            (TabId(2), vec![two.clone()]),
            (TabId(3), vec![three.clone()]),
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
            })
            .into();
        let items = view_items(&read.repos, &|_| Vec::new(), &view_tabs, &read.roots);
        let titles: Vec<&str> = items.iter().map(|i| i.title.as_str()).collect();
        assert_eq!(
            titles,
            vec![
                "repo · straight to the default branch",
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
            let tabs = vec![(TabId(1), vec![one.clone()])];
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
