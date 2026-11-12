//! The Notifications view as a history by day (`sidebar::history`): every
//! kept notification, newest first, under its day ("Today", "Yesterday",
//! then the weekday), narrowed to one kind by 2-7 (1 all), each carrying
//! what it led to and whether the sidebar still shows it. The days before
//! yesterday past the first screenful fold into one row each ("▸ WEDNESDAY
//! 7 more, folded") until "Show them" in its Enter menu unfolds one.
//!
//! Its items keep the shape the view's menus read: a notification's id is
//! `<source>\t<id>` and its data its source's, with the history's own
//! under `notice` (`sidebar`, `led_to`, `kind`).

use std::collections::HashSet;

use serde_json::{Value, json};

use super::views;
use crate::items::Item;
use crate::sidebar::history::{Entry, Kind, Sidebar, kind_of};

/// What the keycaps and the bar count: each key's notifications of the
/// last seven days (1 all, then each kind), how many that is, and how many
/// the sidebar still shows.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub keys: [usize; 7],
    pub week: usize,
    pub shown: usize,
}

/// The prefix of a folded day's id.
pub const FOLD: &str = "fold:";

/// The folded day a row is, by its date.
pub fn fold_of(item: &Item) -> Option<&str> {
    item.id.strip_prefix(FOLD)
}

/// A row's history: whether the sidebar shows it, what it led to, and its
/// kind's key; `None` for a day's heading or a fold.
pub fn notice_of(item: &Item) -> Option<(&str, &str)> {
    let n = item.data.get("notice")?;
    Some((
        n.get("sidebar").and_then(Value::as_str).unwrap_or(""),
        n.get("led_to").and_then(Value::as_str).unwrap_or(""),
    ))
}

/// Which occurrence of its notification a row is, by when it came.
pub fn occurrence_of(item: &Item) -> Option<i64> {
    item.data.pointer("/notice/at").and_then(Value::as_i64)
}

/// Whether the sidebar still shows a row's notification.
pub fn shown(item: &Item) -> bool {
    notice_of(item).is_some_and(|(s, _)| s == Sidebar::Shown.word())
}

/// The counts over the entries of the seven days up to `today`.
pub fn counts(entries: &[&Entry], today: jiff::civil::Date, tz: &jiff::tz::TimeZone) -> Counts {
    let mut c = Counts::default();
    let week_start = today
        .checked_sub(jiff::Span::new().days(6))
        .unwrap_or(today);
    for e in entries {
        if e.sidebar == Sidebar::Shown {
            c.shown += 1;
        }
        if date_of(e.at, tz).is_none_or(|d| d < week_start) {
            continue;
        }
        c.week += 1;
        c.keys[0] += 1;
        if let Some(k) = kind_of(&e.source) {
            c.keys[k.key() - 1] += 1;
        }
    }
    c
}

fn date_of(ms: i64, tz: &jiff::tz::TimeZone) -> Option<jiff::civil::Date> {
    jiff::Timestamp::from_millisecond(ms)
        .ok()
        .map(|t| t.to_zoned(tz.clone()).date())
}

/// A day's name as its heading says it: "Today", "Yesterday", the weekday
/// within the week, else the weekday and the date.
pub fn day_name(day: jiff::civil::Date, today: jiff::civil::Date) -> String {
    let back = (today - day).get_days();
    match back {
        0 => "Today".into(),
        1 => "Yesterday".into(),
        2..=6 => day.strftime("%A").to_string(),
        _ => day.strftime("%A %-d %b").to_string(),
    }
}

/// A notification as the view's row.
fn row_of(e: &Entry) -> Item {
    let mut item = e.item.clone();
    item.id = format!("{}\t{}", e.source, e.id);
    item.time = Some(e.at);
    item.group = None;
    let notice = json!({
        "sidebar": e.sidebar.word(),
        "led_to": e.led_to.as_ref().map(|(w, _)| w.as_str()).unwrap_or(""),
        "kind": kind_of(&e.source).map_or(1, Kind::key),
        "at": e.at,
    });
    match item.data.as_object_mut() {
        Some(o) => {
            // A tab named by one the sidebar no longer shows may be gone,
            // its id given to another since (a restart), so it leads to none.
            if e.sidebar != Sidebar::Shown {
                o.remove("tab");
            }
            o.insert("notice".into(), notice);
        }
        None => item.data = json!({ "notice": notice }),
    }
    item
}

/// The view's rows: the entries (newest first) of `kind` (all when `None`)
/// under their days; a day before yesterday that starts past `screenful`
/// rows, not in `unfolded`, one fold row.
pub fn rows(
    entries: &[&Entry],
    kind: Option<Kind>,
    unfolded: &HashSet<String>,
    today: jiff::civil::Date,
    tz: &jiff::tz::TimeZone,
    screenful: usize,
) -> Vec<Item> {
    let mut days: Vec<(jiff::civil::Date, Vec<&Entry>)> = Vec::new();
    for e in entries
        .iter()
        .filter(|e| kind.is_none() || kind_of(&e.source) == kind)
    {
        let Some(d) = date_of(e.at, tz) else { continue };
        match days.last_mut() {
            Some((day, list)) if *day == d => list.push(e),
            _ => days.push((d, vec![e])),
        }
    }
    let mut out = Vec::new();
    for (day, list) in days {
        let date = day.to_string();
        let name = day_name(day, today);
        let before_yesterday = (today - day).get_days() >= 2;
        if before_yesterday && out.len() >= screenful && !unfolded.contains(&date) {
            let mut fold = Item::new(format!("{FOLD}{date}"), name);
            fold.detail = format!("{} more, folded", list.len());
            fold.data = json!({ "fold": date });
            out.push(fold);
            continue;
        }
        out.push(views::header(&date, &name, list.len()));
        out.extend(list.into_iter().map(row_of));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const UTC: jiff::tz::TimeZone = jiff::tz::TimeZone::UTC;
    /// 2026-11-11T09:00:00Z, a Wednesday.
    const NOW: i64 = 1_794_387_600_000;
    const DAY: i64 = 86_400_000;

    fn today() -> jiff::civil::Date {
        jiff::civil::date(2026, 11, 11)
    }

    fn entry(source: &str, id: &str, at: i64, sidebar: Sidebar) -> Entry {
        Entry {
            source: source.into(),
            id: id.into(),
            at,
            item: Item::new(id, format!("{source} {id}")),
            sidebar,
            led_to: None,
            replaced: false,
        }
    }

    #[test]
    fn rows_sit_under_their_days_newest_first_with_their_state() {
        let a = entry("slack", "a", NOW, Sidebar::Shown);
        let mut b = entry("prs", "b", NOW - DAY, Sidebar::Dismissed);
        b.led_to = Some(("you went to tab 5".into(), NOW));
        let all = [&a, &b];
        let rows = rows(&all, None, &HashSet::new(), today(), &UTC, 50);
        let titles: Vec<&str> = rows.iter().map(|r| r.title.as_str()).collect();
        assert_eq!(titles, ["Today", "slack a", "Yesterday", "prs b"]);
        assert_eq!(rows[1].id, "slack\ta");
        assert!(shown(&rows[1]));
        assert_eq!(
            notice_of(&rows[3]),
            Some(("dismissed", "you went to tab 5"))
        );
    }

    #[test]
    fn a_landing_out_of_the_sidebar_names_no_tab() {
        let mut gone = entry("autolanded", "abc", NOW, Sidebar::Dismissed);
        gone.item.data = json!({ "tab": "@3", "plan": "" });
        let mut shown = entry("autolanded", "def", NOW - 1, Sidebar::Shown);
        shown.item.data = json!({ "tab": "@4" });
        let rows = rows(&[&gone, &shown], None, &HashSet::new(), today(), &UTC, 50);
        assert!(rows[1].data.get("tab").is_none());
        assert_eq!(rows[2].data["tab"], "@4");
    }

    #[test]
    fn a_kind_leaves_only_its_own() {
        let a = entry("slack", "a", NOW, Sidebar::Shown);
        let b = entry("prs", "b", NOW, Sidebar::Shown);
        let all = [&a, &b];
        let rows = rows(&all, Some(Kind::Prs), &HashSet::new(), today(), &UTC, 50);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1].id, "prs\tb");
    }

    #[test]
    fn days_before_yesterday_past_a_screenful_fold_until_unfolded() {
        let a = entry("slack", "a", NOW, Sidebar::Shown);
        let b = entry("slack", "b", NOW - 3 * DAY, Sidebar::Older);
        let c = entry("slack", "c", NOW - 3 * DAY - 1, Sidebar::Older);
        let all = [&a, &b, &c];
        let folded = rows(&all, None, &HashSet::new(), today(), &UTC, 2);
        assert_eq!(folded.len(), 3);
        assert_eq!(fold_of(&folded[2]), Some("2026-11-08"));
        assert_eq!(folded[2].title, "Sunday");
        assert_eq!(folded[2].detail, "2 more, folded");
        let open: HashSet<String> = ["2026-11-08".to_string()].into();
        assert_eq!(rows(&all, None, &open, today(), &UTC, 2).len(), 5);
    }

    #[test]
    fn the_counts_are_the_weeks() {
        let a = entry("slack", "a", NOW, Sidebar::Shown);
        let b = entry("autolanded", "b", NOW - 2 * DAY, Sidebar::Older);
        let old = entry("slack", "c", NOW - 9 * DAY, Sidebar::Older);
        let c = counts(&[&a, &b, &old], today(), &UTC);
        assert_eq!(c.keys, [2, 1, 0, 1, 0, 0, 0]);
        assert_eq!((c.week, c.shown), (2, 1));
    }

    #[test]
    fn day_names() {
        assert_eq!(day_name(today(), today()), "Today");
        assert_eq!(
            day_name(jiff::civil::date(2026, 11, 10), today()),
            "Yesterday"
        );
        assert_eq!(day_name(jiff::civil::date(2026, 11, 9), today()), "Monday");
        assert_eq!(
            day_name(jiff::civil::date(2026, 11, 1), today()),
            "Sunday 1 Nov"
        );
    }
}
