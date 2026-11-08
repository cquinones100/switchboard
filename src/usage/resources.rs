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

use std::collections::{HashMap, HashSet};
use std::time::Duration;

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

/// A kind of process in a tab: its name, how many, their memory.
#[derive(Clone, Debug, PartialEq)]
pub struct Group {
    pub name: String,
    pub count: usize,
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
            let mut own: Vec<&Proc> = Vec::new();
            let mut stack: Vec<&Proc> = tab
                .pids
                .iter()
                .filter_map(|pid| by_pid.get(pid).copied())
                .collect();
            stack.reverse();
            while let Some(p) = stack.pop() {
                if !seen.insert(p.pid) {
                    continue;
                }
                own.push(p);
                if let Some(kids) = children.get(&p.pid) {
                    stack.extend(kids.iter().rev().copied());
                }
            }
            let mut groups: Vec<Group> = Vec::new();
            for p in &own {
                match groups.iter_mut().find(|g| g.name == p.name) {
                    Some(g) => {
                        g.count += 1;
                        g.memory += p.memory;
                    }
                    None => groups.push(Group {
                        name: p.name.clone(),
                        count: 1,
                        memory: p.memory,
                    }),
                }
            }
            groups.sort_by_key(|g| std::cmp::Reverse(g.memory));
            groups.truncate(3);
            TabUsage {
                tab: tab.tab,
                index: tab.index,
                name: tab.name.clone(),
                count: own.len(),
                cpu: own.iter().map(|p| p.cpu).sum(),
                memory: own.iter().map(|p| p.memory).sum(),
                heaviest: groups,
            }
        })
        .collect()
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
/// in its cells. Nothing before the first look.
pub fn items(tabs: &[TabPanes], list: Option<&[Proc]>) -> Vec<Item> {
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
            Item {
                id: u.tab.to_string(),
                title,
                detail: heaviest.clone(),
                state,
                target: Target::Tab(u.tab),
                cells,
                more: vec![heaviest],
                data: json!({ "cpu": u.cpu, "memory": u.memory, "processes": u.count }),
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

/// The view: its table, or nothing yet before the first look.
pub fn view(tabs: &[TabPanes], list: Option<&[Proc]>, at: Option<i64>) -> ViewData {
    ViewData {
        items: items(tabs, list),
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
        let rows: Vec<(Vec<String>, Vec<String>, State)> = items(&tabs, Some(&list))
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
        let order: Vec<String> = items(&[tab(1, "big", 400), tab(2, "spin", 500)], Some(&busy))
            .into_iter()
            .map(|i| i.title)
            .collect();
        assert_eq!(order, vec!["2 spin", "1 big"]);
        assert!(
            items(&tabs, None).is_empty(),
            "nothing before the first look"
        );
        let v = view(&tabs, Some(&list), Some(1));
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
}
