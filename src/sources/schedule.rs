//! When each GitHub and Linear lookup is due, as the sidebar daemon's
//! `polled` and `guarded` decided (lib/sidebar/daemon.js): each runs every
//! `FAST` with the `poll_fast` setting on (off at first; the PR board every 30 seconds),
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
//! GitHub's hour's limit slows its lookups down (`Budget`): with under
//! `LOW` of the hour left, each waits four times its interval, and with
//! under `SCARCE` left only the default branch's CI is asked, until the
//! limit resets; what you ask for by a key (`ask`) is asked all the same.
//! Linear's lookups go on as they were.
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

/// The lookups that ask GitHub, which its hour's limit slows down.
const GITHUB_ALL: [&str; 5] = ["merges", "board", "checks", "mergeQueues", "mainCi"];

/// Under this share of GitHub's hour left, its lookups slow down; under
/// `SCARCE`, all but the default branch's CI stop.
pub const LOW: f64 = 0.2;
pub const SCARCE: f64 = 0.05;

/// How much of GitHub's hour's limit is left, as the lookups pace by it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Budget {
    #[default]
    Plenty,
    /// Under `LOW` left: four times each interval.
    Low,
    /// Under `SCARCE` left: only `mainCi`.
    Scarce,
}

impl Budget {
    /// The budget for a share of the hour left (`None`: nothing known).
    pub fn of(left: Option<f64>) -> Budget {
        match left {
            Some(l) if l < SCARCE => Budget::Scarce,
            Some(l) if l < LOW => Budget::Low,
            _ => Budget::Plenty,
        }
    }
}

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
    budget: Budget,
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
            budget: Budget::Plenty,
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
        let budget = self.budget;
        let mut due = Vec::new();
        for entry in &mut self.entries {
            if entry.busy || (away && entry.last.is_some()) {
                continue;
            }
            let github = GITHUB_ALL.contains(&entry.lookup.name);
            if github && budget == Budget::Scarce && entry.lookup.name != "mainCi" {
                continue;
            }
            let mut interval = if fast {
                entry.lookup.fast
            } else {
                entry.lookup.slow
            };
            if github && budget != Budget::Plenty {
                interval *= 4;
            }
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

    /// Paces GitHub's lookups by what is left of its hour (`due`), giving
    /// the budget it had.
    pub fn set_budget(&mut self, budget: Budget) -> Budget {
        std::mem::replace(&mut self.budget, budget)
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
    fn github_s_lookups_slow_down_near_the_hour_s_limit() {
        assert_eq!(Budget::of(None), Budget::Plenty);
        assert_eq!(Budget::of(Some(0.5)), Budget::Plenty);
        assert_eq!(Budget::of(Some(0.19)), Budget::Low);
        assert_eq!(Budget::of(Some(0.04)), Budget::Scarce);
        let t0 = Instant::now();
        let mut s = Schedule::new(&LOOKUPS, t0);
        for name in s.due(t0 + S(10), false, false) {
            s.done(name);
        }
        assert_eq!(s.set_budget(Budget::Low), Budget::Plenty);
        // The PR board's two minutes are eight under 20%; Linear's aren't.
        let at = s.due(t0 + S(131), false, false);
        assert!(!at.contains(&"board") && at.contains(&"tickets"), "{at:?}");
        for name in at {
            s.done(name);
        }
        assert!(s.due(t0 + S(10 + 480), false, false).contains(&"board"));
        for name in LOOKUPS.map(|l| l.name) {
            s.done(name);
        }
        // Under 5%, only the default branch's CI, at four times its two minutes.
        s.set_budget(Budget::Scarce);
        let at = s.due(t0 + S(10_000), false, false);
        assert!(at.contains(&"mainCi") && at.contains(&"tickets"), "{at:?}");
        assert!(
            !at.iter()
                .any(|n| ["merges", "board", "checks", "mergeQueues"].contains(n))
        );
        assert!(s.ask("board", t0 + S(10_001)), "a key still asks");
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
