//! A routine as it is kept, and its runs: one file a routine,
//! `routines/<id>.json` in the state directory (state, not a file of your
//! own, since the Routines view writes it), and a line a run in
//! `routines/runs-<id>.tsv`: when it was due, when it ran, its tab, how
//! long it took, its cost, its outcome and the reply's first line (or why
//! it didn't run).

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::schedule::Schedule;
use crate::{paths, util};

/// Where a run happens.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TabPolicy {
    /// The same tab each run, its Claude session resumed once it closed.
    #[default]
    SameTab,
    /// A new tab each run.
    NewTab,
}

/// What a run does when its tab's Claude is still working.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WhenBusy {
    /// Waits for the turn to end, up to `WAIT_BUSY`.
    #[default]
    Wait,
    Skip,
}

/// What a run missed while the computer slept does.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WhenMissed {
    /// Runs when the computer wakes, up to `LATE_BY` late.
    #[default]
    OnWaking,
    Skip,
}

/// A prompt of yours, run on a schedule in a Claude tab.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Routine {
    pub id: String,
    pub name: String,
    pub dir: String,
    /// When it runs, as typed, for the box to show again.
    pub when: String,
    pub schedule: Schedule,
    #[serde(default)]
    pub tab_policy: TabPolicy,
    #[serde(default)]
    pub when_busy: WhenBusy,
    #[serde(default)]
    pub when_missed: WhenMissed,
    /// Added to Claude's system prompt in its tab.
    pub prompt: String,
    /// Claude's model, empty for Claude Code's own default.
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub paused: bool,
    /// The Claude session its last run was in, empty before one, and the
    /// directory it was made in, the only one `--resume` finds it from.
    #[serde(default)]
    pub session: String,
    #[serde(default)]
    pub session_dir: String,
    /// The time (seconds) of the last run it handled, run or not; when it
    /// was made, before its first.
    pub handled: i64,
}

impl Routine {
    /// The session to resume in its directory as it is now: its own while
    /// it was made there, else none, whatever changed the directory (the box,
    /// or its file by hand while no server ran).
    pub fn resumable(&self) -> &str {
        if self.session_dir == self.dir {
            &self.session
        } else {
            ""
        }
    }

    /// The words its tab is given for each run.
    pub fn turn(&self) -> String {
        format!("Run {}", self.name)
    }

    /// When it is next due.
    pub fn next(&self, tz: &jiff::tz::TimeZone) -> Option<i64> {
        self.schedule.next_after(self.handled, tz)
    }

    /// Its prompt and model as a short hash, kept for the pane its Claude
    /// started in (`Routines::started`) to tell when they changed since.
    /// FNV-1a, so it stays the same from one build to the next.
    pub fn prompt_hash(&self) -> String {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for b in self.prompt.bytes().chain([0]).chain(self.model.bytes()) {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x100_0000_01b3);
        }
        format!("{h:016x}")
    }
}

/// How a run went.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// It ran and its turn ended.
    Ran,
    /// Missed while the computer slept, and run on waking.
    Late,
    /// Missed while the computer slept, and skipped.
    Missed,
    /// Its tab was busy, and it was skipped.
    Busy,
    /// Its turn ended in an error, Claude exited or the tab closed.
    Failed,
    /// Its directory isn't one to run in (someone else's PR).
    Refused,
}

impl Outcome {
    fn word(self) -> &'static str {
        match self {
            Outcome::Ran => "ran",
            Outcome::Late => "late",
            Outcome::Missed => "missed",
            Outcome::Busy => "busy",
            Outcome::Failed => "failed",
            Outcome::Refused => "refused",
        }
    }

    fn of_word(word: &str) -> Option<Outcome> {
        [
            Outcome::Ran,
            Outcome::Late,
            Outcome::Missed,
            Outcome::Busy,
            Outcome::Failed,
            Outcome::Refused,
        ]
        .into_iter()
        .find(|o| o.word() == word)
    }

    /// Its mark in the week and the runs: ✓ ran, ◌ late, – skipped, ✕
    /// failed.
    pub fn mark(self) -> &'static str {
        match self {
            Outcome::Ran => "✓",
            Outcome::Late => "◌",
            Outcome::Missed | Outcome::Busy => "–",
            Outcome::Failed | Outcome::Refused => "✕",
        }
    }

    /// Whether it ran.
    pub fn ran(self) -> bool {
        matches!(self, Outcome::Ran | Outcome::Late)
    }
}

/// A run, as its line keeps it.
#[derive(Clone, Debug, PartialEq)]
pub struct Run {
    /// When it was due (seconds).
    pub due: i64,
    /// When it started, or 0 for one that didn't.
    pub ran: i64,
    /// Its tab's number, empty for none.
    pub tab: String,
    /// How long it took, in seconds.
    pub secs: i64,
    /// What it cost, in dollars, as Claude Code estimates it.
    pub cost: f64,
    pub outcome: Outcome,
    /// The reply's first line, or why it didn't run.
    pub words: String,
}

impl Run {
    fn line(&self) -> String {
        let clean = |s: &str| s.replace(['\t', '\n', '\r'], " ");
        format!(
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
            self.due,
            self.ran,
            clean(&self.tab),
            self.secs,
            self.cost,
            self.outcome.word(),
            clean(&self.words)
        )
    }

    fn read(line: &str) -> Option<Run> {
        let f: Vec<&str> = line.splitn(7, '\t').collect();
        Some(Run {
            due: f.first()?.parse().ok()?,
            ran: f.get(1)?.parse().ok()?,
            tab: f.get(2)?.to_string(),
            secs: f.get(3)?.parse().ok()?,
            cost: f.get(4)?.parse().ok()?,
            outcome: Outcome::of_word(f.get(5)?)?,
            words: f.get(6).copied().unwrap_or("").to_string(),
        })
    }

    /// When it happened: when it ran, else when it was due.
    pub fn at(&self) -> i64 {
        if self.ran > 0 { self.ran } else { self.due }
    }
}

/// The routines' folder in the state directory.
pub fn dir() -> PathBuf {
    paths::state_dir().join("routines")
}

fn file(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.json"))
}

fn runs_file(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("runs-{id}.tsv"))
}

/// When the routines last changed: the latest modified time of their
/// folder (one made or taken away) and of each routine's file (one changed
/// in place, as by hand).
pub fn stamp(dir: &Path) -> Option<std::time::SystemTime> {
    let folder = fs::metadata(dir).and_then(|m| m.modified()).ok()?;
    let files = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().ends_with(".json"))
        .filter_map(|e| e.metadata().and_then(|m| m.modified()).ok());
    files.chain([folder]).max()
}

/// Every routine kept, oldest first; a file that doesn't read is said on
/// stderr (the server's log) and passed over.
pub fn load_all(dir: &Path) -> Vec<Routine> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<Routine> = entries
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().ends_with(".json"))
        .filter_map(|e| {
            let text = fs::read_to_string(e.path()).ok()?;
            match serde_json::from_str::<Routine>(&text) {
                Ok(r) => Some(r),
                Err(err) => {
                    eprintln!("routines: {} doesn't read: {err}", e.path().display());
                    None
                }
            }
        })
        .collect();
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

/// Writes a routine's file whole.
pub fn save(dir: &Path, routine: &Routine) -> std::io::Result<()> {
    let text = serde_json::to_string_pretty(routine).map_err(std::io::Error::other)?;
    util::write_atomic(file(dir, &routine.id), format!("{text}\n"))
}

/// Takes a routine and its runs away.
pub fn remove(dir: &Path, id: &str) {
    let _ = fs::remove_file(file(dir, id));
    let _ = fs::remove_file(runs_file(dir, id));
}

/// A runs file grown past this is cut to its last `KEEP_DAYS`, so reading
/// one on the server's task stays small: with runs at most every 15
/// minutes (`schedule::LEAST_EVERY`), those days hold well under it.
const RUNS_MOST: u64 = 64 * 1024;

/// Adds a run to its routine's runs; a file grown past `RUNS_MOST` keeps
/// only the runs of the days the view shows.
pub fn append_run(dir: &Path, id: &str, run: &Run) -> std::io::Result<()> {
    fs::create_dir_all(dir)?;
    let file = runs_file(dir, id);
    let mut f = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&file)?;
    f.write_all(run.line().as_bytes())?;
    if f.metadata()?.len() > RUNS_MOST {
        let since = run.at() - super::KEEP_DAYS * 86_400;
        let kept: String = read_runs(dir, id, since).iter().map(Run::line).collect();
        util::write_atomic(&file, kept)?;
    }
    Ok(())
}

/// A routine's runs since `since` (seconds), oldest first.
pub fn read_runs(dir: &Path, id: &str, since: i64) -> Vec<Run> {
    fs::read_to_string(runs_file(dir, id))
        .unwrap_or_default()
        .lines()
        .filter_map(Run::read)
        .filter(|r| r.at() >= since)
        .collect()
}

/// An id for a new routine: the time it was made, in hex, unique among
/// `taken`.
pub fn new_id(now_ms: i64, taken: &[Routine]) -> String {
    let mut n = now_ms;
    loop {
        let id = format!("r{n:x}");
        if !taken.iter().any(|r| r.id == id) {
            return id;
        }
        n += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn routine(id: &str) -> Routine {
        Routine {
            id: id.into(),
            name: "Morning triage".into(),
            dir: "/work/api".into(),
            when: "weekdays at 8:45".into(),
            schedule: Schedule::Days {
                days: super::super::schedule::Days::WEEKDAYS,
                at: 8 * 60 + 45,
            },
            tab_policy: TabPolicy::SameTab,
            when_busy: WhenBusy::Wait,
            when_missed: WhenMissed::OnWaking,
            prompt: "Read the new issues and sort them.".into(),
            model: String::new(),
            paused: false,
            session: String::new(),
            session_dir: String::new(),
            handled: 1_000,
        }
    }

    #[test]
    fn a_routine_is_written_read_back_and_removed_with_its_runs() {
        let dir = std::env::temp_dir().join(format!(
            "sb-routines-{}-{}",
            std::process::id(),
            crate::util::now_ms()
        ));
        let r = routine("r1");
        save(&dir, &r).unwrap();
        let mut other = routine("r2");
        other.paused = true;
        save(&dir, &other).unwrap();
        fs::write(dir.join("broken.json"), "{").unwrap();
        // A file changed in place, as by hand, changes the stamp.
        let before = stamp(&dir).unwrap();
        fs::File::options()
            .write(true)
            .open(dir.join("broken.json"))
            .unwrap()
            .set_modified(before + std::time::Duration::from_secs(5))
            .unwrap();
        assert!(stamp(&dir).unwrap() > before);
        assert_eq!(load_all(&dir), vec![r.clone(), other]);

        let run = Run {
            due: 2_000,
            ran: 2_005,
            tab: "9".into(),
            secs: 42,
            cost: 0.12,
            outcome: Outcome::Ran,
            words: "Sorted\tfour issues".into(),
        };
        append_run(&dir, "r1", &run).unwrap();
        let missed = Run {
            due: 3_000,
            ran: 0,
            tab: String::new(),
            secs: 0,
            cost: 0.0,
            outcome: Outcome::Missed,
            words: "asleep past 8:45 AM, so skipped".into(),
        };
        append_run(&dir, "r1", &missed).unwrap();
        let runs = read_runs(&dir, "r1", 0);
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].words, "Sorted four issues");
        assert_eq!(runs[0].cost, 0.12);
        assert_eq!(runs[1], missed);
        assert_eq!(read_runs(&dir, "r1", 2_500), vec![missed.clone()]);

        // Grown past its most, it keeps only the days the view shows.
        let old = Run {
            due: 10,
            ran: 10,
            ..missed.clone()
        };
        let late = Run {
            due: 10 + 30 * 86_400,
            ran: 0,
            ..missed.clone()
        };
        while fs::metadata(runs_file(&dir, "r2")).map_or(0, |m| m.len()) <= RUNS_MOST {
            append_run(&dir, "r2", &old).unwrap();
        }
        append_run(&dir, "r2", &late).unwrap();
        let kept = read_runs(&dir, "r2", 0);
        assert!(kept.iter().all(|r| r.due == late.due), "the old runs went");

        remove(&dir, "r1");
        assert_eq!(load_all(&dir).len(), 1);
        assert!(read_runs(&dir, "r1", 0).is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_prompts_hash_changes_with_its_words_or_model() {
        let r = routine("r1");
        let mut changed = r.clone();
        changed.prompt.push('!');
        let mut model = r.clone();
        model.model = "opus".into();
        assert_ne!(r.prompt_hash(), changed.prompt_hash());
        assert_ne!(r.prompt_hash(), model.prompt_hash());
        assert_eq!(r.prompt_hash(), routine("r9").prompt_hash());
    }

    #[test]
    fn a_new_id_is_never_one_taken() {
        let taken = vec![routine("r10")];
        assert_eq!(new_id(16, &taken), "r11");
        assert_eq!(new_id(17, &taken), "r11");
    }
}
