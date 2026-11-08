//! `wt-jump`: goes to the tab whose worktree matches a Linear ticket, a
//! GitHub pull request, or part of a branch name: the way back to a tab you
//! already have open, when cycling through them with M-[ and M-] stops
//! scaling. Ported from `bin/wt-jump`.
//!
//! ```text
//! wt-jump ABC-123      the ticket's tab
//! wt-jump 123          the PR's tab
//! wt-jump theme-slot   any tab whose branch, directory or name contains that
//! wt-jump --list       print the tabs instead of jumping
//! ```
//!
//! Tabs are matched on what is on disk, each pane's directory and the
//! branch checked out in it, and on the tab's label, whose `#<n> ` front is
//! its PR. A bare number is looked for as a `pr-<n>` worktree first, which
//! is what `wtpr` makes and costs nothing; only when that finds nothing is
//! GitHub asked what branch the PR is on, so a ticket's worktree that later
//! opened a PR is still reachable by its number.
//!
//! The tabs and panes come from the server (`switchboard list`) where tmux's
//! `list-panes` gave them, and a jump is `select-tab` and `select-pane`, or,
//! for a tab in another session, `switch-client`, which takes the terminal
//! you used last there as tmux's did. A row is the tab's label, as the tab
//! bar shows it, where the tmux version ran the second status line's script
//! for each.

use std::collections::HashMap;
use std::path::Path;
use std::process::{Command, ExitCode, Stdio};

use serde_json::Value;

use crate::integrations::github;
use crate::mux::socket;
use crate::util;
use crate::worktree::{self, fail, mentions};

fn usage() -> ExitCode {
    eprintln!(
        "Usage: wt-jump <ticket-or-pr-or-branch>\n\n\
         Examples:\n  wt-jump ABC-123\n  wt-jump 123\n  wt-jump theme-slot\n  \
         wt-jump --list     print the tabs, jump to none of them"
    );
    ExitCode::from(1)
}

/// One pane of a tab, with what its directory says.
#[derive(Clone, Debug, Default)]
struct Pane {
    tab: String,
    pane: String,
    session: String,
    index: i64,
    /// The tab's label, `#<PR> ` in front when its branch has one.
    label: String,
    path: String,
    agent: String,
    branch: Option<String>,
    root: Option<String>,
    dir: String,
    repo: String,
    pr: Option<String>,
    matched: bool,
}

/// Every pane but the sidebars', from the server's `list`.
fn list_panes(list: &Value) -> Vec<Pane> {
    let mut panes = Vec::new();
    for session in list
        .get("sessions")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let name = session.get("name").and_then(Value::as_str).unwrap_or("");
        for tab in session
            .get("tabs")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let text =
                |v: &Value, key: &str| v.get(key).and_then(Value::as_str).unwrap_or("").to_string();
            for pane in tab
                .get("panes")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let sidebar = pane
                    .pointer("/options/@sidebar")
                    .and_then(Value::as_str)
                    .is_some_and(|v| !v.is_empty() && v != "0");
                if sidebar {
                    continue;
                }
                panes.push(Pane {
                    tab: text(tab, "id"),
                    pane: text(pane, "id"),
                    session: name.to_string(),
                    index: tab.get("index").and_then(Value::as_i64).unwrap_or(0),
                    label: text(tab, "label"),
                    path: text(pane, "cwd"),
                    agent: text(tab, "agent"),
                    ..Pane::default()
                });
            }
        }
    }
    panes
}

fn git(dir: &str, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .arg("--no-optional-locks")
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Each pane with its branch, repository and PR, one git call per distinct
/// directory rather than per pane, since a tab's panes usually share one.
fn describe(panes: Vec<Pane>) -> Vec<Pane> {
    let mut known: HashMap<String, (Option<String>, Option<String>)> = HashMap::new();
    panes
        .into_iter()
        .map(|mut p| {
            let (branch, root) = known
                .entry(p.path.clone())
                .or_insert_with(|| {
                    let branch = git(&p.path, &["rev-parse", "--abbrev-ref", "HEAD"])
                        .filter(|b| b != "HEAD" && !b.is_empty());
                    let root = git(&p.path, &["rev-parse", "--git-common-dir"]).map(|common| {
                        let common = Path::new(&p.path).join(common);
                        let parent = common.parent().unwrap_or(&common).to_path_buf();
                        worktree::normalize(&parent).to_string_lossy().into_owned()
                    });
                    (branch, root)
                })
                .clone();
            p.dir = Path::new(&p.path)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            p.repo = root
                .as_deref()
                .and_then(|r| Path::new(r).file_name())
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| p.dir.clone());
            // The PR read back off what is on screen or on disk: the `#<n> `
            // in front of the label, or a `pr-<n>` directory.
            p.pr = pr_in_label(&p.label).or_else(|| {
                p.dir
                    .strip_prefix("pr-")
                    .and_then(|r| r.split_once('-'))
                    .map(|(n, _)| n)
                    .filter(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
                    .map(str::to_string)
            });
            p.branch = branch;
            p.root = root;
            p
        })
        .collect()
}

fn pr_in_label(label: &str) -> Option<String> {
    let rest = label.strip_prefix('#')?;
    let (n, _) = rest.split_once(' ')?;
    (!n.is_empty() && n.bytes().all(|b| b.is_ascii_digit())).then(|| n.to_string())
}

fn contains(text: Option<&str>, needle: &str) -> bool {
    text.is_some_and(|t| t.to_lowercase().contains(&needle.to_lowercase()))
}

/// What a target matched, how it is said, and the command that opens it.
struct Matches {
    matched: Vec<Pane>,
    label: String,
    create: String,
}

/// A PR's head branch, as `wtpr` checks it out (a fork's as
/// `<owner>-<branch>`), from the first of the repositories that has it.
fn resolve_pr(number: u64, roots: &[String]) -> Option<String> {
    roots.iter().find_map(|root| {
        util::block_on(async {
            let repo = github::repo_of(Path::new(root)).await.ok()?;
            let pr = worktree::pr_details(&repo, number, "wt-jump: the PR's branch")
                .await
                .ok()??;
            Some(match (&pr.head_owner, pr.is_cross_repository) {
                (Some(owner), true) => format!("{owner}-{}", pr.head_ref_name),
                _ => pr.head_ref_name,
            })
        })
    })
}

fn matches_for(target: &str, described: Vec<Pane>) -> Result<Matches, String> {
    if worktree::looks_like_ticket(target) {
        let token = worktree::ticket_from(target)
            .unwrap_or_default()
            .to_lowercase();
        let upper = token.to_uppercase();
        let matched = described
            .into_iter()
            .filter(|p| {
                mentions(&p.dir, &token) || p.branch.as_deref().is_some_and(|b| mentions(b, &token))
            })
            .collect();
        return Ok(Matches {
            matched,
            label: upper.clone(),
            create: format!("wt {upper}"),
        });
    }
    if let Some((number, _)) = worktree::pr_from(target) {
        let label = format!("PR #{number}");
        let token = format!("pr-{number}");
        let front = format!("#{number} ");
        let local: Vec<Pane> = described
            .iter()
            .filter(|p| {
                mentions(&p.dir, &token)
                    || p.branch.as_deref().is_some_and(|b| mentions(b, &token))
                    || p.label.starts_with(&front)
            })
            .cloned()
            .collect();
        if !local.is_empty() {
            return Ok(Matches {
                matched: local,
                label,
                create: format!("wtpr {number}"),
            });
        }
        // Opened from a ticket's worktree, whose name carries the ticket and
        // not the number, so GitHub has to say what branch it is on.
        let mut roots: Vec<String> = Vec::new();
        for root in described.iter().filter_map(|p| p.root.clone()) {
            if !roots.contains(&root) {
                roots.push(root);
            }
        }
        let Some(branch) = resolve_pr(number, &roots) else {
            return Err(format!(
                "GitHub could not find {label} in any repository you have open:\n{}",
                roots
                    .iter()
                    .map(|r| format!("  {r}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            ));
        };
        return Ok(Matches {
            matched: described
                .into_iter()
                .filter(|p| p.branch.as_deref() == Some(branch.as_str()))
                .collect(),
            label: format!("{label} ({branch})"),
            create: format!("wtpr {number}"),
        });
    }
    Ok(Matches {
        matched: described
            .into_iter()
            .filter(|p| {
                contains(p.branch.as_deref(), target)
                    || contains(Some(&p.dir), target)
                    || contains(Some(&p.label), target)
            })
            .collect(),
        label: target.to_string(),
        create: format!("wt {target}"),
    })
}

/// One pane per tab, preferring one that matched over the tab's first, so
/// going to a row lands on the pane that earned it. Tabs whose Claude waits
/// on you come first, then ones that finished while you were elsewhere,
/// then the rest.
fn by_tab(panes: Vec<Pane>) -> Vec<Pane> {
    let mut order: Vec<String> = Vec::new();
    let mut chosen: HashMap<String, Pane> = HashMap::new();
    for p in panes {
        match chosen.get(&p.tab) {
            Some(existing) if existing.matched || !p.matched => {}
            _ => {
                if !chosen.contains_key(&p.tab) {
                    order.push(p.tab.clone());
                }
                chosen.insert(p.tab.clone(), p);
            }
        }
    }
    let urgency = |p: &Pane| match p.agent.as_str() {
        "waiting" => 0,
        "done" => 1,
        _ => 2,
    };
    let mut tabs: Vec<Pane> = order
        .into_iter()
        .filter_map(|t| chosen.remove(&t))
        .collect();
    tabs.sort_by(|a, b| {
        urgency(a)
            .cmp(&urgency(b))
            .then_with(|| a.session.cmp(&b.session))
            .then_with(|| a.index.cmp(&b.index))
    });
    tabs
}

/// Without a label: the repository as well as the branch, because a row
/// reading just "main" is the same row for every repository you have open.
fn fallback(p: &Pane) -> String {
    [
        p.repo.clone(),
        p.pr.as_ref().map(|n| format!("#{n}")).unwrap_or_default(),
        p.branch.clone().unwrap_or_else(|| p.dir.clone()),
    ]
    .into_iter()
    .filter(|s| !s.is_empty())
    .collect::<Vec<_>>()
    .join("  ")
}

/// The rows as printed: where each tab is, then what it shows.
fn render(rows: &[Pane], current_tab: &str) -> Vec<String> {
    let wheres: Vec<String> = rows
        .iter()
        .map(|p| format!("{}:{}", p.session, p.index))
        .collect();
    let width = wheres.iter().map(|w| w.chars().count()).max().unwrap_or(0);
    rows.iter()
        .zip(wheres)
        .map(|(p, w)| {
            let shown = if p.label.is_empty() {
                fallback(p)
            } else {
                p.label.clone()
            };
            let here = if p.tab == current_tab { "  (here)" } else { "" };
            format!("{w:<width$}  {shown}{here}")
        })
        .collect()
}

pub fn main(args: Vec<String>) -> ExitCode {
    // A program you run asks the services whatever Settings says.
    crate::integrations::run_by_you();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        return usage();
    }
    let list_only = args.iter().any(|a| a == "--list");
    let positional: Vec<&String> = args.iter().filter(|a| *a != "--list").collect();
    if positional.len() > 1 || (positional.is_empty() && !list_only) {
        return usage();
    }
    if !socket::in_pane() && !list_only {
        return fail("not inside Switchboard");
    }
    let list = match socket::ask_ok(&["list"]).map(|t| serde_json::from_str::<Value>(&t)) {
        Ok(Ok(list)) => list,
        Ok(Err(e)) => return fail(&format!("the server's list isn't JSON: {e}")),
        Err(e) => return fail(&e.to_string()),
    };
    let panes = list_panes(&list);
    if panes.is_empty() {
        return fail("no tabs found");
    }

    // Asked about this process's own pane rather than the client's current
    // one: a pane that isn't focused still says where it actually is.
    let current_tab = if socket::in_pane() {
        socket::ask_ok(&["display", "tab"])
            .map(|v| v.trim().to_string())
            .unwrap_or_default()
    } else {
        String::new()
    };

    let described = describe(panes);
    let (matched, label, create, targeted) = match positional.first() {
        Some(target) => match matches_for(target, described) {
            Ok(m) => (m.matched, m.label, m.create, true),
            Err(e) => return fail(&e),
        },
        None => (described, String::new(), String::new(), false),
    };
    if matched.is_empty() {
        return fail(&format!(
            "no open tab for {label}.\nOpen one with:\n  {create}"
        ));
    }
    let rows = by_tab(
        matched
            .into_iter()
            .map(|mut p| {
                p.matched = targeted;
                p
            })
            .collect(),
    );

    if list_only {
        for line in render(&rows, &current_tab) {
            println!("{line}");
        }
        return ExitCode::SUCCESS;
    }

    // One match jumps outright; several are asked about.
    let chosen = if rows.len() == 1 {
        &rows[0]
    } else {
        let lines = render(&rows, &current_tab);
        eprintln!("{} tabs match — pick one:", rows.len());
        for (i, line) in lines.iter().enumerate() {
            eprintln!("  {}. {line}", i + 1);
        }
        &rows[worktree::choose_numbered("Select tab [1]: ", rows.len())]
    };
    // A tab in another session takes your terminal along to that session.
    if let Err(e) = socket::ask_ok(&["switch-client", "-t", &chosen.tab])
        .and_then(|_| socket::ask_ok(&["select-pane", "-t", &chosen.pane]))
    {
        return fail(&e.to_string());
    }
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pane(tab: &str, agent: &str, session: &str, index: i64) -> Pane {
        Pane {
            tab: tab.into(),
            pane: format!("%{tab}"),
            agent: agent.into(),
            session: session.into(),
            index,
            ..Pane::default()
        }
    }

    #[test]
    fn tabs_waiting_on_you_come_first_one_row_each() {
        let mut matched = pane("@2", "", "main", 2);
        matched.matched = true;
        let rows = by_tab(vec![
            pane("@1", "", "main", 1),
            pane("@2", "", "main", 2),
            matched,
            pane("@3", "waiting", "main", 3),
            pane("@4", "done", "main", 4),
        ]);
        let tabs: Vec<&str> = rows.iter().map(|p| p.tab.as_str()).collect();
        assert_eq!(tabs, vec!["@3", "@4", "@1", "@2"]);
        assert!(rows[3].matched, "the pane that matched stands for its tab");
    }

    #[test]
    fn a_label_s_front_is_its_pr() {
        assert_eq!(pr_in_label("#12 ENG-4 Fix it").as_deref(), Some("12"));
        assert_eq!(pr_in_label("#x y"), None);
        assert_eq!(pr_in_label("repo"), None);
    }
}
