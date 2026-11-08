//! Where `new <place>: <prompt>` starts its Claude (lib/slack-relay/
//! places.js). A place is a directory (a path starting ~ or /), or a name
//! looked up among the git repositories you work in, found the way
//! `wt-revive` finds them: every open tab's, and every one a Claude session
//! has run in over the last `RECENT_DAYS`. A repository's own name picks
//! its main checkout; otherwise the name is matched against each
//! worktree's branch and directory, so a ticket ID finds its worktree.
//! Exactly one match, or nothing is started. A worktree of someone else's
//! PR, checked out untrusted (src/trust.rs), is never a place: it is left
//! out of the lists, and refused when named.
//!
//! The New tab form's menu of places lists every repository checkout a
//! Claude session has ever run in, or an open tab is in, at its top level,
//! and every directory outside a repository, so long as it still exists:
//! most recently used first, grouped by repository, directories outside
//! any repository last.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::LazyLock;

use regex::Regex;

use super::Relay;
use super::config::Paths;
use crate::util;

pub const RECENT_DAYS: i64 = 14;
/// The most a Slack menu holds.
pub const MAX_PLACES: usize = 100;
/// Slack refuses an option whose value is longer.
const MAX_VALUE: usize = 150;

fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Directories Claude sessions have run in since `since` (ms), each with
/// the time of its session's last activity: the last cwd in each
/// transcript, read from its end, since some are megabytes long.
pub fn session_dirs(paths: &Paths, since: i64) -> Vec<(PathBuf, i64)> {
    static CWD: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#""cwd":"([^"]*)""#).expect("a regex"));
    let mut out = Vec::new();
    let Ok(projects) = std::fs::read_dir(paths.projects_dir()) else {
        return out;
    };
    for project in projects.flatten() {
        let Ok(files) = std::fs::read_dir(project.path()) else {
            continue;
        };
        for file in files.flatten() {
            let path = file.path();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            let Ok(meta) = file.metadata() else {
                continue;
            };
            let at = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0);
            if at < since {
                continue;
            }
            let tail = tail(&path, 65_536);
            if let Some(cwd) = CWD.captures_iter(&tail).last() {
                out.push((PathBuf::from(&cwd[1]), at));
            }
        }
    }
    out
}

fn tail(path: &Path, n: u64) -> String {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut f) = std::fs::File::open(path) else {
        return String::new();
    };
    let size = f.metadata().map(|m| m.len()).unwrap_or(0);
    let length = size.min(n);
    let _ = f.seek(SeekFrom::Start(size - length));
    let mut buf = Vec::new();
    let _ = f.take(length).read_to_end(&mut buf);
    String::from_utf8_lossy(&buf).into_owned()
}

/// The directories the open tabs' panes are in.
fn pane_dirs(cx: &Relay) -> Vec<PathBuf> {
    cx.mux
        .windows()
        .unwrap_or_default()
        .into_iter()
        .flat_map(|w| w.panes.into_iter().filter_map(|p| p.cwd))
        .collect()
}

/// A worktree of a repository in use.
#[derive(Clone, Debug, PartialEq)]
pub struct Place {
    pub path: PathBuf,
    pub branch: String,
    pub repo: String,
    pub main: bool,
}

/// The checkout a directory belongs to: its repository's main checkout,
/// unless it is a bare repository, which has none of its own.
fn main_checkout(dir: &Path) -> Option<PathBuf> {
    let common = git(
        dir,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )?;
    let common = PathBuf::from(common);
    (common.file_name().and_then(|n| n.to_str()) == Some(".git"))
        .then(|| common.parent().map(Path::to_path_buf))
        .flatten()
}

/// Every worktree of every repository in use.
pub fn places(cx: &Relay) -> Vec<Place> {
    let since = util::now_ms() - RECENT_DAYS * 24 * 60 * 60_000;
    let mut dirs = pane_dirs(cx);
    dirs.extend(session_dirs(&cx.paths, since).into_iter().map(|(d, _)| d));
    let mut roots: Vec<PathBuf> = Vec::new();
    for dir in dirs {
        if !dir.exists() {
            continue;
        }
        if let Some(root) = main_checkout(&dir)
            && !roots.contains(&root)
        {
            roots.push(root);
        }
    }
    let mut out = Vec::new();
    for repo in roots {
        let listed = git(&repo, &["worktree", "list", "--porcelain"]).unwrap_or_default();
        let name = repo
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        for block in listed.split("\n\n") {
            let lines: Vec<&str> = block.lines().collect();
            if block.trim().is_empty()
                || lines
                    .iter()
                    .any(|l| l.starts_with("prunable") || *l == "bare")
            {
                continue;
            }
            let Some(tree) = lines.iter().find_map(|l| l.strip_prefix("worktree ")) else {
                continue;
            };
            // Never someone else's PR, which no Claude is started in.
            if crate::trust::in_review_dir_of(&cx.paths.home, Path::new(tree)) {
                continue;
            }
            let branch = lines
                .iter()
                .find_map(|l| l.strip_prefix("branch refs/heads/"))
                .unwrap_or("(detached)");
            out.push(Place {
                path: PathBuf::from(tree),
                branch: branch.to_string(),
                repo: name.clone(),
                main: Path::new(tree) == repo,
            });
        }
    }
    out
}

fn describe(paths: &Paths, place: &Place) -> String {
    format!(
        "`{}` ({}, {})",
        paths.short(&place.path),
        place.repo,
        place.branch
    )
}

/// Where a place is, with how to say it, or why it can't be used.
pub fn resolve_place(cx: &Relay, spec: &str) -> Result<(PathBuf, String), String> {
    if spec.starts_with('~') || spec.starts_with('/') {
        let dir = match spec.strip_prefix('~') {
            Some(rest) if rest.is_empty() || rest.starts_with('/') => {
                cx.paths.home.join(rest.trim_start_matches('/'))
            }
            _ => PathBuf::from(spec),
        };
        // Its `.` and `..` worked out, but a link in it kept as typed, as
        // `path.resolve` did: the reply names, and the tab starts in, the
        // path you gave.
        let dir = crate::worktree::normalize(&dir);
        if !dir.is_dir() {
            return Err(format!(
                "There's no directory `{spec}`, so I didn't start anything."
            ));
        }
        // Never someone else's PR, whose project settings and hooks Claude
        // would load.
        if crate::trust::is_untrusted_of(&cx.paths.home, &dir) {
            return Err(format!(
                "`{spec}` holds someone else's PR, checked out untrusted, so I didn't start anything there."
            ));
        }
        let said = format!("`{}`", cx.paths.short(&dir));
        return Ok((dir, said));
    }
    let all = places(cx);
    let wanted = spec.to_lowercase();
    let repo: Vec<&Place> = all
        .iter()
        .filter(|p| p.main && p.repo.to_lowercase() == wanted)
        .collect();
    let matches: Vec<&Place> = if !repo.is_empty() {
        repo
    } else {
        all.iter()
            .filter(|p| {
                p.branch.to_lowercase().contains(&wanted)
                    || p.path
                        .file_name()
                        .is_some_and(|n| n.to_string_lossy().to_lowercase().contains(&wanted))
            })
            .collect()
    };
    match matches.len() {
        // Never someone else's PR, wherever its worktree was moved.
        1 if crate::trust::is_untrusted_of(&cx.paths.home, &matches[0].path) => Err(format!(
            "`{spec}` is someone else's PR, checked out untrusted, so I didn't start anything there."
        )),
        1 => Ok((matches[0].path.clone(), describe(&cx.paths, matches[0]))),
        0 => Err(format!(
            "Nothing matches `{spec}`: no repository by that name, and no worktree whose branch or directory contains it. Nothing started."
        )),
        n => {
            let mut text = format!(
                "`{spec}` matches {n} places, so I didn't start anything. Name one more closely:\n{}",
                matches
                    .iter()
                    .take(10)
                    .map(|p| format!("• {}", describe(&cx.paths, p)))
                    .collect::<Vec<_>>()
                    .join("\n")
            );
            if n > 10 {
                text.push_str(&format!("\n…and {} more", n - 10));
            }
            Err(text)
        }
    }
}

/// One choice in the New tab form's menu.
#[derive(Clone, Debug, PartialEq)]
pub struct Choice {
    pub label: String,
    pub value: String,
}

/// The menu's groups: each repository by its main checkout's name, then
/// "Other directories".
pub fn place_choices(cx: &Relay) -> Vec<(String, Vec<Choice>)> {
    let now = util::now_ms();
    let mut latest: HashMap<PathBuf, i64> = HashMap::new();
    for dir in pane_dirs(cx) {
        latest.insert(dir, now);
    }
    for (dir, at) in session_dirs(&cx.paths, 0) {
        let known = latest.entry(dir).or_insert(at);
        *known = (*known).max(at);
    }
    // Each directory counted for its checkout's top level.
    let mut tops: HashMap<PathBuf, (Option<PathBuf>, i64)> = HashMap::new();
    for (dir, at) in latest {
        if !dir.is_dir() || crate::trust::in_review_dir_of(&cx.paths.home, &dir) {
            continue;
        }
        let shown = git(
            &dir,
            &[
                "rev-parse",
                "--path-format=absolute",
                "--show-toplevel",
                "--git-common-dir",
            ],
        )
        .unwrap_or_default();
        let mut lines = shown.lines();
        let (top, common) = (lines.next(), lines.next());
        let repo = common
            .map(PathBuf::from)
            .filter(|c| c.file_name().and_then(|n| n.to_str()) == Some(".git"))
            .and_then(|c| c.parent().map(Path::to_path_buf));
        let place = match (&repo, top) {
            (Some(_), Some(top)) => PathBuf::from(top),
            _ => dir,
        };
        let entry = tops.entry(place).or_insert((repo.clone(), at));
        if entry.1 < at {
            *entry = (repo, at);
        }
    }
    let mut sorted: Vec<(PathBuf, Option<PathBuf>, i64)> =
        tops.into_iter().map(|(d, (r, a))| (d, r, a)).collect();
    sorted.sort_by(|a, b| b.2.cmp(&a.2).then_with(|| a.0.cmp(&b.0)));
    let name = |p: &Path| {
        p.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    };
    let choices: Vec<(Option<String>, Choice)> = sorted
        .into_iter()
        .take(MAX_PLACES)
        .map(|(dir, repo, _)| {
            let label = match &repo {
                None => cx.paths.short(&dir),
                Some(r) if *r == dir => name(r),
                Some(_) => format!("⎇ {}", name(&dir)),
            };
            (
                repo.as_deref().map(name),
                Choice {
                    label,
                    value: cx.paths.short(&dir),
                },
            )
        })
        .filter(|(_, c)| c.value.chars().count() <= MAX_VALUE)
        .collect();
    let mut order: Vec<Option<String>> = Vec::new();
    for (group, _) in &choices {
        if !order.contains(group) {
            order.push(group.clone());
        }
    }
    // Directories outside a repository last.
    order.sort_by_key(|g| g.is_none());
    order
        .into_iter()
        .map(|group| {
            let members = choices
                .iter()
                .filter(|(g, _)| *g == group)
                .map(|(_, c)| c.clone())
                .collect();
            (group.unwrap_or_else(|| "Other directories".into()), members)
        })
        .collect()
}
