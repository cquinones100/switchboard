//! A tab's reminder (tmux/remind.sh, M-p's "Remind me about this tab", now
//! in a tab's menu in mission control too): when, as Slack's "Remind me
//! about this" asks (in 20 minutes, in an hour, in 3 hours, tomorrow, next
//! week, or a time typed in: 45m, 2h, 1d, 3pm, 15:30, and here tomorrow
//! 9am too, or tomorrow alone for 9:00, a weekday (`tue 9am`) or a date
//! (`nov 20`, `11/20`, `2027-01-04 3pm`), at 9:00 with no time, a date
//! already past this year being next year's), then what for, which
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
pub const TYPED_HINT: &str =
    "Or type a time: 45m, 2h, 1d, 3pm, 15:30, tomorrow 9am, dec 1, 2027-01-04 3pm.";

/// What a time that can't be read is told, after the text typed.
pub const NOT_A_TIME: &str =
    "isn't a time I know. Try 45m, 2h, 3pm, 15:30, nov 20 or 2027-01-04 3pm.";

/// The time a choice or a typed text names, in seconds, or `None`.
pub fn when(text: &str, now: &Zoned) -> Option<i64> {
    static MINUTES: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^([0-9]+)(m|min|mins|minutes?)$").expect("a regex"));
    static HOURS: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^([0-9]+)(h|hr|hrs|hours?)$").expect("a regex"));
    static DAYS: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^([0-9]+)(d|days?)$").expect("a regex"));
    if let Some(at) = dated(text, now) {
        return at;
    }
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

/// A weekday or a date with an optional time (`tue 9am`, `nov 20`,
/// `dec 1 3pm`, `11/20`, `2027-01-04 15:30`), at 9:00 without one: the
/// time it names, `Some(None)` for one named but not a time to come (a
/// date that isn't, or one past with its year given), `None` when the text
/// names no day, for `when` to read as it reads the rest.
fn dated(text: &str, now: &Zoned) -> Option<Option<i64>> {
    static DAY: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"^(?:(?P<mon>jan|feb|mar|apr|may|jun|jul|aug|sep|oct|nov|dec)[a-z]*\.?\s*(?P<md>[0-9]{1,2})(?:st|nd|rd|th)?|(?P<sm>[0-9]{1,2})/(?P<sd>[0-9]{1,2})|(?P<y>[0-9]{4})-(?P<ym>[0-9]{1,2})-(?P<yd>[0-9]{1,2})|(?P<wd>mon|tue|wed|thu|fri|sat|sun)[a-z]*)(?:,?\s+(?:at\s+)?(?P<time>.+))?$",
        )
        .expect("a regex")
    });
    let lower = text.trim().to_lowercase();
    let m = DAY.captures(&lower)?;
    let time = match m.name("time") {
        Some(t) => {
            let t: String = t.as_str().chars().filter(|c| !c.is_whitespace()).collect();
            Some(clock(&t, true))
        }
        None => None,
    };
    // A weekday's word must be one (`mon`, `monday`, not `month`).
    if m.name("wd").is_some() {
        let whole = lower.split_whitespace().next().unwrap_or("");
        let full = [
            "monday",
            "tuesday",
            "wednesday",
            "thursday",
            "friday",
            "saturday",
            "sunday",
        ];
        if !full
            .iter()
            .any(|f| f.starts_with(whole) && whole.len() >= 3)
        {
            return None;
        }
    }
    let Some(time) = time.unwrap_or(Time::new(9, 0, 0, 0).ok()) else {
        return Some(None);
    };
    let secs = now.timestamp().as_second();
    let at = |date: jiff::civil::Date| -> Option<i64> {
        date.to_datetime(time)
            .to_zoned(now.time_zone().clone())
            .ok()
            .map(|z| z.timestamp().as_second())
    };
    let number = |name: &str| m.name(name).and_then(|v| v.as_str().parse::<i16>().ok());
    if let Some(wd) = m.name("wd") {
        let want = match wd.as_str() {
            "mon" => Weekday::Monday,
            "tue" => Weekday::Tuesday,
            "wed" => Weekday::Wednesday,
            "thu" => Weekday::Thursday,
            "fri" => Weekday::Friday,
            "sat" => Weekday::Saturday,
            _ => Weekday::Sunday,
        };
        let mut day = now.date();
        for _ in 0..8 {
            if day.weekday() == want && at(day).is_some_and(|t| t > secs) {
                return Some(at(day));
            }
            day = day.tomorrow().ok()?;
        }
        return Some(None);
    }
    if let (Some(y), Some(mo), Some(d)) = (number("y"), number("ym"), number("yd")) {
        let date = jiff::civil::Date::new(y, mo as i8, d as i8).ok();
        return Some(date.and_then(at).filter(|t| *t > secs));
    }
    let (mo, d) = match m.name("mon") {
        Some(name) => {
            let months = [
                "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
            ];
            let mo = months.iter().position(|x| *x == name.as_str())? as i16 + 1;
            (mo, number("md")?)
        }
        None => (number("sm")?, number("sd")?),
    };
    let this_year = now.year();
    for year in [this_year, this_year + 1] {
        let Ok(date) = jiff::civil::Date::new(year, mo as i8, d as i8) else {
            return Some(None);
        };
        if let Some(t) = at(date).filter(|t| *t > secs) {
            return Some(Some(t));
        }
    }
    Some(None)
}

/// A clock time, lowercased and without spaces: `3pm`, `15:30`, `9:15am`;
/// a bare hour (`9`) only where `bare` says one can be, since alone a
/// number is a count of something, not a time.
pub(crate) fn clock(text: &str, bare: bool) -> Option<Time> {
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

/// When a reminder is, as remind.sh said it once set: `Fri 3:15 PM`, and
/// beyond this week with its date, `Tue Dec 1, 9:00 AM`.
pub fn said(at: i64) -> String {
    said_from(at, &crate::util::local_now())
}

/// `said` as it is said at `now`.
pub fn said_from(at: i64, now: &Zoned) -> String {
    let z = jiff::Timestamp::from_second(at)
        .unwrap_or(jiff::Timestamp::UNIX_EPOCH)
        .to_zoned(now.time_zone().clone());
    let days = z.date().since(now.date()).map_or(0, |d| d.get_days());
    let clock = twelve(&z);
    if (0..7).contains(&days) {
        format!("{} {clock}", short_day(z.weekday()))
    } else {
        format!(
            "{} {} {}, {clock}",
            short_day(z.weekday()),
            short_month(z.month()),
            z.day()
        )
    }
}

/// A reminder's time read back in full: `Tuesday, November 10 at 9:00 AM`.
pub fn said_long(at: i64, now: &Zoned) -> String {
    let z = jiff::Timestamp::from_second(at)
        .unwrap_or(jiff::Timestamp::UNIX_EPOCH)
        .to_zoned(now.time_zone().clone());
    let day = format!("{:?}", z.weekday());
    let month = MONTHS[(z.month() - 1) as usize];
    let year = if z.year() == now.year() {
        String::new()
    } else {
        format!(", {}", z.year())
    };
    format!("{day}, {month} {}{year} at {}", z.day(), twelve(&z))
}

/// The clock time a time offered in the reminder's box means, at its
/// right: `2:35` today, `Sat 9:00` on another day.
pub fn clock_of(at: i64, now: &Zoned) -> String {
    let z = jiff::Timestamp::from_second(at)
        .unwrap_or(jiff::Timestamp::UNIX_EPOCH)
        .to_zoned(now.time_zone().clone());
    let hour = match z.hour() % 12 {
        0 => 12,
        h => h,
    };
    let clock = format!("{hour}:{:02}", z.minute());
    if z.date() == now.date() {
        clock
    } else {
        format!("{} {clock}", short_day(z.weekday()))
    }
}

/// How long until a reminder, in words: `in 45 minutes`, `in 2 hours 5
/// minutes`, `in 3 days 19 hours`.
pub fn left(at: i64, now_secs: i64) -> String {
    let minutes = ((at - now_secs) as f64 / 60.0).ceil().max(1.0) as i64;
    let unit = |n: i64, one: &str| {
        if n == 1 {
            format!("1 {one}")
        } else {
            format!("{n} {one}s")
        }
    };
    let (days, hours, mins) = (minutes / 1440, minutes % 1440 / 60, minutes % 60);
    if days > 0 {
        if hours > 0 {
            format!("in {} {}", unit(days, "day"), unit(hours, "hour"))
        } else {
            format!("in {}", unit(days, "day"))
        }
    } else if hours > 0 {
        if mins > 0 {
            format!("in {} {}", unit(hours, "hour"), unit(mins, "minute"))
        } else {
            format!("in {}", unit(hours, "hour"))
        }
    } else {
        format!("in {}", unit(mins, "minute"))
    }
}

/// The months' names, from January.
pub const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

/// A month's first three letters: `Dec`.
pub fn short_month(month: i8) -> &'static str {
    &MONTHS[(month.clamp(1, 12) - 1) as usize][..3]
}

/// A weekday's first three letters: `Tue`.
pub fn short_day(day: Weekday) -> &'static str {
    match day {
        Weekday::Monday => "Mon",
        Weekday::Tuesday => "Tue",
        Weekday::Wednesday => "Wed",
        Weekday::Thursday => "Thu",
        Weekday::Friday => "Fri",
        Weekday::Saturday => "Sat",
        Weekday::Sunday => "Sun",
    }
}

/// A time on a twelve-hour clock: `3:15 PM`.
fn twelve(z: &Zoned) -> String {
    let hour = z.hour();
    let (twelve, meridiem) = match hour {
        0 => (12, "AM"),
        1..=11 => (hour, "AM"),
        12 => (12, "PM"),
        _ => (hour - 12, "PM"),
    };
    format!("{twelve}:{:02} {meridiem}", z.minute())
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
    fn dates_and_weekdays_are_read_at_9_00_with_no_time_and_a_past_date_is_next_year_s() {
        // A Wednesday afternoon.
        let now = at("2026-11-04T14:30:00[America/New_York]");
        let secs = |t: &str| Some(at(t).timestamp().as_second());
        assert_eq!(
            when("nov 20", &now),
            secs("2026-11-20T09:00:00[America/New_York]")
        );
        assert_eq!(
            when("11/20", &now),
            secs("2026-11-20T09:00:00[America/New_York]")
        );
        assert_eq!(
            when("Dec 1 3pm", &now),
            secs("2026-12-01T15:00:00[America/New_York]")
        );
        assert_eq!(
            when("2027-01-04 3pm", &now),
            secs("2027-01-04T15:00:00[America/New_York]")
        );
        assert_eq!(
            when("march 3", &now),
            secs("2027-03-03T09:00:00[America/New_York]"),
            "a date past this year is next year's"
        );
        assert_eq!(
            when("tue 9am", &now),
            secs("2026-11-10T09:00:00[America/New_York]")
        );
        assert_eq!(
            when("wednesday 5pm", &now),
            secs("2026-11-04T17:00:00[America/New_York]"),
            "today's weekday at a time still to come is today"
        );
        assert_eq!(when("2020-01-04", &now), None, "a time gone by");
        assert_eq!(when("nov 31", &now), None);
        assert_eq!(when("nov 20 soon", &now), None);
        assert_eq!(when("month", &now), None);
    }

    #[test]
    fn a_far_date_is_said_with_its_date_and_read_back_in_full() {
        let now = at("2026-11-04T14:30:00[America/New_York]");
        let secs = |t: &str| at(t).timestamp().as_second();
        let friday = secs("2026-11-06T15:15:00[America/New_York]");
        assert_eq!(said_from(friday, &now), "Fri 3:15 PM");
        let far = secs("2026-12-01T09:00:00[America/New_York]");
        assert_eq!(said_from(far, &now), "Tue Dec 1, 9:00 AM");
        let tuesday = secs("2026-11-10T09:00:00[America/New_York]");
        assert_eq!(said_long(tuesday, &now), "Tuesday, November 10 at 9:00 AM");
        assert_eq!(
            left(tuesday, now.timestamp().as_second()),
            "in 5 days 18 hours"
        );
        assert_eq!(
            left(
                now.timestamp().as_second() + 2700,
                now.timestamp().as_second()
            ),
            "in 45 minutes"
        );
        assert_eq!(clock_of(tuesday, &now), "Tue 9:00");
        assert_eq!(
            clock_of(secs("2026-11-04T16:35:00[America/New_York]"), &now),
            "4:35"
        );
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
