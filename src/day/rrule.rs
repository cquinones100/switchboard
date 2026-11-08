//! Repeating events, as RFC 5545's RRULE says them, worked out on the wall
//! clock of the event's own time zone: what ical.js's iterator did for
//! lib/sidebar/calendar-feed.js. Occurrences are made a period at a time (a
//! day, a week, a month or a year after the first), each period's dates
//! found from its BY parts, then cut by BYSETPOS, so a rule is walked from
//! its start in order and COUNT counts what it should.
//!
//! What Google's feeds use is covered: DAILY, WEEKLY, MONTHLY and YEARLY,
//! with INTERVAL, COUNT, UNTIL, BYDAY (with an ordinal in a month or a year,
//! "2SU", "-1FR"), BYMONTHDAY (counting back from the month's end when
//! negative), BYMONTH, BYSETPOS and WKST. A rule repeating more often than
//! daily, or by hour, minute, week number or day of the year, isn't one a
//! calendar of meetings has, and is read as no rule at all: the event's
//! first time alone.

use jiff::Timestamp;
use jiff::civil::{Date, DateTime, Weekday};

/// How far a rule is walked, at most: twenty thousand periods is over fifty
/// years of a daily event, as calendar-feed.js gave up after as many steps.
pub const MAX_PERIODS: i64 = 20_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Freq {
    Daily,
    Weekly,
    Monthly,
    Yearly,
}

/// The last time a rule repeats at: a date for an all-day event, else a time
/// on the event's own clock or, as Google writes it, in UTC.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Until {
    Date(Date),
    Local(DateTime),
    Utc(Timestamp),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rule {
    pub freq: Freq,
    pub interval: i64,
    pub count: Option<u32>,
    pub until: Option<Until>,
    /// Each weekday with its ordinal, 0 for every one.
    pub by_day: Vec<(i32, Weekday)>,
    pub by_month_day: Vec<i32>,
    pub by_month: Vec<i8>,
    pub by_set_pos: Vec<i32>,
    pub wkst: Weekday,
}

/// A weekday from its two letters.
pub fn weekday(code: &str) -> Option<Weekday> {
    Some(match code.to_ascii_uppercase().as_str() {
        "MO" => Weekday::Monday,
        "TU" => Weekday::Tuesday,
        "WE" => Weekday::Wednesday,
        "TH" => Weekday::Thursday,
        "FR" => Weekday::Friday,
        "SA" => Weekday::Saturday,
        "SU" => Weekday::Sunday,
        _ => return None,
    })
}

impl Rule {
    /// A rule from its text (`FREQ=WEEKLY;BYDAY=MO,WE`), with its UNTIL read
    /// by `until`, which knows the event's time zone. `None` for a rule this
    /// doesn't walk.
    pub fn parse(text: &str, until: impl Fn(&str) -> Option<Until>) -> Option<Rule> {
        let mut rule = Rule {
            freq: Freq::Daily,
            interval: 1,
            count: None,
            until: None,
            by_day: Vec::new(),
            by_month_day: Vec::new(),
            by_month: Vec::new(),
            by_set_pos: Vec::new(),
            wkst: Weekday::Monday,
        };
        let mut freq = None;
        for part in text.split(';') {
            let Some((name, value)) = part.split_once('=') else {
                continue;
            };
            let list = || value.split(',').map(str::trim).filter(|v| !v.is_empty());
            match name.trim().to_ascii_uppercase().as_str() {
                "FREQ" => {
                    freq = Some(match value.trim().to_ascii_uppercase().as_str() {
                        "DAILY" => Freq::Daily,
                        "WEEKLY" => Freq::Weekly,
                        "MONTHLY" => Freq::Monthly,
                        "YEARLY" => Freq::Yearly,
                        _ => return None,
                    })
                }
                "INTERVAL" => rule.interval = value.trim().parse().ok().filter(|n| *n > 0)?,
                "COUNT" => rule.count = Some(value.trim().parse().ok()?),
                "UNTIL" => rule.until = until(value.trim()),
                "WKST" => rule.wkst = weekday(value.trim()).unwrap_or(Weekday::Monday),
                "BYDAY" => {
                    for v in list() {
                        let split = v.len().checked_sub(2)?;
                        let (n, code) = v.split_at(split);
                        let n = if n.is_empty() { 0 } else { n.parse().ok()? };
                        rule.by_day.push((n, weekday(code)?));
                    }
                }
                "BYMONTHDAY" => {
                    for v in list() {
                        rule.by_month_day.push(v.parse().ok()?);
                    }
                }
                "BYMONTH" => {
                    for v in list() {
                        rule.by_month
                            .push(v.parse().ok().filter(|m| (1..=12).contains(m))?);
                    }
                }
                "BYSETPOS" => {
                    for v in list() {
                        rule.by_set_pos.push(v.parse().ok()?);
                    }
                }
                // What a calendar of meetings never repeats by.
                "BYHOUR" | "BYMINUTE" | "BYSECOND" | "BYWEEKNO" | "BYYEARDAY" => return None,
                _ => {}
            }
        }
        rule.freq = freq?;
        Some(rule)
    }

    /// Walks the occurrences from `start` (which is always the first) in
    /// order, handing each to `keep` until it says to stop, the rule ends
    /// (its COUNT, or past its UNTIL as `past_until` reads it), or
    /// `MAX_PERIODS` have been walked.
    pub fn walk(
        &self,
        start: DateTime,
        past_until: impl Fn(DateTime) -> bool,
        mut keep: impl FnMut(DateTime) -> bool,
    ) {
        if self.count == Some(0) || past_until(start) || !keep(start) {
            return;
        }
        let mut made = 1;
        if self.count.is_some_and(|c| made >= c) {
            return;
        }
        for k in 0..MAX_PERIODS {
            for date in self.dates(start, k) {
                let at = date.to_datetime(start.time());
                if at <= start {
                    continue;
                }
                if past_until(at) {
                    return;
                }
                made += 1;
                if !keep(at) || self.count.is_some_and(|c| made >= c) {
                    return;
                }
            }
        }
    }

    // The dates of the `k`th period after the one `start` is in, in order.
    fn dates(&self, start: DateTime, k: i64) -> Vec<Date> {
        let first = start.date();
        let step = k * self.interval;
        let mut dates = match self.freq {
            Freq::Daily => {
                let Ok(day) = first.checked_add(jiff::Span::new().days(step)) else {
                    return Vec::new();
                };
                let fits = (self.by_month.is_empty() || self.by_month.contains(&day.month()))
                    && (self.by_month_day.is_empty() || month_day_matches(day, &self.by_month_day))
                    && (self.by_day.is_empty()
                        || self.by_day.iter().any(|(_, wd)| *wd == day.weekday()));
                if fits { vec![day] } else { Vec::new() }
            }
            Freq::Weekly => {
                let back = (first.weekday().to_monday_zero_offset()
                    - self.wkst.to_monday_zero_offset())
                .rem_euclid(7);
                let Ok(week) = first.checked_add(jiff::Span::new().days(step * 7 - back as i64))
                else {
                    return Vec::new();
                };
                let days: Vec<Weekday> = if self.by_day.is_empty() {
                    vec![first.weekday()]
                } else {
                    self.by_day.iter().map(|(_, wd)| *wd).collect()
                };
                days.into_iter()
                    .filter_map(|wd| {
                        let ahead = (wd.to_monday_zero_offset()
                            - self.wkst.to_monday_zero_offset())
                        .rem_euclid(7);
                        week.checked_add(jiff::Span::new().days(ahead as i64)).ok()
                    })
                    .filter(|d| self.by_month.is_empty() || self.by_month.contains(&d.month()))
                    .collect()
            }
            Freq::Monthly => {
                let months = first.year() as i64 * 12 + first.month() as i64 - 1 + step;
                let (year, month) = (months.div_euclid(12), (months.rem_euclid(12) + 1) as i8);
                if !self.by_month.is_empty() && !self.by_month.contains(&month) {
                    return Vec::new();
                }
                self.month_dates(year, month, first.day())
            }
            Freq::Yearly => {
                let year = first.year() as i64 + step;
                if !self.by_month.is_empty() {
                    self.by_month
                        .iter()
                        .flat_map(|m| self.month_dates(year, *m, first.day()))
                        .collect()
                } else if !self.by_month_day.is_empty() {
                    (1..=12)
                        .flat_map(|m| self.month_dates(year, m, first.day()))
                        .collect()
                } else if !self.by_day.is_empty() {
                    year_dates(year, &self.by_day)
                } else {
                    i16::try_from(year)
                        .ok()
                        .and_then(|y| Date::new(y, first.month(), first.day()).ok())
                        .into_iter()
                        .collect()
                }
            }
        };
        dates.sort();
        dates.dedup();
        if self.by_set_pos.is_empty() {
            return dates;
        }
        let len = dates.len() as i32;
        let mut picked: Vec<Date> = self
            .by_set_pos
            .iter()
            .filter_map(|p| {
                let at = if *p > 0 { p - 1 } else { len + p };
                (0..len).contains(&at).then(|| dates[at as usize])
            })
            .collect();
        picked.sort();
        picked.dedup();
        picked
    }

    // The dates in one month that the rule's month days or weekdays give,
    // else the day the rule started on.
    fn month_dates(&self, year: i64, month: i8, day: i8) -> Vec<Date> {
        let Ok(first) = i16::try_from(year)
            .map_err(|_| ())
            .and_then(|y| Date::new(y, month, 1).map_err(|_| ()))
        else {
            return Vec::new();
        };
        let last = first.days_in_month();
        if !self.by_month_day.is_empty() {
            return self
                .by_month_day
                .iter()
                .filter_map(|d| {
                    let d = if *d < 0 { last as i32 + 1 + d } else { *d };
                    (1..=last as i32)
                        .contains(&d)
                        .then(|| first.with().day(d as i8).build().ok())?
                })
                .filter(|date| {
                    self.by_day.is_empty()
                        || self.by_day.iter().any(|(_, wd)| *wd == date.weekday())
                })
                .collect();
        }
        if !self.by_day.is_empty() {
            return self
                .by_day
                .iter()
                .flat_map(|(n, wd)| {
                    if *n == 0 {
                        every_weekday(first, last as i16, *wd)
                    } else {
                        i8::try_from(*n)
                            .ok()
                            .and_then(|n| first.nth_weekday_of_month(n, *wd).ok())
                            .into_iter()
                            .collect()
                    }
                })
                .collect();
        }
        (day <= last)
            .then(|| first.with().day(day).build().ok())
            .flatten()
            .into_iter()
            .collect()
    }
}

// Whether a date is one of the month days given, counted from the end when
// negative.
fn month_day_matches(date: Date, days: &[i32]) -> bool {
    let last = date.days_in_month() as i32;
    days.iter().any(|d| {
        let d = if *d < 0 { last + 1 + d } else { *d };
        d == date.day() as i32
    })
}

// Every `wd` among the `len` days from `first`.
fn every_weekday(first: Date, len: i16, wd: Weekday) -> Vec<Date> {
    let ahead =
        (wd.to_monday_zero_offset() - first.weekday().to_monday_zero_offset()).rem_euclid(7);
    (0..)
        .map(|w| ahead as i64 + w * 7)
        .take_while(|d| *d < len as i64)
        .filter_map(|d| first.checked_add(jiff::Span::new().days(d)).ok())
        .collect()
}

// The weekdays of a year a yearly rule with no month names: every one, or
// the nth (from the end when negative).
fn year_dates(year: i64, by_day: &[(i32, Weekday)]) -> Vec<Date> {
    let Ok(first) = i16::try_from(year)
        .map_err(|_| ())
        .and_then(|y| Date::new(y, 1, 1).map_err(|_| ()))
    else {
        return Vec::new();
    };
    let len = first.days_in_year();
    by_day
        .iter()
        .flat_map(|(n, wd)| {
            let all = every_weekday(first, len, *wd);
            if *n == 0 {
                return all;
            }
            let at = if *n > 0 { n - 1 } else { all.len() as i32 + n };
            usize::try_from(at)
                .ok()
                .and_then(|i| all.get(i).copied())
                .into_iter()
                .collect()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use jiff::civil::date;

    fn walk(rule: &str, start: DateTime, n: usize) -> Vec<String> {
        let rule = Rule::parse(rule, |v| {
            let d = v.get(..8)?;
            Some(Until::Date(
                Date::new(
                    d[..4].parse().ok()?,
                    d[4..6].parse().ok()?,
                    d[6..].parse().ok()?,
                )
                .ok()?,
            ))
        })
        .expect("a rule");
        let mut out = Vec::new();
        rule.walk(
            start,
            |at| match rule.until {
                Some(Until::Date(d)) => at.date() > d,
                _ => false,
            },
            |at| {
                out.push(at.date().to_string());
                out.len() < n
            },
        );
        out
    }

    #[test]
    fn weekdays_every_week() {
        let start = date(2026, 9, 1).at(13, 0, 0, 0);
        assert_eq!(
            walk("FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR", start, 7),
            [
                "2026-09-01",
                "2026-09-02",
                "2026-09-03",
                "2026-09-04",
                "2026-09-07",
                "2026-09-08",
                "2026-09-09"
            ]
        );
    }

    #[test]
    fn every_other_week_counts_from_the_week_it_started() {
        let start = date(2026, 10, 7).at(15, 0, 0, 0);
        assert_eq!(
            walk("FREQ=WEEKLY;INTERVAL=2;BYDAY=WE", start, 3),
            ["2026-10-07", "2026-10-21", "2026-11-04"]
        );
    }

    #[test]
    fn the_second_sunday_of_march_and_the_last_friday() {
        let start = date(1970, 3, 8).at(2, 0, 0, 0);
        let years = walk("FREQ=YEARLY;BYMONTH=3;BYDAY=2SU", start, 3);
        assert_eq!(years, ["1970-03-08", "1971-03-14", "1972-03-12"]);
        let start = date(2026, 1, 30).at(9, 0, 0, 0);
        assert_eq!(
            walk("FREQ=MONTHLY;BYDAY=-1FR", start, 3),
            ["2026-01-30", "2026-02-27", "2026-03-27"]
        );
    }

    #[test]
    fn count_and_until_end_it() {
        let start = date(2026, 11, 2).at(9, 0, 0, 0);
        assert_eq!(walk("FREQ=DAILY;COUNT=3", start, 10).len(), 3);
        assert_eq!(
            walk("FREQ=DAILY;UNTIL=20261104T235959Z", start, 10),
            ["2026-11-02", "2026-11-03", "2026-11-04"]
        );
    }

    #[test]
    fn the_last_weekday_of_the_month_by_set_position() {
        let start = date(2026, 10, 30).at(16, 0, 0, 0);
        assert_eq!(
            walk("FREQ=MONTHLY;BYDAY=MO,TU,WE,TH,FR;BYSETPOS=-1", start, 3),
            ["2026-10-30", "2026-11-30", "2026-12-31"]
        );
    }

    #[test]
    fn the_31st_skips_short_months() {
        let start = date(2026, 1, 31).at(9, 0, 0, 0);
        assert_eq!(
            walk("FREQ=MONTHLY", start, 3),
            ["2026-01-31", "2026-03-31", "2026-05-31"]
        );
    }

    #[test]
    fn by_the_hour_is_no_rule() {
        assert!(Rule::parse("FREQ=HOURLY", |_| None).is_none());
        assert!(Rule::parse("FREQ=DAILY;BYHOUR=9,17", |_| None).is_none());
    }
}
