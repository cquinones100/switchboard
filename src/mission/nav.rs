//! The list of mission control's groups and views (boards 64
//! and 65 of the design): each group a section's label, each view a row
//! with what it holds at its right, the view in front lit as a list's
//! chosen row. With Settings' `mission_keep_tabs_sidebar` off it is drawn
//! in the sidebar's place while mission control is open, under a top line
//! that leads back to the tabs and says how many need you (board 64); on,
//! or with the sidebar hidden, as a column of mission control's own at its
//! left, the tabs sidebar beside it (board 65). It took the place of the
//! two-row bar of groups and views across mission control's top.
//!
//! Drawn from the snapshot alone (each view's items, the tabs' states), so
//! nothing here reads a file or starts a process.

use ratatui::style::Style;
use ratatui::text::Span;

use super::views::{self, Group, ViewId};
use super::{Focus, HitBox, Mission, Spot};
use crate::sidebar::model;
use crate::sidebar::render::Line;
use crate::ui::kit::{self, Surface, Tone};

/// The width of mission control's own column, its line beside it apart.
pub const COLUMN: u16 = 23;

/// The groups the list shows, each with its views (places in mission
/// control: 0 the tabs, `i` the `i`th view after), the list's filter
/// leaving out the views whose name doesn't hold it and the groups left
/// with none.
pub fn listed(m: &Mission) -> Vec<(Group, Vec<usize>)> {
    let groups = m.groups();
    let text = m
        .nav_filter
        .as_ref()
        .map(|f| f.typed().trim().to_lowercase())
        .unwrap_or_default();
    if text.is_empty() {
        return groups;
    }
    groups
        .into_iter()
        .map(|(g, held)| {
            let kept = held
                .into_iter()
                .filter(|&i| title(m, i).to_lowercase().contains(&text))
                .collect::<Vec<_>>();
            (g, kept)
        })
        .filter(|(_, held)| !held.is_empty())
        .collect()
}

/// The listed views in the list's order, as `j` and `k` go through them.
pub fn order(m: &Mission) -> Vec<usize> {
    listed(m).into_iter().flat_map(|(_, held)| held).collect()
}

/// Ends the list's filter being typed, its words kept, unless they leave
/// no view: then it goes, the whole list back, so its rows stay within
/// reach of Tab and the keys.
pub fn end_typing(m: &mut Mission) {
    if let Some(f) = m.nav_filter.as_mut() {
        f.keep();
    }
    if m.nav_filter.is_some() && order(m).is_empty() {
        m.nav_filter = None;
    }
}

/// A place's name in the list and the header: the tabs' "Tabs", the Work
/// map's "Map".
pub fn title(m: &Mission, place: usize) -> &str {
    match place.checked_sub(1).and_then(|i| m.snap.views.get(i)) {
        Some(v) => views::bar_title(v),
        None => "Tabs",
    }
}

/// The group a place is in.
pub fn group_of(m: &Mission, place: usize) -> Option<Group> {
    m.groups()
        .into_iter()
        .find(|(_, held)| held.contains(&place))
        .map(|(g, _)| g)
}

/// How many tabs need you, of every tab whatever the filter leaves.
pub fn needing(m: &Mission) -> usize {
    let tabs = if m.snap.all_tabs.is_empty() {
        &m.snap.tabs
    } else {
        &m.snap.all_tabs
    };
    model::needs_you(tabs, &m.snap.rules).len()
}

/// What a view says without opening it, in its tone: how many tabs need
/// you, or how many items it holds (its headings left out); `short` the
/// number alone, for the narrow column.
fn fact(m: &Mission, place: usize, short: bool) -> Option<(String, Tone)> {
    let Some(view) = place.checked_sub(1).and_then(|i| m.snap.views.get(i)) else {
        let needs = needing(m);
        if needs > 0 {
            let words = if short {
                needs.to_string()
            } else {
                format!("{needs} need you")
            };
            return Some((words, Tone::Needs));
        }
        let n = m.snap.tabs.len();
        return (n > 0).then(|| (n.to_string(), Tone::Dim));
    };
    let items = view.data.items.iter().filter(|i| !views::is_header(i));
    // A view of things to deal with says how many; one of settings,
    // figures or keys says nothing.
    let (n, tone) = match view.id {
        ViewId::Ci => (
            items.filter(|i| views::ci_state(i) == "failing").count(),
            Tone::Failed,
        ),
        ViewId::Reminders => (items.count(), Tone::Needs),
        // By source, its notifications whatever is folded or split.
        ViewId::Notifications => (
            super::history::held(&view.data.items).unwrap_or_else(|| items.count()),
            Tone::Accent,
        ),
        ViewId::Inbox | ViewId::Slack => (items.count(), Tone::Accent),
        ViewId::WorkMap
        | ViewId::Reopen
        | ViewId::Prs
        | ViewId::Landing
        | ViewId::Autopilot
        | ViewId::Routines
        | ViewId::Tickets => (items.count(), Tone::Dim),
        ViewId::Calendar
        | ViewId::Relay
        | ViewId::Resources
        | ViewId::Usage
        | ViewId::Keys
        | ViewId::Settings
        | ViewId::Plugin(_) => (0, Tone::Dim),
    };
    if n == 0 {
        return None;
    }
    let words = match view.id {
        ViewId::Ci if !short => format!("{n} failing"),
        _ => n.to_string(),
    };
    Some((words, tone))
}

/// The place the list's chosen row is on: the view a name being typed
/// matches, the one Tab's focus is on, else the view in front.
fn cursor(m: &Mission) -> usize {
    match (m.jump.as_ref().and_then(|j| j.matched), m.focus) {
        (Some(v), _) => v,
        (_, Some(Focus::View(v))) => v,
        _ => m.view,
    }
}

/// The list, `cols` by `rows`, where a click on each part lands
/// (`Spot::View` a view's row, `Spot::Close` the way back to the tabs,
/// `Spot::Filter` the filter), from its own top left, and the first of its
/// groups' lines drawn (`top_of`), which scroll when they don't fit. `column` draws it as
/// mission control's own column (board 65), else in the sidebar's place
/// (board 64).
pub fn lines(
    m: &Mission,
    cols: usize,
    rows: usize,
    column: bool,
) -> (Vec<Line>, Vec<HitBox>, usize) {
    let theme = &m.snap.theme;
    let fill = column.then(|| Surface::Panel.color(theme));
    let mut lines: Vec<Line> = Vec::new();
    let mut hits = Vec::new();
    let blank = || Line {
        spans: vec![Span::raw(" ".repeat(cols))],
        fill,
    };
    let filter: Vec<Span<'static>> = match &m.nav_filter {
        Some(f) => f.spans(Tone::Needs.style(theme), Tone::Dim.style(theme)),
        None => vec![Span::styled("/ filter", Tone::Dim.style(theme))],
    };
    let filter_lit = m.nav_hover == Some(Spot::Filter) && m.nav_filter.is_none();
    let filter: Vec<Span<'static>> = if filter_lit {
        filter
            .into_iter()
            .map(|s| Span::styled(s.content, s.style.bg(Surface::Raised.color(theme))))
            .collect()
    } else {
        filter
    };
    let filter_w = kit::spans_width(&filter);
    if column {
        let mut top = vec![
            Span::raw(" "),
            Span::styled("MISSION CONTROL", Tone::Accent.bold(theme)),
        ];
        if right(&mut top, filter, cols, 1) {
            hits.push(hit(0, cols - 1 - filter_w, filter_w, Spot::Filter));
        }
        lines.push(Line {
            spans: kit::fit(top, cols, Style::new()),
            fill,
        });
        lines.push(blank());
    } else {
        // The way back to the tabs, and how many of them need you meanwhile.
        let back_lit = m.nav_hover == Some(Spot::Close);
        let back_style = |s: Style| {
            if back_lit {
                s.bg(Surface::Raised.color(theme))
            } else {
                s
            }
        };
        let mut top = vec![
            Span::raw(" "),
            Span::styled("‹ ", back_style(Tone::Accent.style(theme))),
            Span::styled("Tabs", back_style(Tone::Dim.style(theme))),
        ];
        hits.push(hit(0, 1, 6, Spot::Close));
        let needs = needing(m);
        // The count gives way to the filter button (not to a filter being
        // typed, which keys reach) when a narrowed list has no room for
        // both, so the button keeps its click.
        let count_fits = m.nav_filter.is_some()
            || kit::spans_width(&top) + 5 + format!("{needs} need you").len() + 2 + filter_w < cols;
        if needs > 0 && count_fits {
            top.push(Span::styled("  ·  ", Tone::Faint.style(theme)));
            top.push(Span::styled(
                format!("{needs} need you"),
                Tone::Needs.style(theme),
            ));
        }
        if right(&mut top, filter, cols, 1) {
            hits.push(hit(0, cols - 1 - filter_w, filter_w, Spot::Filter));
        }
        lines.push(Line {
            spans: kit::fit(top, cols, Style::new()),
            fill,
        });
        lines.push(blank());
        lines.push(Line {
            spans: kit::label("Mission control", Tone::Accent, None, cols, theme),
            fill,
        });
        lines.push(blank());
    }
    let chosen = cursor(m);
    // The groups and views, each row with what a click on it lands on.
    let mut body: Vec<(Line, Option<Spot>)> = Vec::new();
    for (g, held) in listed(m) {
        body.push((
            Line {
                spans: kit::label(g.name(), Tone::Dim, None, cols, theme),
                fill,
            },
            None,
        ));
        for place in held {
            let is_chosen = place == chosen;
            let in_front = place == m.view;
            let name_style = if is_chosen || in_front {
                Tone::Text.bold(theme)
            } else {
                Tone::Text.style(theme)
            };
            let mut spans = vec![
                Span::raw("  "),
                Span::styled(title(m, place).to_string(), name_style),
            ];
            if let Some((words, tone)) = fact(m, place, column) {
                right(
                    &mut spans,
                    vec![Span::styled(words, tone.style(theme))],
                    cols - 1,
                    1,
                );
            }
            let lit = m.nav_hover == Some(Spot::View(place)) && !is_chosen;
            let row = if is_chosen {
                kit::row(spans, cols, true, theme)
            } else if in_front || lit {
                kit::row_resting(spans, cols, theme)
            } else {
                kit::row(spans, cols, false, theme)
            };
            body.push((Line { spans: row, fill }, Some(Spot::View(place))));
        }
        body.push((blank(), None));
    }
    // What the room under the top lines holds, the keys at the foot with a
    // line clear above them: from where the wheel left it, else moved just
    // enough to keep the chosen row in sight.
    let room = rows.saturating_sub(lines.len() + 2);
    let top = top_of(&body, room, m.nav_top, (!m.nav_scrolled).then_some(chosen));
    for (line, spot) in body.into_iter().skip(top).take(room) {
        if let Some(spot) = spot {
            hits.push(hit(lines.len(), 0, cols, spot));
        }
        lines.push(line);
    }
    while lines.len() + 1 < rows {
        lines.push(blank());
    }
    if rows > lines.len() {
        let pairs: &[(&str, &str)] = if column {
            &[("j/k", "view"), ("Enter", "open")]
        } else {
            &[("j/k", "view"), ("Enter", "open"), ("Esc", "tabs")]
        };
        let mut keys = vec![Span::raw(" ")];
        keys.extend(kit::hints(pairs, theme));
        lines.push(Line {
            spans: kit::fit(keys, cols, Style::new()),
            fill,
        });
    }
    lines.truncate(rows);
    (lines, hits, top)
}

/// The first of `body`'s lines to draw in `room` lines: `asked`, kept
/// inside the body, then moved the least that brings the row of `follow`
/// (when it is listed) into sight.
fn top_of(
    body: &[(Line, Option<Spot>)],
    room: usize,
    asked: usize,
    follow: Option<usize>,
) -> usize {
    let most = body.len().saturating_sub(room);
    let mut top = asked.min(most);
    if let Some(at) = follow.and_then(|v| body.iter().position(|(_, s)| *s == Some(Spot::View(v))))
    {
        if at < top {
            top = at;
        } else if room > 0 && at >= top + room {
            top = at + 1 - room;
        }
    }
    top
}

/// `spans` put flush right on `line`, `pad` columns in from `cols`, two
/// clear of what is before at the least, or left off when they don't fit.
fn right(
    line: &mut Vec<Span<'static>>,
    spans: Vec<Span<'static>>,
    cols: usize,
    pad: usize,
) -> bool {
    let used = kit::spans_width(line);
    let w = kit::spans_width(&spans);
    if used + 2 + w + pad > cols {
        return false;
    }
    line.push(Span::raw(" ".repeat(cols - pad - w - used)));
    line.extend(spans);
    true
}

fn hit(y: usize, x: usize, w: usize, spot: Spot) -> HitBox {
    HitBox {
        y: y as u16,
        x0: x as u16,
        x1: (x + w).saturating_sub(1) as u16,
        spot,
    }
}

/// The header that took the bar's place (board 64): a place's name and
/// its group's.
pub fn header_words(m: &Mission, place: usize) -> (&str, &'static str) {
    (title(m, place), group_of(m, place).map_or("", Group::name))
}
