//! M-d's diff story told ahead of time, so M-d opens on the story at once
//! rather than waiting on Haiku: ported from lib/sidebar/story-ahead.js
//! (the server's part) and tmux/diff-story-ahead.sh (`switchboard diff
//! --ahead`, the program's).
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
//!
//! The program compares with the same base M-d would (git.rs), tells the
//! story as M-d would (tell.rs), keeping it in the cache by the diff's own
//! text, and quits once it is told: exit 0 when told (or there was none to
//! tell), else not, with why on stderr, which the server's log keeps. A
//! story told with a warning (its answer mostly named hunks the diff
//! doesn't have) exits 0 with the warning on stderr. The commit told is
//! kept per worktree, and a second run on the same one, or while one runs,
//! does nothing.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use super::tell::{Asked, Heard, Teller};
use super::{git, order, story};
use crate::{paths, settings, util};

/// How long the program gives the story, its own wait and a bit.
const GIVE_UP: Duration = Duration::from_secs(330);
/// How long the server gives the program.
const PROGRAM_TIMEOUT: Duration = Duration::from_secs(360);
/// A commit whose story couldn't be told is tried again after this.
const RETRY: Duration = Duration::from_secs(30 * 60);
/// How often the server looks for a commit to tell.
const LOOK_EVERY: Duration = Duration::from_secs(60);

// ── The program ─────────────────────────────────────────────────────────────

fn state_file(git_dir: &Path) -> PathBuf {
    paths::state_dir()
        .join("diff-story-ahead")
        .join(git_dir.to_string_lossy().replace('/', "%"))
}

/// The lock that keeps a worktree to one run at a time, taken away however
/// the run ends.
struct Lock(PathBuf);

impl Drop for Lock {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir(&self.0);
    }
}

/// `switchboard diff --ahead <dir>`.
pub fn main(dir: &Path) -> ExitCode {
    let Ok(facts) = git::facts(dir) else {
        return ExitCode::SUCCESS;
    };
    let state = state_file(&facts.git_dir);
    if util::read_text(&state).is_some_and(|t| t.trim() == facts.head) {
        return ExitCode::SUCCESS;
    }
    let lock = PathBuf::from(format!("{}.lock", state.display()));
    let _ = std::fs::create_dir_all(state.parent().unwrap_or(Path::new("/")));
    if std::fs::create_dir(&lock).is_err() {
        return ExitCode::SUCCESS;
    }
    let _lock = Lock(lock);
    // A base that couldn't be fetched isn't the one M-d will compare with
    // once it can: left to try again rather than told against the old one.
    let base = match git::cached_base(&facts.top) {
        Some(b) => b,
        None => match git::fetch_base(&facts.top) {
            Some(found) if found.fetched => found.base,
            Some(found) => {
                eprintln!("Could not fetch {}", found.base);
                return ExitCode::from(3);
            }
            None => {
                eprintln!("Couldn't work out the PR's base branch");
                return ExitCode::from(3);
            }
        },
    };
    match tell_ahead(&facts.top, &base) {
        Ok(warning) => {
            if let Some(w) = warning {
                eprintln!("{w}");
            }
            let _ = util::write_atomic(&state, format!("{}\n", facts.head));
            ExitCode::SUCCESS
        }
        Err(why) => {
            eprintln!("{why}");
            ExitCode::from(2)
        }
    }
}

/// Tells the story of the change against `base`, as M-d would; gives any
/// warning it was told with, or why it couldn't be told.
pub fn tell_ahead(top: &Path, base: &str) -> Result<Option<String>, String> {
    let change = git::read_change(top, base).map_err(|e| e.to_string())?;
    let paths: Vec<String> = {
        let mut p: Vec<String> = change.entries.iter().map(|e| e.path.clone()).collect();
        p.sort();
        p
    };
    // Nothing to tell: told, as far as the next commit is concerned.
    if paths.len() < 2 || paths.iter().all(|p| order::generated(p)) {
        return Ok(None);
    }
    let (diffs, hunks) = story::split(&[change.diff.as_str()]);
    let entries: Vec<story::Entry> = change
        .entries
        .iter()
        .map(|e| story::Entry {
            path: e.path.clone(),
            additions: e.additions,
            deletions: e.deletions,
        })
        .collect();
    let commits = story::commits(&change.raw_commits);
    let asked = Asked {
        key: story::key(&top.to_string_lossy(), &paths, &change.diff, &commits),
        input: story::input(&entries, &diffs, &hunks, &commits),
        paths,
        hunks,
    };
    let teller = Teller::default();
    let (tx, rx) = std::sync::mpsc::channel::<Heard>();
    {
        let teller = teller.clone();
        std::thread::spawn(move || {
            teller.tell(&asked, &|heard| {
                if !matches!(heard, Heard::Progress(_)) {
                    let _ = tx.send(heard);
                }
            });
        });
    }
    let heard = rx.recv_timeout(GIVE_UP);
    teller.stop();
    match heard {
        Ok(Heard::Told(story)) => Ok(story::missed_text(&story)),
        Ok(Heard::Untold(why)) => Err(why),
        Ok(Heard::Progress(_)) => Err("the story stopped".into()),
        Err(_) => Err(format!(
            "gave up waiting after {} seconds",
            GIVE_UP.as_secs()
        )),
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
/// tabs' directories in order; `exe` the program to run.
pub fn story_ahead(dirs: &[PathBuf], exe: &str) {
    let mut guard = AHEAD.lock().unwrap_or_else(|e| e.into_inner());
    let state = guard.get_or_insert_with(Ahead::default);
    if state.running || state.looked.is_some_and(|at| at.elapsed() < LOOK_EVERY) {
        return;
    }
    state.looked = Some(Instant::now());
    let heads: Vec<Head> = dirs.iter().filter_map(|d| head_of(d)).collect();
    let Some(head) = next(&heads, &state.told, &state.failed).cloned() else {
        return;
    };
    state.running = true;
    drop(guard);
    let exe = exe.to_string();
    std::thread::spawn(move || {
        let outcome = run_program(&exe, &head.top);
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

/// Runs `switchboard diff --ahead <top>`: the last line it said, or why it
/// failed.
fn run_program(exe: &str, top: &Path) -> Result<String, String> {
    let mut child = Command::new(exe)
        .args(["diff", "--ahead"])
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
