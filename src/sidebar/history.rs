//! The notifications kept as a history by day, for mission control's
//! Notifications view: every notification a source gave over the last two
//! weeks, whether the sidebar still shows it (or it was dismissed, read, or
//! went on its own), and what it led to. The sidebar's own lists are the
//! sources' as they are now (`Server::set_notices`); a notification that
//! leaves its list stays here, `Older`.
//!
//! Each day is a file of its own, `notifications/<YYYY-MM-DD>.json` in the
//! state directory, the entries that came that day, so a change writes only
//! that day; days past `KEEP_DAYS` are removed. Nothing here leaves the
//! computer. The server reads the files once as it starts and writes a day
//! whenever it changes, both off its task (`sidebar/serve.rs`).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::items::Item;

/// How many days are kept, today among them.
pub const KEEP_DAYS: i64 = 14;

/// Whether the sidebar still shows a notification, or why not.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Sidebar {
    Shown,
    Dismissed,
    Read,
    /// Gone from its source's list by itself.
    Older,
}

impl Sidebar {
    /// The word its column shows.
    pub fn word(self) -> &'static str {
        match self {
            Sidebar::Shown => "shown",
            Sidebar::Dismissed => "dismissed",
            Sidebar::Read => "read",
            Sidebar::Older => "older",
        }
    }
}

/// The kinds the view's keycaps filter by, after All, in their order: each
/// with its key and its name. A source belongs to one (`kind_of`), or to
/// none, shown only under All.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Landed,
    Prs,
    Slack,
    Inbox,
    Calendar,
    Notes,
}

impl Kind {
    pub const ALL: [Kind; 6] = [
        Kind::Landed,
        Kind::Prs,
        Kind::Slack,
        Kind::Inbox,
        Kind::Calendar,
        Kind::Notes,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Kind::Landed => "Landed",
            Kind::Prs => "PRs",
            Kind::Slack => "Slack",
            Kind::Inbox => "Inbox",
            Kind::Calendar => "Calendar",
            Kind::Notes => "Notes",
        }
    }

    /// Its keycap's key: 2 for the first, 1 being All.
    pub fn key(self) -> usize {
        Kind::ALL.iter().position(|k| *k == self).unwrap_or(0) + 2
    }

    /// The kind a key filters by; `None` for 1, All.
    pub fn of_key(key: usize) -> Option<Kind> {
        key.checked_sub(2).and_then(|i| Kind::ALL.get(i).copied())
    }
}

/// The kind of a notification, by the source that gave it.
pub fn kind_of(source: &str) -> Option<Kind> {
    match source {
        "autolanded" | "landed" | "autopilot" => Some(Kind::Landed),
        "prs" | "ci" => Some(Kind::Prs),
        "slack" => Some(Kind::Slack),
        "inbox" => Some(Kind::Inbox),
        "calendar" | "meeting" => Some(Kind::Calendar),
        "new-notes" => Some(Kind::Notes),
        _ => None,
    }
}

/// One notification as it was given, and what became of it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub source: String,
    /// Its id in its source's list.
    pub id: String,
    /// When it came (ms): its own time, else when it was first seen.
    pub at: i64,
    /// The notification as its source gave it last, for its menu.
    pub item: Item,
    pub sidebar: Sidebar,
    /// What it led to ("you went to tab 5"), and when (ms).
    #[serde(default)]
    pub led_to: Option<(String, i64)>,
    /// A later occurrence of the same notification has an entry of its
    /// own, which its source's list, marking and what it led to follow;
    /// this one is kept as it was.
    #[serde(default)]
    pub replaced: bool,
}

/// The kept days, by their date (`YYYY-MM-DD`), each its entries in the
/// order they came; and the days changed since they were last written.
#[derive(Debug, Default)]
pub struct History {
    pub days: BTreeMap<String, Vec<Entry>>,
    pub changed: BTreeSet<String>,
    /// The files have been read (or found missing), so a day is written
    /// whole rather than over entries not yet read.
    pub loaded: bool,
    /// The sources that have given their list since the server started:
    /// only those are recorded once the files are read, so a source yet
    /// to answer (a calendar's first read) leaves what was kept of it.
    pub answered: BTreeSet<String>,
}

/// The day a time falls on in `tz`, as its file names it.
pub fn day_of(ms: i64, tz: &jiff::tz::TimeZone) -> String {
    jiff::Timestamp::from_millisecond(ms)
        .map(|t| t.to_zoned(tz.clone()).date().to_string())
        .unwrap_or_default()
}

impl History {
    fn entries_mut(&mut self) -> impl Iterator<Item = (&String, &mut Entry)> {
        self.days
            .iter_mut()
            .flat_map(|(day, entries)| entries.iter_mut().map(move |e| (day, e)))
    }

    /// Every entry, newest first.
    pub fn newest_first(&self) -> Vec<&Entry> {
        let mut all: Vec<&Entry> = self.days.values().flatten().collect();
        all.sort_by_key(|e| std::cmp::Reverse(e.at));
        all
    }

    /// A source's list as it is now: a notification new to it (by its id)
    /// becomes an entry, on the day of its time (else `now`); one kept as
    /// gone by itself, dismissed or read and back is shown again; one dismissed or read that
    /// came back changed (a Slack thread with more in it), and any given
    /// at a time of its own (a PR's checks failing again), is a new
    /// occurrence, an entry of its own on the day of its new time, the
    /// earlier one kept as it was (`Older` when it was shown); one shown
    /// that left the list is `Older`. An occurrence is told apart by its
    /// time, `at`. True when anything changed.
    pub fn record(
        &mut self,
        source: &str,
        items: &[Item],
        now: i64,
        tz: &jiff::tz::TimeZone,
    ) -> bool {
        let mut touched: BTreeSet<String> = BTreeSet::new();
        let mut known: BTreeSet<String> = BTreeSet::new();
        // The items given again as a new occurrence.
        let mut again_ids: BTreeSet<String> = BTreeSet::new();
        // When each new occurrence came, after its earlier one.
        let mut again_at: Vec<(String, i64)> = Vec::new();
        for (day, e) in self.entries_mut() {
            if e.source != source || e.replaced {
                continue;
            }
            known.insert(e.id.clone());
            match items.iter().find(|i| i.id == e.id) {
                Some(item) => {
                    let at_a_new_time = item.time.is_some() && item.time != e.item.time;
                    // A notification with a time of its own is the same one
                    // while its time is (a meeting's countdown, a ticket
                    // renamed). One without is told apart by its words, once
                    // the sidebar no longer shows it (autopilot's line saying
                    // something else); while it shows it, the same one told
                    // anew.
                    let changed = item.time.is_none()
                        && e.sidebar != Sidebar::Shown
                        && (item.title != e.item.title || item.detail != e.item.detail);
                    if at_a_new_time || changed {
                        // Told apart from it by a later time, its own when it
                        // has a new one, else now.
                        let at = if at_a_new_time {
                            item.time.unwrap_or(now)
                        } else {
                            now
                        };
                        again_at.push((e.id.clone(), at.max(e.at + 1)));
                        e.replaced = true;
                        if e.sidebar == Sidebar::Shown {
                            e.sidebar = Sidebar::Older;
                        }
                        again_ids.insert(e.id.clone());
                        touched.insert(day.clone());
                        continue;
                    }
                    // In its source's list, the sidebar shows it, whatever
                    // was kept of it before (a dismissal a restart forgot).
                    let came_back = e.sidebar != Sidebar::Shown;
                    if came_back {
                        e.sidebar = Sidebar::Shown;
                    }
                    if came_back || e.item != *item {
                        e.item = item.clone();
                        touched.insert(day.clone());
                    }
                }
                None if e.sidebar == Sidebar::Shown => {
                    e.sidebar = Sidebar::Older;
                    touched.insert(day.clone());
                }
                None => {}
            }
        }
        let new = |i: &&Item| !known.contains(&i.id) || again_ids.contains(&i.id);
        for item in items.iter().filter(new) {
            let at = again_at
                .iter()
                .find(|(id, _)| *id == item.id)
                .map_or(item.time.unwrap_or(now), |(_, at)| *at);
            let day = day_of(at, tz);
            self.days.entry(day.clone()).or_default().push(Entry {
                source: source.to_string(),
                id: item.id.clone(),
                at,
                item: item.clone(),
                sidebar: Sidebar::Shown,
                led_to: None,
                replaced: false,
            });
            touched.insert(day);
        }
        let any = !touched.is_empty();
        self.changed.extend(touched);
        any
    }

    /// Puts a notification out of the sidebar as `how` (dismissed, read),
    /// when the sidebar still shows it; with `late`, whatever it says, for
    /// what is known only once its list has changed (a read Linear answers
    /// after the menu put it away, a dismissal done by its source).
    pub fn mark(&mut self, source: &str, id: &str, how: Sidebar, late: bool) -> bool {
        let mut touched = None;
        for (day, e) in self.entries_mut() {
            let markable = e.sidebar != how && (late || e.sidebar == Sidebar::Shown);
            if e.source == source && e.id == id && !e.replaced && markable {
                e.sidebar = how;
                touched = Some(day.clone());
            }
        }
        touched.map(|d| self.changed.insert(d)).is_some()
    }

    /// Records what a notification led to, at `at`: the occurrence that
    /// came at `occurrence` when given (a row of the view), else its latest.
    pub fn led_to(
        &mut self,
        source: &str,
        id: &str,
        occurrence: Option<i64>,
        words: &str,
        at: i64,
    ) -> bool {
        let mut touched = None;
        for (day, e) in self.entries_mut() {
            let this_one = match occurrence {
                Some(o) => e.at == o,
                None => !e.replaced,
            };
            if e.source == source && e.id == id && this_one {
                e.led_to = Some((words.to_string(), at));
                touched = Some(day.clone());
            }
        }
        touched.map(|d| self.changed.insert(d)).is_some()
    }

    /// Lets go of the days before the last `KEEP_DAYS`, today among them;
    /// the days let go, whose files go too.
    pub fn prune(&mut self, today: &str) -> Vec<String> {
        let Ok(today) = today.parse::<jiff::civil::Date>() else {
            return Vec::new();
        };
        let Ok(first) = today.checked_sub(jiff::Span::new().days(KEEP_DAYS - 1)) else {
            return Vec::new();
        };
        let first = first.to_string();
        let gone: Vec<String> = self
            .days
            .keys()
            .filter(|d| d.as_str() < first.as_str())
            .cloned()
            .collect();
        for d in &gone {
            self.days.remove(d);
            self.changed.remove(d);
        }
        gone
    }

    /// The days read from their files, taken in with what was recorded
    /// before they were read: an entry kept only in memory stays, one
    /// both have is memory's.
    pub fn take_loaded(&mut self, read: BTreeMap<String, Vec<Entry>>) {
        for (day, entries) in read {
            let mine = self.days.entry(day.clone()).or_default();
            let mut added = false;
            for e in entries {
                let have = mine
                    .iter()
                    .any(|m| m.source == e.source && m.id == e.id && m.at == e.at);
                if !have {
                    mine.push(e);
                    added = true;
                }
            }
            if added {
                mine.sort_by_key(|e| e.at);
            }
        }
        self.loaded = true;
    }

    /// The changed days as they are now, to write, and taken as written.
    pub fn take_changed(&mut self) -> Vec<(String, Vec<Entry>)> {
        let changed = std::mem::take(&mut self.changed);
        changed
            .into_iter()
            .map(|d| {
                let entries = self.days.get(&d).cloned().unwrap_or_default();
                (d, entries)
            })
            .collect()
    }
}

/// Where the days are kept.
pub fn dir() -> PathBuf {
    crate::paths::state_dir().join("notifications")
}

/// Reads every day's file in `dir`; a file that can't be read as entries is
/// passed over.
pub fn read(dir: &Path) -> BTreeMap<String, Vec<Entry>> {
    let mut out = BTreeMap::new();
    let Ok(files) = std::fs::read_dir(dir) else {
        return out;
    };
    for f in files.flatten() {
        let path = f.path();
        let Some(day) = path
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(|n| n.strip_suffix(".json"))
        else {
            continue;
        };
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        if let Ok(entries) = serde_json::from_str::<Vec<Entry>>(&text) {
            out.insert(day.to_string(), entries);
        }
    }
    out
}

/// Writes the changed days into `dir`, a day with no entries removed, and
/// removes the files of the days let go.
pub fn write(dir: &Path, days: &[(String, Vec<Entry>)], gone: &[String]) {
    for (day, entries) in days {
        let path = dir.join(format!("{day}.json"));
        if entries.is_empty() {
            let _ = std::fs::remove_file(&path);
            continue;
        }
        if let Ok(text) = serde_json::to_string_pretty(entries)
            && let Err(e) = crate::util::write_atomic(&path, text)
        {
            eprintln!("switchboard server: notifications history: {e}");
        }
    }
    for day in gone {
        let _ = std::fs::remove_file(dir.join(format!("{day}.json")));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const UTC: jiff::tz::TimeZone = jiff::tz::TimeZone::UTC;
    /// 2026-11-11T09:00:00Z.
    const NOW: i64 = 1_794_387_600_000;

    fn item(id: &str, title: &str) -> Item {
        Item::new(id, title)
    }

    #[test]
    fn a_new_item_becomes_an_entry_once() {
        let mut h = History::default();
        assert!(h.record("slack", &[item("a", "Hi")], NOW, &UTC));
        assert!(!h.record("slack", &[item("a", "Hi")], NOW, &UTC));
        let e = &h.days["2026-11-11"];
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].sidebar, Sidebar::Shown);
        assert_eq!(e[0].at, NOW);
        assert_eq!(h.take_changed().len(), 1);
    }

    #[test]
    fn an_entry_goes_on_the_day_of_its_own_time() {
        let mut h = History::default();
        let mut old = item("a", "Hi");
        old.time = Some(NOW - 86_400_000);
        h.record("slack", &[old], NOW, &UTC);
        assert!(h.days.contains_key("2026-11-10"));
    }

    #[test]
    fn one_leaving_its_list_is_older_and_a_dismissed_one_stays_dismissed() {
        let mut h = History::default();
        h.record("inbox", &[item("a", "A"), item("b", "B")], NOW, &UTC);
        assert!(h.mark("inbox", "a", Sidebar::Dismissed, false));
        h.record("inbox", &[], NOW, &UTC);
        let e = &h.days["2026-11-11"];
        assert_eq!(e[0].sidebar, Sidebar::Dismissed);
        assert_eq!(e[1].sidebar, Sidebar::Older);
        // Another source's list leaves these alone.
        h.record("slack", &[], NOW, &UTC);
        assert_eq!(h.days["2026-11-11"][0].sidebar, Sidebar::Dismissed);
    }

    #[test]
    fn one_back_in_its_list_is_shown_again() {
        let mut h = History::default();
        h.record("slack", &[item("a", "Hi")], NOW, &UTC);
        h.record("slack", &[], NOW, &UTC);
        h.record("slack", &[item("a", "Hi")], NOW, &UTC);
        assert_eq!(h.days["2026-11-11"][0].sidebar, Sidebar::Shown);
        // Dismissed and back unchanged is shown again (a dismissal its
        // source forgot); changed once put away, a new one.
        h.mark("slack", "a", Sidebar::Dismissed, false);
        h.record("slack", &[item("a", "Hi")], NOW, &UTC);
        assert_eq!(h.days["2026-11-11"][0].sidebar, Sidebar::Shown);
        h.mark("slack", "a", Sidebar::Dismissed, false);
        h.record("slack", &[item("a", "Hi, and more")], NOW, &UTC);
        let day = &h.days["2026-11-11"];
        assert_eq!(
            (day[0].sidebar, day[1].sidebar),
            (Sidebar::Dismissed, Sidebar::Shown)
        );
        assert_eq!(day[1].item.title, "Hi, and more");
    }

    #[test]
    fn one_dismissed_and_given_again_at_a_new_time_is_a_new_occurrence() {
        let mut h = History::default();
        let mut failing = item("failing:pr", "Checks failed on PR 1");
        failing.time = Some(NOW - 86_400_000);
        h.record("prs", &[failing.clone()], NOW, &UTC);
        h.mark("prs", "failing:pr", Sidebar::Dismissed, false);
        h.led_to("prs", "failing:pr", None, "you went to tab 5", NOW);
        h.record("prs", &[], NOW, &UTC);
        failing.time = Some(NOW);
        h.record("prs", &[failing], NOW, &UTC);
        // The earlier one kept as it was.
        let was = &h.days["2026-11-10"][0];
        assert_eq!(was.sidebar, Sidebar::Dismissed);
        assert!(was.replaced && was.led_to.is_some());
        let e = &h.days["2026-11-11"][0];
        assert_eq!(
            (e.sidebar, e.at, e.led_to.clone()),
            (Sidebar::Shown, NOW, None)
        );
        // Marking and what it led to follow the new one.
        h.mark("prs", "failing:pr", Sidebar::Dismissed, false);
        assert_eq!(h.days["2026-11-11"][0].sidebar, Sidebar::Dismissed);
        h.record("prs", &[], NOW, &UTC);
        assert_eq!(h.days["2026-11-11"].len(), 1);
    }

    #[test]
    fn occurrences_are_their_own_through_a_reload_and_while_shown() {
        let mut h = History::default();
        let mut failing = item("failing:pr", "Checks failed on PR 1");
        failing.time = Some(NOW - 10);
        h.record("prs", &[failing.clone()], NOW, &UTC);
        // Failing again before the first was put away.
        failing.time = Some(NOW);
        h.record("prs", &[failing], NOW, &UTC);
        let day = h.days["2026-11-11"].clone();
        assert_eq!((day[0].sidebar, day[0].replaced), (Sidebar::Older, true));
        assert_eq!((day[1].sidebar, day[1].at), (Sidebar::Shown, NOW));
        // What the older row led to stays on it.
        h.led_to(
            "prs",
            "failing:pr",
            Some(NOW - 10),
            "opened in the browser",
            NOW,
        );
        assert!(h.days["2026-11-11"][0].led_to.is_some());
        assert!(h.days["2026-11-11"][1].led_to.is_none());
        // Both read back.
        let mut fresh = History::default();
        fresh.take_loaded(h.days.clone());
        assert_eq!(fresh.days["2026-11-11"].len(), 2);
    }

    #[test]
    fn a_title_told_anew_while_shown_is_the_same_one() {
        let mut h = History::default();
        let mut meeting = item("m1", "Standup in 5m");
        meeting.time = Some(NOW);
        h.record("meeting", &[meeting.clone()], NOW, &UTC);
        meeting.title = "Standup now".into();
        h.record("meeting", &[meeting], NOW, &UTC);
        let day = &h.days["2026-11-11"];
        assert_eq!(day.len(), 1);
        assert_eq!(day[0].item.title, "Standup now");
    }

    #[test]
    fn a_timed_one_back_with_new_words_is_the_same_occurrence() {
        let mut h = History::default();
        let mut meeting = item("m1", "Standup in 5m");
        meeting.time = Some(NOW);
        h.record("meeting", &[meeting.clone()], NOW, &UTC);
        // Gone a moment (a restart before the feed answers), back counted down.
        h.record("meeting", &[], NOW, &UTC);
        meeting.title = "Standup in 2m".into();
        h.record("meeting", &[meeting], NOW, &UTC);
        let day = &h.days["2026-11-11"];
        assert_eq!(day.len(), 1);
        assert_eq!(
            (day[0].sidebar, day[0].item.title.as_str()),
            (Sidebar::Shown, "Standup in 2m")
        );
    }

    #[test]
    fn a_new_title_without_a_time_is_a_new_occurrence() {
        let mut h = History::default();
        h.record("autopilot", &[item("said", "Nothing left")], NOW, &UTC);
        h.record("autopilot", &[], NOW, &UTC);
        h.record(
            "autopilot",
            &[item("said", "Stopped for the spend")],
            NOW + 1,
            &UTC,
        );
        let day = &h.days["2026-11-11"];
        assert_eq!(day.len(), 2);
        assert_eq!(
            (day[0].item.title.as_str(), day[0].sidebar),
            ("Nothing left", Sidebar::Older)
        );
        assert_eq!(day[1].item.title, "Stopped for the spend");
    }

    #[test]
    fn a_late_read_takes_the_place_of_dismissed() {
        let mut h = History::default();
        h.record("inbox", &[item("a", "A")], NOW, &UTC);
        h.mark("inbox", "a", Sidebar::Dismissed, false);
        assert!(!h.mark("inbox", "a", Sidebar::Read, false));
        assert!(h.mark("inbox", "a", Sidebar::Read, true));
        assert!(!h.mark("inbox", "a", Sidebar::Read, true));
        assert_eq!(h.days["2026-11-11"][0].sidebar, Sidebar::Read);
    }

    #[test]
    fn led_to_sets_the_words() {
        let mut h = History::default();
        h.record("slack", &[item("a", "Hi")], NOW, &UTC);
        h.take_changed();
        assert!(h.led_to("slack", "a", None, "you replied", NOW + 5));
        assert_eq!(
            h.days["2026-11-11"][0].led_to,
            Some(("you replied".to_string(), NOW + 5))
        );
        assert!(!h.led_to("slack", "nope", None, "x", NOW));
        assert_eq!(h.take_changed().len(), 1);
    }

    #[test]
    fn days_older_than_the_kept_ones_are_let_go() {
        let mut h = History::default();
        for back in [0, 13, 14, 20] {
            let mut i = item(&format!("{back}"), "x");
            i.time = Some(NOW - back * 86_400_000);
            h.record("slack", &[i], NOW, &UTC);
        }
        let gone = h.prune("2026-11-11");
        assert_eq!(gone, ["2026-10-22", "2026-10-28"]);
        assert_eq!(
            h.days.keys().cloned().collect::<Vec<_>>(),
            ["2026-10-29", "2026-11-11"]
        );
    }

    #[test]
    fn days_are_written_and_read_back_and_taken_in_with_memory() {
        let dir = std::env::temp_dir().join(format!("sb-history-{}", std::process::id()));
        let mut h = History::default();
        h.record("slack", &[item("a", "Hi")], NOW, &UTC);
        write(&dir, &h.take_changed(), &[]);
        let read_back = read(&dir);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(read_back["2026-11-11"], h.days["2026-11-11"]);
        let mut fresh = History::default();
        fresh.record("inbox", &[item("b", "B")], NOW + 1, &UTC);
        fresh.take_loaded(read_back);
        let ids: Vec<&str> = fresh.days["2026-11-11"]
            .iter()
            .map(|e| e.id.as_str())
            .collect();
        assert_eq!(ids, ["a", "b"]);
        assert!(fresh.loaded);
    }

    #[test]
    fn the_kinds_and_their_keys() {
        assert_eq!(kind_of("autolanded"), Some(Kind::Landed));
        assert_eq!(kind_of("meeting"), Some(Kind::Calendar));
        assert_eq!(kind_of("habits"), None);
        assert_eq!(Kind::Prs.key(), 3);
        assert_eq!(Kind::of_key(3), Some(Kind::Prs));
        assert_eq!(Kind::of_key(1), None);
    }
}
