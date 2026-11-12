//! When a routine runs, read from what you type in its box: once ("nov 20
//! 3pm", "2027-01-04 9:00", "tomorrow 9am", read by the reminder's own
//! `remind::when`), or repeating ("weekdays at 8:45", "mondays 10:00",
//! "every 30m 9-6 weekdays", "every 2h", "the 1st at 9:00", "daily 7:30").
//! A weekday alone ("mon 9am") is once, as a reminder reads it; plural, or
//! after "every", it repeats.
//!
//! Its times are worked out with jiff in the local time zone from the day
//! and the clock time asked for, so a change of clocks leaves a routine at
//! the time asked for (8:45 stays 8:45), as remind.rs's times do.

use std::sync::LazyLock;

use jiff::civil::{Date, Time, Weekday};
use jiff::tz::TimeZone;
use jiff::{Timestamp, Zoned};
use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::mission::remind;

/// The weekdays a schedule runs on, a bit each from Monday's.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Days(pub u8);

impl Days {
    pub const ALL: Days = Days(0b111_1111);
    pub const WEEKDAYS: Days = Days(0b001_1111);
    pub const WEEKENDS: Days = Days(0b110_0000);

    fn bit(day: Weekday) -> u8 {
        1 << day.to_monday_zero_offset()
    }

    pub fn has(self, day: Weekday) -> bool {
        self.0 & Self::bit(day) != 0
    }

    fn of(day: Weekday) -> Days {
        Days(Self::bit(day))
    }

    /// Monday to Friday, Mondays, Mondays, Wednesdays and Fridays.
    fn words(self) -> String {
        const NAMES: [&str; 7] = [
            "Monday",
            "Tuesday",
            "Wednesday",
            "Thursday",
            "Friday",
            "Saturday",
            "Sunday",
        ];
        if self == Days::ALL {
            return "Every day".into();
        }
        if self == Days::WEEKENDS {
            return "Saturday and Sunday".into();
        }
        let on: Vec<usize> = (0..7).filter(|i| self.0 & (1 << i) != 0).collect();
        let (Some(&first), Some(&last)) = (on.first(), on.last()) else {
            return String::new();
        };
        if on.len() >= 3 && last - first + 1 == on.len() {
            return format!("{} to {}", NAMES[first], NAMES[last]);
        }
        let plural: Vec<String> = on.iter().map(|i| format!("{}s", NAMES[*i])).collect();
        match plural.split_last() {
            Some((last, [])) => last.clone(),
            Some((last, rest)) => format!("{} and {last}", rest.join(", ")),
            None => String::new(),
        }
    }
}

/// When a routine runs. Clock times are minutes from midnight.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Schedule {
    /// Once, at a time (seconds).
    Once { at: i64 },
    /// On some weekdays at a clock time.
    Days { days: Days, at: u16 },
    /// Every so many minutes on some weekdays, within a window of the day
    /// (`from` to `to`, both run at; the whole day for 0 and 1439).
    Every {
        minutes: u32,
        days: Days,
        from: u16,
        to: u16,
    },
    /// On a day of each month (the month's last in a shorter month).
    Monthly { day: i8, at: u16 },
}

/// What a schedule that can't be read is told, after the text typed.
pub const NOT_A_SCHEDULE: &str = "isn't a schedule I can read. Try weekdays at 8:45, every 30m 9-6, the 1st at 9:00 or nov 20 3pm.";

/// The shortest interval between two runs, in minutes.
pub const LEAST_EVERY: u32 = 15;

/// What an interval under `LEAST_EVERY` is told.
pub const TOO_OFTEN: &str = "runs a turn of Claude each time, so it runs every 15 minutes at most: try every 15m or every 30m.";

/// What an interval that doesn't divide the day is told.
pub const UNEVEN: &str = "runs at the same times each day, so the day has to hold it evenly: try every 6h or every 8h, or daily at a time.";

/// Examples, for the box.
pub const EXAMPLES: &str = "weekdays at 8:45 · mondays 10:00 · every 30m 9-6 weekdays · every 2h · the 1st at 9:00 · nov 20 3pm";

const WHOLE_DAY: (u16, u16) = (0, 24 * 60 - 1);

impl Schedule {
    /// Reads what was typed, at `now`: a repeating schedule, else a time
    /// to come once.
    pub fn parse(text: &str, now: &Zoned) -> Result<Schedule, String> {
        let typed = text.trim();
        if typed.is_empty() {
            return Err("Type when it runs.".into());
        }
        let lower = typed
            .to_lowercase()
            .replace(['–', '—'], "-")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        if let Some(found) = repeating(&lower) {
            return found.map_err(|why| format!("\"{typed}\" {why}"));
        }
        match remind::when(&lower, now) {
            Some(at) if at > now.timestamp().as_second() => Ok(Schedule::Once { at }),
            _ => Err(format!("\"{typed}\" {NOT_A_SCHEDULE}")),
        }
    }

    /// Whether it runs once.
    pub fn once(&self) -> bool {
        matches!(self, Schedule::Once { .. })
    }

    /// The first time it runs after `t` (seconds), in `tz`; `None` when it
    /// runs no more.
    pub fn next_after(&self, t: i64, tz: &TimeZone) -> Option<i64> {
        let start = Timestamp::from_second(t).ok()?.to_zoned(tz.clone()).date();
        let at = |date: Date, minutes: u16| -> Option<i64> {
            let time = Time::new((minutes / 60) as i8, (minutes % 60) as i8, 0, 0).ok()?;
            date.to_datetime(time)
                .to_zoned(tz.clone())
                .ok()
                .map(|z| z.timestamp().as_second())
        };
        let days_on = |i: i64| start.checked_add(jiff::Span::new().days(i)).ok();
        match self {
            Schedule::Once { at } => (*at > t).then_some(*at),
            Schedule::Days { days, at: minutes } => (0..=8)
                .filter_map(days_on)
                .filter(|d| days.has(d.weekday()))
                .filter_map(|d| at(d, *minutes))
                .find(|s| *s > t),
            Schedule::Every {
                minutes,
                days,
                from,
                to,
            } => {
                let step = (*minutes).max(1) as usize;
                for d in (0..=8).filter_map(days_on) {
                    if !days.has(d.weekday()) {
                        continue;
                    }
                    for m in (*from..=*to).step_by(step) {
                        if let Some(s) = at(d, m).filter(|s| *s > t) {
                            return Some(s);
                        }
                    }
                }
                None
            }
            Schedule::Monthly { day, at: minutes } => {
                let first = start.first_of_month();
                (0..14)
                    .filter_map(|i| first.checked_add(jiff::Span::new().months(i)).ok())
                    .filter_map(|month| {
                        let last = month.last_of_month().day();
                        Date::new(month.year(), month.month(), (*day).min(last)).ok()
                    })
                    .filter_map(|d| at(d, *minutes))
                    .find(|s| *s > t)
            }
        }
    }

    /// Its next `n` runs after `t`.
    pub fn next(&self, t: i64, n: usize, tz: &TimeZone) -> Vec<i64> {
        let mut out = Vec::new();
        let mut from = t;
        while out.len() < n {
            let Some(at) = self.next_after(from, tz) else {
                break;
            };
            out.push(at);
            from = at;
        }
        out
    }

    /// It in words: "Monday to Friday at 8:45 AM", "Every 30 minutes,
    /// 9:00 AM to 6:00 PM, Monday to Friday", "The 1st of each month at
    /// 9:00 AM", "Once, Friday, November 20 at 3:00 PM".
    pub fn describe(&self, now: &Zoned) -> String {
        match self {
            Schedule::Once { at } => format!("Once, {}", remind::said_long(*at, now)),
            Schedule::Days { days, at } => format!("{} at {}", days.words(), clock_words(*at)),
            Schedule::Every {
                minutes,
                days,
                from,
                to,
            } => {
                let every = match minutes {
                    60 => "Every hour".to_string(),
                    m if m % 60 == 0 => format!("Every {} hours", m / 60),
                    1 => "Every minute".to_string(),
                    m => format!("Every {m} minutes"),
                };
                let mut out = every;
                if (*from, *to) != WHOLE_DAY {
                    out.push_str(&format!(", {} to {}", clock_words(*from), clock_words(*to)));
                }
                if *days != Days::ALL {
                    out.push_str(&format!(", {}", days.words()));
                }
                out
            }
            Schedule::Monthly { day, at } => format!(
                "The {} of each month at {}",
                ordinal(*day),
                clock_words(*at)
            ),
        }
    }
}

/// A clock time in words: "8:45 AM".
pub fn clock_words(minutes: u16) -> String {
    let (h, m) = (minutes / 60, minutes % 60);
    let twelve = match h % 12 {
        0 => 12,
        h => h,
    };
    format!("{twelve}:{m:02} {}", if h < 12 { "AM" } else { "PM" })
}

fn ordinal(n: i8) -> String {
    let suffix = match (n % 10, n % 100) {
        (_, 11..=13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    };
    format!("{n}{suffix}")
}

/// A clock time typed, bare hours allowed: minutes from midnight, and
/// whether it said am or pm.
fn clock_of(text: &str) -> Option<(u16, bool)> {
    let squeezed: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    let time = remind::clock(&squeezed, true)?;
    let meridiem = squeezed.ends_with("am") || squeezed.ends_with("pm");
    Some((time.hour() as u16 * 60 + time.minute() as u16, meridiem))
}

/// A word naming weekdays: a day (`mon`, `monday`, `mondays`), `weekdays`,
/// `weekends`, `daily`, or a range (`mon-fri`). The days, and whether it
/// said them as repeating (plural, or a word that only repeats).
fn day_word(word: &str) -> Option<(Days, bool)> {
    match word {
        "daily" | "day" | "days" => return Some((Days::ALL, true)),
        "weekday" | "weekdays" => return Some((Days::WEEKDAYS, true)),
        "weekend" | "weekends" => return Some((Days::WEEKENDS, true)),
        _ => {}
    }
    if let Some((a, b)) = word.split_once('-') {
        let (Some((from, _)), Some((to, _))) = (one_day(a), one_day(b)) else {
            return None;
        };
        let mut days = 0u8;
        let mut d = from;
        for _ in 0..7 {
            days |= Days::of(d).0;
            if d == to {
                break;
            }
            d = Weekday::from_monday_zero_offset((d.to_monday_zero_offset() + 1) % 7)
                .unwrap_or(Weekday::Monday);
        }
        return Some((Days(days), true));
    }
    one_day(word).map(|(d, plural)| (Days::of(d), plural))
}

/// One weekday, and whether it was plural.
fn one_day(word: &str) -> Option<(Weekday, bool)> {
    const DAYS: [(&str, Weekday); 7] = [
        ("monday", Weekday::Monday),
        ("tuesday", Weekday::Tuesday),
        ("wednesday", Weekday::Wednesday),
        ("thursday", Weekday::Thursday),
        ("friday", Weekday::Friday),
        ("saturday", Weekday::Saturday),
        ("sunday", Weekday::Sunday),
    ];
    let (stem, plural) = match word.strip_suffix('s') {
        Some(s) if s.len() >= 3 && !word.ends_with("tues") && !word.ends_with("thurs") => (s, true),
        _ => (word, false),
    };
    DAYS.iter()
        .find(|(full, _)| stem.len() >= 3 && full.starts_with(stem))
        .map(|(_, d)| (*d, plural))
}

/// The weekdays leading `words` (`mon, wed and fri`, `weekdays`), the
/// words after them, and whether they were said as repeating (plural, or
/// more than one); `None` when they don't start with a day.
fn days_lead<'a>(words: &[&'a str]) -> Option<(Days, Vec<&'a str>, bool)> {
    let mut days = 0u8;
    let mut repeats = false;
    let mut named = 0;
    let mut i = 0;
    while i < words.len() {
        let word = words[i].trim_end_matches(',');
        if i > 0 && (word == "and" || word == "&") {
            i += 1;
            continue;
        }
        match day_word(word) {
            Some((d, plural)) => {
                days |= d.0;
                repeats |= plural;
                named += 1;
                i += 1;
            }
            None => break,
        }
    }
    // Several days named are a schedule, as in "mon, wed and fri".
    (days != 0).then(|| (Days(days), words[i..].to_vec(), repeats || named > 1))
}

/// The clock time ending a schedule ("at 8:45", "9am"), required.
fn time_of(rest: &[&str]) -> Result<u16, String> {
    let rest: Vec<&str> = match rest.first() {
        Some(&"at") => rest[1..].to_vec(),
        _ => rest.to_vec(),
    };
    if rest.is_empty() {
        return Err("needs a time. Try weekdays at 8:45.".into());
    }
    clock_of(&rest.join(" "))
        .map(|(m, _)| m)
        .ok_or_else(|| NOT_A_SCHEDULE.to_string())
}

/// A repeating schedule, `Some(Err)` for one that starts as one and goes
/// wrong, `None` when it doesn't read as repeating (for a time once).
fn repeating(text: &str) -> Option<Result<Schedule, String>> {
    static INTERVAL: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"^([0-9]+)\s*(m|min|mins|minutes?|h|hr|hrs|hours?)$").expect("a regex")
    });
    static MONTHLY: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"^(?:monthly\s+)?(?:on\s+)?the\s+([0-9]{1,2})(?:st|nd|rd|th)?(?:\s+of\s+(?:the|each|every)\s+month)?(?:\s+(.*))?$")
            .expect("a regex")
    });
    static WINDOW: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"^(?:from\s+)?([0-9:apm]+)\s*(?:-|to)\s*([0-9:apm]+)$").expect("a regex")
    });
    if let Some(m) = MONTHLY.captures(text) {
        let day: i8 = m[1].parse().ok()?;
        if !(1..=31).contains(&day) {
            return Some(Err(NOT_A_SCHEDULE.into()));
        }
        let rest: Vec<&str> = m
            .get(2)
            .map(|r| r.as_str().split(' ').collect())
            .unwrap_or_default();
        return Some(time_of(&rest).map(|at| Schedule::Monthly { day, at }));
    }
    let (every, text) = match text.strip_prefix("every ") {
        Some(rest) => (true, rest),
        None => (false, text),
    };
    let words: Vec<&str> = text.split(' ').collect();
    // An interval: its number and unit as one word or two.
    let (interval, rest) = match words.as_slice() {
        [n, unit, rest @ ..] if INTERVAL.is_match(&format!("{n}{unit}")) => {
            (Some(format!("{n}{unit}")), rest.to_vec())
        }
        [one, rest @ ..] if INTERVAL.is_match(one) => (Some(one.to_string()), rest.to_vec()),
        _ => (None, Vec::new()),
    };
    if let Some(interval) = interval {
        if !every {
            // "30m" alone is half an hour from now, once.
            return None;
        }
        let m = INTERVAL.captures(&interval)?;
        let n: u32 = m[1].parse().ok()?;
        let minutes = if m[2].starts_with('h') {
            match n.checked_mul(60) {
                Some(m) => m,
                None => return Some(Err(UNEVEN.into())),
            }
        } else {
            n
        };
        if minutes == 0 {
            return Some(Err(NOT_A_SCHEDULE.into()));
        }
        // Each run is a turn of Claude, which takes minutes and costs.
        if minutes < LEAST_EVERY {
            return Some(Err(TOO_OFTEN.into()));
        }
        // What follows: a window of the day and the days, either first.
        let mut rest = rest;
        let mut window = WHOLE_DAY;
        let mut days = Days::ALL;
        while !rest.is_empty() {
            if let Some((d, after, _)) = days_lead(&rest) {
                days = d;
                rest = after;
                continue;
            }
            // The window is the words up to the next day, or all of them.
            let upto = (1..=rest.len())
                .rev()
                .find(|n| WINDOW.is_match(&rest[..*n].join(" ").replace(" - ", "-")));
            let Some(n) = upto else {
                return Some(Err(NOT_A_SCHEDULE.into()));
            };
            let joined = rest[..n].join(" ").replace(" - ", "-");
            let m = WINDOW.captures(&joined)?;
            let (Some((from, _)), Some((mut to, said))) = (clock_of(&m[1]), clock_of(&m[2])) else {
                return Some(Err(NOT_A_SCHEDULE.into()));
            };
            // "9-6" is nine to six in the evening.
            if to <= from && !said && to < 12 * 60 {
                to += 12 * 60;
            }
            if to <= from {
                return Some(Err(NOT_A_SCHEDULE.into()));
            }
            window = (from, to);
            rest = rest[n..].to_vec();
        }
        // Its runs start again at the window's start each day, so one that
        // doesn't fit the whole day evenly would come round early at
        // midnight.
        if window == WHOLE_DAY && (minutes > 24 * 60 || (24 * 60) % minutes != 0) {
            return Some(Err(UNEVEN.into()));
        }
        // Within a window, the next day's first run comes at least as long
        // after the day's last as runs are kept apart.
        let last = window.0 as u32 + (window.1 - window.0) as u32 / minutes * minutes;
        if 24 * 60 - last + u32::from(window.0) < LEAST_EVERY {
            return Some(Err(TOO_OFTEN.into()));
        }
        return Some(Ok(Schedule::Every {
            minutes,
            days,
            from: window.0,
            to: window.1,
        }));
    }
    let (days, rest, plural) = days_lead(&words)?;
    // A weekday alone, as in "mon 9am", is that day once.
    if !every && !plural {
        return None;
    }
    Some(time_of(&rest).map(|at| Schedule::Days { days, at }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Wednesday, November 4, 2026, 2:30 PM in New York.
    fn now() -> Zoned {
        "2026-11-04T14:30:00[America/New_York]".parse().unwrap()
    }

    fn tz() -> TimeZone {
        TimeZone::get("America/New_York").unwrap()
    }

    fn parse(text: &str) -> Schedule {
        Schedule::parse(text, &now()).unwrap_or_else(|e| panic!("{text}: {e}"))
    }

    fn said(text: &str) -> String {
        parse(text).describe(&now())
    }

    fn when(at: i64) -> String {
        Timestamp::from_second(at)
            .unwrap()
            .to_zoned(tz())
            .strftime("%a %Y-%m-%d %H:%M")
            .to_string()
    }

    fn next_three(text: &str) -> Vec<String> {
        parse(text)
            .next(now().timestamp().as_second(), 3, &tz())
            .into_iter()
            .map(when)
            .collect()
    }

    #[test]
    fn each_form_reads_and_says_itself() {
        assert_eq!(said("weekdays at 8:45"), "Monday to Friday at 8:45 AM");
        assert_eq!(said("Mondays 10:00"), "Mondays at 10:00 AM");
        assert_eq!(said("daily 7:30"), "Every day at 7:30 AM");
        assert_eq!(said("every day at 6pm"), "Every day at 6:00 PM");
        assert_eq!(said("every monday 9am"), "Mondays at 9:00 AM");
        assert_eq!(
            said("mon, wed and fri at 9"),
            "Mondays, Wednesdays and Fridays at 9:00 AM"
        );
        assert_eq!(said("mon-thu 9:15"), "Monday to Thursday at 9:15 AM");
        assert_eq!(said("weekends 10am"), "Saturday and Sunday at 10:00 AM");
        assert_eq!(
            said("every 30m 9–6 weekdays"),
            "Every 30 minutes, 9:00 AM to 6:00 PM, Monday to Friday"
        );
        assert_eq!(
            said("every 30 minutes weekdays 9am-5pm"),
            "Every 30 minutes, 9:00 AM to 5:00 PM, Monday to Friday"
        );
        assert_eq!(said("every 2h"), "Every 2 hours");
        assert_eq!(said("every 7h 8-22"), "Every 7 hours, 8:00 AM to 10:00 PM");
        assert_eq!(said("every 1 hour"), "Every hour");
        assert_eq!(said("the 1st at 9:00"), "The 1st of each month at 9:00 AM");
        assert_eq!(
            said("monthly on the 22nd 4pm"),
            "The 22nd of each month at 4:00 PM"
        );
        assert_eq!(said("nov 20 3pm"), "Once, Friday, November 20 at 3:00 PM");
        assert_eq!(
            said("2027-01-04 9:00"),
            "Once, Monday, January 4, 2027 at 9:00 AM"
        );
        assert_eq!(
            said("tomorrow 9am"),
            "Once, Thursday, November 5 at 9:00 AM"
        );
        // A weekday alone is once, as a reminder reads it.
        assert!(parse("mon 9am").once());
        assert!(!parse("mondays 9am").once());
    }

    #[test]
    fn what_cant_be_read_says_so_and_a_time_gone_by_is_refused() {
        for bad in [
            "",
            "whenever",
            "weekdays",
            "every",
            "the 40th at 9",
            "every 0m",
            "every 5m",
            "every 55m 00:00-23:55",
            "every 1073741825h",
            "every 48h",
            "every 7h",
        ] {
            assert!(Schedule::parse(bad, &now()).is_err(), "{bad}");
        }
        let past = Schedule::parse("2020-01-04 9:00", &now()).unwrap_err();
        assert!(past.contains("isn't a schedule"), "{past}");
        let no_time = Schedule::parse("weekdays", &now()).unwrap_err();
        assert!(no_time.contains("needs a time"), "{no_time}");
    }

    #[test]
    fn the_next_runs_skip_a_weekend_and_a_months_end() {
        assert_eq!(
            next_three("weekdays at 8:45"),
            [
                "Thu 2026-11-05 08:45",
                "Fri 2026-11-06 08:45",
                "Mon 2026-11-09 08:45"
            ]
        );
        assert_eq!(
            next_three("the 31st at 9:00"),
            [
                "Mon 2026-11-30 09:00",
                "Thu 2026-12-31 09:00",
                "Sun 2027-01-31 09:00"
            ]
        );
        assert_eq!(
            next_three("every 2h 9-6"),
            [
                "Wed 2026-11-04 15:00",
                "Wed 2026-11-04 17:00",
                "Thu 2026-11-05 09:00"
            ]
        );
        let once = parse("nov 20 3pm");
        let at = once
            .next_after(now().timestamp().as_second(), &tz())
            .unwrap();
        assert_eq!(when(at), "Fri 2026-11-20 15:00");
        assert_eq!(once.next_after(at, &tz()), None, "once runs once");
    }

    #[test]
    fn a_change_of_clocks_leaves_it_at_the_time_asked_for() {
        // Clocks go back on Sunday, November 1, 2026 in New York.
        let before: Zoned = "2026-10-30T12:00:00[America/New_York]".parse().unwrap();
        let s = Schedule::parse("daily 8:45", &before).unwrap();
        let runs: Vec<String> = s
            .next(before.timestamp().as_second(), 4, &tz())
            .into_iter()
            .map(when)
            .collect();
        assert_eq!(
            runs,
            [
                "Sat 2026-10-31 08:45",
                "Sun 2026-11-01 08:45",
                "Mon 2026-11-02 08:45",
                "Tue 2026-11-03 08:45"
            ]
        );
    }
}
