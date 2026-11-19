//! The sidebar's notifications as a list of their own, in a tab of its own:
//! the Notifications window (lib/sidebar/daemon.js's `noticesWindow`, its
//! notices client and `noticesKey`, render.js's `noticeListLines`). Moving
//! the sidebar's cursor onto the notifications brings it to the front as
//! moving onto a tab does, with every notification whole (a Slack message
//! as Slack draws it, an Inbox item's comment), the one the cursor is on
//! chosen; Enter there opens its menu, and Esc goes back to the tab it was
//! brought in front of (the notice_list setting).
//!
//! The window is a tab marked `@notices_window`, which the tab bar shows
//! only while it is in front and the sidebar, mission control and the tab
//! numbers leave out, kept last among its session's tabs so it never takes
//! a number from one. Its one pane runs `switchboard notifications`, which
//! only waits: the server draws the list over that pane and takes its keys
//! itself (`serve/window.rs`), since the list is the server's own state.
//!
//! This module is the list's lines, a plain function of what is shown, and
//! the program the pane runs.

use std::collections::{HashMap, HashSet};
use std::process::ExitCode;

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;

use super::render::{Line, Owner};
use super::text::{self, width};
use crate::items::Item;
use crate::mission::views::ViewId;
use crate::mux::TabId;
use crate::mux::server::ClientId;
use crate::ui::Theme;
use crate::ui::kit::Mark;

/// The tab option that marks a session's Notifications window.
pub const MARK: &str = "@notices_window";

/// The kinds of notification by the source that gives them, in the order
/// they are shown (daemon.js's `notices`): the meeting about to start,
/// Slack's new messages, what changed on today's calendar, notes written
/// for you, the Linear inbox, your PRs' news, a default branch whose CI fails, the landings its tabs made without asking,
/// what autopilot has to say, the landed tabs it closed
/// without your seeing them, the tabs whose diff finished loading while
/// you were elsewhere, and suggestions. A source not named here goes
/// after them.
pub const ORDER: [&str; 14] = [
    "meeting",
    "slack",
    "calendar",
    "new-notes",
    "inbox",
    "prs",
    "ci",
    "autolanded",
    "autopilot",
    "landed",
    "ready",
    "routines",
    "habits",
    "behind",
];

/// The view a kind of notification is an item of, by its source: a Slack
/// message is the Slack view's, an Inbox notification the Inbox's, the
/// meeting about to start an event of the Calendar's, a PR's news the PR
/// board's, a failing default branch the CI view's, a routine's run that
/// failed or was skipped the Routines view's routine. Its menu is that
/// item's menu there (`mission::choices::notice_choices`), so a new kind
/// that a view lists names its view here and lists no choices of its own.
/// The kinds no view lists as an item are `NO_VIEW`'s.
pub fn view_of(source: &str) -> Option<ViewId> {
    match source {
        "slack" => Some(ViewId::Slack),
        "inbox" => Some(ViewId::Inbox),
        "meeting" => Some(ViewId::Calendar),
        "ci" => Some(ViewId::Ci),
        "prs" => Some(ViewId::Prs),
        "routines" => Some(ViewId::Routines),
        _ => None,
    }
}

/// The kinds no view lists as an item, which lead somewhere of their own
/// (`mission::choices::notice_leads`): a change on the calendar (the
/// Calendar lists the day's events, not what changed), a note written for
/// you, what autopilot said and the landings it tells of (its view's rows
/// are its tabs), a tab whose diff finished loading, which leads to the
/// tab, a suggestion, and main ahead of the running build
/// (src/mux/behind.rs), which leads to the restart.
pub const NO_VIEW: [&str; 8] = [
    "calendar",
    "new-notes",
    "autolanded",
    "autopilot",
    "landed",
    "ready",
    "habits",
    "behind",
];

/// Where a kind of notification comes from, the heading it is listed under
/// with Settings' "Group notifications by source" on (`notices_grouped`,
/// board 115): its view's name where a view lists it (`view_of`), else its
/// app's: Calendar for what changed on the calendar, Notes for a note
/// written for you, Autopilot for what it said and the landings it tells of,
/// Tabs for a tab whose diff finished loading, Switchboard for a suggestion and main ahead of the running build. A new
/// kind names its group here, which a test holds every kind of `ORDER` to.
pub fn group_of(source: &str) -> Option<&'static str> {
    match source {
        "slack" => Some("Slack"),
        "inbox" => Some("Inbox"),
        "ci" => Some("CI"),
        "prs" => Some("PRs"),
        "routines" => Some("Routines"),
        "meeting" | "calendar" => Some("Calendar"),
        "new-notes" => Some("Notes"),
        "autopilot" | "autolanded" | "landed" => Some("Autopilot"),
        "ready" => Some("Tabs"),
        "habits" | "behind" => Some("Switchboard"),
        _ => None,
    }
}

/// The group a notification is listed under: its kind's, and Switchboard
/// for a kind that names none.
pub fn group_name(source: &str) -> &'static str {
    group_of(source).unwrap_or("Switchboard")
}

/// What a group of more than `SPLIT_OVER` is split by, where a notification
/// has it: a Slack message's channel or DM, a PR's and a failing default
/// branch's repository (by its owner too, so a fork and its upstream stay
/// apart), a routine's name.
pub fn part_of(source: &str, item: &Item) -> Option<String> {
    let text = |pointer: &str| {
        item.data
            .pointer(pointer)
            .and_then(serde_json::Value::as_str)
            .filter(|s| !s.is_empty())
    };
    match source {
        // By the conversation, whose name a group DM's rows may each say
        // their own way.
        // A reminder is in no conversation.
        "slack" if text("/kind") == Some("reminder") => None,
        "slack" => text("/message/channel")
            .or_else(|| text("/where"))
            .map(str::to_string),
        "prs" => text("/pr/repo").map(str::to_string),
        "ci" => text("/mainCi/repo").map(str::to_string),
        "routines" => item.title.strip_prefix("Routine ").map(str::to_string),
        _ => None,
    }
}

/// What a notification calls its part: a Slack message's place by name,
/// else the part itself.
pub fn part_name(source: &str, item: &Item) -> Option<String> {
    match source {
        "slack" => item
            .data
            .pointer("/where")
            .and_then(serde_json::Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .or_else(|| part_of(source, item)),
        _ => part_of(source, item),
    }
}

/// A part's name as its sub-heading says it: a repository's without its
/// owner, unless another part of the group has the same name without it.
pub fn part_label(source: &str, part: &str, others: &[&str]) -> String {
    let short = |full: &str| full.rsplit('/').next().unwrap_or(full).to_string();
    if !matches!(source, "prs" | "ci") {
        return part.to_string();
    }
    let name = short(part);
    if others.iter().any(|o| *o != part && short(o) == name) {
        part.to_string()
    } else {
        name
    }
}

/// How many notifications a group holds before it is split by `part_of`.
pub const SPLIT_OVER: usize = 5;

/// A notification's words on its group's line in the sidebar: its title, a
/// Slack message's without its mark.
pub fn words(source: &str, item: &Item) -> String {
    match source {
        // A Slack reminder's own words, which its title doesn't hold.
        "slack"
            if item.data.get("kind").and_then(serde_json::Value::as_str) == Some("reminder")
                && !item.detail.is_empty() =>
        {
            format!("Reminder · {}", item.detail)
        }
        // Who wrote where, then what they wrote, so two messages from the
        // same person in one conversation read apart.
        "slack" => {
            let who = item
                .title
                .split_once(char::is_whitespace)
                .map_or(item.title.clone(), |(_, rest)| {
                    rest.trim_start().to_string()
                });
            if item.detail.is_empty() {
                who
            } else {
                format!("{who} · {}", item.detail)
            }
        }
        // A failing default branch's repository and why first, its title
        // after, so a narrow line keeps them; a routine's why after its
        // name.
        "ci" if !item.detail.is_empty() => format!("{} · {}", item.detail, item.title),
        // A meeting about to start and the tickets it names, without the
        // hint that Enter joins it, which a source's line doesn't do.
        "meeting" if !item.detail.is_empty() => {
            format!("{} · {}", without_join_hint(&item.title), item.detail)
        }
        "meeting" => without_join_hint(&item.title),
        // A note written for you and what wrote it, a landing made without
        // asking, as their kinds' lines said them, first so a narrow line
        // keeps them.
        "new-notes" => {
            let from = item
                .data
                .get("source")
                .and_then(serde_json::Value::as_str)
                .filter(|s| !s.is_empty());
            match from {
                Some(from) => format!("from {from} · {}", item.title),
                None => item.title.clone(),
            }
        }
        // Its hash and item first, as board 117 draws it, the qualifier
        // after them where the line has room.
        "autolanded" => format!("{} · without asking", item.title),
        // A PR's news and its title, as its kind's line had them.
        "prs" if !item.detail.is_empty() => format!("{} · {}", item.title, item.detail),
        // Who did what first, as its kind's line had it, then the ticket.
        "inbox" if !item.detail.is_empty() => {
            let what = item.detail.split(": ").next().unwrap_or("");
            format!("{what} · {}", item.title)
        }
        "routines" if !item.detail.is_empty() => {
            format!("{} · {}", item.title, item.detail)
        }
        _ => item.title.clone(),
    }
}

/// A meeting's title without the hint its kind's line gives that Enter
/// joins its call.
pub fn without_join_hint(title: &str) -> String {
    title.trim_end_matches(" · ↵ joins").to_string()
}

/// The row of the sidebar a kind of notification is on: its group's with
/// `notices_grouped` on, else its own.
pub fn row_of(source: &str) -> &str {
    if crate::settings::is_on("notices_grouped") {
        group_name(source)
    } else {
        source
    }
}

/// The id a notification's item has in its view, from the notification's
/// own: the meeting about to start by its event's (`calendar::meeting_notice`),
/// a PR's news by its PR's on the board (`prnotices::notice`), the rest
/// the same.
pub fn id_in_its_view(source: &str, id: &str) -> String {
    let own = match source {
        "meeting" => id.strip_prefix("meeting:"),
        "prs" => id
            .split_once(':')
            .filter(|(kind, _)| ["merged", "approved", "failing"].contains(kind))
            .map(|(_, board)| board),
        _ => None,
    };
    own.unwrap_or(id).to_string()
}

/// A notification as its view lists it, `item` under its own id: the
/// meeting about to start as its event, which it carries.
pub fn as_its_view_has_it(source: &str, item: &Item) -> Item {
    let mut own = item.clone();
    if source == "meeting" {
        own.data = item.data.get("event").cloned().unwrap_or_default();
    }
    own
}

/// A kind's mark in the sidebar, by its source and its first
/// notification's id (board 1): ◷ for the meeting about to start and what
/// changed on the calendar, @ for what someone sent you on Slack or Linear,
/// ✎ for a note written for you, ✓ for a landing, a tab ready and a PR merged or
/// approved, ✕ for checks that failed, a failing default branch and a
/// routine's run that failed or was skipped, and ◇ for what autopilot or a
/// suggestion has to say.
pub fn mark_of(source: &str, id: &str) -> Mark {
    match source {
        "meeting" | "calendar" => Mark::Reminder,
        "slack" | "inbox" => Mark::Mention,
        "new-notes" => Mark::Note,
        "autolanded" | "landed" | "ready" => Mark::Passed,
        "prs" if id.starts_with("failing:") => Mark::Failed,
        "prs" => Mark::Passed,
        "ci" | "routines" => Mark::Failed,
        _ => Mark::Comment,
    }
}

/// A source's place in `ORDER`.
pub fn rank(source: &str) -> usize {
    ORDER
        .iter()
        .position(|s| *s == source)
        .unwrap_or(ORDER.len())
}

/// A kind's one row in the sidebar, its title and the dim line under it,
/// from its notifications (daemon.js's `notices`): how many there are, and
/// the first of them; the Notifications window lists each.
pub fn summary(source: &str, items: &[Item]) -> (String, String) {
    let Some(first) = items.first() else {
        return (String::new(), String::new());
    };
    let n = items.len();
    let one = n == 1;
    let data = |key: &str| {
        first
            .data
            .get(key)
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .to_string()
    };
    match source {
        // The first message's sender and place, without its mark.
        "slack" => (
            format!("{n} new on Slack"),
            first
                .title
                .split_once(char::is_whitespace)
                .map_or(String::new(), |(_, rest)| rest.trim_start().to_string()),
        ),
        "calendar" if one => (format!("Calendar: {}", first.title), String::new()),
        "calendar" => (format!("{n} calendar changes"), first.title.clone()),
        "new-notes" => (
            if one {
                format!("New note: {}", first.title)
            } else {
                format!("{n} new notes")
            },
            format!("from {}", data("source")),
        ),
        "inbox" => (
            format!("{n} in your Linear inbox"),
            first.detail.split(": ").next().unwrap_or("").to_string(),
        ),
        "prs" if one => (first.title.clone(), first.detail.clone()),
        "prs" => (format!("{n} PR updates"), first.title.clone()),
        // The landings made without asking: the last one, when several.
        "autolanded" if one => (first.title.clone(), "without asking".into()),
        "autolanded" => (
            format!("Landed {n} changes without asking"),
            items.last().map(|i| i.title.clone()).unwrap_or_default(),
        ),
        // The landed tabs closed unseen: the last one closed, when several.
        "landed" if one => (first.title.clone(), first.detail.clone()),
        "landed" => (
            format!("{n} landed tabs closed"),
            items.last().map(|i| i.title.clone()).unwrap_or_default(),
        ),
        "ready" if one => (first.title.clone(), first.detail.clone()),
        "ready" => (format!("{n} tabs ready"), first.title.clone()),
        "habits" if one => (format!("Suggestion: {}", first.title), String::new()),
        "habits" => (format!("{n} suggestions"), first.title.clone()),
        // The next meeting and the mode's word are one each, as they are.
        _ => (first.title.clone(), first.detail.clone()),
    }
}

/// The yellow of the notifications.
const YELLOW: Color = Color::Indexed(220);

/// The footer's words before "m view actions", and those words, which a
/// click on alone opens the window's own menu.
const FOOTER_BEFORE: &str = " jk move  ↵ actions  ";
const VIEW_ACTIONS: &str = "m view actions";

/// The columns of the footer's "m view actions", first and past the last.
pub fn view_actions_at() -> (usize, usize) {
    let from = crate::ui::draw::width(FOOTER_BEFORE);
    (from, from + crate::ui::draw::width(VIEW_ACTIONS))
}

/// What a line of the list is, for a click: the "‹ tabs" at its top, which
/// goes back, or a notification, by its place.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Spot {
    Back,
    Item(usize),
    /// The footer's "m view actions": the window's own menu.
    ViewActions,
}

/// How the list is being looked at: the chosen notification, whether Tab
/// has put the focus on "‹ tabs", and what the mouse is over.
#[derive(Clone, Copy, Debug)]
pub struct Look<'a> {
    pub cursor: usize,
    pub back: bool,
    /// Whether Tab has put the focus on the footer's "m view actions".
    pub menu: bool,
    pub hover: Option<Spot>,
    pub theme: &'a Theme,
    /// Now, in the time zone each notification's time is read in.
    pub now: &'a jiff::Zoned,
    /// A scroll bar at the right while the list runs past the window
    /// (scroll_bars), every line laid out a column narrower for it.
    pub bars: bool,
}

/// A window's rows as last drawn: the size drawn at (columns, rows), and
/// what each row is.
pub type DrawnRows = ((u16, u16), Vec<Option<Spot>>);

/// What the server keeps of each session's window.
#[derive(Debug, Default)]
pub struct Windows {
    /// The notification chosen in each session's window, as the sidebar
    /// names it.
    pub at: HashMap<String, Owner>,
    /// The tab each session's window was brought in front of, which Esc
    /// goes back to.
    pub from: HashMap<String, TabId>,
    /// The sessions whose window has Tab's focus on "‹ tabs".
    pub back: HashSet<String>,
    /// The sessions whose window has Tab's focus on the footer's "m view
    /// actions".
    pub menu: HashSet<String>,
    /// What the mouse is over in a client's window.
    pub hover: HashMap<ClientId, Spot>,
    /// What each row of a session's window was as it was last drawn, at
    /// the size it was drawn at, for the pointer's shape over it, so a
    /// still mouse over the window lays the notifications out once a frame,
    /// not twice. Kept as it draws, which reads the server's state alone.
    pub drawn: std::cell::RefCell<HashMap<String, DrawnRows>>,
}

/// A notification's whole message, wrapped to `cols`: its lines as its
/// source laid them out (a Slack message), else its detail.
fn body(item: &Item, cols: usize) -> Vec<String> {
    let lines: Vec<&str> = if item.lines.is_empty() {
        item.detail.lines().collect()
    } else {
        item.lines.iter().map(String::as_str).collect()
    };
    lines
        .iter()
        .flat_map(|l| {
            if l.trim().is_empty() {
                vec![String::new()]
            } else {
                text::wrap(l, cols)
            }
        })
        .collect()
}

/// Spans cut to `cols` and padded to fill them, on `fill`.
fn line(spans: Vec<Span<'static>>, cols: usize, fill: Option<Color>) -> Line {
    let mut out = Vec::new();
    let mut used = 0;
    for span in spans {
        let room = cols.saturating_sub(used);
        if room == 0 {
            break;
        }
        let w = width(&span.content);
        if w <= room {
            used += w;
            out.push(span);
        } else {
            let cut = text::cut(&span.content, room);
            used += width(&cut);
            out.push(Span::styled(cut, span.style));
        }
    }
    if used < cols {
        out.push(Span::raw(" ".repeat(cols - used)));
    }
    Line { spans: out, fill }
}

/// The list's lines for a pane `cols` by `rows`, and what each line is: the
/// "‹ tabs" and the heading, a double rule, then each notification, its
/// title bold and its whole message under it, the chosen one marked and
/// kept in sight, and the keys at the bottom.
pub fn list_lines(
    items: &[Item],
    look: &Look,
    cols: usize,
    rows: usize,
) -> (Vec<Line>, Vec<Option<Spot>>) {
    // A column kept at the right for the scroll bar.
    let full = cols;
    let cols = cols - usize::from(look.bars && cols > 1);
    let theme = look.theme;
    let fg = |c: Color| Style::new().fg(c);
    let back_style = if look.back || look.hover == Some(Spot::Back) {
        fg(theme.dim).add_modifier(Modifier::REVERSED)
    } else {
        fg(theme.dim)
    };
    let head = vec![
        Span::raw(" "),
        Span::styled("‹ tabs", back_style),
        Span::raw("  "),
        Span::styled(
            format!("NOTIFICATIONS {}", items.len()),
            fg(YELLOW).add_modifier(Modifier::BOLD),
        ),
    ];
    let rule = vec![
        Span::raw(" "),
        Span::styled("═".repeat(cols.saturating_sub(2)), fg(YELLOW)),
    ];
    let mut body_lines: Vec<Line> = Vec::new();
    let mut owners: Vec<Option<Spot>> = Vec::new();
    let (mut cursor_at, mut cursor_end) = (0usize, 0usize);
    for (i, item) in items.iter().enumerate() {
        let chosen = i == look.cursor;
        let fill = if chosen {
            Some(theme.highlight)
        } else if look.hover == Some(Spot::Item(i)) {
            Some(theme.panel)
        } else {
            None
        };
        if chosen {
            cursor_at = body_lines.len();
        }
        // When it came, dim at the first line's right end, the title cut
        // short of it.
        let at = item
            .time
            .map(|ms| crate::ui::when::notice(ms, look.now))
            .unwrap_or_default();
        let stamp_w = if at.is_empty() { 0 } else { width(&at) + 2 };
        let title_w = cols.saturating_sub(3 + stamp_w);
        let title = text::cut(&item.title, title_w);
        let gap = title_w.saturating_sub(width(&title));
        let mut first = vec![
            Span::styled(if chosen { "▌" } else { " " }, fg(YELLOW)),
            Span::raw(" "),
            Span::styled(title, fg(theme.bright).add_modifier(Modifier::BOLD)),
        ];
        if !at.is_empty() {
            first.push(Span::raw(" ".repeat(gap + 2)));
            first.push(Span::styled(at, fg(theme.dim)));
        }
        body_lines.push(line(first, cols, fill));
        owners.push(Some(Spot::Item(i)));
        for l in body(item, cols.saturating_sub(4)) {
            body_lines.push(line(
                vec![Span::raw("   "), Span::styled(l, fg(theme.text))],
                cols,
                fill,
            ));
            owners.push(Some(Spot::Item(i)));
        }
        if chosen {
            cursor_end = body_lines.len() - 1;
        }
        body_lines.push(line(Vec::new(), cols, None));
        owners.push(Some(Spot::Item(i)));
    }
    if items.is_empty() {
        body_lines.push(line(
            vec![Span::styled("   Nothing new.", fg(theme.dim))],
            cols,
            None,
        ));
        owners.push(None);
    }
    // The two heading lines stay; the list scrolls to keep the chosen one
    // in sight.
    let room = rows.saturating_sub(3);
    let top = (cursor_at as isize)
        .min(cursor_end as isize + 1 - room as isize)
        .min(body_lines.len() as isize - room as isize)
        .max(0) as usize;
    let mut lines = vec![line(head, cols, None), line(rule, cols, None)];
    let mut spots = vec![Some(Spot::Back), Some(Spot::Back)];
    let total = body_lines.len();
    lines.extend(body_lines.into_iter().skip(top).take(room));
    spots.extend(owners.into_iter().skip(top).take(room));
    // The list follows the chosen notification, so its bar only shows where
    // that is.
    if look.bars && cols < full {
        let bar = crate::ui::kit::scrollbar(total, top, room, room, theme);
        for (l, mark) in lines.iter_mut().skip(2).zip(bar) {
            l.spans.push(mark);
        }
    }
    while lines.len() < rows.saturating_sub(1) {
        lines.push(line(Vec::new(), cols, None));
        spots.push(None);
    }
    // "m view actions" takes a click (`VIEW_ACTIONS_AT`) and Tab's focus,
    // lit under either.
    let menu_style = if look.menu || look.hover == Some(Spot::ViewActions) {
        fg(theme.dim).add_modifier(Modifier::REVERSED)
    } else {
        fg(theme.dim)
    };
    lines.push(line(
        vec![
            Span::styled(FOOTER_BEFORE, fg(theme.dim)),
            Span::styled(VIEW_ACTIONS, menu_style),
            Span::styled("  esc back", fg(theme.dim)),
        ],
        cols,
        None,
    ));
    spots.push(Some(Spot::ViewActions));
    lines.truncate(rows);
    spots.truncate(rows);
    for l in &mut lines {
        let w: usize = l.spans.iter().map(|s| width(&s.content)).sum();
        if w < full {
            l.spans.push(Span::raw(" ".repeat(full - w)));
        }
    }
    (lines, spots)
}

/// `switchboard notifications`: what the Notifications window's pane runs.
/// The server draws the list over the pane and takes its keys, so this only
/// waits, as long as the pane is there.
pub fn main(args: Vec<String>) -> ExitCode {
    if !args.is_empty() || std::env::var_os("SWITCHBOARD_PANE").is_none() {
        eprintln!(
            "Usage: switchboard notifications   (the Notifications window's pane, started for you)"
        );
        return ExitCode::from(2);
    }
    loop {
        std::thread::park();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(lines: &[Line]) -> Vec<String> {
        lines
            .iter()
            .map(|l| l.text().trim_end().to_string())
            .collect()
    }

    /// 2026-11-11 09:42 UTC, a Wednesday.
    static NOW: std::sync::LazyLock<jiff::Zoned> = std::sync::LazyLock::new(|| {
        jiff::civil::date(2026, 11, 11)
            .at(9, 42, 0, 0)
            .to_zoned(jiff::tz::TimeZone::UTC)
            .unwrap()
    });

    fn look(cursor: usize) -> Look<'static> {
        Look {
            cursor,
            back: false,
            menu: false,
            hover: None,
            theme: &Theme::DARK,
            now: &NOW,
            bars: false,
        }
    }

    #[test]
    fn each_notification_says_when_it_came_at_its_first_lines_end() {
        let at = |days: i64, hour: i8| {
            jiff::civil::date(2026, 11, 11)
                .at(hour, 10, 0, 0)
                .to_zoned(jiff::tz::TimeZone::UTC)
                .unwrap()
                .timestamp()
                .as_millisecond()
                - days * 86_400_000
        };
        let mut today = Item::new("a", "3 new on Slack").detail("Ada · #general");
        today.time = Some(at(0, 9));
        let mut older = Item::new("b", "PR checks failed");
        older.time = Some(at(4, 14));
        let mut meeting = Item::new("c", "Standup");
        meeting.time = Some(at(0, 15));
        let untimed = Item::new("d", "A suggestion");
        let (lines, _) = list_lines(&[today, older, meeting, untimed], &look(0), 40, 16);
        let text = texts(&lines);
        assert_eq!(
            text[2],
            format!("▌ 3 new on Slack{}9:10 AM", " ".repeat(16))
        );
        assert_eq!(text[3], "   Ada · #general");
        assert!(text[5].ends_with("  Sat 2:10 PM"), "{}", text[5]);
        assert!(text[7].ends_with("  Starts 3:10 PM"), "{}", text[7]);
        assert_eq!(text[9], "  A suggestion");
        assert!(text.iter().all(|l| width(l) <= 40));
    }

    #[test]
    fn every_notification_is_listed_whole_the_chosen_one_marked() {
        let items = vec![Item::new("a", "3 new on Slack").detail("Ada · #general"), {
            let mut slack = Item::new("b", "Dee in a DM");
            slack.lines = vec!["first line".into(), String::new(), "• a point".into()];
            slack
        }];
        let (lines, spots) = list_lines(&items, &look(1), 40, 12);
        let text = texts(&lines);
        assert_eq!(text[0], " ‹ tabs  NOTIFICATIONS 2");
        assert_eq!(text[1], format!(" {}", "═".repeat(38)));
        assert_eq!(
            text[2..9],
            [
                "  3 new on Slack",
                "   Ada · #general",
                "",
                "▌ Dee in a DM",
                "   first line",
                "",
                "   • a point",
            ]
        );
        assert!(
            text[11].starts_with(" jk move  ↵ actions  m view actions"),
            "{}",
            text[11]
        );
        assert_eq!(spots[11], Some(Spot::ViewActions));
        let (from, to) = view_actions_at();
        let words: String = text[11].chars().skip(from).take(to - from).collect();
        assert_eq!(words, "m view actions", "only its words take the click");
        assert_eq!(lines[5].fill, Some(Theme::DARK.highlight));
        assert_eq!(lines[2].fill, None);
        assert_eq!(spots[0], Some(Spot::Back));
        assert_eq!(spots[3], Some(Spot::Item(0)));
        assert_eq!(spots[6], Some(Spot::Item(1)));
        assert_eq!(lines.len(), 12);
        for l in &lines {
            assert_eq!(width(&l.text()), 40, "{:?}", l.text());
        }
    }

    #[test]
    fn each_kind_is_one_row_in_the_sidebar_counted_as_the_daemon_counted_it() {
        let slack = |id: &str, who: &str| Item::new(id, format!("◆ {who} · #general"));
        assert_eq!(
            summary("slack", &[slack("a", "Ada"), slack("b", "Dee")]),
            ("2 new on Slack".into(), "Ada · #general".into())
        );
        let change = |t: &str| Item::new(t, t);
        assert_eq!(
            summary("calendar", &[change("Standup moved to 10:00")]),
            ("Calendar: Standup moved to 10:00".into(), String::new())
        );
        assert_eq!(
            summary("calendar", &[change("one"), change("two")]),
            ("2 calendar changes".into(), "one".into())
        );
        let note = Item::new("n", "Plan").data(serde_json::json!({ "source": "Fathom" }));
        assert_eq!(
            summary("new-notes", std::slice::from_ref(&note)),
            ("New note: Plan".into(), "from Fathom".into())
        );
        assert_eq!(summary("new-notes", &[note.clone(), note]).0, "2 new notes");
        let note = Item::new("n", "Plan").data(serde_json::json!({ "source": "Fathom" }));
        assert_eq!(words("new-notes", &note), "from Fathom · Plan");
        assert_eq!(
            words("autolanded", &Item::new("a", "Landed abc1234")),
            "Landed abc1234 · without asking"
        );
        let inbox = Item::new("i", "AB-1 Fix it").detail("Ada commented: looks good");
        assert_eq!(
            summary("inbox", &[inbox]),
            ("1 in your Linear inbox".into(), "Ada commented".into())
        );
        assert_eq!(
            summary("habits", &[change("Turn off the Slack view")]).0,
            "Suggestion: Turn off the Slack view"
        );
        assert_eq!(
            view_of("ci"),
            Some(ViewId::Ci),
            "a failing default branch is the CI view's"
        );
        // Every kind says whose item it is, or that it is no view's.
        for source in ORDER {
            assert_ne!(
                view_of(source).is_some(),
                NO_VIEW.contains(&source),
                "{source} names its view in view_of, or is in NO_VIEW"
            );
        }
        // And where it comes from: its view's name, else its app's.
        for source in ORDER {
            assert!(group_of(source).is_some(), "{source} names its group");
            if let Some(view) = view_of(source) {
                assert_eq!(group_of(source), Some(view.title()));
            }
        }
        assert_eq!(group_of("meeting"), group_of("calendar"));
        assert_eq!(group_of("landed"), Some("Autopilot"));
        assert_eq!(group_of("new-notes"), Some("Notes"));
        assert_eq!(group_name("a kind to come"), "Switchboard");
        let mut sources = vec!["habits", "slack", "inbox", "meeting", "autopilot"];
        sources.sort_by_key(|s| rank(s));
        assert_eq!(
            sources,
            ["meeting", "slack", "inbox", "autopilot", "habits"]
        );
    }

    // A notification finds its item in its view by the id the view gives it.
    #[test]
    fn a_notification_names_its_item_by_its_views_id() {
        assert_eq!(id_in_its_view("meeting", "meeting:ev1"), "ev1");
        assert_eq!(
            id_in_its_view("prs", "merged:https://github.com/o/r/pull/1"),
            "https://github.com/o/r/pull/1"
        );
        assert_eq!(id_in_its_view("slack", "D1:1.5"), "D1:1.5");
        assert_eq!(id_in_its_view("routines", "routine:r1"), "routine:r1");
    }

    #[test]
    fn a_group_is_split_by_channel_repository_or_routine() {
        let slack =
            Item::new("s", "◆ Ada · #general").data(serde_json::json!({ "where": "#general" }));
        assert_eq!(part_of("slack", &slack).as_deref(), Some("#general"));
        // A conversation is one part however its rows name it.
        let dm = |place: &str| {
            Item::new("d", "◆ Ada")
                .data(serde_json::json!({ "where": place, "message": { "channel": "G1" } }))
        };
        assert_eq!(
            part_of("slack", &dm("Ada, Bo")),
            part_of("slack", &dm("Bo, Ada"))
        );
        assert_eq!(
            part_name("slack", &dm("Ada, Bo")).as_deref(),
            Some("Ada, Bo")
        );
        assert_eq!(words("slack", &slack), "Ada · #general");
        assert_eq!(
            words("slack", &slack.clone().detail("ship it")),
            "Ada · #general · ship it"
        );
        let pr = Item::new("p", "PR 3 merged, web-app")
            .data(serde_json::json!({ "pr": { "number": 3, "repo": "o/web-app" } }));
        assert_eq!(part_of("prs", &pr).as_deref(), Some("o/web-app"));
        assert_eq!(
            part_label("prs", "o/web-app", &["o/web-app", "o/tools"]),
            "web-app"
        );
        // A fork and its upstream keep their owners.
        assert_eq!(
            part_label("prs", "o/web-app", &["o/web-app", "me/web-app"]),
            "o/web-app"
        );
        assert_eq!(part_label("slack", "#a/b", &["#a/b", "x/b"]), "#a/b");
        assert_eq!(words("prs", &pr), "PR 3 merged, web-app");
        let inbox = Item::new("i", "AB-1 Fix it").detail("Ada commented: looks good");
        assert_eq!(words("inbox", &inbox), "Ada commented · AB-1 Fix it");
        let failing = Item::new("c", "main failing at abc1234").detail("o/tools · clippy");
        assert_eq!(
            words("ci", &failing),
            "o/tools · clippy · main failing at abc1234"
        );
        let ci = Item::new("c", "Build failing")
            .data(serde_json::json!({ "mainCi": { "repo": "o/tools" } }));
        assert_eq!(part_of("ci", &ci).as_deref(), Some("o/tools"));
        let routine = Item::new("routine:r1", "Routine Standup");
        assert_eq!(part_of("routines", &routine).as_deref(), Some("Standup"));
        assert_eq!(part_of("calendar", &Item::new("e", "Standup moved")), None);
    }

    #[test]
    fn a_long_list_has_a_scroll_bar_at_its_right() {
        let items: Vec<Item> = (0..20)
            .map(|i| Item::new(format!("{i}"), format!("notice {i}")).detail("more"))
            .collect();
        let bars = Look {
            bars: true,
            ..look(15)
        };
        let (lines, _) = list_lines(&items, &bars, 30, 10);
        let last: String = lines
            .iter()
            .map(|l| l.text().chars().last().unwrap_or(' '))
            .collect();
        assert!(last.contains('┃') && last.contains('│'), "{last:?}");
        assert!(lines.iter().all(|l| width(&l.text()) == 30));
        let (lines, _) = list_lines(&items, &look(15), 30, 10);
        assert!(!lines.iter().any(|l| l.text().contains('┃')));
    }

    #[test]
    fn a_long_list_keeps_the_chosen_one_in_sight_and_an_empty_one_says_so() {
        let items: Vec<Item> = (0..20)
            .map(|i| Item::new(format!("{i}"), format!("notice {i}")).detail("more"))
            .collect();
        let (lines, _) = list_lines(&items, &look(15), 30, 10);
        let text = texts(&lines);
        assert!(text.iter().any(|l| l == "▌ notice 15"), "{text:?}");
        assert_eq!(text[0], " ‹ tabs  NOTIFICATIONS 20", "the heading stays");
        let (lines, _) = list_lines(&[], &look(0), 30, 10);
        assert_eq!(texts(&lines)[2], "   Nothing new.");
        let back = Look {
            back: true,
            menu: false,
            ..look(0)
        };
        let (lines, _) = list_lines(&[], &back, 30, 10);
        assert!(
            lines[0].spans[1]
                .style
                .add_modifier
                .contains(Modifier::REVERSED)
        );
    }

    #[test]
    fn each_kind_wears_its_own_mark() {
        assert_eq!(mark_of("meeting", "e"), Mark::Reminder);
        assert_eq!(mark_of("calendar", "c"), Mark::Reminder);
        assert_eq!(mark_of("slack", "s"), Mark::Mention);
        assert_eq!(mark_of("inbox", "i"), Mark::Mention);
        assert_eq!(mark_of("new-notes", "n"), Mark::Note);
        // A PR's news by what happened to it.
        assert_eq!(mark_of("prs", "merged:x"), Mark::Passed);
        assert_eq!(mark_of("prs", "failing:x"), Mark::Failed);
        assert_eq!(mark_of("ci", "c"), Mark::Failed);
        assert_eq!(mark_of("autolanded", "a"), Mark::Passed);
        assert_eq!(mark_of("habits", "h"), Mark::Comment);
    }
}
