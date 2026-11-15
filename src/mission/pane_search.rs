//! M-j's picker finding a tab by what its panes show as well as by its name
//! (Settings' `tab_search_panes`): the panes' text as the server copied it,
//! the tabs whose lines hold what was typed, and the picker's rows put in
//! the order the search ranks them, each tab's matching lines under its row.
//! New with the Rust port; the tmux version's `choose-tree` matched names
//! alone.
//!
//! The server copies each pane's screen and the last lines of its
//! scrollback (`tab_search_lines`) as plain lines, keeping each copy while
//! the pane's output hasn't moved, and runs `search` off its task, so a
//! draw never waits on it. A tab found by name ranks above one found only
//! by what its panes show, those by how many lines matched. Nothing here
//! is written anywhere: the copies live as long as the picker.

use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;

use serde_json::{Value, json};

use super::views;
use crate::items::{Item, Target};
use crate::mux::{PaneId, TabId};
use crate::ui::draw::width;

/// How many of a tab's matching lines show under its row.
pub const SHOWN: usize = 2;

/// A pane's text as copied for a search, its screen's last line last.
#[derive(Clone, Debug)]
pub struct Copied {
    pub tab: TabId,
    pub pane: PaneId,
    pub lines: Arc<Vec<String>>,
}

/// What a tab's panes showed of the text searched for: the pane that
/// showed it last, the last `SHOWN` lines that hold it (in the order the
/// pane shows them), and how many lines in all.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    pub pane: PaneId,
    pub lines: Vec<String>,
    pub count: usize,
}

/// How many lines `tab_search_lines` reads above each pane's screen.
pub fn lines_setting(value: Option<&Value>) -> usize {
    match value.and_then(Value::as_str) {
        Some("200") => 200,
        Some("all") => usize::MAX,
        _ => 1000,
    }
}

/// The tabs whose panes show `text` (lowercased), with what each showed.
/// Panes are taken in their ids' order, so a tab's pane is the same from
/// one search to the next.
pub fn search(copies: &[Copied], text: &str) -> HashMap<TabId, Found> {
    let mut out: HashMap<TabId, Found> = HashMap::new();
    if text.is_empty() {
        return out;
    }
    let mut copies: Vec<&Copied> = copies.iter().collect();
    copies.sort_by_key(|c| c.pane);
    for copy in copies {
        let hits: Vec<&String> = copy
            .lines
            .iter()
            .filter(|l| l.to_lowercase().contains(text))
            .collect();
        if hits.is_empty() {
            continue;
        }
        let found = out.entry(copy.tab).or_insert_with(|| Found {
            pane: copy.pane,
            lines: Vec::new(),
            count: 0,
        });
        // The pane with the most recent match is the one the menu goes to:
        // the later pane, once it has any.
        found.pane = copy.pane;
        found.lines = hits
            .iter()
            .skip(hits.len().saturating_sub(SHOWN))
            .map(|l| l.trim().to_string())
            .collect();
        found.count += hits.len();
    }
    out
}

/// What was found for an earlier text, narrowed to what still holds
/// `text`, to show while the search for it runs: typing on narrows what
/// shows rather than emptying it for a moment.
pub fn narrowed(found: &HashMap<TabId, Found>, text: &str) -> HashMap<TabId, Found> {
    found
        .iter()
        .filter_map(|(tab, f)| {
            let lines: Vec<String> = f
                .lines
                .iter()
                .filter(|l| l.to_lowercase().contains(text))
                .cloned()
                .collect();
            (!lines.is_empty()).then(|| {
                let count = f.count.max(lines.len());
                (
                    *tab,
                    Found {
                        pane: f.pane,
                        lines,
                        count,
                    },
                )
            })
        })
        .collect()
}

/// The picker's rows for `text`: the Work map's open tabs whose name (or
/// what else the filter reads) holds it, in the map's order, then those
/// found only by what their panes show, the most matching lines first;
/// each one found in its panes carries `found` (its pane, its lines, how
/// many more) for its row and its menu.
pub fn found_items(items: &[Item], text: &str, found: &HashMap<TabId, Found>) -> Vec<Item> {
    let mut by_name = Vec::new();
    let mut by_content: Vec<(usize, Item)> = Vec::new();
    for item in items {
        let Target::Tab(tab) = item.target else {
            continue;
        };
        if views::is_header(item) {
            continue;
        }
        let named = views::search_text(item).contains(text);
        let hit = found.get(&tab);
        if !named && hit.is_none() {
            continue;
        }
        let mut item = item.clone();
        if let Some(f) = hit {
            if !item.data.is_object() {
                item.data = json!({});
            }
            item.data["found"] = json!({
                "pane": f.pane.0,
                "lines": f.lines,
                "more": f.count.saturating_sub(f.lines.len()),
                "text": text,
            });
        }
        if named {
            by_name.push(item);
        } else {
            by_content.push((hit.map_or(0, |f| f.count), item));
        }
    }
    by_content.sort_by_key(|(count, _)| std::cmp::Reverse(*count));
    by_name.extend(by_content.into_iter().map(|(_, i)| i));
    by_name
}

/// The lines a picker row shows under it, and how many more its tab holds.
pub fn shown_lines(item: &Item) -> (Vec<&str>, u64) {
    let found = &item.data["found"];
    let lines = found["lines"]
        .as_array()
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    (lines, found["more"].as_u64().unwrap_or(0))
}

/// The pane a row's match is in, for its menu's "Go to the match".
pub fn pane_of(item: &Item) -> Option<PaneId> {
    item.data["found"]["pane"]
        .as_u64()
        .and_then(|p| u32::try_from(p).ok())
        .map(PaneId)
}

/// The pane a row's match is in and the text found there, for its menu's
/// "Go to the match".
pub fn match_of(item: &Item) -> Option<(PaneId, String)> {
    let text = item.data["found"]["text"].as_str()?.to_string();
    Some((pane_of(item)?, text))
}

/// The rows a list item takes: its own and its matching lines.
pub fn height(item: &Item) -> usize {
    1 + shown_lines(item).0.len()
}

/// Which items a list of `room` rows shows with `cursor` in sight, when
/// some take more than one: the first shown and how many.
pub fn window(heights: &[usize], cursor: usize, room: usize) -> (usize, usize) {
    if heights.is_empty() {
        return (0, 0);
    }
    let cursor = cursor.min(heights.len() - 1);
    let mut first = cursor;
    let mut used = heights[cursor];
    while first > 0 && used + heights[first - 1] <= room {
        first -= 1;
        used += heights[first];
    }
    let mut count = cursor + 1 - first;
    while first + count < heights.len() && used + heights[first + count] <= room {
        used += heights[first + count];
        count += 1;
    }
    (first, count)
}

/// Where `needle` (lowercased) first is in `line`, matched without case,
/// by bytes of `line`.
pub fn find(line: &str, needle: &str) -> Option<Range<usize>> {
    if needle.is_empty() {
        return None;
    }
    let wanted: Vec<char> = needle.chars().collect();
    for (start, _) in line.char_indices() {
        let mut at = 0;
        let mut end = start;
        for c in line[start..].chars() {
            let lower: Vec<char> = c.to_lowercase().collect();
            if wanted.len() < at + lower.len() || wanted[at..at + lower.len()] != lower[..] {
                break;
            }
            at += lower.len();
            end += c.len_utf8();
            if at == wanted.len() {
                return Some(start..end);
            }
        }
    }
    None
}

/// A matching line cut to `room` columns around its match: what comes
/// before it, the match, and what comes after, an ellipsis where a side is
/// cut, the match kept in sight with some of what led up to it.
pub fn cut(line: &str, needle: &str, room: usize) -> (String, String, String) {
    let line = line.trim();
    let Some(r) = find(line, needle) else {
        return (head(line, room), String::new(), String::new());
    };
    let (before, hit, after) = (&line[..r.start], &line[r.clone()], &line[r.end..]);
    let hit = head(hit, room);
    let left = room.saturating_sub(width(&hit));
    // The match keeps a third of what's left before it, or more when what
    // follows it is short.
    let before_room = (left / 3).max(left.saturating_sub(width(after))).min(left);
    let before = tail(before, before_room);
    let after = head(after, left.saturating_sub(width(&before)));
    (before, hit, after)
}

/// The start of `s` in `room` columns, an ellipsis ending it when cut.
fn head(s: &str, room: usize) -> String {
    if width(s) <= room {
        return s.to_string();
    }
    let mut out = String::new();
    for c in s.chars() {
        let mut next = out.clone();
        next.push(c);
        if width(&next) + 1 > room {
            break;
        }
        out = next;
    }
    if room > 0 {
        out.push('…');
    }
    out
}

/// The end of `s` in `room` columns, an ellipsis starting it when cut.
fn tail(s: &str, room: usize) -> String {
    if width(s) <= room {
        return s.to_string();
    }
    let mut kept: Vec<char> = Vec::new();
    for c in s.chars().rev() {
        kept.push(c);
        let next: String = kept.iter().rev().collect();
        if width(&next) + 1 > room {
            kept.pop();
            break;
        }
    }
    if room == 0 {
        return String::new();
    }
    let mut out = String::from('…');
    out.extend(kept.iter().rev());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn copy(tab: u32, pane: u32, lines: &[&str]) -> Copied {
        Copied {
            tab: TabId(tab),
            pane: PaneId(pane),
            lines: Arc::new(lines.iter().map(|l| l.to_string()).collect()),
        }
    }

    fn row(tab: u32, name: &str) -> Item {
        Item::new(format!("t{tab}"), name).target(Target::Tab(TabId(tab)))
    }

    #[test]
    fn a_search_finds_lines_without_case_and_keeps_the_last_two() {
        let found = search(
            &[
                copy(
                    1,
                    1,
                    &["  Zebra one", "nothing", "zebra two", "ZEBRA three"],
                ),
                copy(2, 2, &["no match"]),
            ],
            "zebra",
        );
        assert_eq!(found.len(), 1);
        let f = &found[&TabId(1)];
        assert_eq!(f.count, 3);
        assert_eq!(f.lines, vec!["zebra two", "ZEBRA three"]);
        assert_eq!(f.pane, PaneId(1));
    }

    #[test]
    fn a_name_match_ranks_first_then_the_most_lines() {
        let items = vec![row(1, "alpha"), row(2, "zebra tab"), row(3, "gamma")];
        let mut found = HashMap::new();
        for (tab, count) in [(1, 1), (3, 5)] {
            found.insert(
                TabId(tab),
                Found {
                    pane: PaneId(tab),
                    lines: vec!["a zebra".into()],
                    count,
                },
            );
        }
        let out = found_items(&items, "zebra", &found);
        let ids: Vec<&str> = out.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(ids, vec!["t2", "t3", "t1"]);
        assert_eq!(shown_lines(&out[1]), (vec!["a zebra"], 4));
        assert_eq!(pane_of(&out[1]), Some(PaneId(3)));
        assert!(out[0].data.get("found").is_none());
    }

    #[test]
    fn a_line_is_cut_around_its_match() {
        let line = "the quick brown fox jumps over the lazy dog and keeps running";
        let (before, hit, after) = cut(line, "lazy", 24);
        assert_eq!(hit, "lazy");
        assert!(before.starts_with('…'), "{before}");
        assert!(after.ends_with('…'), "{after}");
        assert_eq!(width(&format!("{before}{hit}{after}")), 24);
        assert_eq!(
            cut("short zebra", "zebra", 40),
            ("short ".into(), "zebra".into(), String::new())
        );
        assert_eq!(cut("ÉCOLE here", "école", 40).1, "ÉCOLE");
    }

    #[test]
    fn a_window_keeps_the_cursor_in_sight() {
        assert_eq!(window(&[1, 3, 3, 1], 3, 4), (2, 2));
        assert_eq!(window(&[1, 3, 3, 1], 0, 4), (0, 2));
        assert_eq!(window(&[3], 0, 1), (0, 1));
    }
}
