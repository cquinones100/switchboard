//! A tab's reminder (tmux/remind.sh, M-p's "Remind me about this tab", now
//! in a tab's menu in mission control too): when, as Slack's "Remind me
//! about this" asks (in 20 minutes, in an hour, in 3 hours, tomorrow, next
//! week, or a time typed in: 45m, 2h, 1d, 3pm, 15:30, and here tomorrow
//! 9am too, or tomorrow alone for 9:00), then what for, which
//! may be left empty. The tab waits under the sidebar's Reminders with the
//! time left, and when it comes due it moves to Needs you and is said once,
//! until you go to the tab. The time and note are the tab's `@remind_at`
//! (seconds) and `@remind_note`; `@remind_said` holds the time it was said
//! for, so it is said once however often the server looks.
//!
//! remind.sh worked the times out with BSD `date`; here jiff does, in the
//! local time zone, so a time already past today is tomorrow's by the
//! calendar and a change of clocks overnight leaves it at the time asked for.

use std::sync::LazyLock;

use jiff::Zoned;
use jiff::civil::{Time, Weekday};
use regex::Regex;

/// The times offered, as remind.sh's list gave them.
pub const CHOICES: [&str; 5] = [
    "In 20 minutes",
    "In 1 hour",
    "In 3 hours",
    "Tomorrow at 9:00",
    "Next Monday at 9:00",
];

/// What the list of times says of typing one.
pub const TYPED_HINT: &str = "Or type a time: 45m, 2h, 1d, 3pm, 15:30, tomorrow 9am.";

/// The time a choice or a typed text names, in seconds, or `None`.
pub fn when(text: &str, now: &Zoned) -> Option<i64> {
    static MINUTES: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^([0-9]+)(m|min|mins|minutes?)$").expect("a regex"));
    static HOURS: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^([0-9]+)(h|hr|hrs|hours?)$").expect("a regex"));
    static DAYS: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^([0-9]+)(d|days?)$").expect("a regex"));
    let text: String = text
        .to_lowercase()
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    let secs = now.timestamp().as_second();
    let at_nine = |date: jiff::civil::Date| -> Option<i64> {
        date.to_datetime(Time::new(9, 0, 0, 0).ok()?)
            .to_zoned(now.time_zone().clone())
            .ok()
            .map(|z| z.timestamp().as_second())
    };
    match text.as_str() {
        "in20minutes" => return Some(secs + 20 * 60),
        "in1hour" => return Some(secs + 3600),
        "in3hours" => return Some(secs + 3 * 3600),
        "tomorrowat9:00" => return at_nine(now.date().tomorrow().ok()?),
        "nextmondayat9:00" => {
            // Tomorrow, or the Monday after it (date's `-v+1d -v+mon`).
            let mut day = now.date().tomorrow().ok()?;
            while day.weekday() != Weekday::Monday {
                day = day.tomorrow().ok()?;
            }
            return at_nine(day);
        }
        _ => {}
    }
    // Tomorrow at a time, or at 9:00 with none given.
    if let Some(rest) = text.strip_prefix("tomorrow") {
        let rest = rest.strip_prefix("at").unwrap_or(rest);
        let date = now.date().tomorrow().ok()?;
        if rest.is_empty() {
            return at_nine(date);
        }
        let time = clock(rest, true)?;
        return date
            .to_datetime(time)
            .to_zoned(now.time_zone().clone())
            .ok()
            .map(|z| z.timestamp().as_second());
    }
    let number = |m: &regex::Captures| m[1].parse::<i64>().ok();
    if let Some(m) = MINUTES.captures(&text) {
        return Some(secs + number(&m)? * 60);
    }
    if let Some(m) = HOURS.captures(&text) {
        return Some(secs + number(&m)? * 3600);
    }
    if let Some(m) = DAYS.captures(&text) {
        return Some(secs + number(&m)? * 86_400);
    }
    let time = clock(&text, false)?;
    let today = now
        .date()
        .to_datetime(time)
        .to_zoned(now.time_zone().clone())
        .ok()?;
    if today.timestamp().as_second() > secs {
        return Some(today.timestamp().as_second());
    }
    now.date()
        .tomorrow()
        .ok()?
        .to_datetime(time)
        .to_zoned(now.time_zone().clone())
        .ok()
        .map(|z| z.timestamp().as_second())
}

/// A clock time, lowercased and without spaces: `3pm`, `15:30`, `9:15am`;
/// a bare hour (`9`) only where `bare` says one can be, since alone a
/// number is a count of something, not a time.
fn clock(text: &str, bare: bool) -> Option<Time> {
    static CLOCK: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^([0-9]{1,2})(:([0-9]{2}))?(am|pm)?$").expect("a regex"));
    let m = CLOCK.captures(text)?;
    let mut hour: i8 = m[1].parse().ok()?;
    let minute: i8 = m.get(3).map_or(Ok(0), |v| v.as_str().parse()).ok()?;
    let meridiem = m.get(4).map(|v| v.as_str());
    if !bare && meridiem.is_none() && m.get(2).is_none() {
        return None;
    }
    if meridiem.is_some() && !(1..=12).contains(&hour) {
        return None;
    }
    match meridiem {
        Some("pm") if hour < 12 => hour += 12,
        Some("am") if hour == 12 => hour = 0,
        _ => {}
    }
    if hour > 23 || minute > 59 {
        return None;
    }
    Time::new(hour, minute, 0, 0).ok()
}

/// When a reminder is, as remind.sh said it once set: `Fri 3:15 PM`.
pub fn said(at: i64) -> String {
    let z = crate::util::local_at_ms(at * 1000);
    let hour = z.hour();
    let (twelve, meridiem) = match hour {
        0 => (12, "AM"),
        1..=11 => (hour, "AM"),
        12 => (12, "PM"),
        _ => (hour - 12, "PM"),
    };
    let day = match z.weekday() {
        Weekday::Monday => "Mon",
        Weekday::Tuesday => "Tue",
        Weekday::Wednesday => "Wed",
        Weekday::Thursday => "Thu",
        Weekday::Friday => "Fri",
        Weekday::Saturday => "Sat",
        Weekday::Sunday => "Sun",
    };
    format!("{day} {twelve}:{:02} {meridiem}", z.minute())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str) -> Zoned {
        text.parse().expect("a zoned time")
    }

    #[test]
    fn the_times_offered_and_typed_are_read_as_remind_sh_read_them() {
        // A Wednesday afternoon.
        let now = at("2026-11-04T14:30:00[America/New_York]");
        let secs = now.timestamp().as_second();
        assert_eq!(when("In 20 minutes", &now), Some(secs + 1200));
        assert_eq!(when("45m", &now), Some(secs + 2700));
        assert_eq!(when("2 h", &now), Some(secs + 7200));
        assert_eq!(when("1d", &now), Some(secs + 86_400));
        let tomorrow = at("2026-11-05T09:00:00[America/New_York]");
        assert_eq!(
            when("Tomorrow at 9:00", &now),
            Some(tomorrow.timestamp().as_second())
        );
        let monday = at("2026-11-09T09:00:00[America/New_York]");
        assert_eq!(
            when("Next Monday at 9:00", &now),
            Some(monday.timestamp().as_second())
        );
        let three = at("2026-11-04T15:00:00[America/New_York]");
        assert_eq!(when("3pm", &now), Some(three.timestamp().as_second()));
        let early = at("2026-11-05T09:15:00[America/New_York]");
        assert_eq!(
            when("9:15", &now),
            Some(early.timestamp().as_second()),
            "a time already past today is tomorrow's"
        );
        assert_eq!(when("3", &now), None, "a bare number names no time");
        let nine = at("2026-11-05T09:00:00[America/New_York]");
        for typed in ["tomorrow", "tomorrow 9am", "Tomorrow at 9", "tomorrow 9:00"] {
            assert_eq!(
                when(typed, &now),
                Some(nine.timestamp().as_second()),
                "{typed}"
            );
        }
        let late = at("2026-11-05T15:30:00[America/New_York]");
        assert_eq!(
            when("tomorrow 3:30pm", &now),
            Some(late.timestamp().as_second())
        );
        assert_eq!(when("tomorrow soon", &now), None);

        assert_eq!(when("13pm", &now), None);
        assert_eq!(when("soonish", &now), None);
    }

    #[test]
    fn a_sunday_s_next_monday_is_the_next_day() {
        let now = at("2026-11-08T20:00:00[America/New_York]");
        let monday = at("2026-11-09T09:00:00[America/New_York]");
        assert_eq!(
            when("Next Monday at 9:00", &now),
            Some(monday.timestamp().as_second())
        );
    }
}
