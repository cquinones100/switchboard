//! Several of mission control's views at once (board 11 of the UX 2.0
//! canvas, new with the port, with no tmux-version file behind it): `=`
//! opens the "Views at once" box (`views_box`, board 32), whose Enter shows
//! two to four views under the bar in a layout (2 × 2, one beside two
//! stacked, or side by side), each under a label with its count, and the
//! one with the focus taking the keys as the view in front does alone.
//!
//! What is picked is kept by each view's key (`View::key`, "tabs" for
//! the cards, the same whatever its title says), so a view turned on or off between two draws leaves the
//! others in their places, and saved in the state directory
//! (`mission-grid.json`), so the next `=`, after a restart too, starts
//! from it. The split of the screen is a plain function (`split`): the
//! cells' columns stretch by board 45's weights through `kit::columns`,
//! none under `CELL_MIN`, and a layout whose columns don't fit falls back
//! to 2 × 2, then to the focused view alone.

use std::io;
use std::path::{Path, PathBuf};

use ratatui::style::{Modifier, Style};
use ratatui::text::Span;
use serde::{Deserialize, Serialize};

use super::render::{self, Image, Picture, Shape};
use super::views::{self, View, ViewId};
use super::{HitBox, Mission, Spot};
use crate::items::State;
use crate::sidebar::render::Line;
use crate::sidebar::text::width;
use crate::ui::Theme;
use crate::ui::kit::{self, Col, Tone};

/// The most views shown at once.
pub const MOST: usize = 4;
/// The cards' slug.
pub const TABS: &str = "tabs";
/// The narrowest a cell's columns go, and the fewest rows it has: its
/// label and a board's column heading over one of its cards.
pub const CELL_MIN: (usize, usize) = (34, 9);
/// Each cell's share of the width, board 11's 62 columns at 125.
const WEIGHT: usize = 62;
/// The file the choice is kept in, in the state directory.
const FILE: &str = "mission-grid.json";

/// How the views are laid out.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Layout {
    /// Two over two.
    #[default]
    Grid,
    /// The first over the whole height at the left, the rest stacked at
    /// the right.
    OneTwo,
    /// All side by side.
    Side,
}

impl Layout {
    pub const ALL: [Layout; 3] = [Layout::Grid, Layout::OneTwo, Layout::Side];

    /// Its key in the box.
    pub fn key(self) -> char {
        match self {
            Layout::Grid => '1',
            Layout::OneTwo => '2',
            Layout::Side => '3',
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Layout::Grid => "2 × 2",
            Layout::OneTwo => "1 + 2",
            Layout::Side => "side by side",
        }
    }

    pub fn of_key(c: char) -> Option<Layout> {
        Layout::ALL.into_iter().find(|l| l.key() == c)
    }
}

/// What was picked in the box: the views by slug in their places, the
/// first taking the focus, and the layout.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Choice {
    pub views: Vec<String>,
    pub layout: Layout,
}

impl Default for Choice {
    /// Board 11's: Slack, the Inbox, Usage and the Calendar, 2 × 2.
    fn default() -> Choice {
        Choice {
            views: ["slack", "inbox", "usage", "calendar"]
                .map(String::from)
                .to_vec(),
            layout: Layout::Grid,
        }
    }
}

fn file(state: &Path) -> PathBuf {
    state.join(FILE)
}

/// The choice kept last time; `None` before the first, or for a file that
/// doesn't read as one.
pub fn load(state: &Path) -> Option<Choice> {
    let text = std::fs::read_to_string(file(state)).ok()?;
    serde_json::from_str(&text).ok()
}

pub fn save(state: &Path, choice: &Choice) -> io::Result<()> {
    std::fs::create_dir_all(state)?;
    let json = serde_json::to_string_pretty(choice).map_err(io::Error::other)?;
    crate::util::write_atomic(file(state), json + "\n")
}

/// The views on screen: the choice, the one with the focus (its place),
/// and the cursor each was left at.
#[derive(Clone, Debug, PartialEq)]
pub struct Grid {
    pub choice: Choice,
    pub focus: usize,
    pub cursors: Vec<isize>,
    /// The item each was left on, by its id, which its cursor follows when
    /// its rows move while another has the focus.
    pub chosen: Vec<Option<String>>,
}

impl Grid {
    pub fn new(choice: Choice) -> Grid {
        let cursors = vec![0; choice.views.len()];
        let chosen = vec![None; choice.views.len()];
        Grid {
            choice,
            focus: 0,
            cursors,
            chosen,
        }
    }

    /// The slug of the view with the focus.
    pub fn focused(&self) -> &str {
        &self.choice.views[self.focus]
    }
}

/// A column of cells: its x and width, and its cells top to bottom, each
/// its place among the views shown (`None` for the empty fourth of three
/// in 2 × 2), its first row and its rows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Column {
    pub x: usize,
    pub w: usize,
    pub cells: Vec<(Option<usize>, usize, usize)>,
}

/// Which cells each column stacks, by layout, for `n` views.
fn stacks(layout: Layout, n: usize) -> Vec<Vec<Option<usize>>> {
    match (layout, n) {
        (_, 0 | 1) => vec![vec![Some(0)]],
        (Layout::Side, _) | (_, 2) => (0..n).map(|i| vec![Some(i)]).collect(),
        (Layout::Grid, _) => vec![vec![Some(0), Some(2)], vec![Some(1), (n > 3).then_some(3)]],
        (Layout::OneTwo, _) => vec![vec![Some(0)], (1..n).map(Some).collect()],
    }
}

/// The body `cols` by `rows` split for `n` views in `layout`: the columns
/// share the width by weight a rule apart, and each column's cells share
/// its rows a rule apart, none under `min` (columns, rows). A layout whose
/// cells don't fit gives way to 2 × 2, and that to `None`, the focused
/// view alone.
pub fn split(
    layout: Layout,
    n: usize,
    cols: usize,
    rows: usize,
    min: (usize, usize),
) -> Option<Vec<Column>> {
    let stacked = stacks(layout, n.min(MOST));
    let wide = stacked.len() * min.0 + stacked.len().saturating_sub(1) <= cols;
    let tall = stacked
        .iter()
        .all(|cells| rows.saturating_sub(cells.len() - 1) / cells.len() >= min.1);
    if !(wide && tall) {
        return match layout {
            Layout::Grid => None,
            _ if n > 2 => split(Layout::Grid, n, cols, rows, min),
            _ => None,
        };
    }
    let spec: Vec<Col> = stacked
        .iter()
        .map(|_| Col::Stretch {
            weight: WEIGHT,
            min: min.0,
            max: None,
        })
        .collect();
    let at = kit::columns(&spec, cols, 1, &[]);
    Some(
        stacked
            .into_iter()
            .zip(at)
            .map(|(cells, at)| {
                let (x, w) = at.unwrap_or((0, min.0));
                let k = cells.len();
                let content = rows.saturating_sub(k - 1);
                let mut y = 0;
                let cells = cells
                    .into_iter()
                    .enumerate()
                    .map(|(i, cell)| {
                        let h = content / k + usize::from(i < content % k);
                        let placed = (cell, y, h);
                        y += h + 1;
                        placed
                    })
                    .collect();
                Column { x, w, cells }
            })
            .collect(),
    )
}

/// Where each view shown sits: its place, x, y, width and rows.
pub fn areas(columns: &[Column]) -> Vec<(usize, usize, usize, usize, usize)> {
    let mut out: Vec<_> = columns
        .iter()
        .flat_map(|c| {
            c.cells
                .iter()
                .filter_map(move |&(cell, y, h)| cell.map(|i| (i, c.x, y, c.w, h)))
        })
        .collect();
    out.sort_by_key(|a| a.0);
    out
}

// ── Drawing ─────────────────────────────────────────────────────────────────

/// What a draw gives: the lines, where a click lands, the images and the
/// pictures placed over them.
type Drawn = (Vec<Line>, Vec<HitBox>, Vec<Image>, Vec<Picture>);

/// Mission control with several views at once on a screen `cols` by
/// `rows`: the bar, the views in their cells a rule apart, each under its
/// label and drawn by the code that draws it alone (Usage and the Calendar
/// as a cell's list), and the footer. The view with the focus is drawn
/// with its chosen row and takes clicks on its rows; a click on another
/// gives it the focus. A cell draws no pictures.
pub fn drawn(m: &mut Mission, cols: usize, rows: usize) -> Drawn {
    let mut hits = Vec::new();
    let mut meters = Vec::new();
    let mut lines = render::bar(m, cols, &mut hits);
    let top = lines.len();
    let body_rows = rows.saturating_sub(1 + top);
    let shown = m.grid_shown();
    let (layout, focus) = m
        .grid
        .as_ref()
        .map_or((Layout::Grid, 0), |g| (g.choice.layout, g.focus));
    let focused_at = shown.iter().position(|(c, _)| *c == focus).unwrap_or(0);
    let columns = split(layout, shown.len(), cols, body_rows, CELL_MIN).unwrap_or_else(|| {
        vec![Column {
            x: 0,
            w: cols,
            cells: vec![(Some(focused_at), 0, body_rows)],
        }]
    });
    let mut cells: Vec<Option<Vec<Line>>> = vec![None; shown.len()];
    m.cells.clear();
    for (i, x, y, w, h) in areas(&columns) {
        let Some(&(cell, at)) = shown.get(i) else {
            continue;
        };
        // The keys lay the view out in its cell, under its label.
        m.cells.push((cell, (w as u16, h as u16)));
        let focused = cell == focus;
        let saved = m.grid.as_ref().map_or(0, |g| g.cursors[cell]);
        let (cell_lines, cell_hits, cell_meters) = draw_cell(m, at, focused, saved, w, h);
        let (dx, dy) = (x as u16, (top + y) as u16);
        if focused {
            hits.extend(cell_hits.into_iter().map(|hit| HitBox {
                y: hit.y + dy,
                x0: hit.x0 + dx,
                x1: hit.x1 + dx,
                spot: hit.spot,
            }));
        } else {
            hits.extend((0..h as u16).map(|r| HitBox {
                y: dy + r,
                x0: dx,
                x1: dx + (w as u16).saturating_sub(1),
                spot: Spot::Cell(cell),
            }));
        }
        meters.extend(cell_meters.into_iter().map(|image| Image {
            x: image.x + dx,
            y: image.y + dy,
            ..image
        }));
        cells[i] = Some(cell_lines);
    }
    let theme = m.snap.theme;
    lines.extend(compose(&columns, &cells, cols, body_rows, &theme));
    let m: &Mission = m;
    let view = m.current();
    let (lines, hits, meters) = render::finish(m, view, cols, rows, lines, hits, meters);
    (lines, hits, meters, Vec::new())
}

/// The cells' lines side by side, the columns a rule apart and the cells
/// of a column a rule apart, `┼` where the rules cross.
fn compose(
    columns: &[Column],
    cells: &[Option<Vec<Line>>],
    cols: usize,
    rows: usize,
    theme: &Theme,
) -> Vec<Line> {
    let ruled = |col: &Column, r: usize| col.cells.iter().any(|&(_, y, h)| r == y + h);
    let edge = Style::new().fg(kit::border(theme));
    (0..rows)
        .map(|r| {
            let mut spans = Vec::new();
            for (ci, col) in columns.iter().enumerate() {
                if ci > 0 {
                    let sep = match (ruled(&columns[ci - 1], r), ruled(col, r)) {
                        (true, true) => "┼",
                        (true, false) => "┤",
                        (false, true) => "├",
                        (false, false) => "│",
                    };
                    spans.push(Span::styled(sep, edge));
                }
                if ruled(col, r) {
                    spans.push(kit::rule(col.w, theme));
                    continue;
                }
                let line = col
                    .cells
                    .iter()
                    .find(|&&(_, y, h)| r >= y && r < y + h)
                    .and_then(|&(cell, y, _)| cells.get(cell?)?.as_ref()?.get(r - y).cloned());
                spans.extend(match line {
                    Some(line) => filled(line, col.w),
                    None => vec![Span::raw(" ".repeat(col.w))],
                });
            }
            Line {
                spans: kit::fit(spans, cols, Style::new()),
                fill: None,
            }
        })
        .collect()
}

/// A cell's line `w` wide, its fill (the chosen row's) on every span
/// without a background of its own.
fn filled(line: Line, w: usize) -> Vec<Span<'static>> {
    let spans = kit::fit(line.spans, w, Style::new());
    match line.fill {
        None => spans,
        Some(fill) => spans
            .into_iter()
            .map(|s| {
                let style = if s.style.bg.is_none() {
                    s.style.bg(fill)
                } else {
                    s.style
                };
                Span::styled(s.content, style)
            })
            .collect(),
    }
}

/// One view `w` by `h` as a cell: drawn as the view in front, its own
/// cursor and focus, and with nothing chosen and no filter when it hasn't
/// the focus (`saved` its cursor, which the Settings and Keys boards keep
/// to draw the category or group it was left in). What it changes on `m` to draw is put back.
fn draw_cell(
    m: &mut Mission,
    at: usize,
    focused: bool,
    saved: isize,
    w: usize,
    h: usize,
) -> (Vec<Line>, Vec<HitBox>, Vec<Image>) {
    let kept = (
        m.view,
        m.cursor,
        m.focus,
        m.hover,
        m.side.take(),
        m.ci_check.take(),
        m.filter.take(),
        m.size,
        // Drawn as the view alone, laid out in the cell (`Mission::laid`).
        m.grid.take(),
        // A cell places no pictures, so a diagram stays its text.
        m.snap.cell.take(),
    );
    m.view = at;
    m.size = (w as u16, h as u16);
    if focused {
        m.side = kept.4.clone();
        m.ci_check = kept.5.clone();
        m.filter = kept.6.clone();
    } else {
        // Nothing chosen, but the Settings and Keys boards, which draw the
        // category or group of the row they are on, stay on the row they
        // were left on, the board's own way of showing where it is.
        let view = m.snap.views.get(at.wrapping_sub(1));
        let boarded = view.is_some_and(|v| match v.id {
            ViewId::Settings => render::settings_board_shown(m),
            ViewId::Keys => render::keys_board_shown(m, v, w),
            _ => false,
        });
        m.cursor = if boarded { saved } else { -100 };
        m.focus = None;
        m.hover = None;
    }
    let drawn = {
        let m: &Mission = m;
        let view = m.current();
        let mut lines = vec![label(m, view, w, focused)];
        let mut hits = Vec::new();
        let mut meters = Vec::new();
        match view {
            Some(v) if v.id == ViewId::Usage && v.loaded => {
                usage(m, v, w, h, &mut lines, &mut hits, &mut meters)
            }
            Some(v) if v.id == ViewId::Calendar && super::calendar::is_day(v) => {
                calendar(m, v, w, h, &mut lines, &mut hits)
            }
            _ => {
                render::body(m, view, w, h + 1, &mut lines, &mut hits, &mut meters);
            }
        }
        lines.truncate(h);
        while lines.len() < h {
            lines.push(Line {
                spans: Vec::new(),
                fill: None,
            });
        }
        // What a view lays out past its cell's edge, a board drawn wider
        // than the cell, is neither clicked nor shown over its neighbor.
        hits.retain(|hit| (hit.y as usize) < h && (hit.x0 as usize) < w);
        for hit in &mut hits {
            hit.x1 = hit.x1.min(w.saturating_sub(1) as u16);
        }
        meters.retain(|image| {
            (image.y + image.height) as usize <= h && (image.x + image.width) as usize <= w
        });
        (lines, hits, meters)
    };
    (
        m.view,
        m.cursor,
        m.focus,
        m.hover,
        m.side,
        m.ci_check,
        m.filter,
        m.size,
        m.grid,
        m.snap.cell,
    ) = kept;
    drawn
}

/// A cell's label: the view's name, and what it counts at the cell's right
/// edge, in the accent for the one with the focus.
fn label(m: &Mission, view: Option<&View>, w: usize, focused: bool) -> Line {
    let theme = &m.snap.theme;
    let tone = if focused { Tone::Accent } else { Tone::Dim };
    let mut title = view.map_or("Tabs", views::bar_title).to_uppercase();
    // The Calendar on another day than today names it.
    if let Some(day) = view
        .filter(|v| v.id == ViewId::Calendar)
        .and(m.calendar_day.as_deref())
        .and_then(|d| d.parse::<jiff::civil::Date>().ok())
    {
        let day = day.strftime("%a %b %-d").to_string().to_uppercase();
        title = format!("{title} · {day}");
    }
    // Another day than today counts its events, none of them "left".
    let other_day = m.calendar_day.is_some() && view.is_some_and(|v| v.id == ViewId::Calendar);
    let words = match view.filter(|_| other_day) {
        Some(v) => {
            let n = v
                .data
                .items
                .iter()
                .filter(|i| i.group.as_deref() != Some("All day"))
                .count();
            format!("{n} event{}", if n == 1 { "" } else { "s" })
        }
        None => views::count_words(view, m.snap.tabs.len()),
    };
    let gap = w.saturating_sub(width(&title) + width(&words) + 2).max(1);
    let spans = vec![
        Span::raw(" "),
        Span::styled(title, tone.bold(theme)),
        Span::raw(" ".repeat(gap)),
        Span::styled(words, tone.style(theme)),
    ];
    Line {
        spans: kit::fit(spans, w, Style::new()),
        fill: None,
    }
}

/// A line of dim words, a column in.
fn dim_line(text: &str, w: usize, theme: &Theme) -> Line {
    Line {
        spans: kit::fit(
            vec![
                Span::raw("  "),
                Span::styled(text.to_string(), Tone::Dim.style(theme)),
            ],
            w,
            Style::new(),
        ),
        fill: None,
    }
}

/// Whether a cell `rows` tall under its label leaves an item of its view
/// out, so the cursor passes over it there: the Calendar's events all day,
/// and while it shows `today` those already over, Usage's rows but the tabs that spent today, and
/// the PR and ticket boards' week row when it doesn't fit.
pub fn hidden(view: &View, item: &crate::items::Item, rows: usize, today: bool) -> bool {
    match view.id {
        // A board with no room for a card shows none of its items.
        ViewId::Prs | ViewId::Tickets if !render::cards_fit(rows) => true,
        ViewId::Prs | ViewId::Tickets if !render::week_row_fits(rows) => view
            .columns()
            .and_then(|c| c.last())
            .is_some_and(|week| item.group.as_deref() == Some(*week)),
        ViewId::Calendar if super::calendar::is_day(view) => {
            (today && item.state == State::Idle) || item.group.as_deref() == Some("All day")
        }
        ViewId::Usage if view.loaded => {
            !item.id.starts_with("tab:") || item.detail == "nothing today"
        }
        _ => false,
    }
}

/// A row of a cell's list before it is placed: its spans, the item it is,
/// and the meter over it (its name, how full and its color).
struct ListRow {
    spans: Vec<Span<'static>>,
    item: Option<usize>,
    meter: Option<(String, f64, ratatui::style::Color)>,
}

impl ListRow {
    fn text(spans: Vec<Span<'static>>) -> ListRow {
        ListRow {
            spans,
            item: None,
            meter: None,
        }
    }
}

/// A cell's rows placed under its label, as many as `room` holds, from the
/// first that keeps the chosen row in sight; each item's row chosen or not
/// and taking a click, and its meter at `meter_at` (columns past the row's
/// bar).
#[allow(clippy::too_many_arguments)]
fn place_rows(
    rows: Vec<ListRow>,
    m: &Mission,
    w: usize,
    room: usize,
    meter_at: Option<(usize, usize)>,
    lines: &mut Vec<Line>,
    hits: &mut Vec<HitBox>,
    meters: &mut Vec<Image>,
) {
    let chosen = rows
        .iter()
        .position(|r| r.item.is_some_and(|i| i as isize == m.cursor));
    let first = chosen.map_or(0, |at| (at + 1).saturating_sub(room));
    for row in rows.into_iter().skip(first).take(room) {
        let y = lines.len() as u16;
        if let Some(i) = row.item {
            hits.push(HitBox {
                y,
                x0: 0,
                x1: (w as u16).saturating_sub(1),
                spot: Spot::Index(i as isize),
            });
        }
        if let (Some((name, frac, color)), Some((x, mw))) = (row.meter, meter_at) {
            meters.push(Image {
                name,
                x: (x + 1) as u16,
                y,
                width: mw as u16,
                height: 1,
                shape: Shape::Meter {
                    frac: frac.clamp(0.0, 1.0) as f32,
                    color,
                    tick: None,
                },
            });
        }
        // The chosen row, or the one under the mouse, on the highlight.
        let lit = row
            .item
            .is_some_and(|i| i as isize == m.cursor || m.hover == Some(Spot::Index(i as isize)));
        lines.push(Line {
            spans: kit::row(row.spans, w, lit, &m.snap.theme),
            fill: None,
        });
    }
}

/// `text` flush right in a column `w` wide.
fn right(text: String, w: usize, style: Style) -> Vec<Span<'static>> {
    let pad = w.saturating_sub(width(&text));
    vec![Span::styled(format!("{}{text}", " ".repeat(pad)), style)]
}

/// Usage as a cell: a row a tab that spent today, its number in its color,
/// its name, a meter of its share of the day in its color, the share and
/// the dollars at the right edge; then the 5-hour limit's meter.
fn usage(
    m: &Mission,
    v: &View,
    w: usize,
    h: usize,
    lines: &mut Vec<Line>,
    hits: &mut Vec<HitBox>,
    meters: &mut Vec<Image>,
) {
    let theme = &m.snap.theme;
    let Some(model) = crate::usage::charts::model_of(&v.data.items) else {
        lines.push(dim_line("Nothing spent today yet.", w, theme));
        return;
    };
    // The row's bar takes the first column, and a column is kept clear of
    // the rule at the right.
    let inner = w.saturating_sub(2);
    let names: Vec<String> = model
        .today
        .tabs
        .iter()
        .map(|t| format!("{} {}", t.index, t.name))
        .chain(["5-hour limit".to_string()])
        .collect();
    let dollars: Vec<String> = model
        .today
        .tabs
        .iter()
        .map(|t| format!("${:.2}", t.spent))
        .chain(["100% used".to_string()])
        .collect();
    let spec = [
        Col::Fixed {
            min: 6,
            natural: kit::natural(names.iter().map(String::as_str)),
            cap: 18,
        },
        Col::Stretch {
            weight: 1,
            min: 6,
            max: None,
        },
        Col::Fixed {
            min: 4,
            natural: 4,
            cap: 4,
        },
        Col::Right {
            natural: kit::natural(dollars.iter().map(String::as_str)),
            cap: 10,
        },
    ];
    let at = kit::columns(&spec, inner, 1, &[2]);
    let mut rows = Vec::new();
    if model.today.tabs.is_empty() {
        lines.push(dim_line("Nothing spent today yet.", w, theme));
    }
    for (t, name) in model.today.tabs.iter().zip(&names) {
        // A tab the filter left out has no row of the view's.
        let Some(item) = v
            .data
            .items
            .iter()
            .position(|i| i.id == format!("tab:{}", t.key))
        else {
            continue;
        };
        let color = super::usage_board::color_of(t, theme);
        let mut spans = Vec::new();
        if let Some(col) = at[0] {
            let (number, rest) = name.split_once(' ').unwrap_or((name, ""));
            kit::place(
                &mut spans,
                vec![
                    Span::styled(
                        number.to_string(),
                        Style::new().fg(color).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(format!(" {rest}"), Tone::Text.style(theme)),
                ],
                col,
            );
        }
        if let Some(col) = at[1] {
            kit::place(&mut spans, Vec::new(), col);
        }
        if let Some(col) = at[2] {
            let pct = format!("{:>3}%", (t.share * 100.0).round() as i64);
            kit::place(
                &mut spans,
                vec![Span::styled(pct, Tone::Dim.style(theme))],
                col,
            );
        }
        if let Some(col) = at[3] {
            let text = format!("${:.2}", t.spent);
            kit::place(&mut spans, right(text, col.1, Tone::Text.bold(theme)), col);
        }
        rows.push(ListRow {
            spans,
            item: Some(item),
            meter: Some((format!("grid/usage/{}", t.key), t.share, color)),
        });
    }
    // The limit stays under the tabs, however many they are.
    let mut tail = Vec::new();
    if let Some(limit) = model.limits.iter().find(|l| l.key == "five_hour") {
        tail.push(ListRow::text(Vec::new()));
        let tone = Tone::Needs;
        let mut spans = Vec::new();
        if let Some(col) = at[0] {
            kit::place(
                &mut spans,
                vec![Span::styled(limit.name.clone(), Tone::Text.style(theme))],
                col,
            );
        }
        if let Some(col) = at[1] {
            kit::place(&mut spans, Vec::new(), col);
        }
        if let Some(col) = at[3] {
            let text = format!("{}% used", limit.used.round() as i64);
            kit::place(&mut spans, right(text, col.1, tone.bold(theme)), col);
        }
        tail.push(ListRow {
            spans,
            item: None,
            meter: Some((
                "grid/usage/five_hour".into(),
                limit.used / 100.0,
                tone.fg(theme),
            )),
        });
    }
    let all = h.saturating_sub(lines.len());
    let tail_room = tail.len().min(all);
    place_rows(rows, m, w, all - tail_room, at[1], lines, hits, meters);
    place_rows(tail, m, w, tail_room, at[1], lines, hits, meters);
}

/// A time as the Calendar writes it, `9:30 AM`.
fn clock(at: jiff::Timestamp, tz: &jiff::tz::TimeZone) -> String {
    at.to_zoned(tz.clone()).strftime("%-I:%M %p").to_string()
}

/// The Calendar as a cell: the rest of the day as a list, each event not
/// yet over with its time right-aligned, its title, and a chip ("in 15m",
/// "now", or "moved"), then when the day's last one ends.
fn calendar(
    m: &Mission,
    v: &View,
    w: usize,
    h: usize,
    lines: &mut Vec<Line>,
    hits: &mut Vec<HitBox>,
) {
    use crate::day::calendar::CalEvent;
    let theme = &m.snap.theme;
    let tz = &m.snap.tz;
    let now_ms = m.snap.now_secs * 1000;
    // A feed that failed after it was read keeps its events, said out of
    // date with when it was last read, as the day says it alone.
    if let Some(error) = &v.data.error {
        let updated = v
            .data
            .at
            .filter(|at| *at > 1)
            .map(|at| {
                format!(
                    " (updated {} ago)",
                    crate::sidebar::text::ago(Some(at.div_euclid(1000)), m.snap.now_secs)
                )
            })
            .unwrap_or_default();
        for words in crate::sidebar::text::wrap(&format!("{error}{updated}"), w.saturating_sub(3)) {
            lines.push(Line {
                spans: kit::fit(
                    vec![
                        Span::raw("  "),
                        Span::styled(words, Tone::Needs.style(theme)),
                    ],
                    w,
                    Style::new(),
                ),
                fill: None,
            });
        }
    }
    let left: Vec<(usize, &crate::items::Item, CalEvent)> = v
        .data
        .items
        .iter()
        .enumerate()
        .filter(|(_, i)| !hidden(v, i, usize::MAX, m.calendar_day.is_none()))
        .filter_map(|(at, i)| Some((at, i, serde_json::from_value(i.data.clone()).ok()?)))
        .collect();
    if left.is_empty() {
        let words = if m.calendar_day.is_none() {
            "Nothing left today."
        } else {
            "Nothing that day."
        };
        lines.push(dim_line(words, w, theme));
        return;
    }
    let chip_of = |e: &CalEvent| -> Option<(String, Tone)> {
        if e.moved_from.is_some() {
            return Some(("moved".into(), Tone::Needs));
        }
        // How soon is today's alone.
        if m.calendar_day.is_some() {
            return None;
        }
        let start = e.starts(tz)?.as_millisecond();
        let over = e.ends(tz).is_some_and(|t| t.as_millisecond() <= now_ms);
        if start <= now_ms {
            return (!over).then(|| ("now".into(), Tone::Working));
        }
        // A start under a minute away is "in 1m".
        let minutes = (start - now_ms + 59_999).div_euclid(60_000);
        (minutes <= 60).then(|| (format!("in {minutes}m"), Tone::Working))
    };
    let spec = [
        Col::Fixed {
            min: 8,
            natural: 8,
            cap: 8,
        },
        Col::Stretch {
            weight: 1,
            min: 8,
            max: None,
        },
        Col::Fixed {
            min: 0,
            natural: 10,
            cap: 10,
        },
    ];
    let at = kit::columns(&spec, w.saturating_sub(2), 1, &[2]);
    let mut rows = Vec::new();
    for (index, item, event) in &left {
        let mut spans = Vec::new();
        if let Some(col) = at[0] {
            let time = event.starts(tz).map(|t| clock(t, tz)).unwrap_or_default();
            kit::place(&mut spans, right(time, col.1, Tone::Dim.style(theme)), col);
        }
        if let Some(col) = at[1] {
            kit::place(
                &mut spans,
                vec![Span::styled(item.title.clone(), Tone::Text.style(theme))],
                col,
            );
        }
        if let (Some(col), Some((text, tone))) = (at[2], chip_of(event)) {
            kit::place(&mut spans, kit::chip(&text, tone, m.snap.caps, theme), col);
        }
        rows.push(ListRow {
            spans,
            item: Some(*index),
            meter: None,
        });
    }
    // The latest end, which needn't be the last to start's.
    if let Some(end) = left.iter().filter_map(|(_, _, e)| e.ends(tz)).max() {
        rows.push(ListRow::text(vec![
            Span::raw(" "),
            Span::styled(
                format!("nothing after {}", clock(end, tz)),
                Tone::Dim.style(theme),
            ),
        ]));
    }
    let room = h.saturating_sub(lines.len());
    place_rows(rows, m, w, room, None, lines, hits, &mut Vec::new());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_to_four_views_split_into_each_layout_s_cells() {
        // 2 × 2: four cells, two columns a rule apart, two rows a rule apart.
        let four = areas(&split(Layout::Grid, 4, 125, 21, CELL_MIN).unwrap());
        assert_eq!(
            four,
            vec![
                (0, 0, 0, 62, 10),
                (1, 63, 0, 62, 10),
                (2, 0, 11, 62, 10),
                (3, 63, 11, 62, 10)
            ]
        );
        // Three in 2 × 2 leave the fourth cell empty.
        let cols = split(Layout::Grid, 3, 125, 21, CELL_MIN).unwrap();
        assert_eq!(cols[1].cells[1].0, None);
        assert_eq!(areas(&cols).len(), 3);
        // 1 + 2: the first the whole height, the rest stacked beside it.
        let one_two = areas(&split(Layout::OneTwo, 3, 125, 21, CELL_MIN).unwrap());
        assert_eq!(one_two[0], (0, 0, 0, 62, 21));
        assert_eq!(one_two[1], (1, 63, 0, 62, 10));
        assert_eq!(one_two[2], (2, 63, 11, 62, 10));
        // Two in any layout stand side by side.
        for layout in Layout::ALL {
            let two = areas(&split(layout, 2, 125, 21, CELL_MIN).unwrap());
            assert_eq!(two, vec![(0, 0, 0, 62, 21), (1, 63, 0, 62, 21)]);
        }
        // Side by side: a column each.
        let side = areas(&split(Layout::Side, 4, 160, 20, CELL_MIN).unwrap());
        assert_eq!(side.iter().map(|a| a.3).sum::<usize>() + 3, 160);
        assert!(side.iter().all(|a| a.2 == 0 && a.4 == 20));
    }

    /// Board 11's four views: Slack with four messages not seen, two Inbox
    /// notifications, $8.41 spent by two tabs, and three events left of
    /// the day's four, at noon.
    fn board_11() -> Mission {
        use crate::items::{Item, ViewData};
        use crate::usage::charts::{Limit, Model, SpanSpend, TabSpend};
        use serde_json::json;
        let view = |id: ViewId, title: &str, items: Vec<Item>| View {
            id,
            title: title.into(),
            data: ViewData {
                items,
                ..ViewData::default()
            },
            loaded: true,
        };
        let message = |n: u32, old: bool| {
            Item::new(n.to_string(), format!("Someone · #team {n}")).data(json!({ "old": old }))
        };
        let spend = |n: usize, name: &str, spent: f64| TabSpend {
            key: format!("tab:{n}"),
            tab: Some(n as u32),
            index: n,
            name: name.into(),
            color: Some("82aaff".into()),
            spent,
            share: spent / 8.41,
            ..TabSpend::default()
        };
        let model = Model {
            today: SpanSpend {
                total: 8.41,
                tabs: vec![spend(1, "billing", 6.0), spend(2, "docs", 2.41)],
                ..SpanSpend::default()
            },
            limits: vec![Limit {
                key: "five_hour".into(),
                name: "5-hour limit".into(),
                used: 42.0,
                ..Limit::default()
            }],
            ..Model::default()
        };
        let event = |n: u32, title: &str, start: &str, end: &str, over: bool| {
            Item::new(n.to_string(), title)
                .state(if over { State::Idle } else { State::Active })
                .data(json!({ "title": title, "start": start, "end": end, "you": "going" }))
        };
        let mut m = Mission::new();
        m.snap.rules.ux2 = true;
        m.snap.now_secs = 1_767_268_800; // 2026-01-01 12:00 UTC
        m.snap.views = vec![
            view(
                ViewId::Calendar,
                "Calendar",
                vec![
                    event(
                        1,
                        "Standup",
                        "2026-01-01T09:00:00Z",
                        "2026-01-01T09:15:00Z",
                        true,
                    ),
                    event(
                        2,
                        "Design review",
                        "2026-01-01T12:15:00Z",
                        "2026-01-01T13:00:00Z",
                        false,
                    ),
                    event(
                        3,
                        "Planning",
                        "2026-01-01T14:00:00Z",
                        "2026-01-01T15:00:00Z",
                        false,
                    ),
                    event(
                        4,
                        "One on one",
                        "2026-01-01T16:00:00Z",
                        "2026-01-01T16:30:00Z",
                        false,
                    ),
                ],
            ),
            view(
                ViewId::Slack,
                "Slack",
                (1..=5).map(|n| message(n, n == 3)).collect(),
            ),
            view(
                ViewId::Inbox,
                "Inbox",
                vec![Item::new("a", "Fix the build"), Item::new("b", "Ship it")],
            ),
            view(
                ViewId::Usage,
                "Usage",
                vec![
                    Item::new("spent", "Spent today $8.41").data(json!({ "model": model })),
                    Item::new("tab:tab:1", "1 billing"),
                    Item::new("tab:tab:2", "2 docs"),
                ],
            ),
        ];
        m.grid = Some(Grid::new(Choice::default()));
        m.settle_grid();
        m
    }

    fn texts(lines: &[Line]) -> Vec<String> {
        lines
            .iter()
            .map(|l| l.text().trim_end().to_string())
            .collect()
    }

    #[test]
    fn four_views_show_at_once_with_their_labels_counts_and_rules() {
        let mut m = board_11();
        assert_eq!(m.view, 2, "Slack, the first picked, has the focus");
        let (lines, hits, meters, _) = drawn(&mut m, 125, 24);
        let t = texts(&lines);
        let screen = t.join("\n");
        // The bar: every group a shown view is in lit, the shown views
        // named, and how many at its right.
        assert!(
            t[1].contains("Slack") && t[1].contains("Calendar"),
            "{screen}"
        );
        assert!(t[1].ends_with("4 views at once"), "{screen}");
        // The labels with their counts at each cell's right edge.
        let labels = &t[3];
        assert!(labels.starts_with(" SLACK"), "{screen}");
        assert!(labels.contains("4 new │ INBOX"), "{screen}");
        assert!(labels.ends_with("2 unread"), "{screen}");
        let lower = t.iter().find(|l| l.contains("USAGE")).unwrap();
        assert!(lower.contains("$8.41 today │ CALENDAR"), "{screen}");
        assert!(lower.ends_with("3 left"), "{screen}");
        // The rules cross between the four.
        assert!(t.iter().any(|l| l.contains("─┼─")), "{screen}");
        // The focused view's label is in the accent; another's is dim.
        let theme = m.snap.theme;
        let style_of = |text: &str| {
            lines[3]
                .spans
                .iter()
                .find(|s| s.content.contains(text))
                .map(|s| s.style.fg)
                .unwrap()
        };
        assert_eq!(style_of("SLACK"), Some(Tone::Accent.fg(&theme)));
        assert_eq!(style_of("INBOX"), Some(Tone::Dim.fg(&theme)));
        // Usage's tabs with their meters, the dollars at the right edge,
        // then the 5-hour limit.
        let billing = t.iter().find(|l| l.contains("1 billing")).unwrap();
        assert!(billing.contains("$6.00 │"), "{screen}");
        assert!(screen.contains("42% used"), "{screen}");
        // A meter a tab and the limit's, beside the Inbox board's photos.
        let usage = meters
            .iter()
            .filter(|i| i.name.starts_with("grid/usage/"))
            .count();
        assert_eq!(usage, 3, "a meter a tab and the limit's");
        // The Calendar: what is left, its times, a chip for what is soon,
        // and when the day ends.
        assert!(!screen.contains("Standup"), "{screen}");
        assert!(screen.contains("12:15 PM Design review"), "{screen}");
        assert!(screen.contains("in 15m"), "{screen}");
        assert!(screen.contains("nothing after 4:30 PM"), "{screen}");
        // Events kept from a feed that failed since are said out of date.
        let mut stale = board_11();
        let calendar = stale
            .snap
            .views
            .iter_mut()
            .find(|v| v.id == ViewId::Calendar)
            .unwrap();
        calendar.data.error = Some("Out of date: the feed failed".into());
        let (lines, ..) = drawn(&mut stale, 125, 24);
        let shown = texts(&lines).join("\n");
        assert!(
            shown.contains("Out of date") && shown.contains("Design review"),
            "{shown}"
        );
        // Another day than today is named on the label.
        stale.calendar_day = Some("2025-12-31".into());
        let (lines, ..) = drawn(&mut stale, 125, 24);
        let shown = texts(&lines).join("\n");
        assert!(shown.contains("CALENDAR · WED DEC 31"), "{shown}");
        assert!(shown.contains("4 events"), "{shown}");
        // A tab the filter left out of Usage has no row.
        let mut filtered = board_11();
        let usage = filtered
            .snap
            .views
            .iter_mut()
            .find(|v| v.id == ViewId::Usage)
            .unwrap();
        usage.data.items.retain(|i| i.id != "tab:tab:2");
        let (lines, ..) = drawn(&mut filtered, 125, 24);
        let shown = texts(&lines).join("\n");
        assert!(
            shown.contains("1 billing") && !shown.contains("2 docs"),
            "{shown}"
        );
        // A click on another view gives it the focus; the focused one's
        // rows take theirs.
        assert!(hits.iter().any(|h| h.spot == Spot::Cell(1)));
        assert!(hits.iter().any(|h| h.spot == Spot::Index(0)));
        // The footer names the grid's keys.
        assert!(t.last().unwrap().contains("tab next view"), "{screen}");
        m.next_cell(1);
        assert_eq!(m.view, 3, "Tab gives the Inbox the focus");
        // A view's chosen item is kept by its id while another has the focus.
        m.cursor = 1;
        m.next_cell(-1);
        let inbox = m
            .snap
            .views
            .iter_mut()
            .find(|v| v.id == ViewId::Inbox)
            .unwrap();
        inbox.data.items.reverse();
        m.next_cell(1);
        assert_eq!(m.chosen_item().map(|i| i.id.as_str()), Some("b"));
        assert_eq!(m.cursor, 0);
    }

    #[test]
    fn the_cursor_passes_over_rows_a_cell_leaves_out_and_its_list_keeps_the_chosen_in_sight() {
        let mut m = board_11();
        drawn(&mut m, 125, 24);
        // The Calendar, fourth picked, focused: the meeting already over
        // isn't drawn, so the cursor passes over it.
        m.focus_cell(3);
        assert_eq!(m.view, 1);
        let items = m.current().unwrap().data.items.clone();
        assert!(m.passes_over(&items[0]), "Standup is over");
        // Another day shows its events over or not.
        m.calendar_day = Some("2025-12-31".into());
        assert!(!m.passes_over(&items[0]), "another day's are all shown");
        m.calendar_day = None;
        assert!(!m.passes_over(&items[1]));
        // On a row the cell leaves out, nothing is chosen to act on.
        m.cursor = 0;
        assert!(m.chosen_item().is_none());
        m.cursor = 1;
        assert!(m.chosen_item().is_some());
        // Usage passes over all but the tabs that spent today.
        m.focus_cell(2);
        let items = m.current().unwrap().data.items.clone();
        assert!(m.passes_over(&items[0]), "today's total");
        assert!(!m.passes_over(&items[1]), "a tab's row");
        // A heading Enter acts on (the Work map's) stays a row to rest on.
        m.snap.views.push(View {
            id: ViewId::WorkMap,
            title: "Work map".into(),
            data: crate::items::ViewData {
                items: vec![views::header("repo:app", "app", 1)],
                ..Default::default()
            },
            loaded: true,
        });
        m.view = m.snap.views.len();
        assert!(!m.passes_over(&m.current().unwrap().data.items[0].clone()));
        // Its scope in its title leaves it the same view for the grid.
        m.snap.views.last_mut().unwrap().title = "Work map · open tabs".into();
        assert_eq!(m.place_of("work-map"), Some(m.snap.views.len()));
        // A board's week row, which a short cell leaves out, is no stop.
        let board = View {
            id: ViewId::Prs,
            title: "PRs".into(),
            data: crate::items::ViewData::default(),
            loaded: true,
        };
        let week = board.columns().unwrap().last().unwrap().to_string();
        let merged = crate::items::Item::new("1", "#1").group(week);
        assert!(hidden(&board, &merged, 10, true));
        assert!(!hidden(&board, &merged, 30, true));
        let open = crate::items::Item::new("2", "#2").group(board.columns().unwrap()[0]);
        assert!(!hidden(&board, &open, 8, true));
        assert!(hidden(&board, &open, 7, true), "no room for a card");
        // A cell 16 rows tall gives the board 15 under its label: too few.
        m.snap.views.push(View {
            data: crate::items::ViewData {
                items: vec![merged.clone()],
                ..Default::default()
            },
            ..board
        });
        m.view = m.snap.views.len();
        let cells = std::mem::replace(
            &mut m.cells,
            vec![(m.grid.as_ref().unwrap().focus, (62, 16))],
        );
        assert!(m.passes_over(&merged), "the week row is left out");
        m.cells = cells;
        m.view = 3;
        // Each cell's size is kept for the keys: the focused one's.
        assert_eq!(m.laid(), (62, 9));
        // A short cell shows the chosen event, the list moved up to it.
        m.focus_cell(3);
        m.cursor = 3;
        let (lines, ..) = drawn(&mut m, 125, 7);
        let screen = texts(&lines).join("\n");
        assert!(screen.contains("One on one"), "{screen}");
        assert!(!screen.contains("Design review"), "{screen}");
    }

    #[test]
    fn usage_s_limit_stays_in_sight_under_more_tabs_than_its_cell_holds() {
        use crate::usage::charts::{Model, TabSpend};
        let mut m = board_11();
        let usage = m
            .snap
            .views
            .iter_mut()
            .find(|v| v.id == ViewId::Usage)
            .unwrap();
        let mut model: Model =
            serde_json::from_value(usage.data.items[0].data["model"].clone()).unwrap();
        model.today.tabs = (1..=10)
            .map(|n| TabSpend {
                key: format!("tab:{n}"),
                index: n,
                name: format!("tab{n}"),
                spent: 1.0,
                share: 0.1,
                ..TabSpend::default()
            })
            .collect();
        usage.data.items = std::iter::once(
            crate::items::Item::new("spent", "Spent").data(serde_json::json!({ "model": model })),
        )
        .chain((1..=10).map(|n| crate::items::Item::new(format!("tab:tab:{n}"), format!("{n}"))))
        .collect();
        let (lines, ..) = drawn(&mut m, 125, 24);
        let screen = texts(&lines).join("\n");
        assert!(screen.contains("5-hour limit"), "{screen}");
        assert!(screen.contains("1 tab1"), "{screen}");
    }

    #[test]
    fn the_grid_fills_any_width_and_gives_way_on_a_narrow_one() {
        for cols in [120, 160, 240] {
            let mut m = board_11();
            let (lines, ..) = drawn(&mut m, cols, 24);
            let t = texts(&lines);
            let screen = t.join("\n");
            assert!(t[1].ends_with("4 views at once"), "{cols}: {screen}");
            assert!(t[3].ends_with("2 unread"), "{cols}: {screen}");
            let billing = t.iter().find(|l| l.contains("1 billing")).unwrap();
            assert!(billing.contains("$6.00 │"), "{cols}: {screen}");
            assert!(t.iter().any(|l| l.contains("─┼─")), "{cols}: {screen}");
        }
        // Too narrow for two columns of cells: the focused view alone.
        let mut m = board_11();
        let (lines, ..) = drawn(&mut m, 60, 24);
        let t = texts(&lines);
        assert!(
            t[3].starts_with(" SLACK") && t[3].ends_with("4 new"),
            "{}",
            t.join("\n")
        );
        assert!(!t.iter().any(|l| l.contains("INBOX")));
    }

    #[test]
    fn the_grid_gives_way_once_ux2_is_off() {
        let mut m = board_11();
        m.snap.rules.ux2 = false;
        m.settle_grid();
        assert!(m.grid.is_none());
    }

    #[test]
    fn a_layout_too_wide_gives_way_to_two_by_two_then_to_one_view() {
        let cols = split(Layout::Side, 4, 100, 20, CELL_MIN).unwrap();
        assert_eq!(cols.len(), 2, "four side by side give way to 2 × 2");
        assert!(split(Layout::Grid, 4, 60, 20, CELL_MIN).is_none());
        // Three stacked too short for a board's card give way to 2 × 2.
        let cols = split(Layout::OneTwo, 4, 140, 20, CELL_MIN).unwrap();
        assert_eq!(cols[1].cells.len(), 2, "{cols:?}");
        assert!(split(Layout::Grid, 4, 140, 16, CELL_MIN).is_none());
    }

    #[test]
    fn the_choice_is_kept_for_next_time() {
        let dir = std::env::temp_dir().join(format!("sb-grid-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(load(&dir), None);
        let choice = Choice {
            views: vec!["tabs".into(), "usage".into()],
            layout: Layout::OneTwo,
        };
        save(&dir, &choice).unwrap();
        assert_eq!(load(&dir), Some(choice));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
