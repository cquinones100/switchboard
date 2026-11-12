//! When something came, written for the end of a row: today's clock time
//! alone ("9:42 AM"), then "Yesterday 6:05 PM", the weekday within the
//! week ("Mon 2:10 PM"), the date within the year ("Nov 3, 2:10 PM"), and
//! the date alone before it ("Nov 3 2025"). A time still to come (a
//! meeting's start) reads the same way forward ("Tomorrow 3:00 PM").
//!
//! The Notifications view and window say this rather than how long ago
//! ("2h"), since a notification is looked back on, often days later, and
//! "3d" says less than "Mon 2:10 PM". The clock is 12-hour, as every other
//! time the workspace shows. New with this feature, with no tmux-version
//! file behind it.

use jiff::Zoned;

/// `ms` (since the epoch) as the row's end says it, read in `now`'s time
/// zone; empty for a time that can't be.
pub fn stamp(ms: i64, now: &Zoned) -> String {
    let Ok(at) = jiff::Timestamp::from_millisecond(ms) else {
        return String::new();
    };
    let at = at.to_zoned(now.time_zone().clone());
    let clock = at.strftime("%-I:%M %p");
    let days = (now.date() - at.date()).get_days();
    match days {
        0 => clock.to_string(),
        1 => format!("Yesterday {clock}"),
        -1 => format!("Tomorrow {clock}"),
        2..=6 => format!("{} {clock}", at.strftime("%a")),
        _ if at.year() == now.year() => format!("{}, {clock}", at.strftime("%b %-d")),
        _ => at.strftime("%b %-d %Y").to_string(),
    }
}

/// When a notification came, as its row says it: its stamp, or, for a
/// meeting still to come, "Starts" and when. Compared by the second, since
/// a view's now may be read in whole seconds while a notification that has
/// just come carries its milliseconds.
pub fn notice(ms: i64, now: &Zoned) -> String {
    if ms.div_euclid(1000) > now.timestamp().as_second() {
        format!("Starts {}", stamp(ms, now))
    } else {
        stamp(ms, now)
    }
}

/// `ms` as a clock time alone ("9:42 AM"), for a row under its day's
/// heading.
pub fn clock(ms: i64, now: &Zoned) -> String {
    jiff::Timestamp::from_millisecond(ms)
        .map(|t| {
            t.to_zoned(now.time_zone().clone())
                .strftime("%-I:%M %p")
                .to_string()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2026-11-11 09:42 UTC, a Wednesday.
    fn now() -> Zoned {
        jiff::civil::date(2026, 11, 11)
            .at(9, 42, 0, 0)
            .to_zoned(jiff::tz::TimeZone::UTC)
            .unwrap()
    }

    fn ms(month: i8, day: i8, hour: i8, minute: i8, year: i16) -> i64 {
        jiff::civil::date(year, month, day)
            .at(hour, minute, 0, 0)
            .to_zoned(jiff::tz::TimeZone::UTC)
            .unwrap()
            .timestamp()
            .as_millisecond()
    }

    #[test]
    fn a_stamp_says_as_much_of_the_date_as_it_needs() {
        let n = now();
        assert_eq!(stamp(ms(11, 11, 9, 42, 2026), &n), "9:42 AM");
        assert_eq!(stamp(ms(11, 10, 18, 5, 2026), &n), "Yesterday 6:05 PM");
        assert_eq!(stamp(ms(11, 8, 14, 10, 2026), &n), "Sun 2:10 PM");
        assert_eq!(stamp(ms(10, 3, 14, 10, 2026), &n), "Oct 3, 2:10 PM");
        assert_eq!(stamp(ms(11, 3, 14, 10, 2025), &n), "Nov 3 2025");
        assert_eq!(stamp(ms(11, 12, 15, 0, 2026), &n), "Tomorrow 3:00 PM");
    }

    #[test]
    fn a_meeting_to_come_says_when_it_starts() {
        let n = now();
        assert_eq!(notice(ms(11, 11, 15, 0, 2026), &n), "Starts 3:00 PM");
        assert_eq!(notice(ms(11, 11, 9, 0, 2026), &n), "9:00 AM");
        let just_now = n.timestamp().as_millisecond() + 700;
        assert_eq!(notice(just_now, &n), "9:42 AM");
    }

    #[test]
    fn a_stamp_reads_the_clock_in_nows_time_zone() {
        let n = now().with_time_zone(jiff::tz::TimeZone::fixed(jiff::tz::offset(-5)));
        assert_eq!(stamp(ms(11, 11, 9, 42, 2026), &n), "4:42 AM");
        assert_eq!(clock(ms(11, 11, 9, 42, 2026), &n), "4:42 AM");
    }
}
