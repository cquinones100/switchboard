//! The starter routines: today's todo note, the meeting notes, and the
//! Slack and Calendar tabs that kept those views up to date, which were
//! jobs built into the server with a schedule and a prompt of their own
//! (src/day/todo.rs, src/day/meetings.rs, the day part's loop tabs) and are
//! now routines of yours. Each is a file in `assets/routines/`, made a
//! routine (`routines/<id>.json`) once, the first time a server with them
//! starts, paused unless what it replaces was on: the todo note with the
//! old `todo_note` setting on (at its `todo_note_time`), meeting notes with
//! `meeting_notes` not turned off and Fathom's connector seen working,
//! Slack with `slack_background` off and the Slack view on, Calendar with
//! the Calendar view on, no feed and a day /today saved. A marker,
//! `routines/.starters`, keeps them from being made again, so one you
//! delete stays deleted and no update writes over one you changed.

use std::path::Path;

use jiff::Zoned;
use serde::Deserialize;

use super::routine::{self, Routine};
use super::schedule::Schedule;
use super::{TabPolicy, WhenBusy, WhenMissed};
use crate::{assets, util};

/// The marker in the routines' folder saying the starters were made: a
/// JSON object of each one's id by its kind's key (`Kind::key`), so the
/// Slack view finds its routine whatever it is named.
pub const MARKER: &str = ".starters";

/// A starter as its file holds it.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Starter {
    pub name: String,
    pub when: String,
    pub model: String,
    pub prompt: String,
}

/// Which starter, for whether it starts paused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    TodoNote,
    MeetingNotes,
    Slack,
    Calendar,
}

impl Kind {
    /// Its key in the marker.
    pub fn key(self) -> &'static str {
        match self {
            Kind::TodoNote => "todo-note",
            Kind::MeetingNotes => "meeting-notes",
            Kind::Slack => "slack",
            Kind::Calendar => "calendar",
        }
    }
}

/// Each starter: its kind, its path under `assets/`, and its compiled text.
const FILES: [(Kind, &str, &str); 4] = [
    (
        Kind::TodoNote,
        "routines/todo-note.json",
        include_str!("../../assets/routines/todo-note.json"),
    ),
    (
        Kind::MeetingNotes,
        "routines/meeting-notes.json",
        include_str!("../../assets/routines/meeting-notes.json"),
    ),
    (
        Kind::Slack,
        "routines/slack.json",
        include_str!("../../assets/routines/slack.json"),
    ),
    (
        Kind::Calendar,
        "routines/calendar.json",
        include_str!("../../assets/routines/calendar.json"),
    ),
];

/// What was on before the starters, which decides the ones that run.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Was {
    /// The `todo_note` setting.
    pub todo_note: bool,
    /// The `todo_note_time` setting's hour, 7 to 11.
    pub todo_hour: u8,
    pub meeting_notes: bool,
    /// The server would have kept a Slack tab: `slack_background` off and
    /// the Slack view on.
    pub slack_tab: bool,
    /// The server would have kept a Calendar tab: the Calendar view on, no
    /// feed, and a day /today saved.
    pub calendar_tab: bool,
}

/// Every starter, as the assets hold them now.
pub fn starters() -> Vec<(Kind, Starter)> {
    FILES
        .iter()
        .filter_map(|(kind, path, compiled)| {
            match serde_json::from_str(&assets::text(path, compiled)) {
                Ok(s) => Some((*kind, s)),
                Err(e) => {
                    eprintln!("routines: the starter {path} doesn't read: {e}");
                    None
                }
            }
        })
        .collect()
}

/// The starters as routines of yours, made at `now`, in `home`, each
/// with its kind.
pub fn routines(was: &Was, home: &Path, now: &Zoned, taken: &[Routine]) -> Vec<(Kind, Routine)> {
    let mut made: Vec<Routine> = taken.to_vec();
    let mut out = Vec::new();
    for (kind, s) in starters() {
        let (when, on) = match kind {
            // Every hour from the old setting's, until the note is there,
            // as the todo note was written whenever the server ran after it.
            Kind::TodoNote => (
                format!("every 1h {}-11pm weekdays", was.todo_hour),
                was.todo_note,
            ),
            Kind::MeetingNotes => (s.when.clone(), was.meeting_notes),
            Kind::Slack => (s.when.clone(), was.slack_tab),
            Kind::Calendar => (s.when.clone(), was.calendar_tab),
        };
        let once_a_day = kind == Kind::TodoNote;
        let schedule = match Schedule::parse(&when, now) {
            Ok(schedule) => schedule,
            Err(e) => {
                eprintln!("routines: the starter {} {e}", s.name);
                continue;
            }
        };
        let r = Routine {
            id: routine::new_id(now.timestamp().as_millisecond(), &made),
            name: s.name,
            dir: home.display().to_string(),
            when,
            schedule,
            tab_policy: TabPolicy::SameTab,
            // A busy tab skips the run: each runs again soon (the todo note
            // within the hour, and only once you're here, which a run held
            // for its tab couldn't wait for).
            when_busy: WhenBusy::Skip,
            when_missed: if once_a_day {
                WhenMissed::OnWaking
            } else {
                WhenMissed::Skip
            },
            prompt: s.prompt,
            model: s.model,
            paused: !on,
            session: String::new(),
            session_dir: String::new(),
            // The todo note counts from today's start, so a day whose
            // hour has passed still gets its note on the first tick, as
            // the built-in note did whenever the server ran after its hour.
            handled: if once_a_day {
                now.start_of_day()
                    .map_or(now.timestamp().as_second(), |z| z.timestamp().as_second())
            } else {
                now.timestamp().as_second()
            },
        };
        made.push(r.clone());
        out.push((kind, r));
    }
    out
}

/// Makes the starters in `dir` unless its marker says they were made, and
/// writes the marker; gives the routines made.
pub fn make_once(dir: &Path, was: &Was, home: &Path, now: &Zoned) -> Vec<Routine> {
    let marker = dir.join(MARKER);
    if marker.exists() {
        return Vec::new();
    }
    if let Err(e) = std::fs::create_dir_all(dir) {
        eprintln!("routines: couldn't make {}: {e}", dir.display());
        return Vec::new();
    }
    let made = routines(was, home, now, &routine::load_all(dir));
    let mut ids = serde_json::Map::new();
    for (kind, r) in &made {
        if let Err(e) = routine::save(dir, r) {
            eprintln!("routines: couldn't save the starter {}: {e}", r.name);
        }
        ids.insert(kind.key().into(), r.id.clone().into());
    }
    if let Err(e) = util::write_atomic(&marker, util::json_compact(&serde_json::Value::Object(ids)))
    {
        eprintln!("routines: couldn't mark the starters made: {e}");
    }
    made.into_iter().map(|(_, r)| r).collect()
}

/// Whether `r` is the todo note's starter, its prompt still the one it
/// shipped with, and today's note is there already, so its run is passed
/// over without a turn of Claude: it runs hourly only to catch up on a day
/// the computer was off at its hour. A prompt you changed runs as written.
pub fn done_today(dir: &Path, r: &Routine) -> bool {
    is_todo_note(dir, r)
        && crate::day::Dirs::here()
            .notes
            .join("Todos")
            .join(format!("Today's Todo {}.md", util::ymd(&util::local_now())))
            .exists()
}

/// Whether `r` is the unchanged todo note's starter and `due` (seconds)
/// was another day's run, held while you were away: passed over, as a
/// note is today's alone, written on its weekdays.
pub fn from_another_day(dir: &Path, r: &Routine, due: i64, tz: &jiff::tz::TimeZone) -> bool {
    let day = |t: i64| {
        jiff::Timestamp::from_second(t)
            .map(|t| t.to_zoned(tz.clone()).date())
            .ok()
    };
    day(due) != day(util::now_secs()) && is_todo_note(dir, r)
}

/// Whether `r` is the todo note's starter with the prompt it shipped
/// with, which the tick treats as that note: passed over once the day's is
/// there, and held while you're away. A prompt you changed runs as any
/// routine of yours.
pub fn is_todo_note(dir: &Path, r: &Routine) -> bool {
    id_of(dir, Kind::TodoNote).as_deref() == Some(r.id.as_str())
        && starters()
            .iter()
            .any(|(kind, s)| *kind == Kind::TodoNote && s.prompt == r.prompt)
}

/// The id a starter was made with, while the marker says.
pub fn id_of(dir: &Path, kind: Kind) -> Option<String> {
    util::read_json_object(dir.join(MARKER))
        .get(kind.key())
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> Zoned {
        "2026-11-10T08:00:00[America/New_York]".parse().unwrap()
    }

    #[test]
    fn every_starter_reads_and_its_schedule_does_too() {
        let starters = starters();
        assert_eq!(starters.len(), FILES.len());
        for (_, s) in &starters {
            assert!(!s.name.is_empty() && !s.prompt.is_empty());
            Schedule::parse(&s.when, &now()).unwrap_or_else(|e| panic!("{}: {e}", s.name));
        }
        let made = routines(&Was::default(), Path::new("/home/me"), &now(), &[]);
        assert_eq!(made.len(), FILES.len());
        assert!(made.iter().all(|(_, r)| r.paused && r.dir == "/home/me"));
    }

    #[test]
    fn the_starters_run_as_what_they_replace_did() {
        let was = Was {
            todo_note: true,
            todo_hour: 7,
            meeting_notes: false,
            slack_tab: true,
            calendar_tab: false,
        };
        let made = routines(&was, Path::new("/home/me"), &now(), &[]);
        let todo = &made[0].1;
        assert_eq!(todo.name, "Today's todo note");
        assert_eq!(todo.when, "every 1h 7-11pm weekdays");
        assert!(!todo.paused);
        assert_eq!(
            todo.next(now().time_zone()),
            Some(now().timestamp().as_second() - 3600),
            "today's 7:00 still to come at 8:00"
        );
        assert_eq!(todo.model, "sonnet");
        let paused: Vec<(&str, bool)> = made
            .iter()
            .map(|(_, r)| (r.name.as_str(), r.paused))
            .collect();
        assert_eq!(
            paused,
            [
                ("Today's todo note", false),
                ("Meeting notes", true),
                ("Slack", false),
                ("Calendar", true)
            ]
        );
        let ids: std::collections::HashSet<&str> =
            made.iter().map(|(_, r)| r.id.as_str()).collect();
        assert_eq!(ids.len(), made.len(), "each its own id");
    }

    #[test]
    fn they_are_made_once_and_one_deleted_stays_deleted() {
        let dir = std::env::temp_dir().join(format!(
            "sb-starters-{}-{}",
            std::process::id(),
            util::now_ms()
        ));
        let home = Path::new("/home/me");
        let made = make_once(&dir, &Was::default(), home, &now());
        assert_eq!(routine::load_all(&dir).len(), made.len());
        assert_eq!(
            id_of(&dir, Kind::Slack).as_deref(),
            Some(made[2].id.as_str())
        );
        routine::remove(&dir, &made[0].id);
        assert!(make_once(&dir, &Was::default(), home, &now()).is_empty());
        assert_eq!(routine::load_all(&dir).len(), made.len() - 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
