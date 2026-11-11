//! What changed on today's calendar since you last looked: an event added,
//! moved, renamed or gone, for the sidebar's notifications (the
//! calendar_changes setting). Ported from lib/sidebar/calendar-changes.js.
//! Today's events as you last saw them are kept in `calendar-seen.json` in
//! the state directory, and looking at the Calendar view or dismissing the
//! notification takes the day as it is now. A new day, or the first look,
//! starts from the day as it is, with nothing to say.
//!
//! An event is known by its Google ID from the calendar feed, which a moved
//! occurrence keeps; a day /today saved has none, so there it is its title
//! and start, and a moved one reads as one gone and one added. Where the
//! day came from is kept apart (the feed's events have IDs, /today's none,
//! so their keys can't be compared): each source keeps a baseline of its
//! own, so one standing in for the other a while neither reads as every
//! event new nor takes the other's changes as seen.

use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use jiff::tz::TimeZone;
use regex::Regex;
use serde_json::{Map, Value, json};

use super::calendar::{CalEvent, SavedDay, clock, parse_instant};
use super::ical::iso;
use crate::items::{Item, State};
use crate::util;

fn file(state: &Path) -> PathBuf {
    state.join("calendar-seen.json")
}

/// One change, keyed by the event it is about.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    pub key: String,
    pub text: String,
}

// A time as one instant however it was written (a saved day keeps what
// /today wrote: Z or an offset).
fn instant(at: Option<&str>, tz: &TimeZone) -> Value {
    match at {
        Some(s) => parse_instant(s, tz)
            .map(|t| json!(iso(t)))
            .unwrap_or_else(|| json!(s)),
        None => Value::Null,
    }
}

// Google's ID first, which holds when Google splits a repeating series; the
// feed's own key where there is no Google ID.
fn key_of(e: &CalEvent, tz: &TimeZone) -> String {
    e.id.clone().or_else(|| e.key.clone()).unwrap_or_else(|| {
        let at = instant(e.start.as_deref(), tz);
        format!("{}@{}", e.title, at.as_str().unwrap_or("all day"))
    })
}

fn facts_of(e: &CalEvent, tz: &TimeZone) -> Value {
    json!({
        "title": e.title,
        "start": instant(e.start.as_deref(), tz),
        "end": instant(e.end.as_deref(), tz),
        "allDay": e.all_day,
    })
}

fn source_of(day: &SavedDay) -> &'static str {
    if day.from_feed() { "feed" } else { "saved" }
}

fn snapshot(day: &SavedDay, tz: &TimeZone) -> Value {
    let events: Map<String, Value> = day
        .events
        .iter()
        .map(|e| (key_of(e, tz), facts_of(e, tz)))
        .collect();
    json!({ "date": day.date, "events": events })
}

// An occurrence of a repeating event is keyed with when it was meant to
// start (abc_20261102T140000Z, or uid@time), so a series moved to another
// time keys today's afresh: one gone and one added of the same series are
// that occurrence moved. Only a recognized occurrence time comes off, never
// part of the UID, and a key with none (a one-off event) is a series of its
// own.
fn series_of(key: &str) -> String {
    static GOOGLE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"_\d{8}(T\d{6}Z?)?$").unwrap());
    static FEED: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"@\d{4}-\d\d-\d\dT[\d:.]+Z$").unwrap());
    let key = GOOGLE.replace(key, "");
    FEED.replace(&key, "").into_owned()
}

fn text(facts: &Value, name: &str) -> String {
    facts
        .get(name)
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

fn clock_of(facts: &Value, name: &str, tz: &TimeZone) -> String {
    facts
        .get(name)
        .and_then(Value::as_str)
        .and_then(|s| parse_instant(s, tz))
        .map(|t| clock(t, tz))
        .unwrap_or_default()
}

/// The changes from `seen` (a snapshot, `{ date, events }`) to `day`, in the
/// day's order, then what is gone.
pub fn changes_from(seen: &Value, day: &SavedDay, tz: &TimeZone) -> Vec<Change> {
    let mut was: Map<String, Value> = seen
        .get("events")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let now: Vec<(String, Value)> = day
        .events
        .iter()
        .map(|e| (key_of(e, tz), facts_of(e, tz)))
        .collect();
    let has_now = |k: &str| now.iter().any(|(n, _)| n == k);
    for (key, _) in &now {
        if was.contains_key(key) || series_of(key) == *key {
            continue;
        }
        let old = was
            .keys()
            .find(|k| {
                !has_now(k) && *k != key && series_of(k) != **k && series_of(k) == series_of(key)
            })
            .cloned();
        if let Some(old) = old {
            let facts = was.shift_remove(&old).expect("found above");
            was.insert(key.clone(), facts);
        }
    }
    let mut changes = Vec::new();
    for (key, e) in &now {
        let all_day = e.get("allDay").is_some_and(util::truthy);
        let title = text(e, "title");
        let said = match was.get(key) {
            None => Some(format!(
                "{title} added{}",
                if all_day {
                    String::new()
                } else {
                    format!(", {}", clock_of(e, "start", tz))
                }
            )),
            Some(before) if before.get("start") != e.get("start") => Some(format!(
                "{title} moved{}",
                if all_day {
                    String::new()
                } else {
                    format!(" to {}", clock_of(e, "start", tz))
                }
            )),
            Some(before) if before.get("end") != e.get("end") => Some(format!(
                "{title} now ends{}",
                if all_day {
                    " on another day".to_string()
                } else {
                    format!(" at {}", clock_of(e, "end", tz))
                }
            )),
            Some(before) if text(before, "title") != title => {
                Some(format!("{} renamed {title}", text(before, "title")))
            }
            Some(_) => None,
        };
        if let Some(text) = said {
            changes.push(Change {
                key: key.clone(),
                text,
            });
        }
    }
    for (key, e) in &was {
        if !has_now(key) {
            changes.push(Change {
                key: key.clone(),
                text: format!("{} gone from the calendar", text(e, "title")),
            });
        }
    }
    changes
}

// `{ feed, saved }`: each source's day as last seen.
fn read_seen(state: &Path) -> Map<String, Value> {
    let saved = util::read_json_object(file(state));
    if saved.contains_key("events") {
        Map::new()
    } else {
        saved
    }
}

/// Takes the day as it is now as seen, for its source.
pub fn calendar_seen(state: &Path, day: &SavedDay, tz: &TimeZone) {
    if day.date.is_empty() {
        return;
    }
    let mut seen = read_seen(state);
    seen.insert(source_of(day).into(), snapshot(day, tz));
    let _ = util::write_atomic(file(state), util::json_compact(&Value::Object(seen)));
}

/// What changed since you last looked; the first look at a day, from its
/// source, says nothing, nor does a day that isn't `today`.
pub fn calendar_changes(state: &Path, day: &SavedDay, today: &str, tz: &TimeZone) -> Vec<Change> {
    if day.date.is_empty() || day.date != today {
        return Vec::new();
    }
    let seen = read_seen(state).get(source_of(day)).cloned();
    match seen {
        Some(seen) if seen.get("date").and_then(Value::as_str) == Some(day.date.as_str()) => {
            changes_from(&seen, day, tz)
        }
        _ => {
            calendar_seen(state, day, tz);
            Vec::new()
        }
    }
}

/// Where today's events started when first seen today, kept in
/// `calendar-moves.json` in the state directory whatever you have looked
/// at (unlike `calendar-seen.json`, which looking replaces), so the
/// Calendar view marks an event moved all day: gives, by its key, where
/// each event that has moved since started. A new day starts it afresh.
pub fn moves(
    state: &Path,
    day: &SavedDay,
    today: &str,
    tz: &TimeZone,
) -> std::collections::HashMap<String, String> {
    let mut out = std::collections::HashMap::new();
    if day.date.is_empty() || day.date != today {
        return out;
    }
    let path = state.join("calendar-moves.json");
    let kept = util::read_json_object(&path);
    let mut starts = match kept.get("date").and_then(Value::as_str) {
        Some(date) if date == today => kept
            .get("starts")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default(),
        _ => Map::new(),
    };
    let mut changed = kept.get("date").and_then(Value::as_str) != Some(today);
    // A repeating series moved whole keys today's occurrence afresh (its
    // key holds when it was meant to start): one kept under a key of the
    // same series no event of today's has any more is that occurrence.
    let keys: Vec<String> = day.events.iter().map(|e| key_of(e, tz)).collect();
    for e in day.events.iter().filter(|e| !e.all_day) {
        let key = key_of(e, tz);
        if starts.contains_key(&key) {
            continue;
        }
        let gone = starts
            .keys()
            .find(|k| !keys.contains(k) && series_of(k) == series_of(&key) && **k != key)
            .cloned();
        if let Some(old) = gone
            && let Some(first) = starts.remove(&old)
        {
            starts.insert(key, first);
            changed = true;
        }
    }
    for e in day.events.iter().filter(|e| !e.all_day) {
        let Value::String(now) = instant(e.start.as_deref(), tz) else {
            continue;
        };
        let key = key_of(e, tz);
        match starts.get(&key).and_then(Value::as_str) {
            Some(first) if first != now => {
                out.insert(key, first.to_string());
            }
            Some(_) => {}
            None => {
                starts.insert(key, json!(now));
                changed = true;
            }
        }
    }
    if changed {
        let kept = json!({ "date": today, "starts": starts });
        let _ = util::write_atomic(&path, util::json_compact(&kept));
    }
    out
}

/// Where an event started when first seen today, when it has moved since,
/// from what `moves` gave.
pub fn moved_from(
    e: &CalEvent,
    moves: &std::collections::HashMap<String, String>,
    tz: &TimeZone,
) -> Option<String> {
    if moves.is_empty() || e.all_day {
        return None;
    }
    moves.get(&key_of(e, tz)).cloned()
}

/// The changes as notifications, one each.
pub fn change_notices(changes: &[Change]) -> Vec<Item> {
    changes
        .iter()
        .map(|c| {
            Item::new(format!("calendar:{}", c.key), &c.text)
                .detail("Changed on today's calendar since you last looked at it")
                .state(State::Active)
                .data(json!({ "kind": "calendar-change", "key": c.key }))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tz() -> TimeZone {
        TimeZone::UTC
    }

    fn at(h: u32) -> String {
        format!("2026-11-02T{h:02}:00:00.000Z")
    }

    fn event(id: Option<&str>, title: &str, from: u32, to: u32) -> CalEvent {
        CalEvent {
            id: id.map(str::to_string),
            title: title.into(),
            start: Some(at(from)),
            end: Some(at(to)),
            ..CalEvent::default()
        }
    }

    fn said(seen: &Value, day: &SavedDay) -> Vec<String> {
        static TAIL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r", .*| to .*").unwrap());
        changes_from(seen, day, &tz())
            .into_iter()
            .map(|c| TAIL.replace(&c.text, "").into_owned())
            .collect()
    }

    fn day(events: Vec<CalEvent>) -> SavedDay {
        SavedDay {
            date: "2026-11-02".into(),
            events,
            ..SavedDay::default()
        }
    }

    #[test]
    fn added_moved_renamed_and_gone_by_google_id_when_there_is_one() {
        let facts = |title: &str, from: u32, to: u32| json!({ "title": title, "start": at(from), "end": at(to), "allDay": false });
        let seen = json!({ "date": "2026-11-02", "events": {
            "a1": facts("Standup", 14, 15),
            "b2": facts("Planning", 16, 17),
            "c3": facts("Retro", 18, 19),
            "Lunch@2026-11-02T12:00:00.000Z": facts("Lunch", 12, 13),
        } });
        let today = day(vec![
            event(Some("a1"), "Standup", 15, 16),
            event(Some("b2"), "Sprint planning", 16, 17),
            event(Some("d4"), "1:1", 20, 21),
            event(None, "Lunch", 12, 13),
        ]);
        assert_eq!(
            said(&seen, &today),
            [
                "Standup moved",
                "Planning renamed Sprint planning",
                "1:1 added",
                "Retro gone from the calendar"
            ]
        );
        let series = json!({ "date": "2026-11-02", "events": { "rec_20261102T140000Z": facts("Sync", 14, 15) } });
        assert_eq!(
            said(
                &series,
                &day(vec![event(Some("rec_20261102T150000Z"), "Sync", 15, 16)])
            ),
            ["Sync moved"],
            "a repeating event moved to another time, as one"
        );
        let one_offs = json!({ "date": "2026-11-02", "events": { "event_123@example.com": facts("A", 9, 10) } });
        let mut b = event(None, "B", 15, 16);
        b.key = Some("event_456@example.com".into());
        assert_eq!(
            said(&one_offs, &day(vec![b])),
            ["B added", "A gone from the calendar"],
            "two one-off events stay two"
        );
        let same = day(vec![
            event(Some("a1"), "Standup", 14, 15),
            event(Some("b2"), "Planning", 16, 17),
            event(Some("c3"), "Retro", 18, 19),
            event(None, "Lunch", 12, 13),
        ]);
        assert!(
            changes_from(&seen, &same, &tz()).is_empty(),
            "the same day, nothing"
        );
    }

    #[test]
    fn the_first_look_says_nothing_and_seeing_takes_the_day() {
        let dir = std::env::temp_dir().join(format!(
            "sb-changes-{}-{}",
            std::process::id(),
            util::now_ms()
        ));
        let feed = |events| SavedDay {
            source: Some("ical".into()),
            ..day(events)
        };
        let first = feed(vec![event(Some("a1"), "Standup", 14, 15)]);
        assert!(calendar_changes(&dir, &first, "2026-11-02", &tz()).is_empty());
        let later = feed(vec![
            event(Some("a1"), "Standup", 14, 15),
            event(Some("b2"), "Demo", 16, 17),
        ]);
        let changes = calendar_changes(&dir, &later, "2026-11-02", &tz());
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].text, "Demo added, 4 PM");
        assert!(
            calendar_changes(&dir, &later, "2026-11-03", &tz()).is_empty(),
            "only today's"
        );
        let mut saved = later.clone();
        saved.source = None;
        assert!(
            calendar_changes(&dir, &saved, "2026-11-02", &tz()).is_empty(),
            "the saved day keeps a baseline of its own"
        );
        calendar_seen(&dir, &later, &tz());
        assert!(calendar_changes(&dir, &later, "2026-11-02", &tz()).is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn moves_keeps_the_first_start_seen_today_and_starts_over_on_a_new_day() {
        let dir = std::env::temp_dir().join(format!(
            "sb-moves-{}-{}",
            std::process::id(),
            util::now_ms()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let first = day(vec![event(Some("a1"), "Sync", 14, 15)]);
        assert!(moves(&dir, &first, "2026-11-02", &tz()).is_empty());
        let moved = day(vec![event(Some("a1"), "Sync", 15, 16)]);
        let found = moves(&dir, &moved, "2026-11-02", &tz());
        assert_eq!(found.get("a1").map(String::as_str), Some(at(14).as_str()));
        let e = &moved.events[0];
        assert_eq!(moved_from(e, &found, &tz()), Some(at(14)));
        // Looking again keeps the first start, not the one seen since.
        let again = moves(&dir, &moved, "2026-11-02", &tz());
        assert_eq!(again.get("a1").map(String::as_str), Some(at(14).as_str()));
        // Back where it was is no move.
        assert!(moves(&dir, &first, "2026-11-02", &tz()).is_empty());
        // A repeating series moved whole keys today's occurrence afresh,
        // and keeps where it first started.
        let series = |at: u32, from, to| {
            day(vec![event(
                Some(&format!("std_20261102T{at:02}0000Z")),
                "Standup",
                from,
                to,
            )])
        };
        assert!(moves(&dir, &series(9, 9, 10), "2026-11-02", &tz()).is_empty());
        let later = moves(&dir, &series(10, 10, 11), "2026-11-02", &tz());
        assert_eq!(
            later.get("std_20261102T100000Z").map(String::as_str),
            Some(at(9).as_str())
        );
        // A new day starts afresh.
        let tomorrow = SavedDay {
            date: "2026-11-03".into(),
            ..moved.clone()
        };
        assert!(moves(&dir, &tomorrow, "2026-11-03", &tz()).is_empty());
        assert!(
            moves(&dir, &moved, "2026-11-02", &tz()).is_empty(),
            "a day kept for another starts afresh"
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}
