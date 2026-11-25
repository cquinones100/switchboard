//! Switchboard's own merge queue, for a repository that lands through PRs
//! (`switchboard landing` printing `switchboard-queue`) and has no merge queue of
//! GitHub's, which only an organization's repositories get. New with the
//! Rust version. Without one, a base branch that requires PRs to be up to
//! date makes every merge put every other open PR behind, each updating its
//! branch and running all of CI again before the next can merge; here the
//! PRs wait in the queue without updating, and the queue tests the next few
//! together on one branch, `ci/train-<id>`, and merges them in order once
//! that one run passes.
//!
//! The queue is a file in the main clone's git directory, `pr.queue`, shared
//! by every worktree as `land-lock`'s landing queue is (src/cmd/land_lock.rs),
//! and changed under a mutex directory beside it, `pr.mutex`, so a change
//! made while the queue is read is never lost. It holds the PRs queued, in
//! order, each with the head it was queued at (the worktree's `HEAD`, the
//! only commit of it a train tests) and the worktree, Claude session and
//! pane that queued it
//! (`/queue-and-clean`, through `land-lock queue-pr`); the train under test,
//! its base and each PR's head as tested; the train ahead, built on the
//! train under test's commit and tested beside it; the groups a red train was split
//! into, tested ahead of the rest; the PRs dropped and why, and the last
//! ones merged, which `land-lock pr-status` tells the PR's tab; whether the
//! train's failed jobs were run again, which a red train is once before it
//! is dropped or split; and what
//! holds the queue when it can't go on by itself (no CI on a train, a merge
//! GitHub refused), cleared by any change to the queue.
//!
//! `train` runs it, one step at a time from the workspace's server
//! (`serve`), off the server's task; `land-lock` changes it from the
//! command line and mission control's Landing view.

pub mod serve;
pub mod train;

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::util;

/// How many PRs a train tests together at most.
pub const TRAIN_MAX: usize = 4;

/// How many merged and dropped PRs the file remembers, for their tabs.
const REMEMBERED: usize = 50;

/// How long a mutex left by a killed process stands before it is taken over,
/// how long a change waits between tries, and how many tries it makes.
const STALE_MUTEX_MS: u128 = 10_000;
const MUTEX_WAIT: Duration = Duration::from_millis(20);
const MUTEX_TRIES: u32 = 500;

/// The queue, as `pr.queue` keeps it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Queue {
    pub prs: Vec<Queued>,
    pub train: Option<Train>,
    /// The train ahead: the next PRs merged onto the train under test's
    /// commit, its CI running beside that one's, which becomes the train
    /// under test once that one merges.
    pub ahead: Option<Train>,
    /// The halves of red trains, each tested as a train of its own before
    /// anything else, so the PR at fault is found and the rest go on.
    pub splits: Vec<Vec<u64>>,
    pub dropped: Vec<Dropped>,
    pub merged: Vec<Merged>,
    /// What stopped the queue, which waits until something in it changes.
    pub held: Option<String>,
    /// How many pushes of a train have failed in a row, the queue held at
    /// `TRIES`.
    pub push_failures: u32,
    /// Why the server's last step failed, for the Landing view; cleared by
    /// the next that doesn't.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failed: Option<String>,
}

/// How many times in a row a train's push or a merge may fail, as a
/// network blip would, before the queue holds.
pub const TRIES: u32 = 3;

/// A PR in the queue, with what queued it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Queued {
    pub number: u64,
    /// The head it was queued with, the only one a train tests.
    pub head: String,
    /// Since when (ms).
    pub since: i64,
    /// The worktree it was queued from.
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pane: Option<String>,
}

/// The train under test: its branch, the base it was built on, its tip, and
/// each PR in it with the head it tested, in the order they merge.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Train {
    pub branch: String,
    pub base: String,
    pub commit: String,
    pub prs: Vec<Tested>,
    /// When it was pushed (ms).
    pub since: i64,
    /// Its CI looked passed once: it merges on the next look that agrees,
    /// since a job that waits on another appears only once that one ends.
    pub passed: bool,
    /// How many times in a row a merge of it has failed.
    pub merge_failures: u32,
    /// Where the default branch was after the last of its PRs merged, so a
    /// merge tried again after one failed goes on from there.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub merged_base: Option<String>,
    /// Its CI failed once and its failed jobs were run again, which a train
    /// is at most once: a second failure drops or splits it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rerun: Option<Rerun>,
    /// Built from a red train's split, which a train ahead ended before it
    /// is tested gives back to the splits.
    pub split: bool,
}

/// A red train's failed jobs run again: the GitHub Actions run, their
/// names, and the first one's job, whose failure is the first attempt's
/// until the rerun's own check runs replace it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Rerun {
    pub run: String,
    pub names: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub job: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Tested {
    pub number: u64,
    pub head: String,
}

/// A PR taken out of the queue by the queue, and why.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Dropped {
    pub number: u64,
    pub why: String,
    pub at: i64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Merged {
    pub number: u64,
    pub at: i64,
}

impl Queue {
    /// Whether the queue has anything to do.
    pub fn busy(&self) -> bool {
        self.train.is_some() || self.ahead.is_some() || !self.prs.is_empty()
    }

    pub fn queued(&self, number: u64) -> Option<&Queued> {
        self.prs.iter().find(|p| p.number == number)
    }

    /// Whether the train under test holds `number`.
    pub fn in_train(&self, number: u64) -> bool {
        holds(self.train.as_ref(), number)
    }

    /// Whether the train ahead holds `number`.
    pub fn in_ahead(&self, number: u64) -> bool {
        holds(self.ahead.as_ref(), number)
    }

    /// Whether either train tests `number`.
    pub fn testing(&self, number: u64) -> bool {
        self.in_train(number) || self.in_ahead(number)
    }

    /// A PR's place in line: 1 for each one in the train under test, 2 for
    /// each in the train ahead, the rest after them in their order.
    pub fn line(&self, number: u64) -> Option<u32> {
        self.queued(number)?;
        if self.in_train(number) {
            return Some(1);
        }
        if self.in_ahead(number) {
            return Some(1 + u32::from(self.train.is_some()));
        }
        let ahead = u32::from(self.train.is_some()) + u32::from(self.ahead.is_some());
        let at = self
            .prs
            .iter()
            .filter(|p| !self.testing(p.number))
            .position(|p| p.number == number)?;
        Some(at as u32 + 1 + ahead)
    }

    /// Takes a PR out, from its splits too.
    pub fn remove(&mut self, number: u64) -> Option<Queued> {
        let at = self.prs.iter().position(|p| p.number == number)?;
        for split in &mut self.splits {
            split.retain(|n| *n != number);
        }
        self.splits.retain(|s| !s.is_empty());
        Some(self.prs.remove(at))
    }

    /// A PR dropped by the queue, why kept for its tab.
    pub fn drop_pr(&mut self, number: u64, why: &str, now: i64) -> Option<Queued> {
        let gone = self.remove(number)?;
        self.dropped.retain(|d| d.number != number);
        self.dropped.push(Dropped {
            number,
            why: why.to_string(),
            at: now,
        });
        keep_last(&mut self.dropped);
        Some(gone)
    }

    /// A PR merged by the queue.
    pub fn merged_pr(&mut self, number: u64, now: i64) -> Option<Queued> {
        let gone = self.remove(number);
        self.merged.retain(|m| m.number != number);
        self.merged.push(Merged { number, at: now });
        keep_last(&mut self.merged);
        gone
    }

    /// What `land-lock pr-status` says of a PR.
    pub fn status(&self, number: u64) -> String {
        if let Some(t) = self.train.as_ref().filter(|_| self.in_train(number)) {
            return format!("testing {}", t.branch);
        }
        if let Some(t) = self.ahead.as_ref().filter(|_| self.in_ahead(number)) {
            return format!("testing next {}", t.branch);
        }
        if let Some(line) = self.line(number) {
            return match &self.held {
                Some(held) => format!("held {held} · queued {line} of {}", self.prs.len()),
                None => format!("queued {line} of {}", self.prs.len()),
            };
        }
        if self.merged.iter().any(|m| m.number == number) {
            return "merged".into();
        }
        match self.dropped.iter().find(|d| d.number == number) {
            Some(d) => format!("dropped {}", d.why),
            None => "none".into(),
        }
    }

    /// What `land-lock pr-status` says of a PR while the server doesn't run
    /// the queue, `why` saying why: a PR waiting or under test is held,
    /// since nothing will move it until the queue runs again.
    pub fn status_stopped(&self, number: u64, why: &str) -> String {
        if let Some(t) = self.train.as_ref().filter(|_| self.in_train(number)) {
            return format!("held {why} · testing {}", t.branch);
        }
        if let Some(t) = self.ahead.as_ref().filter(|_| self.in_ahead(number)) {
            return format!("held {why} · testing next {}", t.branch);
        }
        match self.line(number) {
            Some(line) => format!("held {why} · queued {line} of {}", self.prs.len()),
            None => self.status(number),
        }
    }
}

/// Why the workspace's server doesn't run the queues, when a setting
/// stops it (`serve::runs`).
pub fn stopped() -> Option<&'static str> {
    if !crate::settings::is_on("github") {
        return Some("Settings' github is off");
    }
    None
}

fn holds(train: Option<&Train>, number: u64) -> bool {
    train.is_some_and(|t| t.prs.iter().any(|p| p.number == number))
}

fn keep_last<T>(list: &mut Vec<T>) {
    if list.len() > REMEMBERED {
        list.drain(..list.len() - REMEMBERED);
    }
}

/// The queue's file in a repository's shared git directory.
pub fn file(common: &Path) -> PathBuf {
    common.join("pr.queue")
}

fn mutex(common: &Path) -> PathBuf {
    common.join("pr.mutex")
}

/// The queue as it is, empty when there is none or it can't be read.
pub fn read(common: &Path) -> Queue {
    util::read_json(file(common))
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default()
}

/// Changes the queue under its mutex, writing it whole when it changed;
/// one left behind by a killed process for over ten seconds is taken over.
pub fn change<T>(common: &Path, edit: impl FnOnce(&mut Queue) -> T) -> io::Result<T> {
    let lock = mutex(common);
    let mut tries = 0;
    loop {
        match fs::create_dir(&lock) {
            Ok(()) => break,
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                let stale = fs::metadata(&lock)
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|t| t.elapsed().ok())
                    .is_some_and(|age| age.as_millis() > STALE_MUTEX_MS);
                if stale {
                    let _ = fs::remove_dir_all(&lock);
                }
                tries += 1;
                if tries > MUTEX_TRIES {
                    return Err(io::Error::other("the merge queue stayed locked"));
                }
                std::thread::sleep(MUTEX_WAIT);
            }
            Err(e) => return Err(e),
        }
    }
    let result = (|| {
        let before = read(common);
        let mut queue = before.clone();
        let answer = edit(&mut queue);
        if queue != before {
            util::write_atomic(file(common), util::json_compact(&queue))?;
        }
        Ok(answer)
    })();
    let _ = fs::remove_dir_all(&lock);
    result
}

/// `land-lock queue-pr`, `dequeue-pr`, `move-pr`, `pr-status` and
/// `pr-queue`, run from a worktree of the repository (`here`).
pub fn command(command: &str, args: &[String], common: &Path, here: &Path) -> Option<u8> {
    let number = || {
        args.first()
            .and_then(|n| n.trim_start_matches('#').parse::<u64>().ok())
    };
    let usage = |line: &str| {
        eprintln!("usage: land-lock {line}");
        2
    };
    let failed = |e: io::Error| {
        eprintln!("land-lock: {e}");
        1
    };
    Some(match command {
        "queue-pr" => {
            let Some(number) = number() else {
                return Some(usage("queue-pr <number>"));
            };
            let entry = crate::cmd::land_lock::new_entry(&here.to_string_lossy(), None);
            let text = |key: &str| entry.get(key).and_then(|v| v.as_str()).map(str::to_string);
            let Ok(head) = crate::worktree::git_at(here, &["rev-parse", "HEAD"]) else {
                eprintln!("land-lock: no HEAD to queue in {}", here.display());
                return Some(1);
            };
            let queued = Queued {
                number,
                head,
                since: util::now_ms(),
                path: here.to_string_lossy().into_owned(),
                session: text("session"),
                pane: text("pane"),
            };
            match change(common, |q| queue_pr(q, queued)) {
                Ok(Ok(line)) => {
                    println!("queued {line}");
                    0
                }
                Ok(Err(line)) => {
                    println!("#{number} is in the merge queue already at this head, {line}");
                    3
                }
                Err(e) => failed(e),
            }
        }
        "dequeue-pr" => {
            let Some(number) = number() else {
                return Some(usage("dequeue-pr <number>"));
            };
            match change(common, |q| dequeue_pr(q, number)) {
                Ok(true) => 0,
                Ok(false) => {
                    println!("#{number} isn't in the merge queue");
                    3
                }
                Err(e) => failed(e),
            }
        }
        "move-pr" => {
            let (Some(number), Some(way)) = (number(), args.get(1)) else {
                return Some(usage("move-pr <number> up|down|first"));
            };
            if !matches!(way.as_str(), "up" | "down" | "first") {
                return Some(usage("move-pr <number> up|down|first"));
            }
            match change(common, |q| move_pr(q, number, way)) {
                Ok(true) => 0,
                Ok(false) => {
                    println!("#{number} isn't waiting in the merge queue");
                    3
                }
                Err(e) => failed(e),
            }
        }
        "pr-status" => {
            let Some(number) = number() else {
                return Some(usage("pr-status <number>"));
            };
            let queue = read(common);
            match stopped() {
                Some(why) => println!("{}", queue.status_stopped(number, why)),
                None => println!("{}", queue.status(number)),
            }
            0
        }
        "pr-queue" => {
            let queue = read(common);
            if let Some(held) = &queue.held {
                println!("held\t{held}");
            }
            for p in &queue.prs {
                let place = if queue.in_train(p.number) {
                    "testing".to_string()
                } else if queue.in_ahead(p.number) {
                    "next".to_string()
                } else {
                    queue.line(p.number).unwrap_or(0).to_string()
                };
                println!("{place}\t#{}\t{}\t{}", p.number, p.path, p.since);
            }
            0
        }
        _ => return None,
    })
}

/// A PR added at the back, at most once: its place, or, when it is there
/// already at the same head, what `status` says of it; at another head, it
/// keeps its place with the new one. Queuing again is a change, so it
/// clears what held the queue, and a PR dropped before is no longer.
pub fn queue_pr(queue: &mut Queue, queued: Queued) -> Result<String, String> {
    let number = queued.number;
    if let Some(there) = queue.prs.iter_mut().find(|p| p.number == number) {
        if there.head == queued.head {
            queue.held = None;
            queue.push_failures = 0;
            return Err(queue.status(number));
        }
        there.head = queued.head;
        // How long its head has waited for checks starts again.
        there.since = queued.since;
        queue.held = None;
        return Ok(format!(
            "{} of {}",
            queue.line(number).unwrap_or(0),
            queue.prs.len()
        ));
    }
    queue.dropped.retain(|d| d.number != number);
    queue.merged.retain(|m| m.number != number);
    queue.prs.push(queued);
    queue.held = None;
    Ok(format!(
        "{} of {}",
        queue.line(number).unwrap_or(0),
        queue.prs.len()
    ))
}

/// A PR taken out by hand: whether it was there. One in the train under
/// test ends that train and the train ahead, and one in the train ahead
/// ends that one, which `train::step` sees.
pub fn dequeue_pr(queue: &mut Queue, number: u64) -> bool {
    let there = queue.remove(number).is_some();
    if there {
        queue.held = None;
    }
    there
}

/// A waiting PR moved among the waiting (the train's stay where they are):
/// whether it was waiting.
pub fn move_pr(queue: &mut Queue, number: u64, way: &str) -> bool {
    let waiting: Vec<usize> = (0..queue.prs.len())
        .filter(|i| !queue.testing(queue.prs[*i].number))
        .collect();
    let Some(at) = waiting.iter().position(|i| queue.prs[*i].number == number) else {
        return false;
    };
    let to = match way {
        "up" => at.saturating_sub(1),
        "down" => (at + 1).min(waiting.len() - 1),
        _ => 0,
    };
    // Before the waiter it passes going up; going down, after the one it
    // passes, which the removal moved a place left: `waiting[to]` either way.
    let entry = queue.prs.remove(waiting[at]);
    queue.prs.insert(waiting[to].min(queue.prs.len()), entry);
    // A PR moved is tested in the order chosen, not the split's.
    queue.splits.clear();
    if let Some(ahead) = queue.ahead.as_mut() {
        ahead.split = false;
    }
    queue.held = None;
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pr(number: u64) -> Queued {
        Queued {
            number,
            since: number as i64,
            path: format!("/w/{number}"),
            ..Queued::default()
        }
    }

    fn numbers(q: &Queue) -> Vec<u64> {
        q.prs.iter().map(|p| p.number).collect()
    }

    #[test]
    fn queuing_keeps_order_and_refuses_a_pr_twice_at_one_head() {
        let dir = std::env::temp_dir().join(format!("sb-prq-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        for n in [12, 7, 30] {
            assert!(change(&dir, |q| queue_pr(q, pr(n))).unwrap().is_ok());
        }
        let again = change(&dir, |q| queue_pr(q, pr(7))).unwrap();
        assert_eq!(again, Err("queued 2 of 3".to_string()));
        change(&dir, |q| q.held = Some("no CI".into())).unwrap();
        let held = change(&dir, |q| queue_pr(q, pr(7))).unwrap();
        assert_eq!(
            held,
            Err("queued 2 of 3".to_string()),
            "queuing again clears a hold"
        );
        let moved = Queued {
            head: "new".into(),
            ..pr(7)
        };
        assert_eq!(
            change(&dir, |q| queue_pr(q, moved)).unwrap(),
            Ok("2 of 3".into())
        );
        assert_eq!(read(&dir).queued(7).unwrap().head, "new");
        let queue = read(&dir);
        assert_eq!(numbers(&queue), [12, 7, 30]);
        assert!(!mutex(&dir).exists(), "the mutex is let go");
        assert!(change(&dir, |q| dequeue_pr(q, 7)).unwrap());
        assert!(!change(&dir, |q| dequeue_pr(q, 7)).unwrap());
        assert_eq!(numbers(&read(&dir)), [12, 30]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_place_counts_the_train_as_first_and_moves_pass_over_it() {
        let mut q = Queue::default();
        for n in [1, 2, 3, 4] {
            queue_pr(&mut q, pr(n)).unwrap();
        }
        q.train = Some(Train {
            branch: "ci/train-x".into(),
            prs: vec![Tested {
                number: 2,
                head: "h".into(),
            }],
            ..Train::default()
        });
        assert_eq!(
            [1, 2, 3, 4].map(|n| q.line(n)),
            [Some(2), Some(1), Some(3), Some(4)]
        );
        assert_eq!(q.status(2), "testing ci/train-x");
        assert!(!move_pr(&mut q, 2, "first"), "a PR under test stays");
        assert!(move_pr(&mut q, 4, "first"));
        assert_eq!(numbers(&q), [4, 1, 2, 3]);
        assert!(move_pr(&mut q, 4, "down"));
        assert_eq!(numbers(&q), [1, 4, 2, 3]);
        assert!(move_pr(&mut q, 3, "up"));
        assert_eq!(numbers(&q), [1, 3, 4, 2]);
        q.drop_pr(3, "its train failed", 9);
        assert_eq!(q.status(3), "dropped its train failed");
        q.merged_pr(1, 9);
        assert_eq!(q.status(1), "merged");
        assert_eq!(q.status(5), "none");
    }

    #[test]
    fn a_pr_waiting_while_the_queue_is_stopped_is_held() {
        let mut q = Queue::default();
        queue_pr(&mut q, pr(1)).unwrap();
        queue_pr(&mut q, pr(2)).unwrap();
        q.train = Some(Train {
            branch: "ci/train-x".into(),
            prs: vec![Tested {
                number: 1,
                head: "h".into(),
            }],
            ..Train::default()
        });
        let why = "Settings' github is off";
        assert_eq!(
            q.status_stopped(1, why),
            "held Settings' github is off · testing ci/train-x"
        );
        assert_eq!(
            q.status_stopped(2, why),
            "held Settings' github is off · queued 2 of 2"
        );
        q.train = None;
        q.merged_pr(1, 9);
        assert_eq!(q.status_stopped(1, why), "merged");
        assert_eq!(q.status_stopped(5, why), "none");
    }

    #[test]
    fn the_train_ahead_is_second_in_line_and_an_older_file_has_none() {
        let mut q = Queue::default();
        for n in [1, 2, 3, 4] {
            queue_pr(&mut q, pr(n)).unwrap();
        }
        let train = |branch: &str, number: u64| Train {
            branch: branch.into(),
            prs: vec![Tested {
                number,
                head: "h".into(),
            }],
            ..Train::default()
        };
        q.train = Some(train("ci/train-a", 1));
        q.ahead = Some(train("ci/train-b", 3));
        assert_eq!(
            [1, 2, 3, 4].map(|n| q.line(n)),
            [Some(1), Some(3), Some(2), Some(4)]
        );
        assert_eq!(q.status(3), "testing next ci/train-b");
        assert_eq!(q.status_stopped(3, "x"), "held x · testing next ci/train-b");
        assert!(
            !move_pr(&mut q, 3, "first"),
            "a PR in the train ahead stays"
        );
        q.ahead.as_mut().unwrap().split = true;
        assert!(move_pr(&mut q, 4, "first"));
        assert!(
            !q.ahead.as_ref().unwrap().split,
            "a move leaves no split to go back to"
        );
        let older: Queue = serde_json::from_str(r#"{"prs":[],"train":null}"#).unwrap();
        assert!(older.ahead.is_none());
    }
}
