//! `land-lock`: the landing lock of queue-and-clean's "Land on the default
//! branch", ported from claude/skills/queue-and-clean/scripts/land-lock.js.
//! One worktree of a repository lands on its default branch at a time, so
//! two never rebase onto the same one and push over each other. Each tests
//! and reviews before taking it. It lives in the main clone's git directory,
//! shared by every worktree of it, and so does the queue of the worktrees
//! waiting for it, which lands them in its order.
//!
//! There is one lock and queue per branch landed on, so worktrees landing
//! onto different branches never wait on each other. The branch is
//! `--target <branch>` when given, else the worktree's
//! `branch.<branch>.wtnTarget` (the target autopilot cut it from), else the
//! default branch. The default branch keeps the names `land.lock` and
//! `land.queue`, so a landing in flight from before goes on; another has
//! `land.lock.<branch>` and `land.queue.<branch>`, its `%` written `%25` and
//! its `/` `%2F` (`file_suffix`), which src/sources/landing.rs reads back.
//!
//! ```text
//! land-lock take      takes it for this worktree: exit 0 taken (or held by
//!                     this worktree already, so a landing checks it still
//!                     holds it after a wait), 3 held by another or others
//!                     ahead in the queue, which it prints, this worktree
//!                     joining the queue
//! land-lock wait      takes it, waiting in the queue for its turn
//!                     (every LAND_LOCK_POLL seconds, 5 by default); exit 4
//!                     when taken out of the queue meanwhile (mission
//!                     control's Landing view), rather than joining again
//! land-lock release   lets it go, when this worktree holds it, and takes
//!                     this worktree out of the queue
//! land-lock leave     takes this worktree out of the queue
//! land-lock holder    prints which worktree holds it, nothing when none
//! land-lock queue     prints the holder ("landing") and each waiter by its
//!                     place, one a line: <place>\t<path>\t<since>
//! land-lock move <path> up|down|first
//!                     moves a waiter in the queue
//! land-lock phase [words]
//!                     shows the landing's step in this tab, the pane's
//!                     @agent_phase, its loop marked queue-and-clean's
//!                     (@agent_loop), since a session following the skill by
//!                     reading it never invokes it; no words clear it.
//!                     `tests failed [words]` and `review blocked [words]`
//!                     mark a failed stage, which the Landing view draws red
//!                     with the words after them
//! land-lock autolanded <short hash> [--plan <file> | --decided <n>] [item]
//!                     says this tab landed without asking (autopilot's
//!                     "Land without asking"): marks the tab
//!                     @autopilot_autoland with the hash, which the sidebar
//!                     shows, and adds the landing to the notifications
//!                     (autolanded.json in the state directory), with the
//!                     count of its plan's `Decided hands-free` lines (or,
//!                     for an item with no plan, the count given)
//! ```
//!
//! A landing holds it only to rebase, test and push, but one whose change no
//! longer matches its review reviews again and asks again while holding it,
//! which can wait on you for as long as you like, so the lock never expires
//! with time: it is taken over only from a landing that is gone, its
//! worktree removed or no Claude session running in it. A waiter gone the
//! same way is dropped from the queue as it is read.
//!
//! The Node version appended to the sidebar daemon's nudge file after every
//! change, so the sidebar looked again. The server reads `land.lock` and
//! `land.queue` itself on every tick (src/sources/landing.rs), so nothing is
//! nudged, and `TMUX_SIDEBAR_STATE_DIR` went with the nudge. The phase is
//! set on the caller's pane through the server, in place of `tmux set -p`.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use serde_json::{Value, json};

use crate::mux::socket;
use crate::{util, worktree};

/// How long a queue mutex left by a killed process stands before it is
/// taken over.
const STALE_MUTEX_MS: i64 = 10_000;
/// How long a change to the queue waits on the mutex between tries, and how
/// many tries it makes.
const MUTEX_WAIT: Duration = Duration::from_millis(20);
const MUTEX_TRIES: u32 = 500;

const USAGE: &str = "usage: land-lock [--target <branch>] take|wait|release|leave|holder|queue|move|phase [words]|autolanded <short hash> [--plan <file> | --decided <n>] [item]";

pub fn main(mut args: Vec<String>) -> ExitCode {
    let mut given_target = None;
    if let Some(at) = args.iter().position(|a| a == "--target") {
        if at + 1 >= args.len() {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
        given_target = Some(args.remove(at + 1));
        args.remove(at);
    }
    let command = args.first().map(String::as_str).unwrap_or("");
    // The phase needs no repository, so it is set before the lookup below:
    // the landing clears it last, after its cleanup has removed the worktree.
    if command == "phase" {
        phase(&args[1..]);
        return ExitCode::SUCCESS;
    }
    // Needs no repository either: it runs once the landing has pushed,
    // whatever the cleanup has removed since.
    if command == "autolanded" {
        return autolanded(&args[1..]);
    }
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let found = common_dir(&cwd).and_then(|common| {
        let here = worktree::git_at(&cwd, &["rev-parse", "--show-toplevel"]).ok()?;
        Some((common, here))
    });
    let Some((common, here)) = found else {
        eprintln!("not in a git repository");
        return ExitCode::from(2);
    };
    let here = PathBuf::from(here);
    let target = given_target.or_else(|| recorded_target(&here));
    let suffix = target
        .filter(|t| crate::tabs::git::default_branch(&common).as_deref() != Some(t.as_str()))
        .map(|t| file_suffix(&t));
    let lock = Lock::new(&common, here, suffix.as_deref());
    let result = match command {
        "take" => lock.take(false).map(ExitCode::from),
        "wait" => {
            let poll = std::env::var("LAND_LOCK_POLL")
                .ok()
                .and_then(|p| p.trim().parse::<f64>().ok())
                .filter(|p| p.is_finite() && *p >= 0.0)
                .unwrap_or(5.0);
            // A waiter taken out of the queue by someone else (mission
            // control's Landing view) stops, rather than joining again at
            // the back.
            let here = lock.here.to_string_lossy().into_owned();
            let mut waited = false;
            loop {
                if waited {
                    match lock.with_queue(|queue| {
                        let there = queue.iter().any(|e| entry_path(e) == Some(here.as_str()));
                        (None, there)
                    }) {
                        Ok(false) => {
                            println!("taken out of the landing queue");
                            break Ok(ExitCode::from(4));
                        }
                        Ok(true) => {}
                        Err(e) => break Err(e),
                    }
                }
                match lock.take(true) {
                    Ok(0) => break Ok(ExitCode::SUCCESS),
                    Ok(_) => std::thread::sleep(Duration::from_secs_f64(poll)),
                    Err(e) => break Err(e),
                }
                waited = true;
            }
        }
        // Every way out of a landing runs release, so it leaves the queue
        // too: a waiter stopped while its session goes on would otherwise
        // hold up every worktree behind it.
        "release" => lock.release().map(|()| ExitCode::SUCCESS),
        "leave" => lock
            .with_queue(|queue| (Some(without(queue, &lock.here)), ()))
            .map(|()| ExitCode::SUCCESS),
        "holder" => {
            if let Some(path) = lock.holder().and_then(|h| holder_path(&h)) {
                println!("{path}");
            }
            Ok(ExitCode::SUCCESS)
        }
        "queue" => lock.print_queue().map(|()| ExitCode::SUCCESS),
        "move" => {
            let (Some(target), Some(way)) = (args.get(1), args.get(2)) else {
                eprintln!("usage: land-lock move <path> up|down|first");
                return ExitCode::from(2);
            };
            if !matches!(way.as_str(), "up" | "down" | "first") {
                eprintln!("usage: land-lock move <path> up|down|first");
                return ExitCode::from(2);
            }
            lock.move_waiter(&cwd, target, way)
        }
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    result.unwrap_or_else(|e| {
        eprintln!("land-lock: {e}");
        ExitCode::FAILURE
    })
}

/// Shows the landing's step on the caller's pane, and marks its loop
/// queue-and-clean's; no words clear the step and leave the loop. Outside a
/// Switchboard pane it does nothing.
fn phase(words: &[String]) {
    if !socket::in_pane() {
        return;
    }
    let words = worktree::one_line(&words.join(" "));
    if words.is_empty() {
        let _ = socket::ask(&["set", "-u", "@agent_phase"]);
    } else {
        let _ = socket::ask(&["set", "@agent_phase", &words]);
        let _ = socket::ask(&["set", "@agent_loop", "queue"]);
    }
}

/// A landing made without asking, said: the caller's tab marked
/// `@autopilot_autoland` with the hash (its tab whether a tool stands in for
/// Claude's pane or not, since a parked pane keeps its tab), and the landing
/// added to `autolanded.json`, which the server reads into the
/// notifications ("Landed <hash>: <item>"), naming the tab while it is
/// open.
fn autolanded(args: &[String]) -> ExitCode {
    let mut args = args.to_vec();
    let mut plan_file = None;
    if let Some(at) = args.iter().position(|a| a == "--plan")
        && at + 1 < args.len()
    {
        plan_file = Some(PathBuf::from(args.remove(at + 1)));
        args.remove(at);
    }
    // With no plan, the count of the decisions written under the item.
    let mut decided_given = None;
    if let Some(at) = args.iter().position(|a| a == "--decided")
        && at + 1 < args.len()
    {
        decided_given = args.remove(at + 1).parse::<usize>().ok();
        args.remove(at);
    }
    let notes = crate::notes::store::notes_dir();
    // Both resolved, since wtn names the plan by its real path and the
    // notes directory may be a link.
    let notes = notes.canonicalize().unwrap_or(notes);
    let real_plan = plan_file.as_ref().and_then(|f| f.canonicalize().ok());
    let plan = real_plan
        .as_ref()
        .and_then(|f| f.strip_prefix(&notes).ok())
        .map(|rel| rel.to_string_lossy().into_owned())
        .unwrap_or_default();
    let decided = plan_file
        .and_then(|f| fs::read_to_string(f).ok())
        .map(|text| crate::autopilot::landed::decided_in(&text))
        .or(decided_given)
        .unwrap_or(0);
    let Some(hash) = args.first().filter(|h| !h.is_empty()).cloned() else {
        eprintln!(
            "usage: land-lock autolanded <short hash> [--plan <file> | --decided <n>] [item]"
        );
        return ExitCode::from(2);
    };
    let mut tab = String::new();
    if socket::in_pane() {
        let _ = socket::ask(&["set", "@autopilot_autoland", &hash]);
        tab = socket::ask_ok(&["display", "tab"])
            .map(|t| t.trim().to_string())
            .unwrap_or_default();
    }
    let landing = crate::autopilot::landed::Landing {
        hash,
        item: worktree::one_line(&args[1..].join(" ")),
        tab,
        at: util::now_ms(),
        plan,
        decided,
    };
    match crate::autopilot::landed::add(&crate::paths::state_dir(), landing) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("land-lock: couldn't keep the landing: {e}");
            ExitCode::FAILURE
        }
    }
}

/// The lock and queue of one repository, from one of its worktrees.
struct Lock {
    /// `<common git dir>/land.lock`, a directory, made to take the lock.
    dir: PathBuf,
    holder_file: PathBuf,
    queue_file: PathBuf,
    queue_mutex: PathBuf,
    /// The worktree this runs in.
    here: PathBuf,
}

impl Lock {
    /// `suffix` names a branch other than the default one (`file_suffix`).
    fn new(common: &Path, here: PathBuf, suffix: Option<&str>) -> Lock {
        let named = |base: &str| match suffix {
            Some(s) => format!("{base}.{s}"),
            None => base.to_string(),
        };
        let dir = common.join(named("land.lock"));
        let queue = named("land.queue");
        Lock {
            holder_file: dir.join("holder"),
            dir,
            queue_mutex: common.join(format!("{queue}.lock")),
            queue_file: common.join(queue),
            here,
        }
    }

    fn holder(&self) -> Option<Value> {
        util::read_json(&self.holder_file)
    }

    fn hold(&self) -> io::Result<()> {
        let holder = json!({
            "path": self.here.to_string_lossy(),
            "since": util::js_number(util::now_ms() as f64),
        });
        fs::write(&self.holder_file, util::json_compact(&holder))
    }

    /// Changes the queue under its mutex, so two worktrees joining at once
    /// can't lose one; one left behind by a killed process for over ten
    /// seconds is taken over. `change` is given the queue with its gone
    /// waiters dropped, and gives the queue to keep (`None` for as it is)
    /// and its answer. The queue is written whole and renamed into place,
    /// and only when it changed.
    fn with_queue<T>(
        &self,
        change: impl FnOnce(Vec<Value>) -> (Option<Vec<Value>>, T),
    ) -> io::Result<T> {
        let mut tries = 0;
        loop {
            match fs::create_dir(&self.queue_mutex) {
                Ok(()) => break,
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                    let stale = fs::metadata(&self.queue_mutex)
                        .and_then(|m| m.modified())
                        .ok()
                        .and_then(|t| t.elapsed().ok())
                        .is_some_and(|age| age.as_millis() as i64 > STALE_MUTEX_MS);
                    if stale {
                        let _ = fs::remove_dir_all(&self.queue_mutex);
                    }
                    tries += 1;
                    if tries > MUTEX_TRIES {
                        return Err(io::Error::other("the landing queue stayed locked"));
                    }
                    std::thread::sleep(MUTEX_WAIT);
                }
                Err(e) => return Err(e),
            }
        }
        let result = (|| {
            let read = util::read_json(&self.queue_file).unwrap_or(Value::Array(Vec::new()));
            let queue: Vec<Value> = read
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .filter(|entry| entry_path(entry).is_some() && !gone(Some(entry)))
                .collect();
            let (next, answer) = change(queue.clone());
            let next = Value::Array(next.unwrap_or(queue));
            if next != read {
                // Named apart from every queue's file, which the server
                // finds by `land.queue.`.
                let temporary = self.queue_file.with_file_name(format!(
                    "land.tmp.{}.{}",
                    std::process::id(),
                    self.queue_file
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default()
                ));
                fs::write(&temporary, util::json_compact(&next))?;
                fs::rename(&temporary, &self.queue_file)?;
            }
            Ok(answer)
        })();
        let _ = fs::remove_dir_all(&self.queue_mutex);
        result
    }

    /// Takes the lock when it is free (or its holder gone) and no live
    /// waiter is ahead of this worktree in the queue, so a take can't jump
    /// the queue: 0 taken, 3 not, this worktree then in the queue.
    fn take(&self, quiet: bool) -> io::Result<u8> {
        let here = self.here.to_string_lossy().into_owned();
        self.with_queue(|queue| {
            let current = self.holder();
            let held = self.dir.exists();
            if held && current.as_ref().and_then(holder_path).as_deref() == Some(here.as_str()) {
                return (Some(without(queue, &self.here)), Ok(0));
            }
            let mine = queue
                .iter()
                .position(|e| entry_path(e) == Some(here.as_str()));
            let ahead = mine.unwrap_or(queue.len());
            let free = !held || gone(current.as_ref());
            if free && ahead == 0 {
                let made = match fs::create_dir(&self.dir) {
                    Err(e) if e.kind() != io::ErrorKind::AlreadyExists => Err(e),
                    _ => self.hold(),
                };
                return (Some(without(queue, &self.here)), made.map(|()| 0));
            }
            if !quiet {
                let held_by = match (free, current.as_ref()) {
                    (false, Some(current)) => format!(
                        "held by {} since {}",
                        holder_path(current).unwrap_or_default(),
                        clock(current.get("since").and_then(Value::as_f64).unwrap_or(0.0))
                    ),
                    _ => String::new(),
                };
                let waiting = if ahead > 0 {
                    format!("{ahead} ahead in the landing queue")
                } else {
                    String::new()
                };
                let said: Vec<String> = [held_by, waiting]
                    .into_iter()
                    .filter(|s| !s.is_empty())
                    .collect();
                println!("{}", said.join("; "));
            }
            let joined = if mine.is_some() {
                queue
            } else {
                let mut queue = queue;
                queue.push(json!({
                    "path": here,
                    "since": util::js_number(util::now_ms() as f64),
                }));
                queue
            };
            (Some(joined), Ok(3))
        })?
    }

    fn release(&self) -> io::Result<()> {
        self.with_queue(|queue| (Some(without(queue, &self.here)), ()))?;
        let here = self.here.to_string_lossy();
        if self.holder().as_ref().and_then(holder_path).as_deref() == Some(&*here) {
            let _ = fs::remove_dir_all(&self.dir);
        }
        Ok(())
    }

    fn print_queue(&self) -> io::Result<()> {
        let queue = self.with_queue(|queue| (None, queue))?;
        let current = self
            .holder()
            .filter(|h| self.dir.exists() && !gone(Some(h)));
        let mut lines = Vec::new();
        if let Some(current) = &current {
            lines.push(format!(
                "landing\t{}\t{}",
                holder_path(current).unwrap_or_default(),
                current
                    .get("since")
                    .map(util::js_string)
                    .unwrap_or_default()
            ));
        }
        let first = if current.is_some() { 2 } else { 1 };
        for (at, entry) in queue.iter().enumerate() {
            lines.push(format!(
                "{}\t{}\t{}",
                ordinal(at + first),
                entry_path(entry).unwrap_or_default(),
                entry.get("since").map(util::js_string).unwrap_or_default()
            ));
        }
        for line in lines {
            println!("{line}");
        }
        Ok(())
    }

    fn move_waiter(&self, cwd: &Path, target: &str, way: &str) -> io::Result<ExitCode> {
        let given = worktree::normalize(&cwd.join(target));
        let place = given.canonicalize().unwrap_or(given);
        let place = place.to_string_lossy().into_owned();
        let moved = self.with_queue(|queue| {
            let Some(at) = queue
                .iter()
                .position(|e| entry_path(e) == Some(place.as_str()))
            else {
                return (None, false);
            };
            let to = match way {
                "first" => 0,
                "up" => at.saturating_sub(1),
                _ => (at + 1).min(queue.len() - 1),
            };
            let mut next = queue;
            let entry = next.remove(at);
            next.insert(to, entry);
            (Some(next), true)
        })?;
        if !moved {
            eprintln!("{place} isn't waiting in the landing queue");
            return Ok(ExitCode::from(3));
        }
        Ok(ExitCode::SUCCESS)
    }
}

/// The branch the worktree at `here` lands on, as `wtn --target` recorded
/// it, `None` when it named none.
fn recorded_target(here: &Path) -> Option<String> {
    let branch = worktree::git_at(here, &["rev-parse", "--abbrev-ref", "HEAD"]).ok()?;
    worktree::git_at(
        here,
        &["config", "--get", &format!("branch.{branch}.wtnTarget")],
    )
    .ok()
    .filter(|t| !t.is_empty())
}

/// A branch as the end of its lock's and queue's file names: `%` as `%25`
/// and `/` as `%2F`, so it is one file name and reads back as the branch.
pub fn file_suffix(branch: &str) -> String {
    branch.replace('%', "%25").replace('/', "%2F")
}

/// A file name's end back as its branch, `file_suffix` undone.
pub fn branch_of_suffix(suffix: &str) -> String {
    suffix.replace("%2F", "/").replace("%25", "%")
}

/// An entry's or holder's worktree.
fn entry_path(entry: &Value) -> Option<&str> {
    entry
        .get("path")
        .and_then(Value::as_str)
        .filter(|p| !p.is_empty())
}

fn holder_path(holder: &Value) -> Option<String> {
    entry_path(holder).map(str::to_string)
}

fn without(queue: Vec<Value>, here: &Path) -> Vec<Value> {
    let here = here.to_string_lossy();
    queue
        .into_iter()
        .filter(|e| entry_path(e) != Some(&*here))
        .collect()
}

/// The repository's shared git directory, from any of its worktrees, as an
/// absolute path with its links resolved. `--path-format=absolute` would say
/// it at once, but needs git 2.31, so the relative answer is resolved here.
pub fn common_dir(cwd: &Path) -> Option<PathBuf> {
    let common = worktree::git_at(cwd, &["rev-parse", "--git-common-dir"]).ok()?;
    cwd.join(common).canonicalize().ok()
}

/// The worktree a directory is in: the nearest one up holding a `.git`. A
/// session in a worktree nested inside another (the main clone's
/// `.claude/worktrees`) is that one's, not the outer one's.
fn worktree_of(dir: &Path) -> Option<PathBuf> {
    dir.ancestors()
        .find(|at| at.join(".git").exists())
        .map(|at| at.canonicalize().unwrap_or_else(|_| at.to_path_buf()))
}

/// Whether a landing (the holder, or a waiter) is gone: none, its worktree
/// removed, or no running Claude session in it.
fn gone(current: Option<&Value>) -> bool {
    let Some(path) = current.and_then(entry_path) else {
        return true;
    };
    let path = Path::new(path);
    if !path.exists() {
        return true;
    }
    !worktree::live_sessions_in(path).iter().any(|session| {
        session
            .get("cwd")
            .and_then(Value::as_str)
            .and_then(|cwd| worktree_of(Path::new(cwd)))
            .is_some_and(|w| w == path)
    })
}

/// A time in milliseconds as the hour and minute it is here, `HH:MM`.
fn clock(ms: f64) -> String {
    let at = util::local_at_ms(ms as i64);
    format!("{:02}:{:02}", at.hour(), at.minute())
}

/// 1st, 2nd, 3rd, 4th, 11th, 12th, 13th, 21st.
fn ordinal(n: usize) -> String {
    let suffix = if (11..=13).contains(&(n % 100)) {
        "th"
    } else {
        match n % 10 {
            1 => "st",
            2 => "nd",
            3 => "rd",
            _ => "th",
        }
    };
    format!("{n}{suffix}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn places_are_ordinals() {
        let all: Vec<String> = [1, 2, 3, 4, 11, 12, 13, 21, 22, 101, 111]
            .iter()
            .map(|n| ordinal(*n))
            .collect();
        assert_eq!(
            all,
            [
                "1st", "2nd", "3rd", "4th", "11th", "12th", "13th", "21st", "22nd", "101st",
                "111th"
            ]
        );
    }

    #[test]
    fn a_branch_is_one_file_name_and_reads_back() {
        for branch in ["team/next", "a%2Fb/c", "plain"] {
            let suffix = file_suffix(branch);
            assert!(!suffix.contains('/'));
            assert_eq!(branch_of_suffix(&suffix), branch);
        }
        assert_eq!(file_suffix("team/next"), "team%2Fnext");
    }

    #[test]
    fn a_landing_with_no_worktree_is_gone() {
        assert!(gone(None));
        assert!(gone(Some(&json!({ "path": "/no/such/worktree" }))));
        assert!(gone(Some(&json!({ "since": 1 }))));
    }
}
