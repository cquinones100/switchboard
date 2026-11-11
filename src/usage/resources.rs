//! Mission control's Resources view: how much CPU and memory each tab uses,
//! counting every process under its panes (Claude, a dev server, an editor
//! and its language servers, a test run), as a table heaviest first, to tell
//! which tab is slowing the computer down. Ported from
//! lib/sidebar/resources.js.
//!
//! The Node version ran `ps` every `RESOURCES_MS` while a mission control
//! client had the view in front, since macOS gives a process's CPU and
//! memory only through it. Here the kernel is read directly
//! (`procinfo::all_usage`), off the server's task, still only while the view
//! is in front. CPU is the time used between two readings over the time
//! between them, where 100% is one core, as `top` counts it (ps's `%cpu` was
//! a decaying average on macOS and a lifetime one on Linux); memory is
//! resident, so memory two processes share counts in both.
//!
//! New with the port, for mission control's Resources board
//! (src/mission/resources_board.rs): the whole computer's CPU and memory
//! (`procinfo::system`) and Switchboard's own (`own_usage`), each tab's
//! processes kept as a tree, the last two minutes of readings kept
//! (`History`), and the check that a process the menu ends is still the one
//! it offered (`still_under`).

use std::collections::{HashMap, HashSet, VecDeque};
use std::time::{Duration, Instant};

use serde_json::json;

use crate::items::{Column, Item, State, Target, ViewData};
use crate::mux::TabId;
use crate::mux::procinfo::ProcUsage;

/// How often the processes are read again while the view is in front.
pub const RESOURCES_MS: u64 = 3_000;
/// A tab using this much or more needs you: a core pegged, or this much
/// memory.
pub const CPU_HEAVY: f64 = 100.0;
pub const MEMORY_HEAVY: u64 = 4 * 1024 * 1024 * 1024;
/// The gap between the two readings the first look takes, so it has CPU
/// figures from the start.
pub const FIRST_GAP: Duration = Duration::from_millis(250);

/// One process as the view counts it.
#[derive(Clone, Debug, PartialEq)]
pub struct Proc {
    pub pid: u32,
    pub ppid: u32,
    pub name: String,
    /// Percent of one core.
    pub cpu: f64,
    /// Resident bytes.
    pub memory: u64,
}

/// The processes of a reading, with their CPU over the time since the
/// reading before. A process the reading before didn't have, or one whose
/// time went down or whose name changed (its pid reused), counts no CPU
/// until the next reading, since when it started within the gap can't be
/// told.
pub fn between(before: &[ProcUsage], after: &[ProcUsage], elapsed: Duration) -> Vec<Proc> {
    let was: HashMap<u32, &ProcUsage> = before.iter().map(|p| (p.pid, p)).collect();
    let wall = elapsed.as_nanos().max(1) as f64;
    after
        .iter()
        .map(|p| {
            let used = match was.get(&p.pid) {
                Some(b) if b.cpu_ns <= p.cpu_ns && b.name == p.name => p.cpu_ns - b.cpu_ns,
                Some(_) | None => 0,
            };
            Proc {
                pid: p.pid,
                ppid: p.ppid,
                name: p.name.clone(),
                cpu: used as f64 / wall * 100.0,
                memory: p.memory,
            }
        })
        .collect()
}

/// A tab as the view needs it: its number and name, and its panes' own
/// processes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TabPanes {
    pub tab: TabId,
    pub index: usize,
    pub name: String,
    pub pids: Vec<u32>,
}

/// A kind of process in a tab: its name, how many, their CPU and memory.
#[derive(Clone, Debug, PartialEq)]
pub struct Group {
    pub name: String,
    pub count: usize,
    pub cpu: f64,
    pub memory: u64,
}

/// A tab's sums.
#[derive(Clone, Debug, PartialEq)]
pub struct TabUsage {
    pub tab: TabId,
    pub index: usize,
    pub name: String,
    pub count: usize,
    pub cpu: f64,
    pub memory: u64,
    /// Its three heaviest kinds of process, by memory.
    pub heaviest: Vec<Group>,
    /// Its three heaviest kinds of process overall (`weight`'s measure),
    /// for the line under its row on the board.
    pub busiest: Vec<Group>,
    /// Its processes in the order the walk from its panes met them, each
    /// with how deep under its pane it is (a pane's own process at 0).
    pub tree: Vec<(usize, Proc)>,
}

impl TabUsage {
    /// Why the tab needs you, when it does: a core pegged, over
    /// `MEMORY_HEAVY`, or both.
    pub fn why(&self) -> Option<&'static str> {
        match (self.cpu >= CPU_HEAVY, self.memory >= MEMORY_HEAVY) {
            (true, true) => Some("a core pegged, over 4 GB"),
            (true, false) => Some("a core pegged"),
            (false, true) => Some("over 4 GB"),
            (false, false) => None,
        }
    }
}

/// How many readings the history keeps: two minutes at `RESOURCES_MS`.
pub const HISTORY: usize = 40;

/// The last two minutes of readings, for the board's sparklines: each tab's
/// CPU and memory, and the whole computer's CPU (percent of all its
/// cores). A gap longer than `WANT_FOR` (the view put away) starts it over,
/// since it would draw as a flat line.
#[derive(Clone, Debug, Default)]
pub struct History {
    pub tabs: HashMap<TabId, VecDeque<(f64, u64)>>,
    pub system: VecDeque<f64>,
    last: Option<Instant>,
}

impl History {
    /// A reading's figures added; a tab no longer in it is dropped.
    pub fn add(&mut self, at: Instant, usage: &[TabUsage], system: Option<f64>) {
        if self
            .last
            .is_some_and(|last| at.saturating_duration_since(last) > super::WANT_FOR)
        {
            self.tabs.clear();
            self.system.clear();
        }
        self.last = Some(at);
        let open: HashSet<TabId> = usage.iter().map(|u| u.tab).collect();
        self.tabs.retain(|tab, _| open.contains(tab));
        for u in usage {
            let kept = self.tabs.entry(u.tab).or_default();
            kept.push_back((u.cpu, u.memory));
            while kept.len() > HISTORY {
                kept.pop_front();
            }
        }
        if let Some(cpu) = system {
            self.system.push_back(cpu);
            while self.system.len() > HISTORY {
                self.system.pop_front();
            }
        }
    }
}

/// The whole computer and Switchboard itself, for the board's top.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Top {
    /// Percent of all its cores, once two readings have been taken.
    pub cpu: Option<f64>,
    pub cores: usize,
    pub memory_used: u64,
    pub memory_total: u64,
    /// Switchboard's own server and what it runs outside its panes:
    /// percent of one core, bytes.
    pub own_cpu: f64,
    pub own_memory: u64,
}

/// Switchboard's own CPU and memory: the server's process (`me`) and
/// what it started outside its panes (its lookups), never another server's
/// (a `switchboard try`'s), which a name alone wouldn't tell apart. `panes`
/// is every pane's own pid, shown or parked behind a tool, whose processes
/// are the tabs'.
pub fn own_usage(list: &[Proc], panes: &[u32], me: u32) -> (f64, u64) {
    let in_tabs: HashSet<u32> = panes.iter().copied().collect();
    let mut children: HashMap<u32, Vec<&Proc>> = HashMap::new();
    for p in list {
        children.entry(p.ppid).or_default().push(p);
    }
    let mut own: Vec<&Proc> = list.iter().filter(|p| p.pid == me).collect();
    let mut at = 0;
    while let Some(p) = own.get(at).copied() {
        at += 1;
        if let Some(kids) = children.get(&p.pid) {
            own.extend(
                kids.iter()
                    .filter(|k| !in_tabs.contains(&k.pid) && k.pid != me),
            );
        }
    }
    own.into_iter().fold((0.0, 0), |(cpu, memory), p| {
        (cpu + p.cpu, memory + p.memory)
    })
}

/// `2.0 GB` or `202 MB`.
pub fn bytes(n: u64) -> String {
    const GB: f64 = 1024.0 * 1024.0 * 1024.0;
    const MB: f64 = 1024.0 * 1024.0;
    let n = n as f64;
    if n >= GB {
        format!("{:.1} GB", n / GB)
    } else {
        format!("{} MB", (n / MB).round() as u64)
    }
}

/// Each tab's processes: its panes' and every one they started, as a tree
/// from each pane's pid.
pub fn tab_usage(tabs: &[TabPanes], list: &[Proc]) -> Vec<TabUsage> {
    let mut children: HashMap<u32, Vec<&Proc>> = HashMap::new();
    for p in list {
        children.entry(p.ppid).or_default().push(p);
    }
    let by_pid: HashMap<u32, &Proc> = list.iter().map(|p| (p.pid, p)).collect();
    tabs.iter()
        .map(|tab| {
            let mut seen: HashSet<u32> = HashSet::new();
            let mut own: Vec<(usize, &Proc)> = Vec::new();
            let mut stack: Vec<(usize, &Proc)> = tab
                .pids
                .iter()
                .filter_map(|pid| by_pid.get(pid).map(|p| (0, *p)))
                .collect();
            stack.reverse();
            while let Some((depth, p)) = stack.pop() {
                if !seen.insert(p.pid) {
                    continue;
                }
                own.push((depth, p));
                if let Some(kids) = children.get(&p.pid) {
                    stack.extend(kids.iter().rev().map(|k| (depth + 1, *k)));
                }
            }
            let mut groups: Vec<Group> = Vec::new();
            for (_, p) in &own {
                match groups.iter_mut().find(|g| g.name == p.name) {
                    Some(g) => {
                        g.count += 1;
                        g.cpu += p.cpu;
                        g.memory += p.memory;
                    }
                    None => groups.push(Group {
                        name: p.name.clone(),
                        count: 1,
                        cpu: p.cpu,
                        memory: p.memory,
                    }),
                }
            }
            let group_weight =
                |g: &Group| g.cpu / CPU_HEAVY + g.memory as f64 / MEMORY_HEAVY as f64;
            let mut busiest = groups.clone();
            busiest.sort_by(|a, b| group_weight(b).total_cmp(&group_weight(a)));
            busiest.truncate(3);
            groups.sort_by_key(|g| std::cmp::Reverse(g.memory));
            groups.truncate(3);
            TabUsage {
                tab: tab.tab,
                index: tab.index,
                name: tab.name.clone(),
                count: own.len(),
                cpu: own.iter().map(|(_, p)| p.cpu).sum(),
                memory: own.iter().map(|(_, p)| p.memory).sum(),
                heaviest: groups,
                busiest,
                tree: own.into_iter().map(|(d, p)| (d, p.clone())).collect(),
            }
        })
        .collect()
}

/// Whether `pid` is still the process the menu offered to end: it has the
/// same name and is still under one of the tab's panes (`panes`, their own
/// pids), so a pid the kernel has handed to another process is never
/// signaled.
pub fn still_under(list: &[ProcUsage], panes: &[u32], pid: u32, name: &str) -> bool {
    let procs: Vec<Proc> = list
        .iter()
        .map(|p| Proc {
            pid: p.pid,
            ppid: p.ppid,
            name: p.name.clone(),
            cpu: 0.0,
            memory: p.memory,
        })
        .collect();
    let tab = TabPanes {
        tab: TabId(0),
        index: 0,
        name: String::new(),
        pids: panes.to_vec(),
    };
    tab_usage(&[tab], &procs)
        .first()
        .is_some_and(|u| u.tree.iter().any(|(_, p)| p.pid == pid && p.name == name))
}

/// The view's columns, as Activity Monitor lays them out: the tab, then its
/// figures lined up on the right.
pub fn columns() -> Vec<Column> {
    let column = |heading: &str, width, right| Column {
        heading: heading.to_string(),
        width,
        right,
    };
    vec![
        column("Tab", 30, false),
        column("% CPU", 7, true),
        column("Memory", 8, true),
        column("Processes", 11, true),
    ]
}

/// How heavy a tab is overall: its CPU and memory each as a share of what
/// marks a tab as needing you, added, so a core pegged weighs as much as
/// `MEMORY_HEAVY` of memory.
pub fn weight(u: &TabUsage) -> f64 {
    u.cpu / CPU_HEAVY + u.memory as f64 / MEMORY_HEAVY as f64
}

fn figures(cpu: f64, memory: u64, count: usize) -> [String; 3] {
    [format!("{cpu:.1}"), bytes(memory), count.to_string()]
}

/// A row per tab, heaviest first; the chosen row adds its three heaviest
/// kinds of process (`more`), which a filter matches, as it does the figures
/// in its cells. Its `data` carries what the board draws: the figures, why
/// it needs you, its busiest kinds of process, its tree and its history.
/// Nothing before the first look.
pub fn items(tabs: &[TabPanes], list: Option<&[Proc]>, history: &History) -> Vec<Item> {
    let Some(list) = list else {
        return Vec::new();
    };
    let mut usage = tab_usage(tabs, list);
    usage.sort_by(|a, b| weight(b).total_cmp(&weight(a)));
    usage
        .into_iter()
        .map(|u| {
            let heaviest = u
                .heaviest
                .iter()
                .map(|g| {
                    let times = if g.count > 1 {
                        format!(" ×{}", g.count)
                    } else {
                        String::new()
                    };
                    format!("{}{times} {}", g.name, bytes(g.memory))
                })
                .collect::<Vec<_>>()
                .join(", ");
            let title = format!("{} {}", u.index, u.name);
            let state = if u.cpu >= CPU_HEAVY || u.memory >= MEMORY_HEAVY {
                State::Attention
            } else if u.cpu >= 1.0 {
                State::Active
            } else {
                State::Idle
            };
            let mut cells = vec![title.clone()];
            cells.extend(figures(u.cpu, u.memory, u.count));
            let busiest = u
                .busiest
                .iter()
                .map(|g| {
                    let times = if g.count > 1 {
                        format!(" ×{}", g.count)
                    } else {
                        String::new()
                    };
                    let figure = if g.cpu >= 1.0 {
                        format!("{:.0}%", g.cpu)
                    } else {
                        bytes(g.memory)
                    };
                    format!("{}{times} {figure}", g.name)
                })
                .collect::<Vec<_>>()
                .join(" · ");
            let tree: Vec<serde_json::Value> = u
                .tree
                .iter()
                .map(|(depth, p)| {
                    json!({
                        "pid": p.pid,
                        "depth": depth,
                        "name": p.name,
                        "cpu": p.cpu,
                        "memory": p.memory,
                    })
                })
                .collect();
            let past = history.tabs.get(&u.tab);
            let cpu_history: Vec<f64> = past.into_iter().flatten().map(|(c, _)| *c).collect();
            let memory_history: Vec<u64> = past.into_iter().flatten().map(|(_, m)| *m).collect();
            Item {
                id: u.tab.to_string(),
                title,
                detail: heaviest.clone(),
                state,
                target: Target::Tab(u.tab),
                cells,
                more: vec![heaviest],
                data: json!({
                    "cpu": u.cpu,
                    "memory": u.memory,
                    "processes": u.count,
                    "why": u.why(),
                    "busiest": busiest,
                    "tree": tree,
                    "cpuHistory": cpu_history,
                    "memoryHistory": memory_history,
                }),
                ..Item::default()
            }
        })
        .collect()
}

/// The totals row under the table, for every tab together.
pub fn totals(tabs: &[TabPanes], list: Option<&[Proc]>) -> Vec<String> {
    let Some(list) = list else {
        return Vec::new();
    };
    let usage = tab_usage(tabs, list);
    let mut row = vec!["All tabs".to_string()];
    row.extend(figures(
        usage.iter().map(|u| u.cpu).sum(),
        usage.iter().map(|u| u.memory).sum(),
        usage.iter().map(|u| u.count).sum(),
    ));
    row
}

/// The view: its table, or nothing yet before the first look, and the
/// whole computer's figures over it (`head`).
pub fn view(
    tabs: &[TabPanes],
    list: Option<&[Proc]>,
    at: Option<i64>,
    history: &History,
    top: Option<&Top>,
) -> ViewData {
    let head = match top {
        Some(t) => json!({
            "cpu": t.cpu,
            "cores": t.cores,
            "memoryUsed": t.memory_used,
            "memoryTotal": t.memory_total,
            "ownCpu": t.own_cpu,
            "ownMemory": t.own_memory,
            "cpuHistory": history.system,
        }),
        None => serde_json::Value::Null,
    };
    ViewData {
        items: items(tabs, list, history),
        head,
        columns: columns(),
        footer: totals(tabs, list),
        at,
        ..ViewData::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A process as the Node version's test gave it in ps's columns: pid,
    /// parent, resident KB, %cpu, command.
    fn ps(lines: &[(u32, u32, u64, f64, &str)]) -> Vec<Proc> {
        lines
            .iter()
            .map(|&(pid, ppid, rss, cpu, command)| Proc {
                pid,
                ppid,
                name: command
                    .trim_start_matches('-')
                    .rsplit('/')
                    .next()
                    .unwrap()
                    .to_string(),
                cpu,
                memory: rss * 1024,
            })
            .collect()
    }

    fn tab(n: u32, name: &str, pid: u32) -> TabPanes {
        TabPanes {
            tab: TabId(n),
            index: n as usize,
            name: name.into(),
            pids: vec![pid],
        }
    }

    #[test]
    fn each_tab_counts_its_processes_from_its_panes_down_heaviest_first() {
        let list = ps(&[
            (100, 1, 1024, 0.5, "/bin/zsh"),
            (101, 100, 2_097_152, 80.0, "/usr/local/bin/node"),
            (102, 101, 1_048_576, 40.0, "/usr/local/bin/node"),
            (200, 1, 2048, 0.0, "-zsh"),
            (201, 200, 204_800, 0.4, "/opt/homebrew/bin/nvim"),
            (300, 1, 999_999, 99.0, "/Applications/Other.app/Other"),
        ]);
        let tabs = [tab(1, "light", 200), tab(2, "heavy", 100)];
        let rows: Vec<(Vec<String>, Vec<String>, State)> =
            items(&tabs, Some(&list), &History::default())
                .into_iter()
                .map(|i| (i.cells, i.more, i.state))
                .collect();
        let s = |v: &[&str]| v.iter().map(|t| t.to_string()).collect::<Vec<_>>();
        assert_eq!(
            rows,
            vec![
                (
                    s(&["2 heavy", "120.5", "3.0 GB", "3"]),
                    s(&["node ×2 3.0 GB, zsh 1 MB"]),
                    State::Attention
                ),
                (
                    s(&["1 light", "0.4", "202 MB", "2"]),
                    s(&["nvim 200 MB, zsh 2 MB"]),
                    State::Idle
                ),
            ]
        );
        assert_eq!(
            totals(&tabs, Some(&list)),
            s(&["All tabs", "120.9", "3.2 GB", "5"])
        );
        // Heavy overall, not by memory alone: a pegged core outweighs a GB.
        let busy = ps(&[
            (400, 1, 1_048_576, 0.0, "/bin/big"),
            (500, 1, 10240, 150.0, "/bin/spin"),
        ]);
        let order: Vec<String> = items(
            &[tab(1, "big", 400), tab(2, "spin", 500)],
            Some(&busy),
            &History::default(),
        )
        .into_iter()
        .map(|i| i.title)
        .collect();
        assert_eq!(order, vec!["2 spin", "1 big"]);
        assert!(
            items(&tabs, None, &History::default()).is_empty(),
            "nothing before the first look"
        );
        let v = view(&tabs, Some(&list), Some(1), &History::default(), None);
        assert_eq!(v.columns[0].heading, "Tab");
        assert!(v.columns[1..].iter().all(|c| c.right));
        assert_eq!(v.footer[0], "All tabs");
    }

    #[test]
    fn cpu_is_the_time_used_between_two_readings() {
        let reading = |pid, cpu_ns, name: &str| ProcUsage {
            pid,
            ppid: 1,
            name: name.into(),
            cpu_ns,
            memory: 10,
        };
        let before = [
            reading(10, 1_000_000_000, "a"),
            reading(11, 5_000_000_000, "b"),
        ];
        let after = [
            reading(10, 1_500_000_000, "a"),
            reading(11, 1_000, "b"),
            reading(12, 9_000_000_000, "c"),
        ];
        let procs = between(&before, &after, Duration::from_secs(1));
        let cpu: Vec<f64> = procs.iter().map(|p| p.cpu).collect();
        assert_eq!(cpu, vec![50.0, 0.0, 0.0]);
    }

    #[test]
    fn bytes_read_as_megabytes_or_gigabytes() {
        assert_eq!(bytes(1024 * 1024), "1 MB");
        assert_eq!(bytes(3 * 1024 * 1024 * 1024), "3.0 GB");
        assert_eq!(bytes(0), "0 MB");
    }

    #[test]
    fn a_tab_keeps_its_tree_in_order_and_says_why_it_needs_you() {
        let list = ps(&[
            (100, 1, 1024, 0.5, "/bin/zsh"),
            (101, 100, 5_000_000, 120.0, "/usr/local/bin/node"),
            (102, 101, 1024, 0.0, "/bin/sh"),
            (103, 100, 2048, 2.0, "/opt/homebrew/bin/git"),
        ]);
        let usage = tab_usage(&[tab(1, "busy", 100)], &list);
        let u = &usage[0];
        let tree: Vec<(usize, u32)> = u.tree.iter().map(|(d, p)| (*d, p.pid)).collect();
        assert_eq!(tree, vec![(0, 100), (1, 101), (2, 102), (1, 103)]);
        assert_eq!(u.why(), Some("a core pegged, over 4 GB"));
        assert_eq!(u.busiest[0].name, "node");
        let item = &items(&[tab(1, "busy", 100)], Some(&list), &History::default())[0];
        assert_eq!(item.data["why"], "a core pegged, over 4 GB");
        assert_eq!(item.data["busiest"], "node 120% · git 2% · zsh 1 MB");
        assert_eq!(item.data["tree"][2]["depth"], 2);
        let calm = tab_usage(&[tab(1, "calm", 103)], &list);
        assert_eq!(calm[0].why(), None);
    }

    #[test]
    fn the_history_keeps_two_minutes_drops_closed_tabs_and_starts_over_after_a_gap() {
        let list = ps(&[(100, 1, 1024, 5.0, "zsh"), (200, 1, 1024, 1.0, "zsh")]);
        let both = tab_usage(&[tab(1, "a", 100), tab(2, "b", 200)], &list);
        let mut h = History::default();
        let start = Instant::now();
        let step = Duration::from_millis(RESOURCES_MS);
        for n in 0..50u32 {
            h.add(start + step * n, &both, Some(f64::from(n)));
        }
        assert_eq!(h.tabs[&TabId(1)].len(), HISTORY);
        assert_eq!(h.system.len(), HISTORY);
        assert_eq!(h.system.back(), Some(&49.0));
        // Tab 2 closes.
        h.add(start + step * 50, &both[..1], Some(1.0));
        assert!(!h.tabs.contains_key(&TabId(2)));
        // The view was away: the next reading starts over.
        h.add(
            start + step * 50 + super::super::WANT_FOR * 2,
            &both,
            Some(2.0),
        );
        assert_eq!(h.tabs[&TabId(1)].len(), 1);
        assert_eq!(h.system.len(), 1);
        let item = &items(&[tab(1, "a", 100)], Some(&list), &h)[0];
        assert_eq!(item.data["cpuHistory"], json!([5.0]));
    }

    #[test]
    fn a_process_is_ended_only_while_it_is_still_the_one_offered() {
        let reading = |pid, ppid, name: &str| ProcUsage {
            pid,
            ppid,
            name: name.into(),
            cpu_ns: 0,
            memory: 0,
        };
        let list = [
            reading(100, 1, "zsh"),
            reading(101, 100, "node"),
            reading(200, 1, "node"),
        ];
        assert!(still_under(&list, &[100], 101, "node"));
        assert!(!still_under(&list, &[100], 101, "vim"), "its pid reused");
        assert!(
            !still_under(&list, &[100], 200, "node"),
            "not under the tab"
        );
        assert!(!still_under(&list, &[100], 300, "node"), "ended");
    }

    #[test]
    fn switchboard_itself_is_its_server_and_what_it_runs_outside_its_panes() {
        let list = ps(&[
            (10, 1, 1024, 2.0, "switchboard"),
            (11, 1, 1024, 9.0, "switchboard"),
            (12, 10, 1024, 1.0, "git"),
            (13, 12, 1024, 1.0, "ssh"),
            (100, 10, 1024, 0.0, "zsh"),
            (101, 100, 1024, 50.0, "switchboard"),
        ]);
        // The server and its git lookup with what it started; not another
        // server (11), nor what runs in its panes.
        assert_eq!(own_usage(&list, &[100], 10), (4.0, 3 * 1024 * 1024));
    }
}
