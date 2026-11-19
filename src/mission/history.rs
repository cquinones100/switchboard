//! The Notifications view as a history by day (`sidebar::history`): every
//! kept notification, newest first, under its day ("Today", "Yesterday",
//! then the weekday), narrowed to one kind by 2-7 (1 all), each carrying
//! what it led to and whether the sidebar still shows it. The days before
//! yesterday past the first screenful fold into one row each ("▸ WEDNESDAY
//! 7 more, folded") until "Show them" in its Enter menu unfolds one.
//!
//! With Settings' "Group notifications by source" on (`notices_grouped`,
//! board 115) it lists them by where they come from instead (`grouped`):
//! a heading for each source (`sidebar::notices::group_of`) the cursor rests
//! on, the sources ordered by their newest notification and each one's
//! newest first, one of more than five split by its channel, repository or
//! routine under a dim sub-heading, and a folded one its heading alone.
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

/// The part of the notifications naming none, apart from any named Other.
const OTHER: &str = "\u{0}other";

/// The prefix of a source's heading's id, and of a sub-heading's.
pub const GROUP: &str = "group:";
const PART: &str = "part:";

/// The source a row is the heading of, with grouping on.
pub fn group_heading(item: &Item) -> Option<&str> {
    // By its mark too, since a plugin's row may have any id.
    let rests = item.data.get("rests").and_then(Value::as_bool) == Some(true);
    item.id.strip_prefix(GROUP).filter(|_| rests)
}

/// How many notifications a list by source holds, a folded source's and
/// no sub-heading counted: the sum of its headings' counts; `None` for a
/// list with no source's heading, and for one under a filter, whose rows
/// are the ones it matched (none folded or split there).
pub fn held(items: &[Item]) -> Option<usize> {
    if items.iter().any(under_filter) {
        return None;
    }
    let counts: Vec<usize> = items
        .iter()
        .filter(|i| group_heading(i).is_some())
        .map(|i| i.data.get("count").and_then(Value::as_u64).unwrap_or(0) as usize)
        .collect();
    (!counts.is_empty()).then(|| counts.iter().fold(0usize, |a, c| a.saturating_add(*c)))
}

/// Whether a source's heading is folded.
pub fn folded(item: &Item) -> bool {
    item.data.get("folded").and_then(Value::as_bool) == Some(true)
}

/// Whether a source's heading is listed under a filter, which folds none.
pub fn under_filter(item: &Item) -> bool {
    item.data.get("whole").and_then(Value::as_bool) == Some(true)
}

/// Whether a row is a sub-heading within a source (a channel, a
/// repository), which the cursor passes over.
pub fn is_part(item: &Item) -> bool {
    item.id.starts_with(PART) && item.data.get("part").and_then(Value::as_bool) == Some(true)
}

/// The source a notification's row is listed under.
pub fn group_of_row(item: &Item) -> &'static str {
    let source = item.id.split_once('\t').map_or("", |(source, _)| source);
    crate::sidebar::notices::group_name(source)
}

/// Notifications (each id `<source>\t<id>`, as the view has them) by where
/// they come from: a heading per source with its count, the sources in the
/// order of their newest notification, each one's rows newest first. A
/// source in `folded` keeps its heading alone, and one of more than
/// `SPLIT_OVER` whose rows name two parts or more (`notices::part_of`) lists
/// them under a sub-heading each, those with none under Other. `whole` (a
/// filter's text is on) folds and splits nothing, so the filter finds every
/// row and leaves out the headings with none.
pub fn grouped(mut rows: Vec<Item>, folded: &HashSet<String>, whole: bool) -> Vec<Item> {
    use crate::sidebar::notices::{SPLIT_OVER, part_label, part_name, part_of};
    rows.sort_by_key(|r| std::cmp::Reverse(r.time.unwrap_or(0)));
    let mut groups: Vec<(&'static str, Vec<Item>)> = Vec::new();
    for row in rows {
        let name = group_of_row(&row);
        match groups.iter_mut().find(|(g, _)| *g == name) {
            Some((_, list)) => list.push(row),
            None => groups.push((name, vec![row])),
        }
    }
    let mut out = Vec::new();
    for (name, mut list) in groups {
        let is_folded = !whole && folded.contains(name);
        let mut heading = views::header(name, name, list.len());
        heading.id = format!("{GROUP}{name}");
        heading.group = Some(name.to_string());
        heading.data["rests"] = json!(true);
        heading.data["folded"] = json!(is_folded);
        // A filter looks through every source, so none folds under one.
        heading.data["whole"] = json!(whole);
        // When its newest came, folded or not, for its menu.
        heading.data["newest"] = json!(list.first().and_then(|r| r.time));
        out.push(heading);
        if is_folded {
            continue;
        }
        for row in &mut list {
            row.group = Some(name.to_string());
        }
        let part = |row: &Item| {
            let source = row.id.split_once('\t').map_or("", |(source, _)| source);
            part_of(source, row)
        };
        // Split only where two or more parts are named (those naming none
        // under Other, last); else the rows stay newest first as they are.
        let mut parts: Vec<(String, Vec<Item>)> = Vec::new();
        let named: Vec<String> = {
            let mut seen: Vec<String> = Vec::new();
            for p in list.iter().filter_map(part) {
                if !seen.contains(&p) {
                    seen.push(p);
                }
            }
            seen
        };
        if !whole && list.len() > SPLIT_OVER && named.len() >= 2 {
            let mut other: Vec<Item> = Vec::new();
            for row in list.drain(..) {
                match part(&row) {
                    Some(name) => match parts.iter_mut().find(|(p, _)| *p == name) {
                        Some((_, held)) => held.push(row),
                        None => parts.push((name, vec![row])),
                    },
                    None => other.push(row),
                }
            }
            if !other.is_empty() {
                parts.push((OTHER.to_string(), other));
            }
        }
        // Each part named as its newest row names it.
        let source_of = |row: &Item| {
            row.id
                .split_once('\t')
                .map_or("", |(source, _)| source)
                .to_string()
        };
        let names: Vec<String> = parts
            .iter()
            .map(|(key, held)| {
                if key == OTHER {
                    return "Other".to_string();
                }
                held.first()
                    .and_then(|r| part_name(&source_of(r), r))
                    .unwrap_or_else(|| key.clone())
            })
            .collect();
        let all: Vec<&str> = names.iter().map(String::as_str).collect();
        for ((part, held), called) in parts.into_iter().zip(&names) {
            let source = match held.first() {
                Some(row) => source_of(row),
                None => String::new(),
            };
            let label = part_label(&source, called, &all);
            let mut sub = Item::new(format!("{PART}{name}:{part}"), label);
            sub.group = Some(name.to_string());
            sub.data = json!({ "part": true, "count": held.len() });
            out.push(sub);
            out.extend(held);
        }
        out.extend(list);
    }
    out
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

/// A notification as a folded source's heading counts it, by its id and
/// time alone: its words and data are never drawn there, so they aren't
/// copied.
pub fn folded_row_of(e: &Entry) -> Item {
    let mut item = Item::new(format!("{}\t{}", e.source, e.id), "");
    item.time = Some(e.at);
    item
}

/// A notification as the view's row.
pub fn row_of(e: &Entry) -> Item {
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

    fn titles(rows: &[Item]) -> Vec<&str> {
        rows.iter().map(|r| r.title.as_str()).collect()
    }

    #[test]
    fn sources_are_headings_ordered_by_their_newest() {
        let old_slack = entry("slack", "a", NOW - 3 * DAY, Sidebar::Older);
        let landed = entry("autolanded", "b", NOW - DAY, Sidebar::Shown);
        let said = entry("autopilot", "c", NOW - 2 * DAY, Sidebar::Older);
        let meeting = entry("meeting", "d", NOW, Sidebar::Shown);
        let change = entry("calendar", "e", NOW - 4 * DAY, Sidebar::Older);
        let rows: Vec<Item> = [&old_slack, &landed, &said, &meeting, &change]
            .into_iter()
            .map(row_of)
            .collect();
        let out = grouped(rows.clone(), &HashSet::new(), false);
        assert_eq!(
            titles(&out),
            [
                "Calendar",
                "meeting d",
                "calendar e",
                "Autopilot",
                "autolanded b",
                "autopilot c",
                "Slack",
                "slack a",
            ]
        );
        assert_eq!(group_heading(&out[0]), Some("Calendar"));
        assert_eq!(out[0].data["count"], 2);
        assert!(views::is_header(&out[0]) && !views::passed_over(&out[0], false));
        assert_eq!(out[1].group.as_deref(), Some("Calendar"));
        assert_eq!(out[1].id, "meeting\td", "a row keeps the id its menu reads");
        // A folded source is its heading alone, until a filter looks through it.
        let folds: HashSet<String> = ["Autopilot".to_string()].into();
        let out = grouped(rows.clone(), &folds, false);
        assert_eq!(
            titles(&out),
            [
                "Calendar",
                "meeting d",
                "calendar e",
                "Autopilot",
                "Slack",
                "slack a"
            ]
        );
        assert!(folded(&out[3]) && !folded(&out[0]));
        assert_eq!(out[3].data["count"], 2);
        assert_eq!(grouped(rows, &folds, true).len(), 8);
    }

    #[test]
    fn a_source_of_more_than_five_is_split_by_channel() {
        let message = |id: &str, place: &str, back: i64| {
            let mut e = entry("slack", id, NOW - back, Sidebar::Shown);
            e.item.data = json!({ "where": place });
            row_of(&e)
        };
        let five: Vec<Item> = (0..5)
            .map(|i| message(&format!("m{i}"), "#general", i))
            .collect();
        let out = grouped(five.clone(), &HashSet::new(), false);
        assert_eq!(out.len(), 6, "five are listed as they are");
        let mut six = five;
        six.push(message("dm", "Ada", 2));
        let out = grouped(six.clone(), &HashSet::new(), false);
        assert_eq!(
            titles(&out),
            [
                "Slack", "#general", "slack m0", "slack m1", "slack m2", "slack m3", "slack m4",
                "Ada", "slack dm",
            ]
        );
        assert!(is_part(&out[1]) && views::passed_over(&out[1], false));
        assert_eq!(out[1].data["count"], 5);
        assert_eq!(out[7].data["count"], 1);
        // The view's count is its notifications', folded or not.
        assert_eq!(held(&out), Some(6));
        let folds: HashSet<String> = ["Slack".to_string()].into();
        assert_eq!(held(&grouped(six.clone(), &folds, false)), Some(6));
        assert_eq!(held(&[Item::new("part:release", "a plugin's row")]), None);
        assert_eq!(held(&grouped(six.clone(), &folds, true)), None);
        assert!(!is_part(&Item::new("part:release", "a plugin's row")));
        // One channel alone needs no sub-heading, nor does a filter's list.
        let one: Vec<Item> = (0..6)
            .map(|i| message(&format!("m{i}"), "#general", i))
            .collect();
        assert_eq!(grouped(one, &HashSet::new(), false).len(), 7);
        assert_eq!(grouped(six, &HashSet::new(), true).len(), 7);
        // A message naming none after the newest keeps the order newest
        // first, unsplit.
        let mut mixed: Vec<Item> = vec![message("a0", "#general", 0), message("x", "", 1)];
        mixed.extend((2..6).map(|i| message(&format!("a{i}"), "#general", i)));
        let out = grouped(mixed, &HashSet::new(), false);
        assert_eq!(out[2].title, "slack x", "{:?}", titles(&out));
        // One channel and a message naming none: no split either.
        let mut one: Vec<Item> = (0..5)
            .map(|i| message(&format!("m{i}"), "#general", i))
            .collect();
        one.push(message("x", "", 9));
        assert_eq!(grouped(one, &HashSet::new(), false).len(), 7);
    }

    #[test]
    fn a_fork_and_its_upstream_are_split_apart() {
        let pr = |id: &str, repo: &str, back: i64| {
            let mut e = entry("prs", id, NOW - back, Sidebar::Shown);
            e.item.data = json!({ "pr": { "number": 1, "repo": repo } });
            row_of(&e)
        };
        let mut rows: Vec<Item> = (0..3)
            .map(|i| pr(&format!("u{i}"), "team/app", i))
            .collect();
        rows.extend((0..3).map(|i| pr(&format!("f{i}"), "me/app", 10 + i)));
        let out = grouped(rows, &HashSet::new(), false);
        let parts: Vec<&str> = out
            .iter()
            .filter(|r| is_part(r))
            .map(|r| r.title.as_str())
            .collect();
        assert_eq!(parts, ["team/app", "me/app"]);
        // Alone of its name, a repository is named without its owner.
        let mut rows: Vec<Item> = (0..3)
            .map(|i| pr(&format!("u{i}"), "team/app", i))
            .collect();
        rows.extend((0..3).map(|i| pr(&format!("t{i}"), "team/tools", 10 + i)));
        let out = grouped(rows, &HashSet::new(), false);
        let parts: Vec<&str> = out
            .iter()
            .filter(|r| is_part(r))
            .map(|r| r.title.as_str())
            .collect();
        assert_eq!(parts, ["app", "tools"]);
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
