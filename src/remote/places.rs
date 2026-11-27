//! Where a remote front-end starts a Claude: the places both the Slack
//! relay and the phone app offer and the server opens a tab in
//! (`claude-tab`, src/remote/serve.rs). Moved from the relay's
//! src/relay/places.rs (lib/slack-relay/places.js), so the two front-ends
//! list and look up the same places.
//!
//! A place is a directory (a path starting ~ or /), or a name looked up
//! among the git repositories you work in, found the way `wt-revive` finds
//! them: every open tab's, and every one a Claude session has run in over
//! the last `RECENT_DAYS`. A repository's own name picks its main checkout;
//! otherwise the name is matched against each worktree's branch and
//! directory, so a ticket ID finds its worktree. Exactly one match, or
//! nothing is started. A worktree of someone else's PR, checked out
//! untrusted (src/trust.rs), is never a place: it is left out of the lists,
//! and refused when named.
//!
//! The list of places (the relay's New tab form, the phone's New tab
//! sheet) holds every repository checkout a Claude session has ever run
//! in, or an open tab is in, at its top level, and every directory outside
//! a repository, so long as it still exists: most recently used first,
//! grouped by repository, directories outside any repository last. The
//! phone may start a Claude only in one of them (`listed`).
//!
//! Everything here runs git or reads files, so it runs off the server's
//! task.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;

use crate::util;

pub const RECENT_DAYS: i64 = 14;
/// The most a list of places holds (a Slack menu's most).
pub const MAX_PLACES: usize = 100;
/// Slack refuses an option whose value is longer.
const MAX_VALUE: usize = 150;

/// A path with the home shown as `~`, as places.js's `shortPath`: the
/// home as given, or its real path, which a pane's directory may be read
/// as where the home is reached through a link.
pub fn short(home: &Path, path: &Path) -> String {
    let under = |home: &Path| {
        let home = home.to_string_lossy();
        let text = path.to_string_lossy();
        match text.strip_prefix(home.as_ref()) {
            Some(rest)
                if !home.is_empty()
                    && home != "/"
                    && (rest.is_empty() || rest.starts_with('/')) =>
            {
                Some(format!("~{rest}"))
            }
            _ => None,
        }
    };
    under(home)
        .or_else(|| under(&home.canonicalize().ok()?))
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

/// A path as typed (`~`, `~/...` or absolute), under `home`.
fn expand(home: &Path, spec: &str) -> PathBuf {
    match spec.strip_prefix('~') {
        Some(rest) if rest.is_empty() || rest.starts_with('/') => {
            home.join(rest.trim_start_matches('/'))
        }
        _ => PathBuf::from(spec),
    }
}

/// Directories Claude sessions have run in since `since` (ms), each with
/// the time of its session's last activity: the last cwd in each
/// transcript, read from its end, since some are megabytes long.
pub fn session_dirs(home: &Path, since: i64) -> Vec<(PathBuf, i64)> {
    static CWD: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#""cwd":"([^"]*)""#).expect("a regex"));
    let mut out = Vec::new();
    let Ok(projects) = std::fs::read_dir(home.join(".claude/projects")) else {
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

/// What places are found from: the home (for Claude's transcripts and
/// `~`) and the directories the open tabs' panes are in.
#[derive(Clone, Debug)]
pub struct Known {
    pub home: PathBuf,
    pub pane_dirs: Vec<PathBuf>,
}

impl Known {
    /// From the tabs a front-end reads (`Remote::windows`).
    pub fn of(home: &Path, windows: &[super::protocol::Window]) -> Known {
        Known {
            home: home.to_path_buf(),
            pane_dirs: windows
                .iter()
                .flat_map(|w| w.panes.iter().filter_map(|p| p.cwd.clone()))
                .collect(),
        }
    }
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
    let common = crate::worktree::git(
        dir,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )
    .ok()?;
    let common = PathBuf::from(common);
    (common.file_name().and_then(|n| n.to_str()) == Some(".git"))
        .then(|| common.parent().map(Path::to_path_buf))
        .flatten()
}

/// Every worktree of every repository in use.
pub fn places(known: &Known) -> Vec<Place> {
    let since = util::now_ms() - RECENT_DAYS * 24 * 60 * 60_000;
    let mut dirs = known.pane_dirs.clone();
    dirs.extend(session_dirs(&known.home, since).into_iter().map(|(d, _)| d));
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
        let listed =
            crate::worktree::git(&repo, &["worktree", "list", "--porcelain"]).unwrap_or_default();
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
            if crate::trust::in_review_dir_of(&known.home, Path::new(tree)) {
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

fn describe(home: &Path, place: &Place) -> String {
    format!(
        "`{}` ({}, {})",
        short(home, &place.path),
        place.repo,
        place.branch
    )
}

/// Where a place is, with how to say it, or why it can't be used: a path
/// typed, or a name among `places`, as the relay's `new <place>:` takes it.
pub fn resolve_place(known: &Known, spec: &str) -> Result<(PathBuf, String), String> {
    let home = &known.home;
    if spec.starts_with('~') || spec.starts_with('/') {
        // Its `.` and `..` worked out, but a link in it kept as typed, as
        // `path.resolve` did: the reply names, and the tab starts in, the
        // path you gave.
        let dir = crate::worktree::normalize(&expand(home, spec));
        if !dir.is_dir() {
            return Err(format!(
                "There's no directory `{spec}`, so I didn't start anything."
            ));
        }
        // Never someone else's PR, whose project settings and hooks Claude
        // would load.
        if crate::trust::is_untrusted_of(home, &dir) {
            return Err(format!(
                "`{spec}` holds someone else's PR, checked out untrusted, so I didn't start anything there."
            ));
        }
        let said = format!("`{}`", short(home, &dir));
        return Ok((dir, said));
    }
    let all = places(known);
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
        1 if crate::trust::is_untrusted_of(home, &matches[0].path) => Err(format!(
            "`{spec}` is someone else's PR, checked out untrusted, so I didn't start anything there."
        )),
        1 => Ok((matches[0].path.clone(), describe(home, matches[0]))),
        0 => Err(format!(
            "Nothing matches `{spec}`: no repository by that name, and no worktree whose branch or directory contains it. Nothing started."
        )),
        n => {
            let mut text = format!(
                "`{spec}` matches {n} places, so I didn't start anything. Name one more closely:\n{}",
                matches
                    .iter()
                    .take(10)
                    .map(|p| format!("• {}", describe(home, p)))
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

/// One place in the list.
#[derive(Clone, Debug, PartialEq)]
pub struct Choice {
    pub label: String,
    pub value: String,
}

/// The list's groups: each repository by its main checkout's name, then
/// "Other directories", the most recent `MAX_PLACES` places.
pub fn place_choices(known: &Known) -> Vec<(String, Vec<Choice>)> {
    choices_up_to(known, MAX_PLACES)
}

/// `place_choices` up to `most` places.
fn choices_up_to(known: &Known, most: usize) -> Vec<(String, Vec<Choice>)> {
    let home = &known.home;
    let now = util::now_ms();
    let mut latest: HashMap<PathBuf, i64> = HashMap::new();
    for dir in &known.pane_dirs {
        latest.insert(dir.clone(), now);
    }
    for (dir, at) in session_dirs(home, 0) {
        let known = latest.entry(dir).or_insert(at);
        *known = (*known).max(at);
    }
    // Each directory counted for its checkout's top level.
    let mut tops: HashMap<PathBuf, (Option<PathBuf>, i64)> = HashMap::new();
    for (dir, at) in latest {
        if !dir.is_dir() || crate::trust::in_review_dir_of(home, &dir) {
            continue;
        }
        let shown = crate::worktree::git(
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
        // Never someone else's PR, by its folder or by its worktree's mark,
        // read at the checkout's top level, where its `.git` is, from its
        // config file with no git run for each, as `listed` refuses one.
        if crate::trust::in_review_dir_of(home, &place)
            || crate::trust::is_untrusted_without_git(&place)
        {
            continue;
        }
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
        .take(most)
        .map(|(dir, repo, _)| {
            let label = match &repo {
                None => short(home, &dir),
                Some(r) if *r == dir => name(r),
                Some(_) => format!("⎇ {}", name(&dir)),
            };
            (
                repo.as_deref().map(name),
                Choice {
                    label,
                    value: short(home, &dir),
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

/// A place from the list (`value` as `place_choices` gives it), with how
/// to say it, or why it can't be used: the phone's, which starts a Claude
/// only where the list offers one.
pub fn listed(known: &Known, value: &str) -> Result<(PathBuf, String), String> {
    // Every place the list could hold, not only the most recent, which
    // another session may have pushed past since the sheet was read.
    let offered = choices_up_to(known, usize::MAX)
        .into_iter()
        .flat_map(|(_, choices)| choices)
        .any(|c| c.value == value);
    if !offered {
        return Err(format!(
            "{value} isn't among the places to start a Claude in, so nothing started."
        ));
    }
    let dir = expand(&known.home, value);
    // Never someone else's PR, wherever its worktree was moved.
    if crate::trust::is_untrusted_of(&known.home, &dir) {
        return Err(format!(
            "{value} holds someone else's PR, checked out untrusted, so nothing started there."
        ));
    }
    Ok((dir, format!("`{value}`")))
}

/// Text from elsewhere as it may be typed into a pane: without control
/// characters but line breaks and tabs, since an Escape in it would end a
/// paste early or start a key sequence of its own, and a carriage return
/// would press Enter.
pub fn typeable(text: &str) -> String {
    text.replace("\r\n", "\n")
        .chars()
        .filter(|c| !c.is_control() || matches!(c, '\n' | '\t'))
        .collect()
}

/// A prompt quoted for the shell on one line, as `claude <prompt>` is
/// typed into a new tab's shell.
pub fn shell_quote(text: &str) -> String {
    static BREAK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s*\n\s*").expect("a regex"));
    let one_line = BREAK.replace_all(&typeable(text), " ").replace('\t', " ");
    format!("'{}'", one_line.replace('\'', r"'\''"))
}

/// The line typed into a new tab's shell to start Claude with `prompt`,
/// so the shell stays once Claude exits; Claude alone with none.
pub fn claude_line(prompt: &str) -> String {
    if prompt.trim().is_empty() {
        "claude".into()
    } else {
        format!("claude {}", shell_quote(prompt.trim()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "remote-places-{name}-{}-{}",
            std::process::id(),
            util::now_ms()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir.canonicalize().unwrap()
    }

    fn repo(dir: &Path) {
        std::fs::create_dir_all(dir).unwrap();
        assert!(crate::worktree::git_ok(dir, &["init", "-q"]));
    }

    #[test]
    fn a_path_is_short_under_the_home() {
        let home = Path::new("/h");
        assert_eq!(short(home, Path::new("/h/work")), "~/work");
        assert_eq!(short(home, Path::new("/other")), "/other");
        assert_eq!(short(home, Path::new("/hx")), "/hx", "not under /h");
        assert_eq!(expand(home, "~/work"), PathBuf::from("/h/work"));
        assert_eq!(expand(home, "~"), PathBuf::from("/h"));
        assert_eq!(expand(home, "/x"), PathBuf::from("/x"));
    }

    #[test]
    fn a_prompt_is_quoted_on_one_line_without_control_characters() {
        assert_eq!(
            claude_line("fix the 'login'\n  bug"),
            "claude 'fix the '\\''login'\\'' bug'"
        );
        assert_eq!(claude_line("fix\u{1b}[201~ it\r"), "claude 'fix[201~ it'");
        assert_eq!(claude_line("  "), "claude");
    }

    #[test]
    fn the_list_groups_by_repository_and_offers_only_what_it_lists() {
        let root = temp("list");
        let home = root.join("home");
        let work = home.join("work");
        repo(&work);
        let loose = home.join("loose");
        std::fs::create_dir_all(&loose).unwrap();
        let review = home.join(".cache/switchboard/pr-review/o-r/12345");
        repo(&review);
        let known = Known {
            home: home.clone(),
            pane_dirs: vec![work.clone(), loose.clone(), review.clone()],
        };
        let groups = place_choices(&known);
        let names: Vec<&str> = groups.iter().map(|(g, _)| g.as_str()).collect();
        assert_eq!(names, ["work", "Other directories"], "{groups:?}");
        assert_eq!(groups[0].1[0].value, "~/work");
        assert_eq!(groups[1].1[0].value, "~/loose");

        assert_eq!(listed(&known, "~/work").unwrap().0, work);
        let elsewhere = listed(&known, "~/nowhere").unwrap_err();
        assert!(elsewhere.contains("isn't among the places"), "{elsewhere}");
        let untrusted = listed(&known, "~/.cache/switchboard/pr-review/o-r/12345").unwrap_err();
        assert!(untrusted.contains("isn't among the places"), "{untrusted}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_name_finds_its_repository_and_an_untrusted_path_is_refused() {
        let root = temp("resolve");
        let home = root.join("home");
        let work = home.join("work");
        repo(&work);
        let review = home.join(".cache/switchboard/pr-review/o-r/12345");
        std::fs::create_dir_all(&review).unwrap();
        let known = Known {
            home: home.clone(),
            pane_dirs: vec![work.clone()],
        };
        let (dir, said) = resolve_place(&known, "work").unwrap();
        assert_eq!(dir, work);
        assert!(said.starts_with("`~/work` (work, "), "{said}");
        assert!(
            resolve_place(&known, "zzz")
                .unwrap_err()
                .contains("Nothing matches `zzz`")
        );
        assert!(
            resolve_place(&known, "~/no-such-dir")
                .unwrap_err()
                .contains("There's no directory `~/no-such-dir`")
        );
        assert!(
            resolve_place(&known, "~/.cache/switchboard/pr-review/o-r/12345")
                .unwrap_err()
                .contains("holds someone else's PR, checked out untrusted")
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
