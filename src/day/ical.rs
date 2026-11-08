//! Today's events from a Google Calendar feed in iCal format, in the shape
//! /today saves them: what lib/sidebar/calendar-feed.js's `dayFrom` made with
//! ical.js, read here by a parser of its own covering what Google's feeds
//! use (VEVENT, RRULE, EXDATE, RDATE, RECURRENCE-ID, VTIMEZONE and TZID,
//! all-day dates), so no Node package is needed.
//!
//! A time with a TZID is on that zone's wall clock, worked out by jiff from
//! the system's time zone database when the TZID is a name it knows (as
//! Google's always are), else from the feed's own VTIMEZONE (an Outlook
//! invitation's "Eastern Standard Time"), else as local time. A repeating
//! event is walked on its own wall clock (`rrule`), so a meeting at 1 PM
//! stays at 1 PM across a change of daylight saving time.
//!
//! Your reply is the PARTSTAT of the attendee who is the calendar's owner
//! (the feed's X-WR-CALNAME, its address); an event with no attendee who is
//! you is yours. Declined and cancelled events are left out. The call is
//! Google's X-GOOGLE-CONFERENCE, else a Zoom, Meet or Teams address in the
//! location or description; the event's page is built from its UID as Google
//! builds its links, a split series' `_R` part left out as Google leaves it.
//!
//! Two things differ from ical.js: an occurrence moved into today from a day
//! after it is found (ical.js stopped walking the series at the end of
//! today, before reaching it), and an all-day occurrence's Google ID takes
//! its own date, where the JavaScript took the UTC date of its local
//! midnight, a day early east of Greenwich.

use std::collections::HashMap;
use std::sync::LazyLock;

use jiff::civil::{Date, DateTime};
use jiff::tz::{Offset, TimeZone};
use jiff::{SignedDuration, Timestamp, Zoned};
use regex::Regex;

use super::calendar::{CalEvent, Ref, SavedDay};
use super::rrule::{Rule, Until};
use crate::util;

// ── Reading the text ────────────────────────────────────────────────────────

/// One content line: its name (upper case), its parameters (names upper
/// case, quotes taken off) and its value as written.
#[derive(Clone, Debug, PartialEq)]
pub struct Prop {
    pub name: String,
    pub params: Vec<(String, String)>,
    pub value: String,
}

impl Prop {
    pub fn param(&self, name: &str) -> Option<&str> {
        self.params
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }
}

/// A component (VCALENDAR, VEVENT, VTIMEZONE, ...) and those inside it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Component {
    pub name: String,
    pub props: Vec<Prop>,
    pub children: Vec<Component>,
}

impl Component {
    pub fn prop(&self, name: &str) -> Option<&Prop> {
        self.props.iter().find(|p| p.name == name)
    }

    pub fn all(&self, name: &str) -> impl Iterator<Item = &Prop> {
        self.props.iter().filter(move |p| p.name == name)
    }

    /// A text property's value, its escapes undone.
    pub fn text(&self, name: &str) -> Option<String> {
        self.prop(name).map(|p| unescape(&p.value))
    }

    pub fn has(&self, name: &str) -> bool {
        self.prop(name).is_some()
    }
}

/// The feed parsed into its components; the outermost is returned, normally
/// the VCALENDAR. Lines that don't parse are skipped, as a lenient reader
/// should with what a server sends.
pub fn parse(text: &str) -> Component {
    // A line that starts with a space or a tab continues the one before it.
    let mut lines: Vec<String> = Vec::new();
    for raw in text.split('\n') {
        let raw = raw.strip_suffix('\r').unwrap_or(raw);
        match raw.chars().next() {
            Some(' ' | '\t') if !lines.is_empty() => {
                lines.last_mut().expect("checked").push_str(&raw[1..]);
            }
            _ => lines.push(raw.to_string()),
        }
    }
    let mut stack: Vec<Component> = vec![Component::default()];
    for line in lines {
        let Some(prop) = content_line(&line) else {
            continue;
        };
        match prop.name.as_str() {
            "BEGIN" => stack.push(Component {
                name: prop.value.trim().to_ascii_uppercase(),
                ..Component::default()
            }),
            "END" if stack.len() > 1 => {
                let done = stack.pop().expect("checked");
                stack.last_mut().expect("checked").children.push(done);
            }
            "END" => {}
            _ => stack
                .last_mut()
                .expect("there is always one")
                .props
                .push(prop),
        }
    }
    // Components left open at the end are closed.
    while stack.len() > 1 {
        let done = stack.pop().expect("checked");
        stack.last_mut().expect("checked").children.push(done);
    }
    let mut root = stack.pop().expect("there is always one");
    if root.props.is_empty() && root.children.len() == 1 {
        root.children.pop().expect("checked")
    } else {
        root
    }
}

// `NAME;PARAM=value;PARAM="quoted:value":the value`.
fn content_line(line: &str) -> Option<Prop> {
    let mut quoted = false;
    let mut colon = None;
    for (i, c) in line.char_indices() {
        match c {
            '"' => quoted = !quoted,
            ':' if !quoted => {
                colon = Some(i);
                break;
            }
            _ => {}
        }
    }
    let colon = colon?;
    let (head, value) = (&line[..colon], &line[colon + 1..]);
    let mut parts = split_unquoted(head, ';').into_iter();
    let name = parts.next()?.trim().to_ascii_uppercase();
    if name.is_empty() {
        return None;
    }
    let params = parts
        .filter_map(|p| {
            let (n, v) = p.split_once('=')?;
            Some((
                n.trim().to_ascii_uppercase(),
                v.trim().trim_matches('"').to_string(),
            ))
        })
        .collect();
    Some(Prop {
        name,
        params,
        value: value.to_string(),
    })
}

fn split_unquoted(text: &str, at: char) -> Vec<&str> {
    let mut out = Vec::new();
    let mut quoted = false;
    let mut from = 0;
    for (i, c) in text.char_indices() {
        if c == '"' {
            quoted = !quoted;
        } else if c == at && !quoted {
            out.push(&text[from..i]);
            from = i + c.len_utf8();
        }
    }
    out.push(&text[from..]);
    out
}

/// A TEXT value with its escapes undone: `\n`, `\,`, `\;`, `\\`.
pub fn unescape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n' | 'N') => out.push('\n'),
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
}

// ── Times and time zones ────────────────────────────────────────────────────

/// A DATE or DATE-TIME value: a date (an all-day event), a time on a named
/// zone's wall clock (`None` for a floating time, read as local), or UTC.
#[derive(Clone, Debug, PartialEq)]
pub enum When {
    Date(Date),
    Local(DateTime, Option<String>),
    Utc(Timestamp),
}

impl When {
    pub fn is_date(&self) -> bool {
        matches!(self, When::Date(_))
    }
}

/// A value read with its property's TZID and VALUE parameters.
pub fn when_of(prop: &Prop) -> Option<When> {
    when_from(prop.value.trim(), prop.param("TZID"), prop.param("VALUE"))
}

// `20261028` as a date.
fn ymd_of(text: &str) -> Option<Date> {
    if text.len() != 8 || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Date::new(
        text[0..4].parse().ok()?,
        text[4..6].parse().ok()?,
        text[6..8].parse().ok()?,
    )
    .ok()
}

fn when_from(value: &str, tzid: Option<&str>, kind: Option<&str>) -> Option<When> {
    if kind.is_some_and(|k| k.eq_ignore_ascii_case("DATE")) || value.len() == 8 {
        return ymd_of(value.get(..8)?).map(When::Date);
    }
    let (body, utc) = match value.strip_suffix(['Z', 'z']) {
        Some(b) => (b, true),
        None => (value, false),
    };
    let (date, time) = body.split_once(['T', 't'])?;
    if time.len() != 6 || !time.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let dt = ymd_of(date)?.to_datetime(
        jiff::civil::Time::new(
            time[0..2].parse().ok()?,
            time[2..4].parse().ok()?,
            time[4..6].parse().ok()?,
            0,
        )
        .ok()?,
    );
    if utc {
        return dt
            .to_zoned(TimeZone::UTC)
            .ok()
            .map(|z| When::Utc(z.timestamp()));
    }
    Some(When::Local(dt, tzid.map(str::to_string)))
}

// One rule of a VTIMEZONE: from its onset, offsets go from `from` to `to`.
#[derive(Clone, Debug)]
struct Observance {
    start: DateTime,
    from: i32,
    to: i32,
    rule: Option<Rule>,
    rdates: Vec<DateTime>,
}

#[derive(Clone, Debug)]
enum Zone {
    Known(TimeZone),
    Rules(Vec<Observance>),
}

/// The time zones a feed's times are read in: each TZID it names, and the
/// local zone for floating times and all-day dates.
pub struct Zones {
    named: HashMap<String, Zone>,
    pub local: TimeZone,
}

fn offset_secs(text: &str) -> Option<i32> {
    let text = text.trim();
    let (sign, digits) = match text.as_bytes().first()? {
        b'+' => (1, &text[1..]),
        b'-' => (-1, &text[1..]),
        _ => (1, text),
    };
    if digits.len() < 4 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let h: i32 = digits[0..2].parse().ok()?;
    let m: i32 = digits[2..4].parse().ok()?;
    let s: i32 = digits.get(4..6).and_then(|s| s.parse().ok()).unwrap_or(0);
    Some(sign * (h * 3600 + m * 60 + s))
}

impl Zones {
    /// The zones of a calendar: each VTIMEZONE in it, as the system's
    /// database knows its name, else by its own rules.
    pub fn of(calendar: &Component, local: TimeZone) -> Zones {
        let mut named = HashMap::new();
        for tz in calendar.children.iter().filter(|c| c.name == "VTIMEZONE") {
            let Some(id) = tz.prop("TZID").map(|p| p.value.trim().to_string()) else {
                continue;
            };
            let zone = match TimeZone::get(&id) {
                Ok(known) => Zone::Known(known),
                Err(_) => Zone::Rules(
                    tz.children
                        .iter()
                        .filter(|c| c.name == "STANDARD" || c.name == "DAYLIGHT")
                        .filter_map(observance)
                        .collect(),
                ),
            };
            named.insert(id, zone);
        }
        Zones { named, local }
    }

    /// The instant a time on `tzid`'s wall clock is.
    pub fn instant(&self, dt: DateTime, tzid: Option<&str>) -> Option<Timestamp> {
        let Some(id) = tzid else {
            return self.local.to_ambiguous_timestamp(dt).compatible().ok();
        };
        match self.named.get(id) {
            Some(Zone::Known(tz)) => tz.to_ambiguous_timestamp(dt).compatible().ok(),
            Some(Zone::Rules(rules)) if !rules.is_empty() => {
                let offset = Offset::from_seconds(offset_at(rules, dt)).ok()?;
                offset.to_timestamp(dt).ok()
            }
            _ => TimeZone::get(id)
                .unwrap_or_else(|_| self.local.clone())
                .to_ambiguous_timestamp(dt)
                .compatible()
                .ok(),
        }
    }

    /// The instant a value is; a date is its local midnight.
    pub fn resolve(&self, when: &When) -> Option<Timestamp> {
        match when {
            When::Utc(ts) => Some(*ts),
            When::Local(dt, tzid) => self.instant(*dt, tzid.as_deref()),
            When::Date(d) => self.instant(d.to_datetime(jiff::civil::Time::midnight()), None),
        }
    }
}

fn observance(c: &Component) -> Option<Observance> {
    let start = match when_of(c.prop("DTSTART")?)? {
        When::Local(dt, _) => dt,
        When::Date(d) => d.to_datetime(jiff::civil::Time::midnight()),
        When::Utc(ts) => ts.to_zoned(TimeZone::UTC).datetime(),
    };
    let from = offset_secs(&c.prop("TZOFFSETFROM")?.value)?;
    let to = offset_secs(&c.prop("TZOFFSETTO")?.value)?;
    let rule = c.prop("RRULE").and_then(|p| {
        Rule::parse(&p.value, |v| {
            Some(match when_from(v, None, None)? {
                When::Date(d) => Until::Date(d),
                When::Local(dt, _) => Until::Local(dt),
                When::Utc(ts) => Until::Utc(ts),
            })
        })
    });
    let rdates = c
        .all("RDATE")
        .flat_map(|p| p.value.split(',').map(str::to_string).collect::<Vec<_>>())
        .filter_map(|v| match when_from(v.trim(), None, None)? {
            When::Local(dt, _) => Some(dt),
            _ => None,
        })
        .collect();
    Some(Observance {
        start,
        from,
        to,
        rule,
        rdates,
    })
}

// The offset in force at a wall-clock time by a VTIMEZONE's own rules: that
// of the observance whose last onset before it is the latest. Onsets are
// compared on the wall clock, which is out by the change itself only in the
// hour a clock goes back or forward.
fn offset_at(rules: &[Observance], at: DateTime) -> i32 {
    let mut best: Option<(DateTime, i32)> = None;
    for obs in rules {
        let mut last = None;
        if let Some(rule) = &obs.rule {
            rule.walk(
                obs.start,
                |onset| match rule.until {
                    Some(Until::Local(u)) => onset > u,
                    Some(Until::Utc(u)) => Offset::from_seconds(obs.from)
                        .ok()
                        .and_then(|o| o.to_timestamp(onset).ok())
                        .is_some_and(|ts| ts > u),
                    Some(Until::Date(d)) => onset.date() > d,
                    None => false,
                },
                |onset| {
                    if onset > at {
                        return false;
                    }
                    last = Some(onset);
                    true
                },
            );
        } else if obs.start <= at {
            last = Some(obs.start);
        }
        for r in &obs.rdates {
            if *r <= at && last.is_none_or(|l| *r > l) {
                last = Some(*r);
            }
        }
        if let Some(onset) = last
            && best.is_none_or(|(b, _)| onset > b)
        {
            best = Some((onset, obs.to));
        }
    }
    best.map(|(_, to)| to).unwrap_or_else(|| {
        rules
            .iter()
            .min_by_key(|o| o.start)
            .map(|o| o.from)
            .unwrap_or(0)
    })
}

// ── Durations ───────────────────────────────────────────────────────────────

/// How long an event lasts: whole days for an all-day one, else a time.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Length {
    Days(i64),
    Time(SignedDuration),
}

// An ISO 8601 duration as iCal writes it: `PT1H30M`, `P1D`, `P2W`, `-PT15M`.
fn duration(text: &str) -> Option<Length> {
    let text = text.trim();
    let (sign, rest) = match text.strip_prefix('-') {
        Some(r) => (-1, r),
        None => (1, text.strip_prefix('+').unwrap_or(text)),
    };
    let rest = rest.strip_prefix(['P', 'p'])?;
    let (mut days, mut secs) = (0i64, 0i64);
    let mut in_time = false;
    let mut number = String::new();
    for c in rest.chars() {
        match c.to_ascii_uppercase() {
            'T' => in_time = true,
            d if d.is_ascii_digit() => number.push(d),
            unit => {
                let n: i64 = number.parse().ok()?;
                number.clear();
                match (unit, in_time) {
                    ('W', false) => days += n * 7,
                    ('D', false) => days += n,
                    ('H', true) => secs += n * 3600,
                    ('M', true) => secs += n * 60,
                    ('S', true) => secs += n,
                    _ => return None,
                }
            }
        }
    }
    Some(if secs == 0 {
        Length::Days(sign * days)
    } else {
        Length::Time(SignedDuration::from_secs(sign * (days * 86_400 + secs)))
    })
}

// ── Today's events ──────────────────────────────────────────────────────────

static CALL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"https://[\w.-]*(zoom\.us/j|meet\.google\.com|teams\.microsoft\.com/l/meetup-join)/[^\s"<>\\]+"#)
        .unwrap()
});

/// At most this many tickets and pull requests are taken from an event.
pub const MAX_REFS: usize = 4;

static LINEAR_LINK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"https://linear\.app/[\w-]+/issue/([A-Za-z][A-Za-z0-9]*-\d+)[^\s"<>]*"#).unwrap()
});
static GITHUB_LINK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"https://github\.com/([\w.-]+)/([\w.-]+)/pull/(\d+)").unwrap());
static BARE_ID: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[A-Z][A-Z0-9]{1,9}-\d{1,6}").unwrap());
static TAG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<[^>]+>").unwrap());
static URL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"https?://\S+").unwrap());

/// The tickets and pull requests an event's description or location names,
/// for the next meeting's notification and the Calendar view's menu: Linear
/// and GitHub links, each with where it goes, then ticket IDs written bare
/// (ABC-123), which have no link, at most `MAX_REFS`.
pub fn refs_from(text: &str) -> Vec<Ref> {
    // Links are found in the raw text, an HTML link's address being inside
    // its tag; bare IDs in the words alone.
    let plain = URL
        .replace_all(&TAG.replace_all(text, " "), " ")
        .into_owned();
    let mut refs: Vec<Ref> = Vec::new();
    let mut add = |r: Ref| {
        if refs.len() < MAX_REFS && !refs.iter().any(|x| x.id == r.id) {
            refs.push(r);
        }
    };
    for m in LINEAR_LINK.captures_iter(text) {
        add(Ref {
            id: m[1].to_uppercase(),
            url: Some(m[0].to_string()),
        });
    }
    for m in GITHUB_LINK.captures_iter(text) {
        add(Ref {
            id: format!("{}#{}", &m[2], &m[3]),
            url: Some(m[0].to_string()),
        });
    }
    let word = |c: char| c.is_alphanumeric() || c == '_';
    for m in BARE_ID.find_iter(&plain) {
        let before = plain[..m.start()].chars().next_back();
        let after = plain[m.end()..].chars().next();
        if before.is_some_and(|c| word(c) || c == '/' || c == '-')
            || after.is_some_and(|c| word(c) || c == '-')
        {
            continue;
        }
        let id = m.as_str();
        if ["UTF-", "ISO-", "SHA-"].iter().any(|p| id.starts_with(p)) {
            continue;
        }
        add(Ref {
            id: id.to_string(),
            url: None,
        });
    }
    refs
}

/// An instant as JavaScript's `toISOString` writes it:
/// `2026-10-28T17:00:00.000Z`.
pub fn iso(ts: Timestamp) -> String {
    ts.strftime("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()
}

/// The event's ID in Google's API, which RSVP answers by
/// (`respond_to_event`): the UID before `@google.com`, with `_` and the
/// occurrence's UTC start (its date for an all-day one) for a repeating
/// event. `None` for an event Google didn't make.
pub fn id_of(uid: &str, start: Timestamp, day: Option<Date>, recurring: bool) -> Option<String> {
    let base = uid.strip_suffix("@google.com")?;
    // A series split in two ("this and following") has _R and the split's
    // time after its ID in the feed, which Google's links leave out.
    static SPLIT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"_R\d{8}(T\d{6}Z?)?$").unwrap());
    let base = SPLIT.replace(base, "");
    if !recurring {
        return Some(base.into_owned());
    }
    let stamp = match day {
        Some(d) => d.strftime("%Y%m%d").to_string(),
        None => start.strftime("%Y%m%dT%H%M%SZ").to_string(),
    };
    Some(format!("{base}_{stamp}"))
}

/// Google's link to an event, or one occurrence of a repeating one: the
/// base64 of its ID and the calendar's address.
pub fn link_of(
    uid: &str,
    owner: Option<&str>,
    start: Timestamp,
    day: Option<Date>,
    recurring: bool,
) -> Option<String> {
    let owner = owner?;
    let id = id_of(uid, start, day, recurring)?;
    Some(format!(
        "https://www.google.com/calendar/event?eid={}",
        util::base64(format!("{id} {owner}").as_bytes()).trim_end_matches('=')
    ))
}

fn you_of(event: &Component, owner: Option<&str>) -> &'static str {
    let me = owner.and_then(|o| {
        let mailto = format!("mailto:{}", o.to_lowercase());
        event
            .all("ATTENDEE")
            .find(|a| a.value.trim().to_lowercase() == mailto)
    });
    let Some(me) = me else {
        return "yours";
    };
    match me
        .param("PARTSTAT")
        .unwrap_or("")
        .to_ascii_uppercase()
        .as_str()
    {
        "ACCEPTED" => "going",
        "TENTATIVE" => "maybe",
        "DECLINED" => "declined",
        _ => "not answered",
    }
}

// An event's start, end and length, read once.
struct Timing {
    start: When,
    length: Length,
}

fn timing(event: &Component, zones: &Zones) -> Option<Timing> {
    let start = when_of(event.prop("DTSTART")?)?;
    let length = if let Some(end) = event.prop("DTEND").and_then(when_of) {
        match (&start, &end) {
            (When::Date(a), When::Date(b)) => Length::Days(b.since(*a).ok()?.get_days() as i64),
            _ => Length::Time(zones.resolve(&end)?.duration_since(zones.resolve(&start)?)),
        }
    } else if let Some(d) = event.prop("DURATION").and_then(|p| duration(&p.value)) {
        d
    } else if start.is_date() {
        Length::Days(1)
    } else {
        Length::Time(SignedDuration::ZERO)
    };
    Some(Timing { start, length })
}

// An occurrence's start and end as instants, and its date when all-day.
fn span_of(
    start: &When,
    length: Length,
    zones: &Zones,
) -> Option<(Timestamp, Timestamp, Option<Date>)> {
    let from = zones.resolve(start)?;
    let (to, day) = match (start, length) {
        (When::Date(d), Length::Days(n)) => (
            zones.resolve(&When::Date(d.checked_add(jiff::Span::new().days(n)).ok()?))?,
            Some(*d),
        ),
        (When::Date(d), Length::Time(t)) => (from.checked_add(t).ok()?, Some(*d)),
        (_, Length::Days(n)) => (
            from.checked_add(SignedDuration::from_hours(24 * n)).ok()?,
            None,
        ),
        (_, Length::Time(t)) => (from.checked_add(t).ok()?, None),
    };
    Some((from, to, day))
}

// Adds an occurrence: the event, its start and end, its date when all-day,
// whether it repeats, and when it was meant to start.
type Add<'a> = dyn FnMut(&Component, Timestamp, Timestamp, Option<Date>, bool, Timestamp) + 'a;

/// Today's events from the feed's text, in /today's shape, for the day
/// `now` falls on in its own time zone.
pub fn day_from(text: &str, now: &Zoned) -> SavedDay {
    let root = parse(text);
    let calendar = if root.name == "VCALENDAR" {
        &root
    } else {
        root.children
            .iter()
            .find(|c| c.name == "VCALENDAR")
            .unwrap_or(&root)
    };
    let zones = Zones::of(calendar, now.time_zone().clone());
    let owner = calendar
        .text("X-WR-CALNAME")
        .filter(|name| name.contains('@'));
    let today = now.date();
    let day_start = zones
        .resolve(&When::Date(today))
        .unwrap_or_else(|| now.timestamp());
    let day_end = today
        .tomorrow()
        .ok()
        .and_then(|d| zones.resolve(&When::Date(d)))
        .unwrap_or_else(|| day_start + SignedDuration::from_hours(24));

    let vevents: Vec<&Component> = calendar
        .children
        .iter()
        .filter(|c| c.name == "VEVENT")
        .collect();
    // The series (and one-off events) by UID, a later one of the same UID
    // replacing an earlier as the feed's Map did; then each moved or changed
    // occurrence under its series, or on its own when the feed has no series
    // for it.
    let mut order: Vec<String> = Vec::new();
    let mut masters: HashMap<String, &Component> = HashMap::new();
    for v in vevents.iter().filter(|v| !v.has("RECURRENCE-ID")) {
        let uid = v.text("UID").unwrap_or_default();
        if !masters.contains_key(&uid) {
            order.push(uid.clone());
        }
        masters.insert(uid, v);
    }
    let mut exceptions: HashMap<String, Vec<&Component>> = HashMap::new();
    let mut lone: Vec<&Component> = Vec::new();
    for v in vevents.iter().filter(|v| v.has("RECURRENCE-ID")) {
        let uid = v.text("UID").unwrap_or_default();
        if masters.contains_key(&uid) {
            exceptions.entry(uid).or_default().push(v);
        } else {
            lone.push(v);
        }
    }

    let mut events = Vec::new();
    let mut add = |event: &Component,
                   from: Timestamp,
                   to: Timestamp,
                   day: Option<Date>,
                   recurring: bool,
                   original: Timestamp| {
        if event
            .text("STATUS")
            .is_some_and(|s| s.eq_ignore_ascii_case("CANCELLED"))
        {
            return;
        }
        let you = you_of(event, owner.as_deref());
        if you == "declined" || !(from < day_end && to > day_start) {
            return;
        }
        let location = event.text("LOCATION").unwrap_or_default();
        let description = event.text("DESCRIPTION").unwrap_or_default();
        let call = event
            .prop("X-GOOGLE-CONFERENCE")
            .map(|p| p.value.trim().to_string())
            .filter(|c| !c.is_empty())
            .or_else(|| {
                CALL.find(&format!("{location} {description}"))
                    .map(|m| m.as_str().to_string())
            })
            .filter(|c| c.starts_with("https://"));
        let uid = event.text("UID").unwrap_or_default();
        let original_day = if recurring {
            day.map(|_| zones.local.to_datetime(original).date())
        } else {
            day
        };
        let id = owner
            .as_ref()
            .and_then(|_| id_of(&uid, original, original_day, recurring));
        events.push(CalEvent {
            title: event
                .text("SUMMARY")
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "(No title)".into()),
            all_day: day.is_some(),
            start: day.is_none().then(|| iso(from)),
            end: day.is_none().then(|| iso(to)),
            you: you.to_string(),
            join: call,
            link: link_of(&uid, owner.as_deref(), original, original_day, recurring),
            calendar: id.as_ref().and(owner.clone()),
            id,
            // The event as the feed knows it (its UID, and for a repeating
            // one which occurrence, by when it was meant to start, so moving
            // it keeps it), whether or not there is a Google ID: what
            // calendar changes follow it by.
            key: Some(if recurring {
                format!("{uid}@{}", iso(original))
            } else {
                uid.clone()
            }),
            refs: refs_from(&format!("{description} {location}")),
        });
    };

    let single = |event: &Component, add: &mut Add| {
        let Some(t) = timing(event, &zones) else {
            return;
        };
        let Some((from, to, day)) = span_of(&t.start, t.length, &zones) else {
            return;
        };
        let rid = event
            .prop("RECURRENCE-ID")
            .and_then(when_of)
            .and_then(|w| zones.resolve(&w));
        add(event, from, to, day, rid.is_some(), rid.unwrap_or(from));
    };

    for uid in &order {
        let master = masters[uid];
        let Some(t) = timing(master, &zones) else {
            continue;
        };
        let (civil, tzid): (DateTime, Option<String>) = match &t.start {
            When::Date(d) => (d.to_datetime(jiff::civil::Time::midnight()), None),
            When::Local(dt, tz) => (*dt, tz.clone()),
            When::Utc(ts) => (ts.to_zoned(TimeZone::UTC).datetime(), Some("UTC".into())),
        };
        let back = |dt: DateTime| -> When {
            match &t.start {
                When::Date(_) => When::Date(dt.date()),
                When::Local(_, tz) => When::Local(dt, tz.clone()),
                When::Utc(_) => When::Utc(
                    dt.to_zoned(TimeZone::UTC)
                        .map(|z| z.timestamp())
                        .unwrap_or(Timestamp::UNIX_EPOCH),
                ),
            }
        };
        let rule = master.prop("RRULE").and_then(|p| {
            Rule::parse(&p.value, |v| {
                Some(match when_from(v, None, None)? {
                    When::Date(d) => Until::Date(d),
                    When::Local(dt, _) => Until::Local(dt),
                    When::Utc(ts) => Until::Utc(ts),
                })
            })
        });
        let rdates: Vec<When> = master
            .all("RDATE")
            .flat_map(|p| {
                p.value
                    .split(',')
                    .filter_map(|v| {
                        when_from(
                            v.trim(),
                            p.param("TZID").or(tzid.as_deref()),
                            p.param("VALUE"),
                        )
                    })
                    .collect::<Vec<_>>()
            })
            .collect();
        if rule.is_none() && rdates.is_empty() {
            single(master, &mut add);
            continue;
        }
        let excluded: Vec<Timestamp> = master
            .all("EXDATE")
            .flat_map(|p| {
                p.value
                    .split(',')
                    .filter_map(|v| {
                        when_from(
                            v.trim(),
                            p.param("TZID").or(tzid.as_deref()),
                            p.param("VALUE"),
                        )
                    })
                    .collect::<Vec<_>>()
            })
            .filter_map(|w| zones.resolve(&w))
            .collect();
        // Moved or changed occurrences, by when they were meant to start.
        let mut moved: HashMap<i64, &Component> = HashMap::new();
        for e in exceptions.get(uid).into_iter().flatten() {
            if let Some(rid) = e
                .prop("RECURRENCE-ID")
                .and_then(when_of)
                .and_then(|w| zones.resolve(&w))
            {
                moved.insert(rid.as_millisecond(), e);
            }
        }
        // The occurrences up to the end of today, the rule's and RDATE's.
        let mut starts: Vec<When> = Vec::new();
        let until_tz = tzid.clone();
        if let Some(rule) = &rule {
            rule.walk(
                civil,
                |at| match rule.until {
                    Some(Until::Date(d)) => at.date() > d,
                    Some(Until::Local(u)) => at > u,
                    Some(Until::Utc(u)) => zones
                        .instant(at, until_tz.as_deref())
                        .is_some_and(|ts| ts > u),
                    None => false,
                },
                |at| {
                    let when = back(at);
                    match zones.resolve(&when) {
                        Some(ts) if ts >= day_end => false,
                        _ => {
                            starts.push(when);
                            true
                        }
                    }
                },
            );
        } else {
            starts.push(t.start.clone());
        }
        starts.extend(
            rdates
                .into_iter()
                .filter(|w| zones.resolve(w).is_some_and(|ts| ts < day_end)),
        );
        let mut done: Vec<i64> = Vec::new();
        for when in starts {
            let Some(at) = zones.resolve(&when) else {
                continue;
            };
            if excluded.contains(&at) || done.contains(&at.as_millisecond()) {
                continue;
            }
            done.push(at.as_millisecond());
            if let Some(e) = moved.get(&at.as_millisecond()) {
                let length = timing(e, &zones).map(|t| t.length).unwrap_or(t.length);
                let start = e.prop("DTSTART").and_then(when_of).unwrap_or(when.clone());
                if let Some((from, to, day)) = span_of(&start, length, &zones) {
                    add(e, from, to, day, true, at);
                }
                continue;
            }
            let Some((from, to, day)) = span_of(&when, t.length, &zones) else {
                continue;
            };
            if to > day_start {
                add(master, from, to, day, true, at);
            }
        }
        // An occurrence of a later day moved into today: the walk above
        // stopped before reaching when it was meant to be.
        for (rid, e) in &moved {
            if done.contains(rid) {
                continue;
            }
            let Ok(original) = Timestamp::from_millisecond(*rid) else {
                continue;
            };
            if original < day_end || excluded.contains(&original) {
                continue;
            }
            let length = timing(e, &zones).map(|t| t.length).unwrap_or(t.length);
            if let Some(start) = e.prop("DTSTART").and_then(when_of)
                && let Some((from, to, day)) = span_of(&start, length, &zones)
            {
                add(e, from, to, day, true, original);
            }
        }
    }
    for e in lone {
        single(e, &mut add);
    }
    SavedDay {
        date: util::ymd(now),
        updated: util::now_ms(),
        source: Some("ical".into()),
        events,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tz() -> TimeZone {
        TimeZone::get("America/New_York").expect("the system knows New York")
    }

    fn noon() -> Zoned {
        jiff::civil::date(2026, 10, 28)
            .at(12, 0, 0, 0)
            .to_zoned(tz())
            .unwrap()
    }

    fn b64(text: &str) -> String {
        // The link's eid, decoded back to "<id> <owner>".
        let table = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut bits = 0u32;
        let mut n = 0;
        let mut out = Vec::new();
        for c in text.bytes() {
            let v = table.iter().position(|t| *t == c).unwrap() as u32;
            bits = bits << 6 | v;
            n += 6;
            if n >= 8 {
                n -= 8;
                out.push((bits >> n & 0xff) as u8);
            }
        }
        String::from_utf8(out).unwrap()
    }

    // The feed the JavaScript's test read, from test/tmux-sidebar.test.js.
    const FEED: &[&str] = &[
        "BEGIN:VCALENDAR",
        "VERSION:2.0",
        "X-WR-CALNAME:me@example.com",
        "BEGIN:VTIMEZONE",
        "TZID:America/New_York",
        "BEGIN:DAYLIGHT",
        "TZOFFSETFROM:-0500",
        "TZOFFSETTO:-0400",
        "TZNAME:EDT",
        "DTSTART:19700308T020000",
        "RRULE:FREQ=YEARLY;BYMONTH=3;BYDAY=2SU",
        "END:DAYLIGHT",
        "BEGIN:STANDARD",
        "TZOFFSETFROM:-0400",
        "TZOFFSETTO:-0500",
        "TZNAME:EST",
        "DTSTART:19701101T020000",
        "RRULE:FREQ=YEARLY;BYMONTH=11;BYDAY=1SU",
        "END:STANDARD",
        "END:VTIMEZONE",
        // Every weekday at 1 PM, since September, the call in Google's own field.
        "BEGIN:VEVENT",
        "UID:standup@google.com",
        "SUMMARY:Standup",
        "DTSTART;TZID=America/New_York:20260901T130000",
        "DTEND;TZID=America/New_York:20260901T131500",
        "RRULE:FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR",
        "X-GOOGLE-CONFERENCE:https://meet.google.com/abc-defg-hij",
        "ATTENDEE;PARTSTAT=ACCEPTED:mailto:me@example.com",
        "END:VEVENT",
        // Every Wednesday at 3 PM, but today's moved to 4 PM, a Zoom link in its location.
        "BEGIN:VEVENT",
        "UID:review@google.com",
        "SUMMARY:Review",
        "DTSTART;TZID=America/New_York:20261007T150000",
        "DTEND;TZID=America/New_York:20261007T160000",
        "RRULE:FREQ=WEEKLY;BYDAY=WE",
        "ATTENDEE;PARTSTAT=NEEDS-ACTION:mailto:me@example.com",
        "END:VEVENT",
        "BEGIN:VEVENT",
        "UID:review@google.com",
        "RECURRENCE-ID;TZID=America/New_York:20261028T150000",
        "SUMMARY:Review (moved)",
        "DTSTART;TZID=America/New_York:20261028T160000",
        "DTEND;TZID=America/New_York:20261028T170000",
        "LOCATION:https://acme.zoom.us/j/123?pwd=x",
        "ATTENDEE;PARTSTAT=TENTATIVE:mailto:me@example.com",
        "END:VEVENT",
        "BEGIN:VEVENT",
        "UID:nope@google.com",
        "SUMMARY:Declined",
        "DTSTART:20261028T170000Z",
        "DTEND:20261028T180000Z",
        "ATTENDEE;PARTSTAT=DECLINED:mailto:me@example.com",
        "END:VEVENT",
        "BEGIN:VEVENT",
        "UID:ooo@google.com",
        "SUMMARY:Holiday",
        "DTSTART;VALUE=DATE:20261028",
        "DTEND;VALUE=DATE:20261029",
        "END:VEVENT",
        "BEGIN:VEVENT",
        "UID:old@google.com",
        "SUMMARY:Yesterday",
        "DTSTART:20261027T170000Z",
        "DTEND:20261027T180000Z",
        "END:VEVENT",
        "END:VCALENDAR",
    ];

    fn by_title(day: &SavedDay) -> HashMap<String, CalEvent> {
        day.events
            .iter()
            .map(|e| (e.title.clone(), e.clone()))
            .collect()
    }

    #[test]
    fn a_feed_gives_todays_events_repeats_and_moved_ones_included() {
        let day = day_from(&FEED.join("\r\n"), &noon());
        assert_eq!(day.date, "2026-10-28");
        assert_eq!(day.source.as_deref(), Some("ical"));
        let events = by_title(&day);
        let mut titles: Vec<&String> = events.keys().collect();
        titles.sort();
        assert_eq!(titles, ["Holiday", "Review (moved)", "Standup"]);
        let standup = &events["Standup"];
        assert_eq!(standup.start.as_deref(), Some("2026-10-28T17:00:00.000Z"));
        assert_eq!(standup.you, "going");
        assert_eq!(
            standup.join.as_deref(),
            Some("https://meet.google.com/abc-defg-hij")
        );
        let eid = standup
            .link
            .as_deref()
            .unwrap()
            .split("eid=")
            .nth(1)
            .unwrap();
        assert_eq!(b64(eid), "standup_20261028T170000Z me@example.com");
        let moved = &events["Review (moved)"];
        assert_eq!(moved.start.as_deref(), Some("2026-10-28T20:00:00.000Z"));
        assert_eq!(moved.you, "maybe");
        assert_eq!(
            moved.join.as_deref(),
            Some("https://acme.zoom.us/j/123?pwd=x")
        );
        assert!(events["Holiday"].all_day);
        assert_eq!(events["Holiday"].you, "yours");
        // Google's own ID for each, which RSVP answers by: a moved occurrence
        // by when it was meant to start.
        assert_eq!(standup.id.as_deref(), Some("standup_20261028T170000Z"));
        assert_eq!(moved.id.as_deref(), Some("review_20261028T190000Z"));
        assert_eq!(moved.calendar.as_deref(), Some("me@example.com"));
        let eid = moved.link.as_deref().unwrap().split("eid=").nth(1).unwrap();
        assert_eq!(b64(eid), "review_20261028T190000Z me@example.com");
    }

    #[test]
    fn an_excluded_day_and_a_cancelled_occurrence_are_left_out() {
        let feed = [
            "BEGIN:VCALENDAR",
            "X-WR-CALNAME:me@example.com",
            "BEGIN:VEVENT",
            "UID:daily@google.com",
            "SUMMARY:Daily",
            "DTSTART;TZID=America/New_York:20261001T090000",
            "DTEND;TZID=America/New_York:20261001T091500",
            "RRULE:FREQ=DAILY",
            "EXDATE;TZID=America/New_York:20261028T090000",
            "END:VEVENT",
            "BEGIN:VEVENT",
            "UID:sync@google.com",
            "SUMMARY:Sync",
            "DTSTART;TZID=America/New_York:20261001T100000",
            "DTEND;TZID=America/New_York:20261001T103000",
            "RRULE:FREQ=DAILY;COUNT=200",
            "END:VEVENT",
            "BEGIN:VEVENT",
            "UID:sync@google.com",
            "RECURRENCE-ID;TZID=America/New_York:20261028T100000",
            "SUMMARY:Sync",
            "STATUS:CANCELLED",
            "DTSTART;TZID=America/New_York:20261028T100000",
            "DTEND;TZID=America/New_York:20261028T103000",
            "END:VEVENT",
            "BEGIN:VEVENT",
            "UID:short@google.com",
            "SUMMARY:Short",
            "DTSTART:20261020T140000Z",
            "DTEND:20261020T150000Z",
            "RRULE:FREQ=DAILY;UNTIL=20261025T000000Z",
            "END:VEVENT",
            "END:VCALENDAR",
        ];
        let day = day_from(&feed.join("\n"), &noon());
        assert!(day.events.is_empty(), "{:?}", day.events);
    }

    #[test]
    fn a_time_zone_the_system_doesnt_name_is_read_from_the_feed() {
        // Outlook's own name for New York, which only the feed's VTIMEZONE
        // explains; daylight saving time still on in late October.
        let feed = [
            "BEGIN:VCALENDAR",
            "BEGIN:VTIMEZONE",
            "TZID:Eastern Standard Time",
            "BEGIN:STANDARD",
            "DTSTART:16010101T020000",
            "TZOFFSETFROM:-0400",
            "TZOFFSETTO:-0500",
            "RRULE:FREQ=YEARLY;BYDAY=1SU;BYMONTH=11",
            "END:STANDARD",
            "BEGIN:DAYLIGHT",
            "DTSTART:16010101T020000",
            "TZOFFSETFROM:-0500",
            "TZOFFSETTO:-0400",
            "RRULE:FREQ=YEARLY;BYDAY=2SU;BYMONTH=3",
            "END:DAYLIGHT",
            "END:VTIMEZONE",
            "BEGIN:VEVENT",
            "UID:abc123",
            "SUMMARY:Vendor call",
            "DTSTART;TZID=Eastern Standard Time:20261028T110000",
            "DTEND;TZID=Eastern Standard Time:20261028T120000",
            "DESCRIPTION:Join: https://teams.microsoft.com/l/meetup-join/19%3a\\, about AB-12",
            "END:VEVENT",
            "END:VCALENDAR",
        ];
        let day = day_from(&feed.join("\r\n"), &noon());
        assert_eq!(day.events.len(), 1);
        let e = &day.events[0];
        assert_eq!(e.start.as_deref(), Some("2026-10-28T15:00:00.000Z"));
        assert_eq!(
            e.join.as_deref(),
            Some("https://teams.microsoft.com/l/meetup-join/19%3a,")
        );
        assert_eq!(e.id, None, "no Google ID for an event Google didn't make");
        assert_eq!(e.key.as_deref(), Some("abc123"));
        assert_eq!(
            e.refs,
            [Ref {
                id: "AB-12".into(),
                url: None
            }]
        );
    }

    #[test]
    fn an_all_day_event_across_days_and_a_folded_line() {
        let feed = [
            "BEGIN:VCALENDAR",
            "BEGIN:VEVENT",
            "UID:trip@google.com",
            "SUMMARY:Off",
            " site",
            "DTSTART;VALUE=DATE:20261027",
            "DTEND;VALUE=DATE:20261030",
            "END:VEVENT",
            "BEGIN:VEVENT",
            "UID:weekly@google.com",
            "SUMMARY:Focus day",
            "DTSTART;VALUE=DATE:20261007",
            "RRULE:FREQ=WEEKLY",
            "END:VEVENT",
            "END:VCALENDAR",
        ];
        let day = day_from(&feed.join("\r\n"), &noon());
        let titles: Vec<&str> = day.events.iter().map(|e| e.title.as_str()).collect();
        assert_eq!(titles, ["Offsite", "Focus day"]);
        assert!(day.events.iter().all(|e| e.all_day && e.start.is_none()));
        assert_eq!(
            day.events[1].key.as_deref(),
            Some("weekly@google.com@2026-10-28T04:00:00.000Z")
        );
    }

    #[test]
    fn a_meetings_description_gives_its_tickets_and_pull_requests_links_first() {
        let refs = refs_from(
            "Agenda: <a href=\"https://linear.app/acme/issue/AB-12/fix-it\">AB-12</a>, https://github.com/o/repo/pull/45 and CD-7; UTF-8 isn't one",
        );
        assert_eq!(
            refs,
            [
                Ref {
                    id: "AB-12".into(),
                    url: Some("https://linear.app/acme/issue/AB-12/fix-it".into())
                },
                Ref {
                    id: "repo#45".into(),
                    url: Some("https://github.com/o/repo/pull/45".into())
                },
                Ref {
                    id: "CD-7".into(),
                    url: None
                },
            ]
        );
    }

    #[test]
    fn durations_read_as_ical_writes_them() {
        assert_eq!(
            duration("PT1H30M"),
            Some(Length::Time(SignedDuration::from_secs(5400)))
        );
        assert_eq!(duration("P2D"), Some(Length::Days(2)));
        assert_eq!(duration("P1W"), Some(Length::Days(7)));
        assert_eq!(duration("nonsense"), None);
    }
}
