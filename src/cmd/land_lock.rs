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
//! Each queue is changed under a mutex directory, `land.mutex` and
//! `land.mutex.<branch>`, named apart from both so no branch's queue can be
//! another's mutex.
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
//!                     control's Landing view), or, after a requeue, while
//!                     it reviewed, rather than joining again
//! land-lock release   lets it go, when this worktree holds it, and takes
//!                     this worktree out of the queue
//! land-lock requeue   lets it go, when this worktree holds it, and puts
//!                     this worktree first in the queue, marked reviewing,
//!                     in one step, so no other take slips in between: a
//!                     landing reviewing its change again keeps its turn
//!                     without holding up the others, since a take passes
//!                     over a waiter marked reviewing; its next take or
//!                     wait drops the mark, ready to land
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
//! land-lock landed <short hash> <branch>
//!                     says this tab's work landed straight on <branch>, as
//!                     every direct landing does once it has pushed: marks
//!                     the tab @tab_landed, with the repository and its
//!                     main clone's branch, which files it under Merged in
//!                     the sidebar and mission control while the tab is in
//!                     that main clone on that branch; on the default
//!                     branch, with Settings' target_follows_default on, it
//!                     opens a tab running `update-target` for each branch
//!                     items land on (src/cmd/update_target.rs)
//! land-lock autolanded <short hash> [--plan <file> | --decided <n>] [--not-applied <labels>] [item]
//!                     says this tab landed without asking (autopilot's
//!                     "Land without asking"): marks the tab
//!                     @autopilot_autoland with the hash, which the sidebar
//!                     shows, and adds the landing to the notifications
//!                     (autolanded.json in the state directory), with the
//!                     count of its plan's `Decided hands-free` lines (or,
//!                     for an item with no plan, the count given), and
//!                     what the change needs to take effect that a
//!                     hands-free tab left for you (`--not-applied`); said
//!                     again for the same hash, it updates that landing
//! land-lock record clear | <tests|review|leak> <json> | found <json> | output <tests|review> <file>
//!                     keeps what this landing has done for the Landing
//!                     view's cells and detail, in landing.json in the
//!                     worktree's own git directory: a key's JSON object
//!                     (`tests` {ok, passed, failed, total, file}, `review`
//!                     {round, reviewer, clean_rounds, findings, summary},
//!                     `leak` {found}) in place of the last, stamped with
//!                     `at`; `found` adds an earlier round's finding
//!                     {round, title, fixed_by} to the list; `output` keeps
//!                     the file whole beside it (landing-output.log) and its
//!                     last 40 lines in the record; `clear` starts it again
//! ```
//!
//! A landing holds it only to rebase, test and push: one whose change no
//! longer matches its review lets it go to review again (`requeue`), and
//! takes it back first in line. It can still wait on you while holding it
//! (a failing default branch, a push question asked again in a tab that
//! landed without asking), so the lock never expires with time: it is taken over only from a landing that is gone, its
//! worktree removed, or neither the Claude session that took it nor any
//! other running in it. A waiter gone the same way is dropped from the
//! queue as it is read.
//!
//! `update-target`, which is no Claude session, holds a target's lock as its
//! process: its entries name the process (`pid`) in place of a session, and
//! count as there while it runs.
//!
//! Each entry, the holder's and every waiter's, names the worktree and since
//! when, and the Claude session that wrote it (`CLAUDE_CODE_SESSION_ID`,
//! which Claude Code sets in every command it runs) and its pane
//! (`SWITCHBOARD_PANE`) when there is one. The session keeps the landing
//! alive wherever it works: one started in the main clone that went into
//! the worktree to land it has the clone as its directory, so by its
//! directory alone it read as gone and its lock was taken over mid-landing.
//! A take from the worktree that holds it or waits, run by another session
//! than its entry names, names that one in its place, so a landing a new
//! session takes up is kept while that one runs. An entry without one,
//! written by a build from before, is read by its worktree alone.
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
/// `requeue`'s mark, in the worktree's own git directory.
const REQUEUED: &str = "land-requeued";

const USAGE: &str = "usage: land-lock [--target <branch>] take|wait|release|requeue|leave|holder|queue|move|phase [words]|record ...|landed <short hash> <branch>|autolanded <short hash> [--plan <file> | --decided <n>] [--not-applied <labels>] [item]";

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
    if command == "landed" {
        return landed(&args[1..]);
    }
    if command == "record" {
        return record(&args[1..]);
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
            lock.wait(Duration::from_secs_f64(poll)).map(|code| {
                if code == 4 {
                    println!("taken out of the landing queue");
                }
                ExitCode::from(code)
            })
        }
        // Every way out of a landing runs release, so it leaves the queue
        // too: a waiter stopped while its session goes on would otherwise
        // hold up every worktree behind it.
        "release" => lock.release().map(|()| ExitCode::SUCCESS),
        "requeue" => lock.requeue().map(|()| ExitCode::SUCCESS),
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

/// How many of the last output's lines the record keeps.
const OUTPUT_TAIL: usize = 40;

const RECORD_USAGE: &str = "usage: land-lock record clear | <tests|review|leak> <json> | found <json> | output <tests|review> <file>";

/// `land-lock record`, as the module's comment says.
fn record(args: &[String]) -> ExitCode {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let Some(dir) = worktree::git_at(&cwd, &["rev-parse", "--show-toplevel"])
        .ok()
        .and_then(|top| crate::sources::landing::own_git_dir(Path::new(&top)))
    else {
        eprintln!("not in a git repository");
        return ExitCode::from(2);
    };
    let words: Vec<&str> = args.iter().map(String::as_str).collect();
    match record_in(&dir, &words, util::now_ms()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(RecordError::Usage) => {
            eprintln!("{RECORD_USAGE}");
            ExitCode::from(2)
        }
        Err(RecordError::Io(e)) => {
            eprintln!("land-lock record: {e}");
            ExitCode::FAILURE
        }
    }
}

enum RecordError {
    Usage,
    Io(io::Error),
}

impl From<io::Error> for RecordError {
    fn from(e: io::Error) -> Self {
        RecordError::Io(e)
    }
}

/// Changes the record in the git directory `dir` as `words` say, at `now`
/// (ms), the rest of it kept.
fn record_in(dir: &Path, words: &[&str], now: i64) -> Result<(), RecordError> {
    use crate::sources::landing::{OUTPUT, RECORD};
    let file = dir.join(RECORD);
    let object = |text: &str| -> Result<Value, RecordError> {
        match serde_json::from_str::<Value>(text) {
            Ok(Value::Object(mut map)) => {
                map.insert("at".into(), util::js_number(now as f64));
                Ok(Value::Object(map))
            }
            _ => Err(RecordError::Usage),
        }
    };
    let mut data = util::read_json_object(&file);
    match words {
        ["clear"] => {
            let _ = fs::remove_file(dir.join(OUTPUT));
            data.clear();
        }
        [key @ ("tests" | "review" | "leak"), text] => {
            data.insert(key.to_string(), object(text)?);
        }
        ["found", text] => {
            let found = object(text)?;
            match data.get_mut("found").and_then(Value::as_array_mut) {
                Some(list) => list.push(found),
                None => {
                    data.insert("found".into(), json!([found]));
                }
            }
        }
        ["output", of @ ("tests" | "review"), path] => {
            let bytes = fs::read(path)?;
            util::write_atomic(dir.join(OUTPUT), &bytes)?;
            let text = String::from_utf8_lossy(&bytes);
            // Kept as text alone: no escape sequence of a log reaches the
            // record the Landing view draws.
            let lines: Vec<String> = text
                .lines()
                .map(|l| {
                    l.chars()
                        .filter(|c| *c == '\t' || !c.is_control())
                        .collect()
                })
                .collect();
            let tail = &lines[lines.len().saturating_sub(OUTPUT_TAIL)..];
            data.insert(
                "output".into(),
                json!({ "of": of, "tail": tail, "lines": lines.len(), "at": util::js_number(now as f64) }),
            );
        }
        _ => return Err(RecordError::Usage),
    }
    util::write_atomic(&file, util::json_compact(&Value::Object(data)))?;
    Ok(())
}

/// Shows the landing's step on the caller's pane, and marks its loop
/// queue-and-clean's; no words clear the step and leave the loop. Outside a
/// Switchboard pane it does nothing.
fn phase(words: &[String]) {
    set_phase(&words.join(" "));
}

/// `phase`, its words as one string: `update-target` marks its steps so.
pub(crate) fn set_phase(words: &str) {
    if !socket::in_pane() {
        return;
    }
    let words = worktree::one_line(words);
    if words.is_empty() {
        let _ = socket::ask(&["set", "-u", "@agent_phase"]);
    } else {
        let _ = socket::ask(&["set", "@agent_phase", &words]);
        let _ = socket::ask(&["set", "@agent_loop", "queue"]);
    }
}

/// A direct landing, said: the caller's tab marked `@tab_landed` with the
/// hash, the branch it landed on, the main clone's branch and the
/// repository (its main clone's git directory), read from git's files where
/// this runs, which the sidebar files under Merged while the tab is in that
/// main clone on that branch. Outside a Switchboard pane it does nothing.
fn landed(args: &[String]) -> ExitCode {
    let [hash, branch] = args else {
        eprintln!("usage: land-lock landed <short hash> <branch>");
        return ExitCode::from(2);
    };
    let (hash, branch) = (hash.trim(), branch.trim());
    if hash.is_empty() || branch.is_empty() || hash.contains(' ') || branch.contains(' ') {
        eprintln!("usage: land-lock landed <short hash> <branch>");
        return ExitCode::from(2);
    }
    if !socket::in_pane() {
        return ExitCode::SUCCESS;
    }
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let clone = crate::tabs::git::locate(&cwd)
        .and_then(|here| crate::tabs::git::locate(here.common_dir.parent()?));
    let Some(clone) = clone else {
        eprintln!("not in a git repository with a main clone");
        return ExitCode::from(2);
    };
    let mark = format!(
        "{hash} {branch} {} {}",
        clone.branch,
        clone.common_dir.display()
    );
    let _ = socket::ask(&["set", crate::sidebar::model::LANDED, &mark]);
    // A landing on the default branch carries on to the branches items land
    // on, each brought up to date in a tab of its own.
    let default = crate::tabs::git::default_branch(&clone.common_dir);
    if default.as_deref() == Some(branch)
        && crate::settings::is_on("target_follows_default")
        && let Some(root) = clone.common_dir.parent()
    {
        crate::cmd::update_target::follow_from_pane(root, branch);
    }
    ExitCode::SUCCESS
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
    // What a hands-free tab left for you to apply, rather than ask.
    let mut not_applied = String::new();
    if let Some(at) = args.iter().position(|a| a == "--not-applied")
        && at + 1 < args.len()
    {
        not_applied = worktree::one_line(&args.remove(at + 1));
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
            "usage: land-lock autolanded <short hash> [--plan <file> | --decided <n>] [--not-applied <labels>] [item]"
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
        not_applied,
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
pub(crate) struct Lock {
    /// `<common git dir>/land.lock`, a directory, made to take the lock.
    dir: PathBuf,
    holder_file: PathBuf,
    queue_file: PathBuf,
    queue_mutex: PathBuf,
    /// The worktree this runs in.
    here: PathBuf,
    /// The process that holds or waits for it, when no Claude session does
    /// (`update-target`): its entries name it, and it counts as there while
    /// it runs.
    pid: Option<u32>,
}

impl Lock {
    /// `suffix` names a branch other than the default one (`file_suffix`).
    fn new(common: &Path, here: PathBuf, suffix: Option<&str>) -> Lock {
        let named = |base: &str| match suffix {
            Some(s) => format!("{base}.{s}"),
            None => base.to_string(),
        };
        let dir = common.join(named("land.lock"));
        Lock {
            holder_file: dir.join("holder"),
            dir,
            queue_mutex: common.join(named("land.mutex")),
            queue_file: common.join(named("land.queue")),
            here,
            pid: None,
        }
    }

    /// The lock of `branch` (the default branch's own files when it is the
    /// default one) for the worktree at `here`, held by this process,
    /// which keeps it while it runs whether or not a Claude session does:
    /// `update-target`'s.
    pub(crate) fn for_process(common: &Path, here: PathBuf, branch: &str) -> Lock {
        let suffix = (crate::tabs::git::default_branch(common).as_deref() != Some(branch))
            .then(|| file_suffix(branch));
        Lock {
            pid: Some(std::process::id()),
            ..Lock::new(common, here, suffix.as_deref())
        }
    }

    /// Another running process that holds the lock, or waits for it, for
    /// this same worktree (a second `update-target` for the branch): its
    /// pid. Two would share the worktree, each taking the other's place.
    pub(crate) fn other_process_here(&self) -> io::Result<Option<i64>> {
        let here = self.here.to_string_lossy().into_owned();
        let mine = self.pid.map(i64::from);
        let other = |entry: &Value| {
            let pid = entry.get("pid").and_then(Value::as_i64)?;
            (entry_path(entry) == Some(here.as_str()) && Some(pid) != mine && util::is_alive(pid))
                .then_some(pid)
        };
        let holder = self
            .holder()
            .filter(|_| self.dir.exists())
            .as_ref()
            .and_then(other);
        if holder.is_some() {
            return Ok(holder);
        }
        self.with_queue(|queue| (None, queue.iter().find_map(other)))
    }

    fn entry(&self, path: &str) -> Value {
        new_entry(path, self.pid)
    }

    /// Takes it, waiting in the queue for its turn every `poll`: 0 taken,
    /// 4 taken out of the queue meanwhile (mission control's Landing view),
    /// rather than joining again at the back. A landing `requeue` put
    /// first in line that was taken out while it reviewed (its mark left,
    /// `REQUEUED`, no take since) is out before its wait starts, and says
    /// so too.
    pub(crate) fn wait(&self, poll: Duration) -> io::Result<u8> {
        let here = self.here.to_string_lossy().into_owned();
        let mut waited = self.requeued_mark().is_some_and(|m| m.exists());
        loop {
            if waited {
                let there = self.with_queue(|queue| {
                    let there = queue.iter().any(|e| entry_path(e) == Some(here.as_str()));
                    (None, there)
                })?;
                if !there {
                    if let Some(mark) = self.requeued_mark() {
                        let _ = fs::remove_file(mark);
                    }
                    return Ok(4);
                }
            }
            if self.take(true)? == 0 {
                return Ok(0);
            }
            std::thread::sleep(poll);
            waited = true;
        }
    }

    fn holder(&self) -> Option<Value> {
        util::read_json(&self.holder_file)
    }

    fn hold(&self) -> io::Result<()> {
        let holder = self.entry(&self.here.to_string_lossy());
        util::write_atomic(&self.holder_file, util::json_compact(&holder))
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
        self.change_queue(false, change)
    }

    /// `with_queue`, with this worktree's own place, when `taking_up` (a
    /// take), named for the session this runs in before the gone waiters
    /// are dropped, so a waiter whose session ended and that another took
    /// up keeps its place.
    fn change_queue<T>(
        &self,
        taking_up: bool,
        change: impl FnOnce(Vec<Value>) -> (Option<Vec<Value>>, T),
    ) -> io::Result<T> {
        let here = self.here.to_string_lossy().into_owned();
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
                .map(|entry| {
                    let mine = taking_up && entry_path(&entry) == Some(here.as_str());
                    if mine {
                        renamed(&entry).unwrap_or(entry)
                    } else {
                        entry
                    }
                })
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
    pub(crate) fn take(&self, quiet: bool) -> io::Result<u8> {
        let here = self.here.to_string_lossy().into_owned();
        // Taking (or waiting) again is the requeued landing back: its
        // place, if it still has one, is ready.
        if let Some(mark) = self.requeued_mark() {
            let _ = fs::remove_file(mark);
        }
        self.change_queue(true, |queue| {
            let current = self.holder();
            let held = self.dir.exists();
            if held && current.as_ref().and_then(holder_path).as_deref() == Some(here.as_str()) {
                // A process taking up its worktree's lock from one that
                // ended (a run cut short, its tab closed) names itself, so
                // the lock counts as held while it runs.
                let taken_up = self.pid.and_then(|pid| {
                    let holder = current.as_ref()?;
                    (holder.get("pid").and_then(Value::as_i64) != Some(i64::from(pid))).then(|| {
                        let mut now = self.entry(&here);
                        if let Some(since) = holder.get("since") {
                            now["since"] = since.clone();
                        }
                        now
                    })
                });
                let named = match taken_up.or_else(|| current.as_ref().and_then(renamed)) {
                    Some(holder) => {
                        util::write_atomic(&self.holder_file, util::json_compact(&holder))
                    }
                    None => Ok(()),
                };
                return (Some(without(queue, &self.here)), named.map(|()| 0));
            }
            let mine = queue
                .iter()
                .position(|e| entry_path(e) == Some(here.as_str()));
            // A waiter reviewing its change again keeps its place without
            // holding up those behind it.
            let ahead = queue[..mine.unwrap_or(queue.len())]
                .iter()
                .filter(|e| !reviewing(e))
                .count();
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
            let mut joined = queue;
            match mine {
                Some(at) => {
                    if let Some(entry) = joined[at].as_object_mut() {
                        entry.remove("reviewing");
                    }
                }
                None => joined.push(self.entry(&here)),
            }
            (Some(joined), Ok(3))
        })?
    }

    pub(crate) fn release(&self) -> io::Result<()> {
        if let Some(mark) = self.requeued_mark() {
            let _ = fs::remove_file(mark);
        }
        self.with_queue(|queue| (Some(without(queue, &self.here)), ()))?;
        let here = self.here.to_string_lossy();
        if self.holder().as_ref().and_then(holder_path).as_deref() == Some(&*here) {
            let _ = fs::remove_dir_all(&self.dir);
        }
        Ok(())
    }

    /// Lets the lock go, when this worktree holds it, and puts this
    /// worktree first in the queue marked reviewing, under the queue's
    /// mutex, which every take holds, so none slips in between.
    pub(crate) fn requeue(&self) -> io::Result<()> {
        let here = self.here.to_string_lossy().into_owned();
        self.change_queue(true, |queue| {
            let mut entry = queue
                .iter()
                .find(|e| entry_path(e) == Some(here.as_str()))
                .cloned()
                .unwrap_or_else(|| self.entry(&here));
            entry["reviewing"] = Value::Bool(true);
            let mut next = without(queue, &self.here);
            next.insert(0, entry);
            let holds = self.holder().as_ref().and_then(holder_path).as_deref() == Some(&*here);
            let freed = if holds {
                fs::remove_dir_all(&self.dir).or_else(|e| match e.kind() {
                    io::ErrorKind::NotFound => Ok(()),
                    _ => Err(e),
                })
            } else {
                Ok(())
            };
            (Some(next), freed)
        })??;
        if let Some(mark) = self.requeued_mark() {
            util::write_atomic(mark, "")?;
        }
        Ok(())
    }

    /// The mark `requeue` leaves in the worktree's own git directory until
    /// its next take, by which a wait tells a landing taken out of the
    /// queue during its review from one that never joined it.
    fn requeued_mark(&self) -> Option<PathBuf> {
        crate::sources::landing::own_git_dir(&self.here).map(|d| d.join(REQUEUED))
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

/// An entry for the worktree at `path`, taken or joined now, naming the
/// Claude session and the pane this runs in when it runs in one, or, for a
/// process that holds it itself (`pid`), that process and the pane alone.
fn new_entry(path: &str, pid: Option<u32>) -> Value {
    let mut entry = json!({
        "path": path,
        "since": util::js_number(util::now_ms() as f64),
    });
    let mut named = vec![("pane", "SWITCHBOARD_PANE")];
    match pid {
        Some(pid) => entry["pid"] = json!(pid),
        None => named.push(("session", "CLAUDE_CODE_SESSION_ID")),
    }
    for (key, variable) in named {
        if let Some(value) = std::env::var(variable).ok().filter(|v| !v.is_empty()) {
            entry[key] = Value::String(value);
        }
    }
    entry
}

/// An entry of this worktree's, named for the session and pane this runs in
/// when it names others (a session that ended and another that took the
/// landing up, or a build from before that named none), its place and since
/// when kept; `None` when it names them already, or this runs in no session.
fn renamed(entry: &Value) -> Option<Value> {
    if entry.get("pid").is_some() {
        return None;
    }
    let mut now = new_entry(entry_path(entry)?, None);
    let session = now.get("session")?;
    if entry.get("session") == Some(session) && entry.get("pane") == now.get("pane") {
        return None;
    }
    if let Some(since) = entry.get("since") {
        now["since"] = since.clone();
    }
    Some(now)
}

/// An entry's or holder's worktree.
fn entry_path(entry: &Value) -> Option<&str> {
    entry
        .get("path")
        .and_then(Value::as_str)
        .filter(|p| !p.is_empty())
}

/// The Claude session that wrote an entry, `None` in one written outside
/// Claude or by a build from before.
fn entry_session(entry: &Value) -> Option<&str> {
    entry
        .get("session")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
}

/// Whether a waiter is reviewing its change again (`requeue`), so a take
/// passes over it.
fn reviewing(entry: &Value) -> bool {
    entry.get("reviewing").and_then(Value::as_bool) == Some(true)
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
/// removed, the process that holds it itself (`pid`) ended, or neither the
/// session that wrote it running, wherever it works, nor any running in its
/// worktree.
fn gone(current: Option<&Value>) -> bool {
    let Some(entry) = current else {
        return true;
    };
    let Some(path) = entry_path(entry) else {
        return true;
    };
    let path = Path::new(path);
    if !path.exists() {
        return true;
    }
    if entry_session(entry).is_some_and(worktree::session_runs) {
        return false;
    }
    if let Some(pid) = entry.get("pid").and_then(Value::as_i64) {
        return !util::is_alive(pid);
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
    fn a_record_merges_its_keys_and_keeps_the_output_s_tail() {
        let scratch = crate::tabs::git::tests::Scratch::new("land-record");
        let dir = &scratch.0;
        let read = || util::read_json(dir.join(crate::sources::landing::RECORD)).unwrap();
        let ok = |words: &[&str], now| assert!(record_in(dir, words, now).is_ok(), "{words:?}");
        ok(&["tests", r#"{"passed":412,"failed":0,"total":412}"#], 5);
        ok(&["review", r#"{"round":1,"reviewer":"codex"}"#], 6);
        ok(
            &["found", r#"{"round":1,"title":"a","fixed_by":"abc1234"}"#],
            7,
        );
        ok(&["found", r#"{"round":2,"title":"b"}"#], 8);
        let r = read();
        assert_eq!(r["tests"]["passed"], 412);
        assert_eq!(r["tests"]["at"], 5);
        assert_eq!(r["review"]["round"], 1, "another key kept");
        assert_eq!(r["found"].as_array().unwrap().len(), 2, "found adds");
        ok(
            &[
                "review",
                r#"{"round":2,"reviewer":"codex","clean_rounds":1}"#,
            ],
            9,
        );
        assert_eq!(read()["review"]["round"], 2, "a key replaced whole");

        let out = dir.join("out.txt");
        let text: String = (1..=50).map(|n| format!("line {n}\n")).collect();
        fs::write(&out, text).unwrap();
        ok(&["output", "tests", out.to_str().unwrap()], 10);
        let r = read();
        let tail = r["output"]["tail"].as_array().unwrap();
        assert_eq!((tail.len(), &tail[0]), (40, &json!("line 11")));
        assert_eq!(r["output"]["lines"], 50);
        assert!(dir.join(crate::sources::landing::OUTPUT).is_file());

        assert!(matches!(
            record_in(dir, &["tests", "not json"], 11),
            Err(RecordError::Usage)
        ));
        assert!(matches!(
            record_in(dir, &["other", "{}"], 11),
            Err(RecordError::Usage)
        ));
        ok(&["clear"], 12);
        assert_eq!(read(), json!({}));
        assert!(!dir.join(crate::sources::landing::OUTPUT).exists());
    }

    #[test]
    fn a_requeued_landing_lets_the_lock_go_and_keeps_its_turn() {
        let scratch = crate::tabs::git::tests::Scratch::new("land-requeue");
        let common = scratch.0.join("git");
        fs::create_dir_all(&common).unwrap();
        let lock = |name: &str| {
            let here = scratch.0.join(name);
            fs::create_dir_all(here.join(".git")).unwrap();
            Lock {
                pid: Some(std::process::id()),
                ..Lock::new(&common, here, None)
            }
        };
        let (a, b, c) = (lock("a"), lock("b"), lock("c"));
        let waiters = || -> Vec<(String, bool)> {
            a.with_queue(|q| {
                let names = q
                    .iter()
                    .map(|e| {
                        let path = Path::new(entry_path(e).unwrap());
                        let name = path.file_name().unwrap().to_string_lossy().into_owned();
                        (name, reviewing(e))
                    })
                    .collect();
                (None, names)
            })
            .unwrap()
        };
        let held_by = || {
            a.holder()
                .filter(|_| a.dir.exists())
                .and_then(|h| holder_path(&h))
        };
        let path_of = |l: &Lock| Some(l.here.to_string_lossy().into_owned());

        assert_eq!(a.take(true).unwrap(), 0);
        assert_eq!(b.take(true).unwrap(), 3);
        a.requeue().unwrap();
        assert_eq!(held_by(), None, "the lock is free");
        assert_eq!(waiters(), [("a".into(), true), ("b".into(), false)]);

        assert_eq!(
            b.take(true).unwrap(),
            0,
            "a take passes over a reviewing waiter"
        );
        assert_eq!(c.take(true).unwrap(), 3);
        assert_eq!(waiters(), [("a".into(), true), ("c".into(), false)]);

        c.requeue().unwrap();
        assert_eq!(held_by(), path_of(&b), "a waiter requeued only moves first");
        assert_eq!(waiters(), [("c".into(), true), ("a".into(), true)]);
        assert_eq!(c.take(true).unwrap(), 3, "held by b");
        assert_eq!(a.take(true).unwrap(), 3, "held by b");
        assert_eq!(waiters(), [("c".into(), false), ("a".into(), false)]);

        b.release().unwrap();
        assert_eq!(a.take(true).unwrap(), 3, "c is ready and ahead");
        assert_eq!(c.wait(Duration::ZERO).unwrap(), 0, "c takes it next");
        assert_eq!(held_by(), path_of(&c));

        a.requeue().unwrap();
        a.with_queue(|q| (Some(without(q, &a.here)), ())).unwrap();
        c.release().unwrap();
        assert_eq!(
            a.wait(Duration::ZERO).unwrap(),
            4,
            "taken out of the queue while it reviewed, it lands nothing"
        );
        assert_eq!(held_by(), None);
        assert_eq!(
            a.wait(Duration::ZERO).unwrap(),
            0,
            "a wait of its own takes it"
        );
    }

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
    fn no_branch_s_queue_is_a_mutex() {
        let locks: Vec<Lock> = [None, Some("team%2Fnext"), Some("lock")]
            .into_iter()
            .map(|suffix| Lock::new(Path::new("/git"), PathBuf::from("/w"), suffix))
            .collect();
        let names = |pick: fn(&Lock) -> &PathBuf| -> Vec<String> {
            locks
                .iter()
                .map(|l| pick(l).file_name().unwrap().to_string_lossy().into_owned())
                .collect()
        };
        let mutexes = names(|l| &l.queue_mutex);
        assert_eq!(
            mutexes,
            ["land.mutex", "land.mutex.team%2Fnext", "land.mutex.lock"]
        );
        assert_eq!(
            names(|l| &l.queue_file),
            ["land.queue", "land.queue.team%2Fnext", "land.queue.lock"]
        );
        for name in names(|l| &l.queue_file).iter().chain(&names(|l| &l.dir)) {
            assert!(!mutexes.contains(name), "{name} is a mutex too");
        }
    }

    #[test]
    fn a_landing_with_no_worktree_is_gone() {
        assert!(gone(None));
        assert!(gone(Some(&json!({ "path": "/no/such/worktree" }))));
        assert!(gone(Some(&json!({ "since": 1 }))));
    }

    #[test]
    fn an_entry_names_its_session_when_it_has_one() {
        assert_eq!(entry_session(&json!({ "session": "s1" })), Some("s1"));
        assert_eq!(entry_session(&json!({ "session": "" })), None);
        assert_eq!(entry_session(&json!({ "path": "/w" })), None);
    }
}
