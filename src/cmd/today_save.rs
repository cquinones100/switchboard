//! `today-save`: Save the day /today read, for mission control's Calendar
//! view, which reads it from `calendar.json` in the state directory. Ported
//! from `claude/skills/today/scripts/save.js`; the saving is
//! `day::calendar::save_today`.
//!
//!   switchboard today-save < day.json
//!
//! day.json: `{ "date": "2026-10-28", "events": [{ "title": "Standup",
//! "start": "2026-10-28T13:00:00-04:00", "end": "2026-10-28T13:15:00-04:00",
//! "allDay": false, "you": "going", "join": "https://…zoom.us/j/…",
//! "link": "https://www.google.com/calendar/event?eid=…" }] }`. Only those
//! fields are kept, and only https links.

use std::io::Read;
use std::process::ExitCode;

use crate::day::calendar;
use crate::paths;

pub fn main(_args: Vec<String>) -> ExitCode {
    let mut input = String::new();
    if let Err(e) = std::io::stdin().read_to_string(&mut input) {
        eprintln!("Not JSON: {e}");
        return ExitCode::FAILURE;
    }
    match calendar::save_today(&paths::state_dir(), &input) {
        Ok(said) => {
            println!("{said}");
            ExitCode::SUCCESS
        }
        Err(why) => {
            eprintln!("{why}");
            ExitCode::FAILURE
        }
    }
}
