//! The sidebar's lines (lib/sidebar/render.js's `sidebarLines`): a plain
//! function from what is known to the lines to draw, each exactly the
//! sidebar's width, so the tests can read them as text. At the top the
//! session's name and its counts; then the tabs in their groups, each with
//! its badge, number and name in its project's color, and under it what it
//! is doing; then, pinned to the bottom under their own heading, the
//! notifications, which never scroll away. The header stays at the top just
//! as they do, so on a short screen only the groups and tabs scroll, the
//! filter line saying how many tabs are above (`▲ 3 more`) and the list's
//! last row how many are cut below (`▼ 2 more`); beside a tab in sight
//! where the list was last shown, the list holds there, so going from tab
//! to tab doesn't move it.
//!
//! Each line says what it belongs to (`Owner`), for a click on it, and the
//! list says where it scrolled to (`Scroll`), for C-d and C-u to move by.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;

use super::model::{self, Group, Rules, TabModel, TabState};
use super::text::{self, Badge, SPINNER, width};
use crate::items::{Item, State};
use crate::mux::TabId;
use crate::ui::Theme;

/// The most lines a tab's name wraps to.
pub const NAME_LINES: usize = 3;

/// The yellow of the notifications and of a filter being typed.
const YELLOW: Color = Color::Indexed(220);

/// What a line of the sidebar belongs to: a tab, or a notification (its
/// source and its item's id), for the cursor and a click; or the row saying
/// how many tabs are out of sight above or below, which a click scrolls.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Owner {
    Tab(TabId),
    Notice { source: String, id: String },
    ScrollUp,
    ScrollDown,
}

impl Owner {
    pub fn tab(&self) -> Option<TabId> {
        match self {
            Owner::Tab(t) => Some(*t),
            _ => None,
        }
    }

    /// Whether two stops share a row of the sidebar: the same tab, or
    /// notifications of one kind, which the sidebar shows as one row.
    pub fn same_row(&self, other: &Owner) -> bool {
        match (self, other) {
            (Owner::Notice { source: a, .. }, Owner::Notice { source: b, .. }) => a == b,
            _ => self == other,
        }
    }
}

/// A notification as the sidebar lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notice {
    pub owner: Owner,
    pub title: String,
    pub detail: String,
}

/// A plugin's short list under the tabs (its `sidebar` count of items, from
/// sidebar.json or what it prints): its title, how many items it has and
/// how many need you, why its last run failed, and its first items.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Section {
    pub title: String,
    pub count: usize,
    pub attention: usize,
    pub error: Option<String>,
    pub items: Vec<Item>,
}

impl Section {
    /// A plugin's list as the sidebar shows it: its first `shown` items.
    pub fn of(title: &str, items: &[Item], error: Option<&str>, shown: usize) -> Section {
        Section {
            title: title.to_string(),
            count: items.len(),
            attention: items.iter().filter(|i| i.state == State::Attention).count(),
            error: error.map(str::to_string),
            items: items.iter().take(shown).cloned().collect(),
        }
    }
}

/// A / filter on the tabs, being typed or kept.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Filter {
    pub text: String,
    pub typing: bool,
}

/// How the sidebar is being looked at.
#[derive(Clone, Copy, Debug)]
pub struct View<'a> {
    /// Whether it has the focus.
    pub focused: bool,
    /// The cursor, while it has the focus.
    pub cursor: Option<&'a Owner>,
    /// The spinner's frame.
    pub frame: usize,
    /// The tab it sits beside.
    pub here: TabId,
    pub filter: Option<&'a Filter>,
    /// Where its list was scrolled to, kept while it has the focus.
    pub top: Option<usize>,
    /// Where the session's list was last shown, which it holds to while
    /// the tab it sits beside is in sight there, so going from tab to tab
    /// doesn't move the list.
    pub keep: Option<usize>,
    /// What the mouse is over.
    pub hover: Option<&'a Owner>,
}

/// One line: its spans, exactly the sidebar's width, and a background for
/// the whole line (the chosen tab's highlight, a hovered row's shade).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Line {
    pub spans: Vec<Span<'static>>,
    pub fill: Option<Color>,
}

impl Line {
    fn blank(cols: usize) -> Line {
        Line {
            spans: vec![Span::raw(" ".repeat(cols))],
            fill: None,
        }
    }

    /// The line as text.
    pub fn text(&self) -> String {
        self.spans.iter().map(|s| s.content.as_ref()).collect()
    }
}

/// Spans being put together into a line.
#[derive(Default)]
struct Row {
    spans: Vec<Span<'static>>,
}

impl Row {
    fn put(&mut self, text: impl Into<String>, style: Style) -> &mut Row {
        let text = text.into();
        if !text.is_empty() {
            self.spans.push(Span::styled(text, style));
        }
        self
    }

    /// The line, cut to `cols` (an ellipsis in place of what didn't fit)
    /// and padded to fill them.
    fn fit(self, cols: usize, fill: Option<Color>) -> Line {
        let mut spans = Vec::new();
        let total: usize = self.spans.iter().map(|s| width(&s.content)).sum();
        let mut used = 0;
        if total <= cols {
            spans = self.spans;
            used = total;
        } else if cols > 0 {
            // As text.js's `cut` over the whole line: what fits before the
            // last column, then the ellipsis, in the style it cut into.
            for span in self.spans {
                let w = width(&span.content);
                if used + w < cols {
                    used += w;
                    spans.push(span);
                    continue;
                }
                let mut kept = String::new();
                let mut before = 0;
                for c in span.content.chars() {
                    let cw = text::width_after(c, before);
                    if used + cw > cols - 1 {
                        break;
                    }
                    kept.push(c);
                    used += cw;
                    before = cw;
                }
                kept.push('…');
                used += 1;
                spans.push(Span::styled(kept, span.style));
                break;
            }
        }
        if used < cols {
            spans.push(Span::raw(" ".repeat(cols - used)));
        }
        Line { spans, fill }
    }
}

/// Where the list scrolled to: the line at its top, how many lines it has
/// room for of how many, each line's tab, and the chosen tab's first line,
/// all counted in the list of groups and tabs alone, under the header.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Scroll {
    pub top: usize,
    pub room: usize,
    pub length: usize,
    pub owners: Vec<Option<TabId>>,
    pub cursor_at: usize,
}

/// What `sidebar_lines` gives: the lines to draw (as many as the rows), each
/// one's owner, where the list scrolled to, and whether a badge on it spins.
#[derive(Clone, Debug, Default)]
pub struct Lines {
    pub lines: Vec<Line>,
    pub owners: Vec<Option<Owner>>,
    pub scroll: Scroll,
    pub spins: bool,
}

/// A tab's badge: a colored pill three columns wide, the spinner turning
/// while Claude works; an idle tab's mark faint, with no pill.
fn badge(badge: Badge, frame: usize, theme: &Theme) -> Span<'static> {
    let look = badge.look();
    let Some(color) = look.color else {
        return Span::styled(format!(" {} ", look.mark), Style::new().fg(theme.faint));
    };
    let mark = if badge.spins() {
        SPINNER[frame % SPINNER.len()]
    } else {
        look.mark
    };
    // A two-column mark (the pin) takes the space after it, so every pill
    // is three columns and the text after it lines up.
    let after = if width(mark) > 1 { "" } else { " " };
    Span::styled(
        format!(" {mark}{after}"),
        Style::new()
            .bg(Color::Indexed(color))
            .fg(Color::Indexed(look.ink.unwrap_or(255)))
            .add_modifier(Modifier::BOLD),
    )
}

fn fg(c: Color) -> Style {
    Style::new().fg(c)
}

/// An item's mark by its state, as mission control's lists draw it.
fn item_mark(item: &Item, theme: &Theme) -> Span<'static> {
    match item.state {
        State::Attention => Span::styled("●", bold(YELLOW)),
        State::Active => Span::styled("●", bold(Color::Indexed(67))),
        State::Idle => Span::styled("○", fg(theme.faint)),
    }
}

fn bold(c: Color) -> Style {
    Style::new().fg(c).add_modifier(Modifier::BOLD)
}

/// The counts beside the session's name: the failing builds first, in their
/// red, so a long session name cuts the others before it; then how many
/// wait on you, have finished and are working (a loop's too).
fn counts(row: &mut Row, tabs: &[TabModel], r: &Rules, theme: &Theme) {
    let mut items: Vec<(String, Style)> = Vec::new();
    let failing = tabs.iter().filter(|t| model::build_failing(t, r)).count();
    if failing > 0 {
        items.push((
            format!("{} {failing}", Badge::Failing.look().mark),
            bold(Color::Indexed(160)),
        ));
    }
    for (badge, n) in [
        (
            Badge::Waiting,
            tabs.iter().filter(|t| t.state == TabState::Waiting).count(),
        ),
        (
            Badge::Done,
            tabs.iter().filter(|t| t.state == TabState::Done).count(),
        ),
        (
            Badge::Working,
            tabs.iter().filter(|t| t.state.busy()).count(),
        ),
    ] {
        if n == 0 {
            continue;
        }
        let mark = if badge == Badge::Working {
            "⋯"
        } else {
            badge.look().mark
        };
        items.push((
            format!("{mark} {n}"),
            bold(badge.color().unwrap_or(theme.text)),
        ));
    }
    for (i, (text, style)) in items.into_iter().enumerate() {
        if i > 0 {
            row.put("  ", fg(theme.faint));
        }
        row.put(text, style);
    }
}

/// The sidebar for one tab of a session: its session's tabs, most in need of
/// you first, the plugins' lists, then the notifications. `tabs` are the session's (filtered
/// already, when a filter is kept), `frozen` the layout held while browsing.
#[allow(clippy::too_many_arguments)]
pub fn sidebar_lines(
    tabs: &[TabModel],
    frozen: Option<&model::Frozen>,
    notices: &[Notice],
    sections: &[Section],
    session: &str,
    cols: usize,
    rows: usize,
    view: &View<'_>,
    r: &Rules,
    theme: &Theme,
) -> Lines {
    // The header (the session's pill and counts, and the filter line) stays
    // at the top whatever the list's scroll, as the notifications stay at
    // the bottom; only the groups and tabs between them scroll.
    let mut head: Vec<Line> = Vec::new();
    let mut lines: Vec<Line> = Vec::new();
    let mut owners: Vec<Option<TabId>> = Vec::new();
    let mut headings: Vec<usize> = Vec::new();
    let mut spins = false;
    let mut cursor_at = 0;
    let mut cursor_end = 0;
    let selected = if view.focused {
        view.cursor.and_then(Owner::tab)
    } else {
        Some(view.here)
    };
    let here = tabs.iter().find(|t| t.id == view.here);

    // The session's name in a pill of its tab's color, and the counts.
    let mut pill_row = Row::default();
    let pill = match here.and_then(|t| t.color) {
        Some(c) => Style::new()
            .bg(c.into())
            .fg(Color::Indexed(232))
            .add_modifier(Modifier::BOLD),
        None => Style::new()
            .bg(theme.highlight)
            .fg(theme.bright)
            .add_modifier(Modifier::BOLD),
    };
    pill_row
        .put(" ", Style::new())
        .put(format!(" {session} "), pill)
        .put("  ", Style::new());
    counts(&mut pill_row, tabs, r, theme);
    head.push(pill_row.fit(cols, None));
    // A / filter being typed, or kept, in the line under it.
    match view.filter {
        Some(f) => {
            let mut row = Row::default();
            row.put(" ", Style::new())
                .put(format!("/{}", f.text), fg(YELLOW));
            if f.typing {
                row.put("▏", fg(theme.dim));
            }
            head.push(row.fit(cols, None));
        }
        None => head.push(Line::blank(cols)),
    }

    for group in model::grouped(tabs, frozen, r) {
        headings.push(lines.len());
        // Build failing's heading is in the failing badge's red, so it reads
        // at a glance.
        let mut heading = Row::default();
        heading.put(" ", Style::new());
        if group.group == Group::Failing {
            heading.put(
                format!("✗ {}", group.group.title().to_uppercase()),
                bold(Color::Indexed(160)),
            );
        } else {
            heading.put(group.group.title().to_uppercase(), fg(theme.dim));
        }
        heading.put(format!(" {}", group.tabs.len()), fg(theme.faint));
        lines.push(heading.fit(cols, None));
        owners.push(None);
        let last = group.tabs.len().saturating_sub(1);
        for (i, tab) in group.tabs.iter().enumerate() {
            let chosen = selected == Some(tab.id);
            if chosen {
                cursor_at = lines.len();
            }
            let fill = if chosen {
                Some(theme.highlight)
            } else if view.hover == Some(&Owner::Tab(tab.id)) {
                Some(theme.panel)
            } else {
                None
            };
            let bar_color = tab.color.map(Color::from).unwrap_or(theme.faint);
            let bar = (if chosen { "▌" } else { "▏" }, fg(bar_color));
            let name_style = if tab.id == view.here {
                bold(theme.bright)
            } else {
                fg(theme.text)
            };
            // The name wraps under itself, up to NAME_LINES lines.
            let indent = 6 + tab.index.to_string().len();
            let room = cols.saturating_sub(indent);
            let wrapped = text::wrap(&tab.name, room);
            let name: Vec<String> = if wrapped.len() > NAME_LINES {
                let mut kept = wrapped[..NAME_LINES - 1].to_vec();
                kept.push(text::cut(&wrapped[NAME_LINES - 1..].join(" "), room));
                kept
            } else {
                wrapped
            };
            let state = model::badge_state(tab, r);
            spins |= state.spins();
            let mut first = Row::default();
            first
                .put(bar.0, bar.1)
                .spans
                .push(badge(state, view.frame, theme));
            first
                .put(" ", Style::new())
                .put(tab.index.to_string(), fg(theme.faint))
                .put(" ", Style::new())
                .put(name[0].clone(), name_style);
            let mut tab_lines = vec![first.fit(cols, fill)];
            for line in &name[1..] {
                let mut row = Row::default();
                row.put(bar.0, bar.1)
                    .put(" ".repeat(indent - 1), Style::new())
                    .put(line.clone(), name_style);
                tab_lines.push(row.fit(cols, fill));
            }
            // What it is doing, packed onto as few lines as fit, breaking
            // only between its parts, so a PR number is never cut. The PR's
            // number, unless its name already says it; a failing check,
            // which needs you, so the sidebar says so too.
            let pr = tab
                .pr
                .map(|n| format!("#{n}"))
                .filter(|pr| !tab.name.contains(pr.as_str()));
            let failing = tab
                .checks
                .filter(|c| c.fail > 0)
                .map(|c| format!("✗{} failing", c.fail));
            let mut parts: Vec<String> = model::state_words(tab, r)
                .split(" · ")
                .filter(|p| !p.is_empty())
                .map(str::to_string)
                .collect();
            parts.extend(pr);
            parts.extend(failing.clone());
            if tab.notes_here {
                parts.push("✎ Notes here".into());
            }
            if tab.autopilot {
                parts.push("auto".into());
            }
            let mut details: Vec<String> = Vec::new();
            for part in parts {
                match details.last_mut() {
                    Some(last) if width(&format!("{last} · {part}")) <= cols.saturating_sub(6) => {
                        last.push_str(" · ");
                        last.push_str(&part);
                    }
                    _ => details.push(part),
                }
            }
            let state_color = state.color().unwrap_or(theme.dim);
            let color = if tab.state == TabState::Idle {
                theme.dim
            } else {
                state_color
            };
            for detail in details {
                let mut row = Row::default();
                row.put(bar.0, bar.1).put("     ", Style::new());
                // The failing checks in bold red whatever the tab is doing,
                // so a build failing under a working tab still stands out.
                match failing
                    .as_deref()
                    .and_then(|f| detail.find(f).map(|at| (f, at)))
                {
                    Some((f, at)) => {
                        row.put(detail[..at].to_string(), fg(color))
                            .put(f.to_string(), bold(Color::Indexed(160)))
                            .put(detail[at + f.len()..].to_string(), fg(color));
                    }
                    None => {
                        row.put(detail, fg(color));
                    }
                }
                tab_lines.push(row.fit(cols, fill));
            }
            for line in tab_lines {
                lines.push(line);
                owners.push(Some(tab.id));
            }
            if chosen {
                cursor_end = lines.len() - 1;
            }
            // A faint rule between tabs, which also spaces them apart.
            if i < last {
                let mut rule = Row::default();
                rule.put(" ", Style::new())
                    .put("─".repeat(cols.saturating_sub(2)), fg(theme.highlight));
                lines.push(rule.fit(cols, None));
                owners.push(None);
            }
        }
        lines.push(Line::blank(cols));
        owners.push(None);
    }

    // Each plugin that asks for it, its first items under its title, after
    // the tabs and scrolling with them.
    for section in sections {
        let mut heading = Row::default();
        heading
            .put(" ", Style::new())
            .put(section.title.to_uppercase(), fg(theme.dim))
            .put(format!(" {}", section.count), fg(theme.faint));
        if section.attention > 0 {
            heading.put(format!(" ! {}", section.attention), bold(YELLOW));
        }
        lines.push(heading.fit(cols, None));
        owners.push(None);
        if let Some(error) = &section.error {
            let mut row = Row::default();
            row.put("  ", Style::new())
                .put(error.clone(), fg(Color::Indexed(167)));
            lines.push(row.fit(cols, None));
            owners.push(None);
        }
        for item in &section.items {
            let mut row = Row::default();
            row.put("  ", Style::new());
            row.spans.push(item_mark(item, theme));
            row.put(" ", Style::new())
                .put(item.title.clone(), fg(theme.text));
            lines.push(row.fit(cols, None));
            owners.push(None);
        }
        lines.push(Line::blank(cols));
        owners.push(None);
    }

    // Notifications, pinned to the bottom under a double rule and their own
    // heading, so they read as no tab and never scroll away.
    let mut notice_lines: Vec<Line> = Vec::new();
    let mut notice_owners: Vec<Option<Owner>> = Vec::new();
    if !notices.is_empty() {
        notice_lines.push(Line::blank(cols));
        let mut rule = Row::default();
        rule.put(" ", Style::new())
            .put("═".repeat(cols.saturating_sub(2)), fg(YELLOW));
        notice_lines.push(rule.fit(cols, None));
        let mut heading = Row::default();
        heading
            .put(" ", Style::new())
            .put("NOTIFICATIONS", bold(YELLOW));
        notice_lines.push(heading.fit(cols, None));
        notice_owners.extend([None, None, None]);
        for n in notices {
            // The sidebar's cursor on it, as on a tab.
            let fill = if view.focused && view.cursor.is_some_and(|c| c.same_row(&n.owner)) {
                Some(theme.highlight)
            } else if view.hover == Some(&n.owner) {
                Some(theme.panel)
            } else {
                None
            };
            let mut row = Row::default();
            row.put(if fill.is_some() { "▌" } else { " " }, fg(YELLOW))
                .put(" ", Style::new())
                .put("●", fg(YELLOW))
                .put(" ", Style::new())
                .put(n.title.clone(), bold(theme.bright));
            notice_lines.push(row.fit(cols, fill));
            notice_owners.push(Some(n.owner.clone()));
            if !n.detail.is_empty() {
                let mut row = Row::default();
                row.put("    ", Style::new())
                    .put(n.detail.clone(), fg(theme.dim));
                notice_lines.push(row.fit(cols, fill));
                notice_owners.push(Some(n.owner.clone()));
            }
        }
    }

    // The list scrolls to keep the chosen tab in sight between the header
    // and the notifications, from where it was scrolled to (view.top, as C-d
    // and C-u leave it) when it is kept. Else it holds where the session's
    // list was last shown (view.keep) while the tab is in sight there, so
    // going from tab to tab doesn't move it, and otherwise stays at the top
    // while the tab is in sight there, or scrolls to the heading of the
    // tab's group. A list too long for its room keeps its last row to say
    // how many tabs are cut below.
    let length = lines.len();
    let space = rows.saturating_sub(head.len() + notice_lines.len());
    let room = if length > space {
        space.saturating_sub(1)
    } else {
        space
    };
    let in_sight = |at: usize| at <= cursor_at && cursor_end < at + room;
    let steady = match view.keep {
        Some(keep) if in_sight(keep) => keep,
        _ if cursor_end < room => 0,
        _ => headings
            .iter()
            .rev()
            .find(|h| **h <= cursor_at)
            .copied()
            .unwrap_or(0),
    };
    let wanted = view.top.unwrap_or(steady) as isize;
    // Never further than shows the list's end whole.
    let top = wanted
        .max((cursor_end + 1) as isize - room as isize)
        .min(cursor_at as isize)
        .min(length as isize - space as isize)
        .max(0) as usize;
    let whole = length.saturating_sub(top) <= space;
    let end = if whole { length } else { top + room };
    let tabs_in = |from: usize, to: usize| -> Vec<TabId> {
        let mut seen: Vec<TabId> = Vec::new();
        for t in owners[from.min(length)..to.min(length)].iter().flatten() {
            if !seen.contains(t) {
                seen.push(*t);
            }
        }
        seen
    };
    // The tabs out of sight above, said on the filter line while no filter
    // is there, and below, on the list's last row; a click on either
    // scrolls as C-u or C-d does, and lights under the mouse as a tab does.
    let more = |mark: &str, owner: &Owner, n: usize| {
        let fill = (view.hover == Some(owner)).then_some(theme.panel);
        let said = if n > 0 {
            format!("{mark} {n} more")
        } else {
            format!("{mark} more")
        };
        let mut row = Row::default();
        row.put(" ", Style::new()).put(said, fg(theme.faint));
        row.fit(cols, fill)
    };
    let mut head_owners: Vec<Option<Owner>> = vec![None; head.len()];
    if top > 0 && view.filter.is_none() {
        // A tab cut at the top counts as above.
        head[1] = more("▲", &Owner::ScrollUp, tabs_in(0, top).len());
        head_owners[1] = Some(Owner::ScrollUp);
    }
    let below = (!whole).then(|| {
        // A tab cut at the bottom counts as below.
        let shown = tabs_in(top, end);
        tabs_in(end, length)
            .into_iter()
            .filter(|t| !shown.contains(t) || owners.get(end) == Some(&Some(*t)))
            .count()
    });
    let mut shown: Vec<Line> = head;
    let mut shown_owners: Vec<Option<Owner>> = head_owners;
    shown.extend(lines.into_iter().skip(top).take(end - top));
    shown_owners.extend(owners[top..end].iter().map(|o| o.map(Owner::Tab)));
    if let Some(n) = below {
        shown.push(more("▼", &Owner::ScrollDown, n));
        shown_owners.push(Some(Owner::ScrollDown));
    }
    let list_end = rows.saturating_sub(notice_lines.len()).max(shown.len());
    while shown.len() < list_end {
        shown.push(Line::blank(cols));
        shown_owners.push(None);
    }
    shown.extend(notice_lines);
    shown_owners.extend(notice_owners);
    shown.truncate(rows);
    shown_owners.truncate(rows);
    Lines {
        lines: shown,
        owners: shown_owners,
        scroll: Scroll {
            top,
            room: space,
            length,
            owners,
            cursor_at,
        },
        spins,
    }
}

/// Draws the lines into `area` of a buffer, a cell for each column as
/// `text::width` counts it, so what was measured and what is drawn agree.
pub fn draw(buf: &mut Buffer, area: Rect, lines: &[Line]) {
    let area = area.intersection(buf.area);
    for (row, line) in lines.iter().enumerate().take(area.height as usize) {
        let y = area.y + row as u16;
        for x in area.left()..area.right() {
            let cell = &mut buf[(x, y)];
            cell.reset();
            if let Some(fill) = line.fill {
                cell.set_bg(fill);
            }
        }
        let mut x = area.x;
        for span in &line.spans {
            let mut before = 0;
            for c in span.content.chars() {
                let w = text::width_after(c, before);
                before = w;
                if w == 0 {
                    // Drawn on the character before it.
                    if x > area.x {
                        let cell = &mut buf[(x - 1, y)];
                        let mut symbol = cell.symbol().to_string();
                        symbol.push(c);
                        cell.set_symbol(&symbol);
                    }
                    continue;
                }
                if x + w as u16 > area.right() {
                    break;
                }
                let cell = &mut buf[(x, y)];
                cell.set_char(c);
                cell.set_style(span.style);
                for skip in 1..w as u16 {
                    let next = &mut buf[(x + skip, y)];
                    next.set_symbol("");
                    next.set_style(span.style);
                }
                x += w as u16;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::facts::{Checks, PrState};
    use crate::sidebar::model::tests::{NOW_MS, rules, tab};
    use crate::sidebar::model::{MergeState, TabLoop};

    fn view(here: u32) -> View<'static> {
        View {
            focused: false,
            cursor: None,
            frame: 0,
            here: TabId(here),
            filter: None,
            top: None,
            keep: None,
            hover: None,
        }
    }

    fn texts(out: &Lines) -> Vec<String> {
        out.lines
            .iter()
            .map(|l| l.text().trim_end().to_string())
            .collect()
    }

    fn lines(tabs: &[TabModel], cols: usize, rows: usize, v: &View<'_>) -> Lines {
        sidebar_lines(
            tabs,
            None,
            &[],
            &[],
            "s",
            cols,
            rows,
            v,
            &rules(),
            &Theme::DARK,
        )
    }

    #[test]
    fn every_line_is_the_sidebars_width_however_long_the_names() {
        let mut long = tab(
            1,
            &"a very long name that goes on and on ".repeat(4),
            TabState::Waiting,
        );
        long.wait = Some("permission: Bash a very long command line indeed".into());
        let tabs = vec![long, tab(22, "日本語のタブ名前", TabState::Working)];
        let out = lines(&tabs, 30, 20, &view(1));
        assert_eq!(out.lines.len(), 20);
        for line in &out.lines {
            assert_eq!(width(&line.text()), 30, "{:?}", line.text());
        }
        // The name wraps to three lines at most, the last cut.
        let name_lines = out
            .owners
            .iter()
            .zip(&out.lines)
            .filter(|(o, l)| **o == Some(Owner::Tab(TabId(1))) && l.text().contains("a very"))
            .count();
        assert_eq!(name_lines, 3);
        assert!(out.spins, "a working tab's badge spins");
    }

    #[test]
    fn a_detail_too_long_breaks_between_its_parts() {
        let mut t = tab(1, "work", TabState::Idle);
        t.claude = None;
        t.command = Some("cat".into());
        t.since = Some(rules().now_secs() - 300);
        t.pr = Some(12345);
        let out = lines(&[t], 26, 8, &view(1));
        let text = texts(&out);
        assert_eq!(text[4..6], ["▌     idle for 5m · cat", "▌     #12345"]);
    }

    #[test]
    fn tabs_are_set_apart_by_a_rule_and_grouped_under_headings() {
        let tabs = vec![tab(1, "one", TabState::Idle), tab(2, "two", TabState::Idle)];
        let out = lines(&tabs, 30, 14, &view(1));
        let text: Vec<String> = out.lines.iter().map(Line::text).collect();
        assert!(text[2].starts_with(" IDLE 2"));
        assert_eq!(text[4], format!(" {} ", "─".repeat(28)));
        assert_eq!(out.owners[4], None);
        assert_eq!(out.owners[3], Some(Owner::Tab(TabId(1))));
        assert_eq!(out.owners[5], Some(Owner::Tab(TabId(2))));
    }

    #[test]
    fn the_header_counts_and_a_pr_said_once() {
        let r = rules();
        let mut failing = tab(2, "red idle", TabState::Idle);
        failing.pr = Some(12);
        failing.pr_state = Some(PrState::Open);
        failing.checks = Some(Checks {
            pass: 3,
            fail: 1,
            pending: 0,
        });
        let mut waiting = tab(1, "Fix #7 regressions", TabState::Waiting);
        waiting.pr = Some(7);
        let tabs = vec![waiting, failing, tab(3, "busy", TabState::Working)];
        let out = sidebar_lines(
            &tabs,
            None,
            &[],
            &[],
            "main",
            40,
            30,
            &view(1),
            &r,
            &Theme::DARK,
        );
        let text = texts(&out).join("\n");
        assert!(
            text.lines()
                .next()
                .unwrap()
                .starts_with("  main   ✗ 1  ! 1  ⋯ 1"),
            "{text}"
        );
        assert!(text.contains("✗ BUILD FAILING 1"), "{text}");
        assert!(text.contains(" ✗  2 red idle"), "{text}");
        assert!(text.contains("✗1 failing"), "{text}");
        assert_eq!(text.matches("#7").count(), 1, "said by its name already");
        assert!(text.contains("#12"));
        // The failing count in bold red, whatever the tab does.
        let red = out
            .lines
            .iter()
            .flat_map(|l| &l.spans)
            .find(|s| s.content == "✗1 failing")
            .unwrap();
        assert_eq!(red.style.fg, Some(Color::Indexed(160)));
        assert!(red.style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn badges_wear_their_states_colors_and_marks() {
        let r = rules();
        let mut queued = tab(2, "queued third", TabState::Idle);
        queued.merge = Some(MergeState::Queued);
        queued.position = Some(3);
        let mut landed = tab(4, "landed", TabState::Idle);
        landed.merge = Some(MergeState::Merged);
        let mut pinned = tab(5, "pinned", TabState::Working);
        pinned.pinned = true;
        let mut reviewing = tab(6, "review", TabState::Background);
        reviewing.background = 2;
        reviewing.tab_loop = Some(TabLoop::Run(crate::sidebar::claude::LoopRun {
            kind: "review",
            round: 2,
        }));
        let mut later = tab(7, "later", TabState::Idle);
        later.remind_at = Some(NOW_MS / 1000 + 3600);
        let tabs = vec![queued, landed, pinned, reviewing, later];
        let out = sidebar_lines(
            &tabs,
            None,
            &[],
            &[],
            "s",
            40,
            40,
            &view(2),
            &r,
            &Theme::DARK,
        );
        let text = texts(&out).join("\n");
        assert!(text.contains("IN MERGE QUEUE 1"));
        assert!(text.contains(" »  2 queued third"), "{text}");
        assert!(text.contains(" ◆  4 landed"), "{text}");
        assert!(text.contains(" 📌 5 pinned"), "{text}");
        assert!(text.contains(" ⏰ 7 later"), "{text}");
        let review_badge = out
            .lines
            .iter()
            .flat_map(|l| &l.spans)
            .find(|s| s.content == " ⠋ " && s.style.bg == Some(Color::Indexed(97)));
        assert!(review_badge.is_some(), "the review loop's own color");
        // The spinner turns with the frame.
        let turned = sidebar_lines(
            &tabs,
            None,
            &[],
            &[],
            "s",
            40,
            40,
            &View {
                frame: 1,
                ..view(2)
            },
            &r,
            &Theme::DARK,
        );
        assert!(texts(&turned).join("\n").contains(" ⠙  6 review"));
    }

    #[test]
    fn notifications_sit_at_the_bottom_under_their_own_heading() {
        let t = tab(1, "work", TabState::Idle);
        let notice = Notice {
            owner: Owner::Notice {
                source: "slack".into(),
                id: "n1".into(),
            },
            title: "3 new on Slack".into(),
            detail: "Ada · #general".into(),
        };
        let out = sidebar_lines(
            std::slice::from_ref(&t),
            None,
            std::slice::from_ref(&notice),
            &[],
            "s",
            40,
            20,
            &view(1),
            &rules(),
            &Theme::DARK,
        );
        assert_eq!(out.lines.len(), 20);
        let text = texts(&out);
        assert_eq!(
            text[16..],
            [
                format!(" {}", "═".repeat(38)),
                " NOTIFICATIONS".to_string(),
                "  ● 3 new on Slack".to_string(),
                "    Ada · #general".to_string(),
            ]
        );
        assert_eq!(out.owners[18], Some(notice.owner.clone()));
        assert_eq!(out.owners[19], Some(notice.owner.clone()));
        let chosen = sidebar_lines(
            std::slice::from_ref(&t),
            None,
            std::slice::from_ref(&notice),
            &[],
            "s",
            40,
            20,
            &View {
                focused: true,
                cursor: Some(&notice.owner),
                ..view(1)
            },
            &rules(),
            &Theme::DARK,
        );
        assert!(chosen.lines[18].text().starts_with("▌ ● 3 new on Slack"));
        assert_eq!(chosen.lines[18].fill, Some(Theme::DARK.highlight));
        let none = lines(&[t], 40, 20, &view(1));
        assert!(!texts(&none).iter().any(|l| l.contains("NOTIFICATIONS")));
    }

    #[test]
    fn a_plugins_first_items_sit_under_the_tabs_with_its_count() {
        let t = tab(1, "work", TabState::Idle);
        let items = vec![
            Item::new("1", "api rolling out").state(State::Attention),
            Item::new("2", "web").state(State::Active),
            Item::new("3", "jobs"),
        ];
        let section = Section::of("Deploys", &items, Some("one failed"), 2);
        assert_eq!(section.count, 3);
        assert_eq!(section.attention, 1);
        let out = sidebar_lines(
            std::slice::from_ref(&t),
            None,
            &[],
            std::slice::from_ref(&section),
            "s",
            40,
            20,
            &view(1),
            &rules(),
            &Theme::DARK,
        );
        let text = texts(&out);
        let at = text.iter().position(|l| l.starts_with(" DEPLOYS")).unwrap();
        assert_eq!(
            text[at..at + 4],
            [
                " DEPLOYS 3 ! 1",
                "  one failed",
                "  ● api rolling out",
                "  ● web",
            ]
        );
        assert_eq!(text[at + 4], "", "a blank line after it");
        assert!(at > 2, "under the tabs");
        assert!(out.owners[at..at + 4].iter().all(Option::is_none));
    }

    #[test]
    fn the_list_holds_where_it_was_scrolled_while_the_cursor_is_in_sight() {
        let tabs: Vec<TabModel> = (1..=8)
            .map(|n| {
                let mut t = tab(n, &format!("tab {n}"), TabState::Idle);
                t.claude = None;
                t
            })
            .collect();
        let at = |cursor: u32, top: Option<usize>| {
            let owner = Owner::Tab(TabId(cursor));
            let v = View {
                focused: true,
                cursor: Some(&owner),
                top,
                ..view(1)
            };
            lines(&tabs, 30, 10, &v).scroll
        };
        let free = at(3, None);
        assert_eq!(free.top, 0, "nothing kept: as little scrolling as it takes");
        assert_eq!(
            at(3, Some(9)).top,
            free.cursor_at,
            "kept past the cursor: back up to it"
        );
        assert_eq!(at(5, Some(4)).top, 4, "the cursor in sight: the list holds");
        let last = at(8, Some(0));
        assert!(last.top > 0 && last.top <= last.cursor_at);
        assert_eq!(
            at(8, Some(99)).top,
            last.length - last.room,
            "never past the end"
        );
    }

    #[test]
    fn on_a_short_screen_the_header_stays_says_what_is_out_of_sight_and_the_list_holds_still() {
        // Three groups of two-line tabs: 3 waiting, 4 working, 5 idle.
        let tabs: Vec<TabModel> = (1..=12)
            .map(|n| {
                let state = match n {
                    1..=3 => TabState::Waiting,
                    4..=7 => TabState::Working,
                    _ => TabState::Idle,
                };
                let mut t = tab(n, &format!("tab {n}"), state);
                t.since = Some(rules().now_secs() - 60);
                t
            })
            .collect();
        let draw = |here: u32, keep: Option<usize>, filter: Option<&Filter>| {
            let v = View {
                keep,
                filter,
                ..view(here)
            };
            lines(&tabs, 30, 16, &v)
        };
        for here in [1, 5, 7, 12] {
            assert!(
                texts(&draw(here, None, None))[0].contains(" s "),
                "the session's pill first, beside tab {here}"
            );
        }
        let first = draw(1, None, None);
        let text = texts(&first);
        assert_eq!(text[1], "", "at the top, the filter line is blank");
        assert_eq!(
            text[15], " ▼ 8 more",
            "and the last row says how many tabs are cut below"
        );
        assert_eq!(first.owners[15], Some(Owner::ScrollDown));
        // Beside a tab in sight at the top, the same list, but for which tab
        // is the one it sits beside.
        let unmarked = |out: &Lines| -> Vec<String> {
            texts(out)[1..]
                .iter()
                .map(|l| l.replace(['▌', '▏'], ""))
                .collect()
        };
        assert_eq!(
            unmarked(&draw(2, None, None)),
            unmarked(&first),
            "two tabs in sight show the same list"
        );
        let low = draw(7, None, None);
        let text = texts(&low);
        assert!(
            text[2].starts_with(" WORKING 4"),
            "a tab below the fold scrolls the list to its group's heading: {text:?}"
        );
        assert_eq!(
            unmarked(&draw(5, None, None)),
            unmarked(&low),
            "and every tab in that group shows it the same"
        );
        assert_eq!(
            text[1], " ▲ 3 more",
            "the filter line says how many are above"
        );
        assert_eq!(low.owners[1], Some(Owner::ScrollUp));
        assert_eq!(text[15], " ▼ 5 more");
        // Each tab's rows still name it, shifted under the header.
        for (owner, line) in low.owners.iter().zip(&text) {
            if let Some(Owner::Tab(t)) = owner
                && line.contains("tab ")
            {
                assert!(
                    line.contains(&format!("tab {} ", t.0))
                        || line.ends_with(&format!("tab {}", t.0)),
                    "{line:?} belongs to {t:?}"
                );
            }
        }
        assert!(low.owners.contains(&Some(Owner::Tab(TabId(7)))));
        let kept = Filter {
            text: "tab".into(),
            typing: false,
        };
        assert!(
            texts(&draw(7, None, Some(&kept)))[1].contains("/tab"),
            "a filter keeps its line"
        );
        // Under the mouse, the more row lights as a tab's does.
        let lit = lines(
            &tabs,
            30,
            16,
            &View {
                hover: Some(&Owner::ScrollDown),
                ..view(7)
            },
        );
        assert_eq!(lit.lines[15].fill, Some(Theme::DARK.panel));

        // A group taller than the list: beside a tab in sight where the list
        // was last shown (keep), it is shown there too.
        let many: Vec<TabModel> = (1..=20)
            .map(|n| tab(n, &format!("tab {n}"), TabState::Working))
            .collect();
        let tall = |here: u32, keep: Option<usize>| {
            lines(&many, 30, 40, &View { keep, ..view(here) })
                .scroll
                .top
        };
        let front = tall(15, None);
        assert!(front > 0);
        assert_eq!(
            tall(14, Some(front)),
            front,
            "going to a tab in sight leaves the list where it was"
        );
        assert!(
            tall(20, Some(front)) > front,
            "and a tab out of sight still scrolls it"
        );
    }

    #[test]
    fn a_filter_shows_under_the_header_and_the_chosen_tab_is_lit() {
        let tabs = vec![tab(1, "one", TabState::Idle), tab(2, "two", TabState::Idle)];
        let filter = Filter {
            text: "tw".into(),
            typing: true,
        };
        let owner = Owner::Tab(TabId(2));
        let v = View {
            focused: true,
            cursor: Some(&owner),
            filter: Some(&filter),
            hover: Some(&Owner::Tab(TabId(1))),
            ..view(1)
        };
        let out = lines(&tabs, 30, 12, &v);
        assert!(out.lines[1].text().starts_with(" /tw▏"));
        let two = out
            .owners
            .iter()
            .position(|o| *o == Some(owner.clone()))
            .unwrap();
        assert_eq!(out.lines[two].fill, Some(Theme::DARK.highlight));
        assert!(out.lines[two].text().starts_with("▌"));
        let one = out
            .owners
            .iter()
            .position(|o| *o == Some(Owner::Tab(TabId(1))))
            .unwrap();
        assert_eq!(
            out.lines[one].fill,
            Some(Theme::DARK.panel),
            "under the mouse"
        );
    }

    #[test]
    fn drawing_puts_each_character_in_the_columns_it_takes() {
        let line = Row::default();
        let mut row = line;
        row.put("a📌b", Style::new());
        let lines = vec![row.fit(6, Some(Color::Indexed(238)))];
        let mut buf = Buffer::empty(Rect::new(0, 0, 8, 1));
        draw(&mut buf, Rect::new(1, 0, 6, 1), &lines);
        assert_eq!(buf[(1, 0)].symbol(), "a");
        assert_eq!(buf[(2, 0)].symbol(), "📌");
        assert_eq!(buf[(4, 0)].symbol(), "b");
        assert_eq!(buf[(6, 0)].bg, Color::Indexed(238));
        assert_eq!(buf[(7, 0)].symbol(), " ", "nothing past its area");
    }
}
