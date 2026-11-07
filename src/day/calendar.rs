//! Mission control's Calendar view: today, read straight from your
//! calendar's iCal feed when its secret address is stored (the secret
//! `google-calendar-ical`), else from the day /today last saved
//! (`calendar.json` in the state directory, which `switchboard today-save`
//! writes; the Calendar tab runs /today every 30 minutes). Ported from
//! lib/sidebar/calendar.js and calendar-feed.js; drawing the day is
//! mission control's, which takes the view's items from here.
//!
//! The feed is read every `FEED_POLL_MS` in the background while you're
//! here, and when you go to the view; what is shown is only ever the last
//! answer, in the shape /today saves, so the view takes either. A day saved
//! before today, or more than `STALE_MS` ago, is out of date, and the view
//! says so rather than passing it off as today's; the server then opens a
//! Calendar tab in the background unless one already keeps it, at most once
//! every `REOPEN_MS`.
//!
//! The feed is lost once its looks have failed one after another for
//! `FEED_DOWN_MS`, three at least, and stays lost until one succeeds: the
//! view then falls back to /today's day. Only looks count, a run of them
//! broken by a gap of more than two polls starting over, so time away (no
//! looks while lookups pause) never makes a feed lost by itself.
//!
//! An invitation is answered (RSVP) through Claude's Google Calendar
//! connector, as Google's own buttons answer it, with the calendar_rsvp
//! setting on; only an event from the feed, which has its Google ID.

use std::path::{Path, PathBuf};
use std::time::Duration;

use jiff::tz::TimeZone;
use jiff::{Timestamp, Zoned};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::items::{Item, State, Target, ViewData};
use crate::{connector, net, platform, settings, util};

pub const FEED_POLL_MS: i64 = 5 * 60_000;
pub const FEED_DOWN_MS: i64 = 15 * 60_000;
pub const STALE_MS: i64 = 45 * 60_000;
pub const REOPEN_MS: i64 = 10 * 60_000;
/// The secret the feed's address is stored under.
pub const SECRET: &str = "google-calendar-ical";
/// The next meeting's notification shows from this long before it starts
/// until this long after.
pub const MEETING_BEFORE_MS: i64 = 5 * 60_000;
pub const MEETING_AFTER_MS: i64 = 10 * 60_000;

/// What the view says when neither the feed nor /today is set up, from
/// lib/sidebar/setup-help.js.
pub const SETUP: &str = "The calendar isn't set up. Either: (1) give mission control your Google Calendar's secret address: in Google Calendar, open Settings, your calendar, Integrate calendar, copy \"Secret address in iCal format\", then run switchboard platform secret-set google-calendar-ical in a terminal, paste it and press Ctrl-D; mission control reads it every five minutes. Or (2) connect Google Calendar in Claude (/mcp in Claude Code) and open a Calendar tab (M-p, \"Open a Calendar tab\"), which runs /today every 30 minutes.";

/// A ticket or pull request an event names, with its link when it has one.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ref {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

/// One event, as /today saves it and the feed gives it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CalEvent {
    pub title: String,
    #[serde(default)]
    pub all_day: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<String>,
    /// "going", "yours", "maybe" or "not answered".
    #[serde(default = "not_answered")]
    pub you: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub join: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<String>,
    /// Google's ID for it, which RSVP answers by; the feed's alone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// The calendar it is on (your address), for RSVP.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calendar: Option<String>,
    /// The feed's own key for it (UID, and the occurrence).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub refs: Vec<Ref>,
}

fn not_answered() -> String {
    "not answered".into()
}

/// A time as JavaScript's `Date.parse` takes the ones these files hold: an
/// instant with its offset, else a time or a date read as local.
pub fn parse_instant(text: &str, tz: &TimeZone) -> Option<Timestamp> {
    let text = text.trim();
    if let Ok(ts) = text.parse::<Timestamp>() {
        return Some(ts);
    }
    if let Ok(dt) = text.parse::<jiff::civil::DateTime>() {
        return tz.to_ambiguous_timestamp(dt).compatible().ok();
    }
    let d = text.parse::<jiff::civil::Date>().ok()?;
    tz.to_ambiguous_timestamp(d.to_datetime(jiff::civil::Time::midnight()))
        .compatible()
        .ok()
}

impl CalEvent {
    pub fn starts(&self, tz: &TimeZone) -> Option<Timestamp> {
        parse_instant(self.start.as_deref()?, tz)
    }

    pub fn ends(&self, tz: &TimeZone) -> Option<Timestamp> {
        parse_instant(self.end.as_deref()?, tz)
    }

    /// Whether you said yes, or it is your own: drawn solid rather than in
    /// outline.
    pub fn going(&self) -> bool {
        self.you == "going" || self.you == "yours"
    }
}

/// A day of events: what /today saves and the feed gives (`source` "ical").
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SavedDay {
    pub date: String,
    #[serde(default)]
    pub updated: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default)]
    pub events: Vec<CalEvent>,
}

impl SavedDay {
    /// A day from its JSON, an event that doesn't read (no title) left out.
    pub fn from_json(value: &Value) -> Option<SavedDay> {
        let date = value.get("date")?.as_str()?.to_string();
        let updated = value.get("updated").and_then(Value::as_f64).unwrap_or(0.0) as i64;
        let source = value
            .get("source")
            .and_then(Value::as_str)
            .map(str::to_string);
        let events = value
            .get("events")
            .and_then(Value::as_array)
            .map(|list| {
                list.iter()
                    .filter_map(|e| serde_json::from_value(e.clone()).ok())
                    .collect()
            })
            .unwrap_or_default();
        Some(SavedDay {
            date,
            updated,
            source,
            events,
        })
    }

    pub fn from_feed(&self) -> bool {
        self.source.as_deref() == Some("ical")
    }
}

/// `calendar.json` in the state directory.
pub fn saved_file(state: &Path) -> PathBuf {
    state.join("calendar.json")
}

/// The day /today last saved, read again only when the file's modified time
/// changes.
#[derive(Default)]
pub struct Saved {
    mtime: Option<std::time::SystemTime>,
    day: Option<SavedDay>,
}

impl Saved {
    pub fn read(&mut self, state: &Path) -> Option<&SavedDay> {
        let file = saved_file(state);
        let mtime = std::fs::metadata(&file).and_then(|m| m.modified()).ok();
        if mtime.is_none() {
            self.mtime = None;
            self.day = None;
        } else if mtime != self.mtime {
            self.mtime = mtime;
            self.day = util::read_json(&file).and_then(|v| SavedDay::from_json(&v));
        }
        self.day.as_ref()
    }
}

/// What has come of reading the feed.
#[derive(Clone, Debug, Default)]
pub struct Feed {
    pub day: Option<SavedDay>,
    pub error: Option<String>,
    /// Whether there is an address; `None` before the first look.
    pub configured: Option<bool>,
    pub fetched_at: i64,
    failing_since: Option<i64>,
    failures: u32,
    failed_at: i64,
    lost: bool,
}

/// How a look at the feed went.
#[derive(Clone, Debug)]
pub enum Fetched {
    /// No address stored.
    NoAddress,
    Day(SavedDay),
    Failed(String),
}

impl Feed {
    pub fn configured(&self) -> bool {
        self.configured == Some(true)
    }

    /// Lost: failing for `FEED_DOWN_MS`, and not read since.
    pub fn down(&self) -> bool {
        self.configured() && self.lost
    }

    /// A look that failed, at `now`.
    pub fn failed(&mut self, why: String, now: i64) {
        self.error = Some(why);
        if !self.lost && self.failed_at != 0 && now - self.failed_at > 2 * FEED_POLL_MS {
            self.failing_since = None;
            self.failures = 0;
        }
        let since = *self.failing_since.get_or_insert(now);
        self.failures += 1;
        self.failed_at = now;
        if self.failures >= 3 && now - since >= FEED_DOWN_MS {
            self.lost = true;
        }
    }

    /// Takes a look's answer.
    pub fn take(&mut self, fetched: Fetched, now: i64) {
        match fetched {
            Fetched::NoAddress => self.configured = Some(false),
            Fetched::Failed(why) => {
                self.configured = Some(true);
                self.failed(why, now);
            }
            Fetched::Day(day) => {
                self.configured = Some(true);
                self.day = Some(day);
                self.error = None;
                self.failing_since = None;
                self.failures = 0;
                self.lost = false;
                self.fetched_at = now;
            }
        }
    }
}

/// Reads the feed: the address from the stored secret, then the feed over
/// HTTPS, made into today's events. Blocking work (the Keychain is a
/// process on macOS) runs off the runtime's threads.
pub async fn fetch_feed() -> Fetched {
    let url = tokio::task::spawn_blocking(|| platform::secret(SECRET))
        .await
        .ok()
        .flatten();
    let Some(url) = url else {
        return Fetched::NoAddress;
    };
    if !url.starts_with("https://calendar.google.com/calendar/ical/") {
        return Fetched::Failed(format!(
            "The stored {SECRET} isn't a Google Calendar iCal address."
        ));
    }
    let audit = net::Audit {
        service: "Google Calendar",
        protocol: "iCal",
        source: "sidebar: calendar feed",
    };
    let headers = [("Accept", "text/calendar")];
    match net::request_json("GET", &url, &headers, None, audit).await {
        Ok(r) if r.ok() => match r.body {
            Value::String(text) => {
                let now = util::local_now();
                Fetched::Day(
                    tokio::task::spawn_blocking(move || super::ical::day_from(&text, &now))
                        .await
                        .unwrap_or_default(),
                )
            }
            _ => Fetched::Failed("Couldn't read the calendar feed: it isn't iCal".into()),
        },
        Ok(r) => Fetched::Failed(format!(
            "Couldn't read the calendar feed: Google answered {}",
            r.status
        )),
        // Why, without net's own "Couldn't reach Google Calendar", which
        // would say it twice.
        Err(e) => {
            let text = e.to_string();
            let first = text.lines().next().unwrap_or("");
            let why = first
                .strip_prefix("Couldn't reach ")
                .and_then(|rest| rest.split_once(": "))
                .map_or(first, |(_, why)| why);
            Fetched::Failed(format!("Couldn't read the calendar feed: {why}"))
        }
    }
}

/// The feed's day while it has read today's, else /today's; with the feed
/// lost, /today's once the Calendar tab has read today since the feed last
/// did.
pub fn read_day(feed: &Feed, saved: Option<&SavedDay>, today: &str) -> Option<SavedDay> {
    let Some(from_feed) = feed.day.as_ref().filter(|d| d.date == today) else {
        return saved.cloned();
    };
    if feed.down()
        && let Some(s) = saved.filter(|s| s.date == today && s.updated > from_feed.updated)
    {
        return Some(s.clone());
    }
    Some(from_feed.clone())
}

fn fresh(day: Option<&SavedDay>, today: &str, now: i64) -> bool {
    day.is_some_and(|d| d.date == today && now - d.updated <= STALE_MS)
}

/// Whether the day needs a Calendar tab to catch up: with no feed, a saved
/// day that isn't today's or is over `STALE_MS` old; with the feed lost,
/// unless the day shown is /today's and fresh.
pub fn calendar_stale(feed: &Feed, saved: Option<&SavedDay>, today: &str, now: i64) -> bool {
    if !feed.configured() {
        return saved.is_some() && !fresh(saved, today, now);
    }
    let shown = read_day(feed, saved, today);
    feed.down()
        && !(shown.as_ref().is_some_and(|d| !d.from_feed()) && fresh(shown.as_ref(), today, now))
}

/// `1:30 PM`, or `1 PM` on the hour, as calendar.js's `clock`.
pub fn clock(at: Timestamp, tz: &TimeZone) -> String {
    let t = at.to_zoned(tz.clone());
    let (h, m) = (t.hour(), t.minute());
    let hour = if h % 12 == 0 { 12 } else { h % 12 };
    let half = if h < 12 { "AM" } else { "PM" };
    if m == 0 {
        format!("{hour} {half}")
    } else {
        format!("{hour}:{m:02} {half}")
    }
}

/// `1 – 2 PM`, or `11 AM – 1 PM` across noon.
pub fn range(start: Timestamp, end: Timestamp, tz: &TimeZone) -> String {
    let (from, to) = (clock(start, tz), clock(end, tz));
    if from[from.len() - 2..] == to[to.len() - 2..] {
        format!("{} – {to}", &from[..from.len() - 3])
    } else {
        format!("{from} – {to}")
    }
}

/// An event's item id: its Google ID or feed key, else its title and start.
pub fn event_id(e: &CalEvent) -> String {
    e.id.clone()
        .or_else(|| e.key.clone())
        .unwrap_or_else(|| format!("{}@{}", e.title, e.start.as_deref().unwrap_or("all day")))
}

/// Everything the Calendar view says, from what is known now.
pub struct Look<'a> {
    pub feed: &'a Feed,
    pub saved: Option<&'a SavedDay>,
    /// Whether a tab already keeps the calendar (a Calendar tab running
    /// Claude, or one looping /today).
    pub looping: bool,
    pub now: &'a Zoned,
}

/// The Calendar view: its events in the order the cursor walks them,
/// all-day first, then by start, each item's `data` the event itself (for
/// its menu: join, open, RSVP, its tickets), `group` "All day" for those.
/// With no day yet, it says how to set it up, or that the feed is being
/// read, or that the Calendar tab is looking; with yesterday's, that there
/// is none for today yet and how it catches up.
pub fn calendar_view(look: &Look) -> ViewData {
    let today = util::ymd(look.now);
    let now_ms = look.now.timestamp().as_millisecond();
    let tz = look.now.time_zone();
    let feed = look.feed;
    let Some(day) = read_day(feed, look.saved, &today) else {
        let mut view = ViewData::default();
        if feed.configured() && !feed.down() {
            view.error = Some(match &feed.error {
                Some(e) => format!("Couldn't read your calendar: {e}"),
                None => "Reading your calendar...".into(),
            });
        } else if look.looping {
            view.error = Some("Nothing saved yet: the Calendar tab is looking.".into());
        } else if feed.configured() {
            view.error = Some(format!(
                "{} Opening a Calendar tab to read it with /today.",
                feed.error.as_deref().unwrap_or("")
            ));
        } else {
            view.setup = Some(SETUP.into());
        }
        return view;
    };
    let catching = if feed.configured() && !feed.down() {
        feed.error
            .clone()
            .unwrap_or_else(|| "reading the calendar feed".into())
    } else if look.looping {
        "the Calendar tab fetches it every 30 minutes".into()
    } else {
        "opening a Calendar tab to fetch it".into()
    };
    let mut view = ViewData {
        at: (day.updated > 0).then_some(day.updated),
        ..ViewData::default()
    };
    if day.date != today {
        view.error = Some(format!(
            "No calendar for today yet (last saved {}): {catching}.",
            day.date
        ));
        return view;
    }
    if calendar_stale(feed, look.saved, &today, now_ms) {
        view.error = Some(format!("Out of date: {catching}."));
    } else if feed.configured() && !feed.down() {
        view.error = feed.error.clone();
    }
    let mut timed: Vec<&CalEvent> = day.events.iter().filter(|e| !e.all_day).collect();
    timed.sort_by_key(|e| e.starts(tz).map(|t| t.as_millisecond()).unwrap_or(i64::MAX));
    let midnight = tz
        .to_ambiguous_timestamp(look.now.date().to_datetime(jiff::civil::Time::midnight()))
        .compatible()
        .map(|t| t.as_millisecond())
        .unwrap_or(now_ms);
    view.items = day
        .events
        .iter()
        .filter(|e| e.all_day)
        .chain(timed)
        .map(|e| {
            let (start, end) = (e.starts(tz), e.ends(tz));
            let over = end.is_some_and(|t| t.as_millisecond() <= now_ms);
            let mut item = Item::new(event_id(e), &e.title)
                .detail(match (e.all_day, start, end) {
                    (true, _, _) => "all day".to_string(),
                    (false, Some(s), Some(t)) => range(s, t, tz),
                    _ => String::new(),
                })
                .state(if over {
                    State::Idle
                } else if e.going() {
                    State::Active
                } else {
                    State::Attention
                })
                .time(start.map(|t| t.as_millisecond()).unwrap_or(midnight))
                .data(serde_json::to_value(e).unwrap_or(Value::Null));
            if let Some(url) = e.join.clone().or_else(|| e.link.clone()) {
                item = item.target(Target::Url(url));
            }
            if e.all_day {
                item = item.group("All day");
            }
            item
        })
        .collect();
    view
}

/// Whether a tab named `name` keeps the calendar: the Calendar tab with
/// Claude in it, or one looping /today (or /switchboard:today).
pub fn keeps_calendar(name: &str, runs_claude: bool, loop_prompt: Option<&str>) -> bool {
    (name == "Calendar" && runs_claude)
        || loop_prompt.is_some_and(|p| crate::setup::plugin::runs(p, "today"))
}

// ── The next meeting ────────────────────────────────────────────────────────

/// The meeting about to start, or just started, not dismissed: from five
/// minutes before it until ten after, and not once it has ended.
pub fn next_meeting<'a>(
    day: Option<&'a SavedDay>,
    gone: &[String],
    now: &Zoned,
) -> Option<&'a CalEvent> {
    let tz = now.time_zone();
    let now_ms = now.timestamp().as_millisecond();
    let mut soon: Vec<(&CalEvent, i64)> = day?
        .events
        .iter()
        .filter(|e| !e.all_day && !meeting_gone(e, gone, tz))
        .filter_map(|e| {
            let start = e.starts(tz)?.as_millisecond();
            let end = e.ends(tz)?.as_millisecond();
            (start - now_ms < MEETING_BEFORE_MS
                && now_ms - start < MEETING_AFTER_MS
                && end > now_ms)
                .then_some((e, start))
        })
        .collect();
    soon.sort_by_key(|(_, s)| *s);
    soon.first().map(|(e, _)| *e)
}

// The start as one instant, however a source writes it (the feed in UTC, a
// saved day with its offset).
fn instant(start: Option<&str>, tz: &TimeZone) -> String {
    match start {
        Some(s) => parse_instant(s, tz)
            .map(super::ical::iso)
            .unwrap_or_else(|| s.to_string()),
        None => "undefined".into(),
    }
}

// The title as a saved day keeps it, cut at 200 characters.
fn title_of(e: &CalEvent) -> String {
    e.title.chars().take(200).collect()
}

/// The keys a dismissed meeting is remembered by, so it holds whichever
/// source the day is read from: one from the feed by its ID and start (one
/// occurrence of a repeating one), and by its title and start as seen with
/// an ID; one from a saved day, by its title and start alone. From
/// lib/sidebar/daemon.js.
pub fn meeting_keys(e: &CalEvent, tz: &TimeZone) -> Vec<String> {
    let at = instant(e.start.as_deref(), tz);
    match &e.id {
        Some(id) => vec![format!("i:{id}@{at}"), format!("t1:{}@{at}", title_of(e))],
        None => vec![format!("t0:{}@{at}", title_of(e))],
    }
}

/// Whether a meeting was dismissed: a feed event by its ID, or by a title
/// dismissed from a saved day, which couldn't tell two same-titled meetings
/// apart; a saved day's event by either title key. So a meeting of the same
/// title and time as one dismissed from the feed stays.
pub fn meeting_gone(e: &CalEvent, gone: &[String], tz: &TimeZone) -> bool {
    let at = instant(e.start.as_deref(), tz);
    let has = |k: String| gone.contains(&k);
    let t0 = format!("t0:{}@{at}", title_of(e));
    match &e.id {
        Some(id) => has(format!("i:{id}@{at}")) || has(t0),
        None => has(t0) || has(format!("t1:{}@{at}", title_of(e))),
    }
}

/// The meetings dismissed from the notifications, from
/// `notices-dismissed.json` in the state directory.
pub fn dismissed_meetings(state: &Path) -> Vec<String> {
    util::read_json(state.join("notices-dismissed.json"))
        .and_then(|v| v.get("meeting").cloned())
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default()
}

/// Remembers a meeting dismissed from the notifications, keeping the file's
/// other keys (the Linear inbox's).
pub fn dismiss_meeting(state: &Path, e: &CalEvent, tz: &TimeZone) {
    let file = state.join("notices-dismissed.json");
    let mut saved = util::read_json_object(&file);
    let mut keys = dismissed_meetings(state);
    keys.extend(meeting_keys(e, tz));
    let skip = keys.len().saturating_sub(200);
    saved.insert("meeting".into(), json!(keys[skip..]));
    let _ = util::write_atomic(&file, util::json_compact(&Value::Object(saved)));
}

/// The next meeting as a notification: its title and how soon, its tickets,
/// Enter joining its call.
pub fn meeting_notice(e: &CalEvent, now: &Zoned) -> Item {
    let tz = now.time_zone();
    let start = e.starts(tz);
    let until = start
        .map(|s| {
            ((s.as_millisecond() - now.timestamp().as_millisecond()) as f64 / 60_000.0).round()
                as i64
        })
        .unwrap_or(0);
    let mut item = Item::new(
        format!("meeting:{}", event_id(e)),
        format!(
            "{} {}{}",
            e.title,
            if until > 0 {
                format!("in {until}m")
            } else {
                "now".into()
            },
            if e.join.is_some() {
                " · ↵ joins"
            } else {
                ""
            }
        ),
    )
    .detail(
        e.refs
            .iter()
            .map(|r| r.id.clone())
            .collect::<Vec<_>>()
            .join(" · "),
    )
    .state(State::Attention)
    .data(json!({ "kind": "meeting", "event": e }));
    if let Some(s) = start {
        item = item.time(s.as_millisecond());
    }
    if let (Some(s), Some(t)) = (start, e.ends(tz)) {
        item.lines.push(range(s, t, tz));
    }
    if let Some(join) = &e.join {
        item.lines.push("Enter: join its call".into());
        item = item.target(Target::Url(join.clone()));
    }
    item.lines.extend(e.refs.iter().map(|r| r.id.clone()));
    item
}

// ── RSVP ────────────────────────────────────────────────────────────────────

/// An answer to an invitation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rsvp {
    Yes,
    Maybe,
    No,
}

impl Rsvp {
    /// What Google's API calls it.
    pub fn status(self) -> &'static str {
        match self {
            Rsvp::Yes => "accepted",
            Rsvp::Maybe => "tentative",
            Rsvp::No => "declined",
        }
    }

    pub fn words(self) -> &'static str {
        match self {
            Rsvp::Yes => "yes",
            Rsvp::Maybe => "maybe",
            Rsvp::No => "no",
        }
    }

    /// The menu's key and label, as the daemon's menu had them.
    pub fn choice(self) -> (&'static str, &'static str) {
        match self {
            Rsvp::Yes => ("y", "RSVP: Yes, going"),
            Rsvp::Maybe => ("m", "RSVP: Maybe"),
            Rsvp::No => ("n", "RSVP: No, not going"),
        }
    }
}

/// The answers an event's menu offers: none with the calendar_rsvp setting
/// off, for an event not from the feed (no Google ID), or for one of your
/// own; else every answer but the one already given.
pub fn rsvp_choices(e: &CalEvent) -> Vec<Rsvp> {
    if !settings::is_on("calendar_rsvp") || e.id.is_none() || e.you == "yours" {
        return Vec::new();
    }
    [Rsvp::Yes, Rsvp::Maybe, Rsvp::No]
        .into_iter()
        .filter(|r| match e.you.as_str() {
            "going" => *r != Rsvp::Yes,
            "maybe" => *r != Rsvp::Maybe,
            _ => true,
        })
        .collect()
}

/// Answers an invitation through Claude's Google Calendar connector; gives
/// what to say: that it was answered, or why it couldn't be.
pub async fn rsvp(e: &CalEvent, answer: Rsvp) -> String {
    let Some(id) = &e.id else {
        return "Couldn't answer it: it has no Google Calendar ID.".into();
    };
    let input = json!({
        "eventId": id,
        "calendarId": e.calendar.clone().unwrap_or_else(|| "primary".into()),
        "responseStatus": answer.status(),
    });
    match connector::call_connector(
        "mcp__claude_ai_Google_Calendar__respond_to_event",
        &input,
        Some(Duration::from_secs(120)),
    )
    .await
    {
        Ok(_) => format!("Answered {}: {}.", answer.words(), e.title),
        Err(err) => format!("Couldn't answer it: {}", err.message),
    }
}

/// Saves the day /today read, as the skill's scripts/save.js did: only the
/// fields the view uses, only https links, an answer it doesn't know taken
/// as not answered. Gives what to tell the skill.
pub fn save_today(state: &Path, input: &str) -> Result<String, String> {
    let day: Value = serde_json::from_str(input).map_err(|e| format!("Not JSON: {e}"))?;
    let date = day.get("date").and_then(Value::as_str).unwrap_or("");
    let ymd = regex::Regex::new(r"^\d{4}-\d{2}-\d{2}$").expect("a valid pattern");
    if !ymd.is_match(date) {
        return Err("date must be YYYY-MM-DD".into());
    }
    let Some(list) = day.get("events").and_then(Value::as_array) else {
        return Err("events must be a list".into());
    };
    let tz = TimeZone::system();
    let https = |v: Option<&Value>| {
        v.and_then(Value::as_str)
            .filter(|u| {
                u.starts_with("https://") && u.len() > 8 && !u.chars().any(char::is_whitespace)
            })
            .map(str::to_string)
    };
    let events: Vec<Value> = list
        .iter()
        .filter_map(|e| {
            let title = e.get("title")?.as_str()?;
            let all_day = e.get("allDay").is_some_and(util::truthy);
            let at = |k: &str| {
                e.get(k)
                    .and_then(Value::as_str)
                    .filter(|s| parse_instant(s, &tz).is_some())
            };
            if !all_day && (at("start").is_none() || at("end").is_none()) {
                return None;
            }
            let you = e.get("you").and_then(Value::as_str).unwrap_or("");
            let mut out = serde_json::Map::new();
            out.insert(
                "title".into(),
                json!(title.chars().take(200).collect::<String>()),
            );
            out.insert("allDay".into(), json!(all_day));
            if !all_day {
                out.insert("start".into(), json!(at("start")));
                out.insert("end".into(), json!(at("end")));
            }
            out.insert(
                "you".into(),
                json!(
                    if ["going", "yours", "maybe", "not answered"].contains(&you) {
                        you
                    } else {
                        "not answered"
                    }
                ),
            );
            if let Some(j) = https(e.get("join")) {
                out.insert("join".into(), json!(j));
            }
            if let Some(l) = https(e.get("link")) {
                out.insert("link".into(), json!(l));
            }
            Some(Value::Object(out))
        })
        .collect();
    let n = events.len();
    let saved = json!({ "date": date, "updated": util::now_ms(), "events": events });
    util::write_atomic(saved_file(state), util::json_compact(&saved)).map_err(|e| e.to_string())?;
    Ok(format!(
        "Saved {n} event{} for mission control.",
        if n == 1 { "" } else { "s" }
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ny() -> TimeZone {
        TimeZone::get("America/New_York").unwrap()
    }

    fn at(h: i8, m: i8) -> Timestamp {
        jiff::civil::date(2026, 11, 2)
            .at(h, m, 0, 0)
            .to_zoned(ny())
            .unwrap()
            .timestamp()
    }

    #[test]
    fn times_read_as_the_view_writes_them() {
        assert_eq!(clock(at(13, 0), &ny()), "1 PM");
        assert_eq!(clock(at(9, 5), &ny()), "9:05 AM");
        assert_eq!(range(at(13, 0), at(14, 30), &ny()), "1 – 2:30 PM");
        assert_eq!(range(at(11, 0), at(13, 0), &ny()), "11 AM – 1 PM");
    }

    #[test]
    fn a_lost_feed_falls_back_to_the_day_today_saved() {
        let today = "2026-11-02";
        let now = 1_000_000_000_000;
        let mut feed = Feed {
            day: Some(SavedDay {
                date: today.into(),
                updated: now - 3_600_000,
                source: Some("ical".into()),
                events: Vec::new(),
            }),
            configured: Some(true),
            ..Feed::default()
        };
        let saved = |ago: i64| SavedDay {
            date: today.into(),
            updated: now - ago,
            source: None,
            events: Vec::new(),
        };
        let run = |feed: &Feed, saved_ago: i64| {
            let s = saved(saved_ago);
            (
                calendar_stale(feed, Some(&s), today, now),
                read_day(feed, Some(&s), today).unwrap().from_feed(),
            )
        };
        assert_eq!(
            run(&feed, 86_400_000),
            (false, true),
            "a feed reading needs no tab"
        );
        let failed = |feed: &mut Feed, looks: &[i64]| {
            for ago in looks {
                feed.failed(
                    "Couldn't read the calendar feed: offline".into(),
                    now - ago * 60_000,
                );
            }
        };
        let mut one = feed.clone();
        failed(&mut one, &[1]);
        assert_eq!(
            run(&one, 86_400_000),
            (false, true),
            "one failed look is not a lost feed"
        );
        let mut lost = feed.clone();
        failed(&mut lost, &[20, 15, 10, 5, 0]);
        assert_eq!(
            run(&lost, 86_400_000),
            (true, true),
            "a lost feed with nothing newer saved opens a Calendar tab"
        );
        let mut away = feed.clone();
        failed(&mut away, &[75, 70, 0]);
        assert_eq!(
            run(&away, 86_400_000),
            (false, true),
            "time away between failed looks starts the count over"
        );
        assert_eq!(
            run(&lost, 60_000),
            (false, false),
            "and then shows the day /today saved"
        );
        assert_eq!(
            run(&lost, 3_660_000),
            (true, true),
            "a saved day older than the feed's last keeps nothing"
        );
        feed.take(Fetched::NoAddress, now);
        assert!(!calendar_stale(&feed, None, today, now));
    }

    #[test]
    fn a_dismissed_meeting_stays_dismissed_whichever_source_the_day_is_read_from() {
        let start = Some("2026-10-30T18:00:00Z".to_string());
        let feed = CalEvent {
            id: Some("a1".into()),
            title: "Interview".into(),
            start: start.clone(),
            ..CalEvent::default()
        };
        let other = CalEvent {
            id: Some("b2".into()),
            ..feed.clone()
        };
        // The saved day writes the same instant with its offset.
        let saved = CalEvent {
            title: "Interview".into(),
            start: Some("2026-10-30T14:00:00-04:00".into()),
            ..CalEvent::default()
        };
        let tz = ny();
        let gone = meeting_keys(&feed, &tz);
        assert!(meeting_gone(&feed, &gone, &tz));
        assert!(
            !meeting_gone(&other, &gone, &tz),
            "another of the same title and time stays"
        );
        assert!(
            meeting_gone(&saved, &gone, &tz),
            "as the saved day shows it"
        );
        let gone_saved = meeting_keys(&saved, &tz);
        assert!(
            meeting_gone(&feed, &gone_saved, &tz),
            "dismissed from the saved day, it stays gone once the feed loads"
        );
    }
}
