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
use crate::mux::TabId;
use crate::mux::server::ClientId;
use crate::ui::Theme;

/// The tab option that marks a session's Notifications window.
pub const MARK: &str = "@notices_window";

/// The yellow of the notifications.
const YELLOW: Color = Color::Indexed(220);

/// What a line of the list is, for a click: the "‹ tabs" at its top, which
/// goes back, or a notification, by its place.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Spot {
    Back,
    Item(usize),
}

/// How the list is being looked at: the chosen notification, whether Tab
/// has put the focus on "‹ tabs", and what the mouse is over.
#[derive(Clone, Copy, Debug)]
pub struct Look<'a> {
    pub cursor: usize,
    pub back: bool,
    pub hover: Option<Spot>,
    pub theme: &'a Theme,
}

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
    /// What the mouse is over in a client's window.
    pub hover: HashMap<ClientId, Spot>,
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
        body_lines.push(line(
            vec![
                Span::styled(if chosen { "▌" } else { " " }, fg(YELLOW)),
                Span::raw(" "),
                Span::styled(
                    item.title.clone(),
                    fg(theme.bright).add_modifier(Modifier::BOLD),
                ),
            ],
            cols,
            fill,
        ));
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
    lines.extend(body_lines.into_iter().skip(top).take(room));
    spots.extend(owners.into_iter().skip(top).take(room));
    while lines.len() < rows.saturating_sub(1) {
        lines.push(line(Vec::new(), cols, None));
        spots.push(None);
    }
    lines.push(line(
        vec![Span::styled(" jk move  ↵ actions  esc back", fg(theme.dim))],
        cols,
        None,
    ));
    spots.push(None);
    lines.truncate(rows);
    spots.truncate(rows);
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

    fn look(cursor: usize) -> Look<'static> {
        Look {
            cursor,
            back: false,
            hover: None,
            theme: &Theme::DARK,
        }
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
        assert_eq!(text[11], " jk move  ↵ actions  esc back");
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
}
