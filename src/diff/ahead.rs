//! M-d's diff story told ahead of time, so M-d opens on the story at once
//! rather than waiting on Haiku: the server's part, ported from
//! lib/sidebar/story-ahead.js. The telling itself is the diff's
//! (`switchboard-diff`'s `ahead`), which the server hands a tab's new
//! commit as the plugin API's `Event::Commit`, by running
//! `switchboard diff --ahead <dir>`.
//!
//! When a tab's branch has a commit whose story isn't told yet, the server
//! runs `switchboard diff --ahead <dir>` in the background, one branch at a
//! time, each commit once, a failed one tried again after half an hour. The
//! branch's commit is read from git's files, not by starting git, since the
//! server looks every minute. The default branch counts too, since commits
//! not pushed yet are its diff; one with none is told in a moment, its diff
//! being empty. Only with the diff_story and diff_story_ahead settings on
//! (diff_order too, which the story needs), and github, which finds the
//! PR's base.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use switchboard_plugin_api::Event;

use crate::{settings, util};

/// How long the server gives the program.
const PROGRAM_TIMEOUT: Duration = Duration::from_secs(360);
/// A commit whose story couldn't be told is tried again after this.
const RETRY: Duration = Duration::from_secs(30 * 60);
/// How often the server looks for a commit to tell.
const LOOK_EVERY: Duration = Duration::from_secs(60);

// ── The program ─────────────────────────────────────────────────────────────

/// `switchboard diff --ahead [--plan <note>] <dir>`: the commit handed to
/// the diff as the event it is. Exit 0 when its story is told (or there was
/// none to tell), else not, with why on stderr, which the server's log
/// keeps.
pub fn main(dir: &Path, plan: &str) -> ExitCode {
    let event = Event::Commit {
        dir: dir.to_path_buf(),
        plan: plan.to_string(),
    };
    match super::plugin().on(event, super::host::core()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(why) => {
            eprintln!("{why}");
            ExitCode::from(2)
        }
    }
}

// ── The server's part ───────────────────────────────────────────────────────

/// A repository's top, its branch and the branch's commit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Head {
    pub top: PathBuf,
    pub branch: String,
    pub sha: String,
}

/// A directory's repository: its top, its branch and the branch's commit,
/// from the files under .git; `None` outside one or on no branch.
pub fn head_of(dir: &Path) -> Option<Head> {
    let top = dir
        .ancestors()
        .find(|d| d.join(".git").exists())?
        .to_path_buf();
    let mut git_dir = top.join(".git");
    if git_dir.is_file() {
        let text = util::read_text(&git_dir)?;
        let pointer = text
            .lines()
            .find_map(|l| l.strip_prefix("gitdir: "))?
            .trim();
        git_dir = top.join(pointer);
    }
    let common = match util::read_text(git_dir.join("commondir")) {
        Some(c) => git_dir.join(c.trim()),
        None => git_dir.clone(),
    };
    let head = util::read_text(git_dir.join("HEAD"))?;
    let reference = head.trim().strip_prefix("ref: ")?.to_string();
    let branch = reference.strip_prefix("refs/heads/")?.to_string();
    let sha = util::read_text(common.join(&reference))
        .map(|s| s.trim().to_string())
        .or_else(|| {
            util::read_text(common.join("packed-refs"))?
                .lines()
                .find(|l| l.ends_with(&format!(" {reference}")))
                .and_then(|l| l.split(' ').next())
                .map(str::to_string)
        })
        .filter(|s| !s.is_empty())?;
    Some(Head { top, branch, sha })
}

#[derive(Default)]
struct Ahead {
    told: HashMap<PathBuf, String>,
    failed: HashMap<(PathBuf, String), Instant>,
    running: bool,
    looked: Option<Instant>,
}

static AHEAD: Mutex<Option<Ahead>> = Mutex::new(None);

/// Whether it is time to look for a commit to tell: none being told, and
/// none looked for in the last minute. The first look comes a minute after
/// the server starts, as the daemon's minute timer first fired then.
pub fn due() -> bool {
    let mut guard = AHEAD.lock().unwrap_or_else(|e| e.into_inner());
    let state = guard.get_or_insert_with(|| Ahead {
        looked: Some(Instant::now()),
        ..Ahead::default()
    });
    !state.running && state.looked.is_none_or(|at| at.elapsed() >= LOOK_EVERY)
}

/// Whether stories are told ahead now: the settings it needs on, and Claude
/// there to tell them (else every commit would be fetched for nothing).
pub fn wanted() -> bool {
    ["diff_order", "diff_story", "diff_story_ahead", "github"]
        .iter()
        .all(|k| settings::is_on(k))
        && super::tell::claude_command().is_some()
}

/// The first of `heads` whose commit isn't told, nor failed lately, each
/// repository once.
fn next<'a>(
    heads: &'a [Head],
    told: &HashMap<PathBuf, String>,
    failed: &HashMap<(PathBuf, String), Instant>,
) -> Option<&'a Head> {
    let mut seen: Vec<&Path> = Vec::new();
    heads.iter().find(|h| {
        if seen.contains(&h.top.as_path()) {
            return false;
        }
        seen.push(&h.top);
        told.get(&h.top) != Some(&h.sha)
            && failed
                .get(&(h.top.clone(), h.sha.clone()))
                .is_none_or(|at| at.elapsed() >= RETRY)
    })
}

/// Tells the story of the first tab's branch with a commit not told yet, in
/// the background, at most once a minute and one at a time. `dirs` are the
/// tabs' directories in order, each with its tab's plan note ("" for
/// none); `exe` the program to run.
pub fn story_ahead(dirs: &[(PathBuf, String)], exe: &str) {
    let mut guard = AHEAD.lock().unwrap_or_else(|e| e.into_inner());
    let state = guard.get_or_insert_with(Ahead::default);
    if state.running || state.looked.is_some_and(|at| at.elapsed() < LOOK_EVERY) {
        return;
    }
    state.looked = Some(Instant::now());
    let heads: Vec<Head> = dirs.iter().filter_map(|(d, _)| head_of(d)).collect();
    let Some(head) = next(&heads, &state.told, &state.failed).cloned() else {
        return;
    };
    let plan = dirs
        .iter()
        .find(|(d, _)| head_of(d).is_some_and(|h| h.top == head.top))
        .map(|(_, p)| p.clone())
        .unwrap_or_default();
    state.running = true;
    drop(guard);
    let exe = exe.to_string();
    std::thread::spawn(move || {
        let outcome = run_program(&exe, &head.top, &plan);
        let mut guard = AHEAD.lock().unwrap_or_else(|e| e.into_inner());
        let state = guard.get_or_insert_with(Ahead::default);
        state.running = false;
        match outcome {
            Ok(said) => {
                if !said.is_empty() {
                    eprintln!("story ahead: {}: told, but {said}", head.branch);
                }
                state.told.insert(head.top.clone(), head.sha.clone());
            }
            Err(why) => {
                state
                    .failed
                    .insert((head.top.clone(), head.sha.clone()), Instant::now());
                eprintln!(
                    "story ahead: {}: not told ({why}); trying again later",
                    head.branch
                );
            }
        }
    });
}

/// Runs `switchboard diff --ahead [--plan <note>] <top>`: the last line it
/// said, or why it failed.
fn run_program(exe: &str, top: &Path, plan: &str) -> Result<String, String> {
    let mut cmd = Command::new(exe);
    cmd.args(["diff", "--ahead"]);
    if !plan.is_empty() {
        cmd.args(["--plan", plan]);
    }
    let mut child = cmd
        .arg(top)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    let mut stderr = child.stderr.take();
    let reader = std::thread::spawn(move || {
        let mut text = String::new();
        if let Some(e) = stderr.as_mut() {
            use std::io::Read;
            let _ = e.read_to_string(&mut text);
        }
        text
    });
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() >= PROGRAM_TIMEOUT => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "it took over {} seconds",
                    PROGRAM_TIMEOUT.as_secs()
                ));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(500)),
            Err(e) => return Err(e.to_string()),
        }
    };
    let said = reader.join().unwrap_or_default();
    let last = said
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .trim()
        .to_string();
    if status.success() {
        Ok(last)
    } else if last.is_empty() {
        Err(format!("exit {}", status.code().unwrap_or(-1)))
    } else {
        Err(last)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_branchs_commit_is_read_from_gits_files() {
        let dir = std::env::temp_dir().join(format!("sb-ahead-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let git_dir = dir.join(".git");
        std::fs::create_dir_all(git_dir.join("refs/heads")).unwrap();
        std::fs::write(git_dir.join("HEAD"), "ref: refs/heads/feature\n").unwrap();
        std::fs::write(
            git_dir.join("packed-refs"),
            "# pack-refs\nabc123 refs/heads/feature\n",
        )
        .unwrap();
        std::fs::create_dir_all(dir.join("src")).unwrap();
        let head = head_of(&dir.join("src")).expect("a repository");
        assert_eq!(head.branch, "feature");
        assert_eq!(head.sha, "abc123", "from packed-refs");
        std::fs::write(git_dir.join("refs/heads/feature"), "def456\n").unwrap();
        assert_eq!(head_of(&dir).unwrap().sha, "def456", "a loose ref first");
        std::fs::write(git_dir.join("HEAD"), "def456\n").unwrap();
        assert_eq!(head_of(&dir), None, "on no branch");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_next_branch_is_one_not_told_nor_failed_lately_each_repository_once() {
        let h = |top: &str, sha: &str| Head {
            top: PathBuf::from(top),
            branch: "b".into(),
            sha: sha.into(),
        };
        let heads = [h("/a", "1"), h("/a", "2"), h("/b", "3"), h("/c", "4")];
        let mut told = HashMap::new();
        let mut failed = HashMap::new();
        assert_eq!(
            next(&heads, &told, &failed).unwrap().top,
            PathBuf::from("/a")
        );
        told.insert(PathBuf::from("/a"), "1".to_string());
        assert_eq!(
            next(&heads, &told, &failed).unwrap().top,
            PathBuf::from("/b"),
            "a repository's first tab only"
        );
        failed.insert((PathBuf::from("/b"), "3".to_string()), Instant::now());
        assert_eq!(
            next(&heads, &told, &failed).unwrap().top,
            PathBuf::from("/c")
        );
    }
}
