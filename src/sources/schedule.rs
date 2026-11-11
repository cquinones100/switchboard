//! When each GitHub and Linear lookup is due, as the sidebar daemon's
//! `polled` and `guarded` decided (lib/sidebar/daemon.js): each runs every
//! `FAST` with the `poll_fast` setting on (the PR board every 30 seconds),
//! else at its own slow interval; none is asked again while its last ask is
//! still waiting on its answer, whoever asked, since a late answer would
//! overwrite a newer one; none is asked while you're away, and all are
//! asked at once when you're back.
//!
//! The daemon had a timer per lookup; here the server's tick asks `due`
//! every couple of seconds, and a lookup is due once its interval has passed
//! since it was last asked, give or take `SLACK`, so a tick landing a moment
//! early doesn't put it off a whole tick. The first asks come a few seconds
//! after starting, spread out, as the daemon's first `setTimeout`s were,
//! and, as those weren't held while you were away, even with no one there,
//! so the views have something to show when a client first attaches.
//!
//! Something that has just asked GitHub about a PR (M-m's pr-ready.sh, a
//! skill) appends to `github` in the state directory, and the lookups that
//! show PRs are asked again, at most every `GITHUB_NOW` (`github_now`).
//!
//! Time is passed in, so the tests can say what happens when.

use std::time::{Duration, Instant};

/// How often a lookup runs with the `poll_fast` setting on.
pub const FAST: Duration = Duration::from_secs(15);
/// How early a tick may come and still count as on time.
pub const SLACK: Duration = Duration::from_secs(1);
/// The least time between two asks from the `github` file.
pub const GITHUB_NOW: Duration = Duration::from_secs(15);

/// One lookup the schedule keeps: its name, its intervals, and how long
/// after starting it is first asked.
#[derive(Clone, Copy, Debug)]
pub struct Lookup {
    pub name: &'static str,
    pub fast: Duration,
    pub slow: Duration,
    pub first: Duration,
}

/// The daemon's lookups, with their intervals (each file's `_POLL_MS` and
/// `_SLOW_MS`) and first asks.
pub const LOOKUPS: [Lookup; 7] = [
    Lookup {
        name: "merges",
        fast: FAST,
        slow: Duration::from_secs(60),
        first: Duration::from_secs(2),
    },
    Lookup {
        name: "board",
        fast: Duration::from_secs(30),
        slow: Duration::from_secs(120),
        first: Duration::from_secs(4),
    },
    Lookup {
        name: "tickets",
        fast: FAST,
        slow: Duration::from_secs(120),
        first: Duration::from_secs(5),
    },
    Lookup {
        name: "checks",
        fast: FAST,
        slow: Duration::from_secs(120),
        first: Duration::from_secs(7),
    },
    Lookup {
        name: "mergeQueues",
        fast: FAST,
        slow: Duration::from_secs(60),
        first: Duration::from_secs(7),
    },
    Lookup {
        name: "linearInbox",
        fast: FAST,
        slow: Duration::from_secs(120),
        first: Duration::from_secs(8),
    },
    // The default branch's CI (mainci.rs): every two minutes, poll_fast or
    // not, since a run takes minutes to say anything new.
    Lookup {
        name: "mainCi",
        fast: Duration::from_secs(120),
        slow: Duration::from_secs(120),
        first: Duration::from_secs(9),
    },
];

/// The lookups that show PRs, asked again from the `github` file.
pub const GITHUB_LOOKUPS: [&str; 4] = ["merges", "board", "checks", "mergeQueues"];

struct Entry {
    lookup: Lookup,
    last: Option<Instant>,
    busy: bool,
}

/// Each lookup's last ask and whether it is still waiting on its answer.
pub struct Schedule {
    started: Instant,
    entries: Vec<Entry>,
    was_away: bool,
    github_asked: Option<Instant>,
}

impl Default for Schedule {
    fn default() -> Schedule {
        Schedule::new(&LOOKUPS, Instant::now())
    }
}

impl Schedule {
    pub fn new(lookups: &[Lookup], now: Instant) -> Schedule {
        Schedule {
            started: now,
            entries: lookups
                .iter()
                .map(|&lookup| Entry {
                    lookup,
                    last: None,
                    busy: false,
                })
                .collect(),
            was_away: false,
            github_asked: None,
        }
    }

    /// The lookups due now, each marked asked and waiting: every one whose
    /// interval has passed (`fast` with the `poll_fast` setting on, else its
    /// slow one), none while `away` but its first ask after starting, and
    /// every one not already waiting on the tick you come back.
    pub fn due(&mut self, now: Instant, fast: bool, away: bool) -> Vec<&'static str> {
        let back = self.was_away && !away;
        self.was_away = away;
        let started = self.started;
        let mut due = Vec::new();
        for entry in &mut self.entries {
            if entry.busy || (away && entry.last.is_some()) {
                continue;
            }
            let interval = if fast {
                entry.lookup.fast
            } else {
                entry.lookup.slow
            };
            let ready = back
                || match entry.last {
                    None => now + SLACK >= started + entry.lookup.first,
                    Some(last) => now + SLACK >= last + interval,
                };
            if ready {
                entry.last = Some(now);
                entry.busy = true;
                due.push(entry.lookup.name);
            }
        }
        due
    }

    /// Asks one lookup now, out of its turn (going to its view, the
    /// `github` file, a refresh asked for): `false`, and nothing asked, while
    /// it is still waiting on its last answer.
    pub fn ask(&mut self, name: &str, now: Instant) -> bool {
        match self.entries.iter_mut().find(|e| e.lookup.name == name) {
            Some(entry) if !entry.busy => {
                entry.last = Some(now);
                entry.busy = true;
                true
            }
            _ => false,
        }
    }

    /// A lookup answered (or was skipped once asked), so it may be asked
    /// again.
    pub fn done(&mut self, name: &str) {
        if let Some(entry) = self.entries.iter_mut().find(|e| e.lookup.name == name) {
            entry.busy = false;
        }
    }

    /// Whether a lookup is still waiting on its answer.
    pub fn busy(&self, name: &str) -> bool {
        self.entries.iter().any(|e| e.lookup.name == name && e.busy)
    }

    /// Whether the `github` file may ask the PR lookups again now: at most
    /// once every `GITHUB_NOW`.
    pub fn github_now(&mut self, now: Instant) -> bool {
        if self
            .github_asked
            .is_some_and(|at| now.duration_since(at) < GITHUB_NOW)
        {
            return false;
        }
        self.github_asked = Some(now);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: fn(u64) -> Duration = Duration::from_secs;

    fn one(fast: u64, slow: u64, first: u64) -> Vec<Lookup> {
        vec![Lookup {
            name: "x",
            fast: S(fast),
            slow: S(slow),
            first: S(first),
        }]
    }

    #[test]
    fn first_asked_soon_after_starting_then_on_its_interval() {
        let t0 = Instant::now();
        let mut s = Schedule::new(&one(15, 60, 2), t0);
        assert!(s.due(t0, true, false).is_empty(), "not at once");
        assert_eq!(s.due(t0 + S(2), true, false), vec!["x"]);
        s.done("x");
        assert!(s.due(t0 + S(12), true, false).is_empty());
        // A tick a moment early still counts.
        assert_eq!(s.due(t0 + S(16), true, false), vec!["x"]);
        s.done("x");
        // With poll_fast off, the slow interval.
        assert!(s.due(t0 + S(40), false, false).is_empty());
        assert_eq!(s.due(t0 + S(76), false, false), vec!["x"]);
    }

    #[test]
    fn never_twice_at_once() {
        let t0 = Instant::now();
        let mut s = Schedule::new(&one(15, 60, 0), t0);
        assert_eq!(s.due(t0, true, false), vec!["x"]);
        assert!(s.due(t0 + S(60), true, false).is_empty(), "still waiting");
        assert!(!s.ask("x", t0 + S(60)), "asked out of turn, still waiting");
        assert!(s.busy("x"));
        s.done("x");
        assert!(s.ask("x", t0 + S(61)));
        s.done("x");
        assert!(
            s.due(t0 + S(70), true, false).is_empty(),
            "an ask out of turn counts as the last"
        );
        assert_eq!(s.due(t0 + S(76), true, false), vec!["x"]);
    }

    #[test]
    fn paused_while_away_and_all_asked_on_return() {
        let t0 = Instant::now();
        let mut s = Schedule::new(&LOOKUPS, t0);
        // The first asks after starting go out with no one there, as the
        // daemon's first timeouts did; then nothing until you're back.
        let first = s.due(t0 + S(10), true, true);
        assert_eq!(first.len(), LOOKUPS.len(), "each one's first ask");
        for name in first {
            s.done(name);
        }
        assert!(s.due(t0 + S(600), true, true).is_empty());
        let back = s.due(t0 + S(601), true, false);
        assert_eq!(back.len(), LOOKUPS.len(), "every one on return");
        for name in back {
            s.done(name);
        }
        // Back on their intervals after that.
        assert!(s.due(t0 + S(603), true, false).is_empty());
        // One still waiting when you come back isn't asked twice.
        s.ask("board", t0 + S(700));
        s.due(t0 + S(701), true, true);
        assert!(!s.due(t0 + S(800), true, false).contains(&"board"));
    }

    #[test]
    fn the_github_file_asks_at_most_every_fifteen_seconds() {
        let t0 = Instant::now();
        let mut s = Schedule::new(&LOOKUPS, t0);
        assert!(s.github_now(t0));
        assert!(!s.github_now(t0 + S(14)));
        assert!(s.github_now(t0 + S(15)));
    }
}
