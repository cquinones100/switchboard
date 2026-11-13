//! UX 2.0's Usage view as a board, board 24 of the UX 2.0 canvas ("Usage
//! as charts"): the day's spend as a chart stacked by tab, the tabs beside
//! it with each one's spend and share, the rate limits as meters with their
//! pace, and the request audit with a sparkline and a meter a service. New
//! with UX 2.0, with no tmux-version file behind it.
//!
//! Everything it draws comes from the model `usage::charts` works out off
//! the server's task (the view's first row carries it), so drawing reads
//! no file. The cursor moves through the tabs, then the services, which
//! are the view's own rows, so the filter and Enter work on them as on the
//! plain list; `w` shows the week by day in place of today by quarter hour.
//! The chart, the meters and the sparklines are image atoms
//! (src/ui/images.rs) with their text forms where the terminal draws none.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;

use super::render::{Image, Shape};
use super::views::View;
use super::{HitBox, Mission, Spot};
use crate::items::Item;
use crate::requests::{ServiceCount, Tally};
use crate::sidebar::render::Line;
use crate::sidebar::text::{cut, width};
use crate::status::theme::RED;
use crate::ui::Theme;
use crate::ui::images::{Bar, Marks};
use crate::ui::kit::{self, Col, Tone};
use crate::usage::charts::{Limit, Model, SpanSpend, TabSpend};

/// How wide the screen must be for the board; narrower, the plain list.
pub const BOARD_AT: usize = 136;
/// The tabs' column beside the chart at 160 columns, its share of a
/// screen of any width (board 45), and the least it takes.
const SIDE_W: usize = 54;
const SIDE_MIN: usize = 34;
/// The chart's rows.
const CHART_ROWS: usize = 8;
/// The amounts left of the chart.
const AXIS_W: usize = 8;

/// The tabs' column on a screen `cols` wide, which keeps its share.
fn side_w(cols: usize) -> usize {
    kit::beside(cols, SIDE_W, SIDE_MIN, 0)
}

/// Where bar `b` of `bins` starts in a chart `chart_w` wide: the bars
/// share its width, so they reach the tabs whatever the screen.
fn bar_x(b: usize, bins: usize, chart_w: usize) -> usize {
    b * chart_w / bins.max(1)
}

/// Spans put together left to right, each at a column of its own.
#[derive(Default)]
struct Placer {
    spans: Vec<Span<'static>>,
    at: usize,
}

impl Placer {
    /// Spaces up to column `x` (nothing when already past it).
    fn to(&mut self, x: usize) -> &mut Placer {
        if x > self.at {
            self.spans.push(Span::raw(" ".repeat(x - self.at)));
            self.at = x;
        }
        self
    }

    fn put(&mut self, text: impl AsRef<str>, style: Style) -> &mut Placer {
        let text = text.as_ref();
        if !text.is_empty() {
            self.at += width(text);
            self.spans.push(Span::styled(text.to_string(), style));
        }
        self
    }

    /// `text` ending at column `right`, after a column at least.
    fn right(&mut self, text: impl AsRef<str>, right: usize, style: Style) -> &mut Placer {
        let text = text.as_ref();
        let x = right.saturating_sub(width(text)).max(self.at + 1);
        self.to(x).put(text, style)
    }

    fn line(self, cols: usize) -> Line {
        Line {
            spans: kit::fit(self.spans, cols, Style::new()),
            fill: None,
        }
    }

    fn spans(self, cols: usize) -> Vec<Span<'static>> {
        kit::fit(self.spans, cols, Style::new())
    }
}

/// A row on the panel's surface while the mouse is over it, as a list's
/// row lights.
fn under_mouse(spans: Vec<Span<'static>>, lit: bool, theme: &Theme) -> Vec<Span<'static>> {
    if !lit {
        return spans;
    }
    spans
        .into_iter()
        .map(|s| {
            let style = s.style.bg(theme.panel);
            Span::styled(s.content, style)
        })
        .collect()
}

/// A tab's color, from its `rrggbb`.
pub(crate) fn color_of(tab: &TabSpend, theme: &Theme) -> Color {
    tab.color
        .as_deref()
        .filter(|hex| hex.len() == 6)
        .and_then(|hex| {
            let n = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
            Some(Color::Rgb(n(0)?, n(2)?, n(4)?))
        })
        .unwrap_or(theme.dim)
}

/// The top of the chart's scale: the busiest bin rounded up to 1, 2 or 5
/// of its power of ten.
fn nice_max(v: f64) -> f64 {
    if v <= 0.0 {
        return 1.0;
    }
    let power = 10f64.powf(v.log10().floor());
    for step in [1.0, 2.0, 2.5, 5.0, 10.0] {
        if v <= step * power {
            return step * power;
        }
    }
    10.0 * power
}

fn dollars(v: f64) -> String {
    if v >= 100.0 {
        format!("${v:.0}")
    } else {
        format!("${v:.2}")
    }
}

/// `1,234`.
fn count(n: usize) -> String {
    crate::usage::sessions::thousands(n)
}

/// A while ago, in a few characters.
fn ago(ms: i64) -> String {
    let s = (ms / 1000).max(0);
    match s {
        0..60 => format!("{s}s ago"),
        60..3600 => format!("{}m ago", s / 60),
        _ => format!("{}h ago", s / 3600),
    }
}

fn clock(ms: i64, tz: &jiff::tz::TimeZone, format: &str) -> String {
    jiff::Timestamp::from_millisecond(ms)
        .map(|t| t.to_zoned(tz.clone()).strftime(format).to_string())
        .unwrap_or_default()
}

/// Counts by name, the most first, `GraphQL 400 · REST 120`.
fn by_count(counts: &indexmap::IndexMap<String, usize>) -> String {
    let mut list: Vec<(&String, &usize)> = counts.iter().collect();
    list.sort_by(|a, b| b.1.cmp(a.1));
    list.iter()
        .map(|(name, n)| format!("{name} {}", count(**n)))
        .collect::<Vec<_>>()
        .join(" · ")
}

/// Where each of the view's rows the board can choose is: the tabs' by
/// their key, the services' by their name.
fn index_of(items: &[Item], id: &str) -> Option<usize> {
    items.iter().position(|i| i.id == id)
}

fn tab_item(key: &str) -> String {
    format!("tab:{key}")
}

fn service_item(service: &str) -> String {
    format!("requests:{service}")
}

/// The span with no more bins than the chart has columns: neighbors
/// merged, their time and each tab's part added up, so the bars, the
/// labels under them and the clicks on them agree.
fn fitted(span: &SpanSpend, cols: usize) -> SpanSpend {
    let room = cols.saturating_sub(side_w(cols) + AXIS_W + 4).max(1);
    let n = span.bins.len();
    if n <= room {
        return span.clone();
    }
    let by = n.div_ceil(room);
    let bins = span
        .bins
        .chunks(by)
        .map(|group| {
            let mut sum = vec![0.0; span.tabs.len()];
            for bin in group {
                for (i, v) in bin.iter().enumerate() {
                    if let Some(s) = sum.get_mut(i) {
                        *s += v;
                    }
                }
            }
            sum
        })
        .collect();
    SpanSpend {
        bins,
        bin_ms: span.bin_ms * by as i64,
        ..span.clone()
    }
}

/// The board's lines from row `top` on, the images it places, and where a
/// click on a tab, a bar or a service lands; `false` with nothing drawn
/// when the view carries no model yet.
#[allow(clippy::too_many_arguments)]
pub fn board(
    view: &View,
    m: &Mission,
    cols: usize,
    rows: usize,
    lines: &mut Vec<Line>,
    hits: &mut Vec<HitBox>,
    images: &mut Vec<Image>,
) -> bool {
    let items = &view.data.items;
    let Some(model) = crate::usage::charts::model_of(items) else {
        return false;
    };
    let theme = &m.snap.theme;
    let now_ms = m.snap.now_secs * 1000;
    let week = m.usage_week;
    let span = &fitted(if week { &model.week } else { &model.today }, cols);
    let cursor = usize::try_from(m.cursor).ok();
    let chosen_id = cursor.and_then(|c| items.get(c)).map(|i| i.id.as_str());
    let top = lines.len();
    let (hits_before, images_before) = (hits.len(), images.len());

    heading(span, week, cols, theme, lines);
    lines.push(Line {
        spans: vec![Span::raw(" ".repeat(cols))],
        fill: None,
    });
    // The tabs in the view's order, which is the cursor's, each with its
    // place among the chart's parts; one with nothing in the span (a tab of
    // the week, today) drawn at none, its place past the parts.
    let mut past = span.tabs.len();
    let tab_rows: Vec<(usize, TabSpend)> = items
        .iter()
        .filter_map(|i| i.data.get("tab_key")?.as_str())
        .filter_map(|key| {
            if let Some(at) = span.tabs.iter().position(|t| t.key == key) {
                return Some((at, span.tabs[at].clone()));
            }
            let t = model.week.tabs.iter().find(|t| t.key == key)?;
            past += 1;
            Some((
                past,
                TabSpend {
                    spent: 0.0,
                    share: 0.0,
                    first_ms: None,
                    rate: None,
                    ..t.clone()
                },
            ))
        })
        .collect();
    let tabs: Vec<(usize, &TabSpend)> = tab_rows.iter().map(|(at, t)| (*at, t)).collect();
    let chosen_tab = tab_rows
        .iter()
        .find(|(_, t)| chosen_id == Some(tab_item(&t.key).as_str()))
        .map(|(at, _)| *at);
    chart_and_tabs(
        span, &tabs, chosen_tab, items, m, cols, lines, hits, images, week,
    );
    limits(&model, cols, theme, now_ms, lines, images);
    requests(&model, week, items, chosen_id, m, cols, lines, hits, images);
    sessions(items, chosen_id, m, cols, lines, hits);
    // Scrolled up as far as the chosen row (and the line under a chosen
    // service) needs to be in sight above the footer, what goes off the top
    // taken out, an image cut by the top with it.
    let last = rows.saturating_sub(2);
    let chosen_bottom = cursor.and_then(|c| {
        let y = hits[hits_before..]
            .iter()
            .filter(|h| h.spot == Spot::Index(c as isize))
            .map(|h| h.y as usize)
            .max()?;
        Some(y.min(lines.len().saturating_sub(1)))
    });
    if let Some(bottom) = chosen_bottom
        && bottom > last
    {
        let by = bottom - last;
        lines.drain(top..(top + by).min(lines.len()));
        let mut kept = Vec::new();
        for mut h in hits.drain(hits_before..) {
            if h.y as usize >= top + by {
                h.y -= by as u16;
                kept.push(h);
            }
        }
        hits.extend(kept);
        let mut kept = Vec::new();
        for mut i in images.drain(images_before..) {
            if i.y as usize >= top + by {
                i.y -= by as u16;
                kept.push(i);
            }
        }
        images.extend(kept);
    }
    true
}

/// Each open Claude session's context, model and age, the fullest first
/// (`usage::sessions::sessions_section`, what the Sessions view showed):
/// a row each, its context as a meter, one at 80% or more in the needs
/// tone.
fn sessions(
    items: &[Item],
    chosen_id: Option<&str>,
    m: &Mission,
    cols: usize,
    lines: &mut Vec<Line>,
    hits: &mut Vec<HitBox>,
) {
    let theme = &m.snap.theme;
    let rows: Vec<(usize, &Item)> = items
        .iter()
        .enumerate()
        .filter(|(_, i)| crate::usage::sessions::is_session_row(i))
        .collect();
    if rows.is_empty() {
        return;
    }
    lines.push(Line {
        spans: vec![kit::rule(cols, theme)],
        fill: None,
    });
    lines.push(Line {
        spans: kit::label("Sessions", Tone::Dim, Some(rows.len()), cols, theme),
        fill: None,
    });
    // The name up to 32 columns, the context's meter, and the facts
    // stretching into the rest, the meter dropped first on a narrow screen.
    let at = kit::columns(
        &[
            Col::Fixed {
                min: 8,
                natural: kit::natural(rows.iter().map(|(_, i)| i.title.as_str())),
                cap: 32,
            },
            Col::Fixed {
                min: METER_CELLS,
                natural: METER_CELLS,
                cap: METER_CELLS,
            },
            Col::Stretch {
                weight: 1,
                min: 10,
                max: None,
            },
        ],
        cols.saturating_sub(2),
        2,
        &[1],
    );
    for (index, item) in rows {
        let chosen = chosen_id == Some(item.id.as_str());
        let lit = m.hover == Some(Spot::Index(index as isize));
        let used = item.data["used"].as_f64();
        let tone = if used.is_some_and(|u| u >= 80.0) {
            Tone::Needs
        } else {
            Tone::Accent
        };
        let name = if chosen {
            Tone::Text.bold(theme)
        } else {
            Tone::Text.style(theme)
        };
        let mut line = Vec::new();
        let mut put = |spans: Vec<Span<'static>>, slot: Option<(usize, usize)>| {
            if let Some((x, w)) = slot {
                kit::place(&mut line, spans, (x + 1, w));
            }
        };
        put(vec![Span::styled(item.title.clone(), name)], at[0]);
        put(
            kit::meter(
                used.map_or(0, |u| u.clamp(0.0, 100.0).round() as u8),
                METER_CELLS,
                tone,
                theme,
            ),
            at[1],
        );
        put(
            vec![Span::styled(item.detail.clone(), Tone::Dim.style(theme))],
            at[2],
        );
        hits.push(HitBox {
            y: lines.len() as u16,
            x0: 0,
            x1: cols.saturating_sub(1) as u16,
            spot: Spot::Index(index as isize),
        });
        lines.push(Line {
            spans: under_mouse(kit::row(line, cols, chosen, theme), lit && !chosen, theme),
            fill: None,
        });
    }
}

/// A session's context meter's cells.
const METER_CELLS: usize = 10;

/// "Today $8.41", what the amount is, and where it comes from.
fn heading(span: &SpanSpend, week: bool, cols: usize, theme: &Theme, lines: &mut Vec<Line>) {
    let mut p = Placer::default();
    p.put(" ", Style::new())
        .put(
            if week { "This week " } else { "Today " },
            Tone::Text.bold(theme),
        )
        .put(dollars(span.total), Tone::Accent.bold(theme))
        .put(
            "   Claude Code's estimate at API prices, interactive sessions only",
            Tone::Dim.style(theme),
        )
        .right(
            "from each session's statusline",
            cols.saturating_sub(1),
            Tone::Faint.style(theme),
        );
    lines.push(p.line(cols));
}

/// The chart stacked by tab, its amounts at the left and the hours (or
/// days) under it, "now" over its right edge; beside it, the tabs with
/// their spend and share, and the chosen one's rate under them.
#[allow(clippy::too_many_arguments)]
fn chart_and_tabs(
    span: &SpanSpend,
    tabs: &[(usize, &TabSpend)],
    chosen_tab: Option<usize>,
    items: &[Item],
    m: &Mission,
    cols: usize,
    lines: &mut Vec<Line>,
    hits: &mut Vec<HitBox>,
    images: &mut Vec<Image>,
    week: bool,
) {
    let theme = &m.snap.theme;
    let tz = &m.snap.tz;
    let side_cols = side_w(cols);
    let side_x = cols.saturating_sub(side_cols);
    let bins = span.bins.len().max(1);
    let chart_w = side_x.saturating_sub(AXIS_W + 4).max(1);
    let chart_x = 1 + AXIS_W;
    let top = lines.len();
    let busiest = span
        .bins
        .iter()
        .map(|b| b.iter().sum::<f64>())
        .fold(0.0, f64::max);
    let max = nice_max(busiest);

    // The labels' row: the chart's and the tabs'.
    let mut p = Placer::default();
    p.put(" ", Style::new()).put(
        if week {
            "SPEND BY DAY"
        } else {
            "SPEND BY QUARTER HOUR"
        },
        Tone::Dim.bold(theme),
    );
    if !week {
        p.to(chart_x + chart_w.saturating_sub(3))
            .put("now", Style::new().fg(RED));
    }
    p.to(side_x).put("BY TAB", Tone::Dim.bold(theme)).right(
        "share",
        cols.saturating_sub(2),
        Tone::Faint.style(theme),
    );
    lines.push(p.line(cols));

    // The tabs' rows beside the chart's, as many as its height leaves
    // room for beside the rate, the window moving with the chosen one, so
    // the limits always follow the chart; then the chosen one's rate.
    let room_for = CHART_ROWS - 2;
    let chosen_at = tabs
        .iter()
        .position(|(at, _)| chosen_tab == Some(*at))
        .unwrap_or(0);
    let first = (chosen_at + 1)
        .saturating_sub(room_for)
        .min(tabs.len().saturating_sub(room_for));
    let mut side: Vec<(Vec<Span<'static>>, Option<usize>)> = tabs
        .iter()
        .skip(first)
        .take(room_for)
        .map(|(at, t)| {
            let chosen = chosen_tab == Some(*at);
            let mut row = Placer::default();
            row.put(
                if t.index > 0 {
                    format!("{:>2}", t.index)
                } else {
                    " –".to_string()
                },
                Style::new()
                    .fg(color_of(t, theme))
                    .add_modifier(Modifier::BOLD),
            )
            .put(" ", Style::new());
            let name_w = side_cols.saturating_sub(4 + 2 + 9 + 5 + 2);
            row.put(
                cut(&t.name, name_w),
                if chosen {
                    Tone::Text.bold(theme)
                } else {
                    Tone::Text.style(theme)
                },
            );
            row.right(dollars(t.spent), side_cols - 9, Tone::Text.style(theme))
                .right(
                    format!("{}%", (t.share * 100.0).round() as i64),
                    side_cols - 3,
                    Tone::Dim.style(theme),
                );
            (
                {
                    let index = index_of(items, &tab_item(&t.key));
                    let lit =
                        !chosen && index.is_some_and(|i| m.hover == Some(Spot::Index(i as isize)));
                    under_mouse(
                        kit::row(row.spans(side_cols - 2), side_cols - 1, chosen, theme),
                        lit,
                        theme,
                    )
                },
                index_of(items, &tab_item(&t.key)),
            )
        })
        .collect();
    if let Some(t) = chosen_tab.and_then(|i| span.tabs.get(i))
        && let (Some(first), Some(rate)) = (t.first_ms, t.rate)
    {
        let since = if week {
            clock(first, tz, "%a %-I:%M %p")
        } else {
            clock(first, tz, "%-I:%M %p")
        };
        let mut row = Placer::default();
        row.put(
            if t.index > 0 {
                format!("  tab {} since {since}: ", t.index)
            } else {
                format!("  {} since {since}: ", t.name)
            },
            Tone::Dim.style(theme),
        )
        .put(format!("${rate:.2} an hour"), Tone::Text.bold(theme));
        side.push((Vec::new(), None));
        side.push((row.spans(side_cols - 1), None));
    }
    if tabs.is_empty() {
        let mut row = Placer::default();
        row.put("  nothing spent yet", Tone::Faint.style(theme));
        side.push((row.spans(side_cols - 1), None));
    }

    let body = CHART_ROWS.max(side.len().saturating_sub(1));
    for r in 0..=body {
        let y = lines.len();
        let mut p = Placer::default();
        if r < CHART_ROWS {
            let label = if r == 0 {
                Some(dollars(max))
            } else if r == CHART_ROWS / 2 {
                Some(dollars(max / 2.0))
            } else if r == CHART_ROWS - 1 {
                Some(dollars(0.0))
            } else {
                None
            };
            if let Some(label) = label {
                p.right(label, chart_x - 1, Tone::Faint.style(theme));
            }
        } else if r == CHART_ROWS {
            axis(span, week, chart_x, chart_w, tz, theme, &mut p);
        }
        p.to(side_x);
        if let Some((spans, index)) = side.get(r) {
            if let Some(index) = index {
                hits.push(HitBox {
                    y: y as u16,
                    x0: side_x as u16,
                    x1: cols.saturating_sub(1) as u16,
                    spot: Spot::Index(*index as isize),
                });
            }
            p.spans.extend(spans.clone());
            p.at += side_cols;
        }
        lines.push(p.line(cols));
    }

    // A click on a bar chooses the tab with the most of it.
    for (b, bin) in span.bins.iter().enumerate() {
        let most = bin
            .iter()
            .enumerate()
            .filter(|(_, v)| **v > 0.0)
            .max_by(|a, c| a.1.total_cmp(c.1))
            .and_then(|(i, _)| span.tabs.get(i))
            .and_then(|t| index_of(items, &tab_item(&t.key)));
        let Some(index) = most else {
            continue;
        };
        for r in 0..CHART_ROWS {
            hits.push(HitBox {
                y: (top + 1 + r) as u16,
                x0: (chart_x + bar_x(b, bins, chart_w)) as u16,
                x1: (chart_x + bar_x(b + 1, bins, chart_w)).saturating_sub(1) as u16,
                spot: Spot::Index(index as isize),
            });
        }
    }
    let bars: Vec<Bar> = span
        .bins
        .iter()
        .map(|bin| {
            bin.iter()
                .enumerate()
                .map(|(i, v)| {
                    (
                        *v as f32,
                        span.tabs.get(i).map_or(theme.dim, |t| color_of(t, theme)),
                    )
                })
                .collect()
        })
        .collect();
    images.push(Image {
        name: "usage/chart".into(),
        x: chart_x as u16,
        y: (top + 1) as u16,
        width: chart_w as u16,
        height: CHART_ROWS as u16,
        shape: Shape::Series {
            bars,
            max: max as f32,
            over: None,
            marks: Marks {
                outline: chosen_tab,
                grid: vec![0.0, (max / 2.0) as f32, max as f32],
                now: (!week).then_some(RED),
            },
        },
    });
}

/// The hours under today's chart, each at its first quarter; the days
/// under the week's.
#[allow(clippy::too_many_arguments)]
fn axis(
    span: &SpanSpend,
    week: bool,
    chart_x: usize,
    chart_w: usize,
    tz: &jiff::tz::TimeZone,
    theme: &Theme,
    p: &mut Placer,
) {
    for b in 0..span.bins.len() {
        let at = span.start_ms + b as i64 * span.bin_ms;
        let label = if week {
            // A day's middle, which a day of 23 or 25 hours still holds.
            clock(at + span.bin_ms / 2, tz, "%a")
        } else {
            let on_the_hour = jiff::Timestamp::from_millisecond(at)
                .map(|t| t.to_zoned(tz.clone()).minute() < 15)
                .unwrap_or(false);
            if !on_the_hour {
                continue;
            }
            clock(at, tz, "%-I%P")
        };
        let x = chart_x + bar_x(b, span.bins.len(), chart_w);
        if x < p.at + 1 && p.at > chart_x || x + width(&label) > chart_x + chart_w + 2 {
            continue;
        }
        p.to(x).put(label, Tone::Faint.style(theme));
    }
}

/// The rate limits: a meter each, its pace a tick on the 5-hour one, its
/// percentage and when it resets, and where the pace takes it by then.
fn limits(
    model: &Model,
    cols: usize,
    theme: &Theme,
    now_ms: i64,
    lines: &mut Vec<Line>,
    images: &mut Vec<Image>,
) {
    if model.limits.is_empty() {
        return;
    }
    lines.push(Line {
        spans: vec![kit::rule(cols, theme)],
        fill: None,
    });
    let mut p = Placer::default();
    p.put(" LIMITS", Tone::Dim.bold(theme));
    if let Some(at) = model.limits_at {
        p.right(
            format!("as the last statusline saw them, {}", ago(now_ms - at)),
            cols.saturating_sub(1),
            Tone::Faint.style(theme),
        );
    }
    lines.push(p.line(cols));
    let now = now_ms.div_euclid(1000);
    let details: Vec<String> = model.limits.iter().map(|l| limit_detail(l, now)).collect();
    let at = limit_columns(
        kit::natural(model.limits.iter().map(|l| l.name.as_str())),
        kit::natural(details.iter().map(String::as_str)),
        cols,
    );
    for (limit, detail) in model.limits.iter().zip(&details) {
        lines.push(limit_row(
            limit,
            detail,
            &at,
            cols,
            theme,
            lines.len(),
            images,
        ));
    }
}

/// A limit row's columns from x 2: its name, the meter stretching into
/// what the rest leave, its percentage, and when it resets; on a narrow
/// screen the reset goes first.
fn limit_columns(name: usize, detail: usize, cols: usize) -> Vec<Option<(usize, usize)>> {
    kit::columns(
        &[
            Col::Fixed {
                min: 4,
                natural: name,
                cap: 20,
            },
            Col::Stretch {
                weight: 1,
                min: 10,
                max: None,
            },
            Col::Fixed {
                min: 4,
                natural: 4,
                cap: 4,
            },
            Col::Fixed {
                min: 0,
                natural: detail,
                cap: 70,
            },
        ],
        cols.saturating_sub(3),
        2,
        &[3],
    )
    .into_iter()
    .map(|c| c.map(|(x, w)| (x + 2, w)))
    .collect()
}

/// When a limit resets, its own words, and where the pace takes it.
fn limit_detail(limit: &Limit, now: i64) -> String {
    let mut detail = Vec::new();
    if let Some(d) = &limit.detail {
        detail.push(d.clone());
    }
    detail.push(crate::usage::sessions::resets(limit.resets_at, now));
    if let Some(pace) = limit.pace {
        detail.push(format!("at this pace, {}% by then", pace.round() as i64));
    }
    detail.join(" · ")
}

fn limit_row(
    limit: &Limit,
    detail: &str,
    at: &[Option<(usize, usize)>],
    cols: usize,
    theme: &Theme,
    y: usize,
    images: &mut Vec<Image>,
) -> Line {
    let tone = if limit.used >= 100.0 {
        Tone::Failed
    } else if limit.used >= crate::usage::sessions::LIMIT_FULL {
        Tone::Needs
    } else {
        Tone::Done
    };
    let mut spans = Vec::new();
    if let Some(name) = at[0] {
        kit::place(
            &mut spans,
            vec![Span::styled(limit.name.clone(), Tone::Text.style(theme))],
            name,
        );
    }
    if let Some((x, w)) = at[1] {
        images.push(Image {
            name: format!("usage/limit/{}", limit.key),
            x: x as u16,
            y: y as u16,
            width: w as u16,
            height: 1,
            shape: Shape::Meter {
                frac: (limit.used / 100.0) as f32,
                color: tone.fg(theme),
                tick: (limit.key == "five_hour")
                    .then_some(limit.pace)
                    .flatten()
                    .map(|p| (p / 100.0) as f32),
            },
        });
        kit::place(&mut spans, Vec::new(), (x, w));
    }
    if let Some(pct) = at[2] {
        kit::place(
            &mut spans,
            vec![Span::styled(
                format!("{:>3}%", limit.used.round() as i64),
                tone.bold(theme),
            )],
            pct,
        );
    }
    if let Some(d) = at[3] {
        kit::place(
            &mut spans,
            vec![Span::styled(detail.to_string(), Tone::Dim.style(theme))],
            d,
        );
    }
    Line {
        spans: kit::fit(spans, cols, Style::new()),
        fill: None,
    }
}

/// The request audit: the counts by protocol, then a row a service, its
/// protocols, a sparkline by hour (or day), a meter of its share of the
/// busiest, its count, and under the chosen one what made them.
#[allow(clippy::too_many_arguments)]
fn requests(
    model: &Model,
    week: bool,
    items: &[Item],
    chosen_id: Option<&str>,
    m: &Mission,
    cols: usize,
    lines: &mut Vec<Line>,
    hits: &mut Vec<HitBox>,
    images: &mut Vec<Image>,
) {
    let theme = &m.snap.theme;
    let Some((today, this_week)) = &model.requests else {
        return;
    };
    let tally: &Tally = if week { this_week } else { today };
    lines.push(Line {
        spans: vec![kit::rule(cols, theme)],
        fill: None,
    });
    let mut p = Placer::default();
    p.put(" REQUESTS", Tone::Dim.bold(theme)).put(
        format!(
            "   {} {} to {} service{}",
            count(tally.total),
            if week { "this week" } else { "today" },
            tally.services.len(),
            if tally.services.len() == 1 { "" } else { "s" }
        ),
        Tone::Text.style(theme),
    );
    p.right(
        &model.requests_file,
        cols.saturating_sub(1),
        Tone::Faint.style(theme),
    );
    lines.push(p.line(cols));
    if tally.total > 0 {
        let mut p = Placer::default();
        p.put(
            format!("  by protocol: {}", by_count(&tally.protocols)),
            Tone::Dim.style(theme),
        );
        lines.push(p.line(cols));
    }
    let busiest = tally.services.first().map_or(1, |s| s.total.max(1));
    // The services in the view's order, which is the cursor's; one with
    // nothing today left out of today's board, and in the week's drawn at
    // none.
    let shown: Vec<ServiceCount> = items
        .iter()
        .filter(|i| week || !crate::usage::charts::is_week_only(i))
        .filter_map(|i| i.id.strip_prefix("requests:"))
        .filter(|name| *name != "uncounted")
        .map(|name| {
            tally
                .services
                .iter()
                // A day's tally may name it in another case than the week's.
                .find(|s| s.service.eq_ignore_ascii_case(name))
                .map(|s| ServiceCount {
                    service: name.to_string(),
                    ..s.clone()
                })
                .unwrap_or_else(|| ServiceCount {
                    service: name.to_string(),
                    total: 0,
                    protocols: Default::default(),
                    by_time: vec![0; if week { 7 } else { 24 }],
                    sources: Default::default(),
                })
        })
        .collect();
    // The protocols' column is as wide as the line under it needs too, what
    // made the requests, so that line isn't cut under a short name.
    let name_w = kit::natural(shown.iter().map(|s| s.service.as_str()));
    let protocols_w = shown
        .iter()
        .map(|s| width(&protocols_of(s)).max(width(&made_by(s)).saturating_sub(name_w + 2)))
        .max()
        .unwrap_or(0);
    let at = service_columns(name_w, protocols_w, cols);
    if !shown.is_empty() {
        let mut spans = Vec::new();
        let over = [
            (at[2], if week { "by day" } else { "by hour" }),
            (
                at[3],
                if week {
                    "share of the week"
                } else {
                    "share of today"
                },
            ),
        ];
        for (col, text) in over {
            if let Some(col) = col {
                kit::place(
                    &mut spans,
                    vec![Span::styled(text, Tone::Faint.style(theme))],
                    col,
                );
            }
        }
        lines.push(Line {
            spans: kit::fit(spans, cols, Style::new()),
            fill: None,
        });
    }
    for s in &shown {
        let index = index_of(items, &service_item(&s.service));
        let chosen = index.is_some() && chosen_id == Some(service_item(&s.service).as_str());
        let lit = !chosen && index.is_some_and(|i| m.hover == Some(Spot::Index(i as isize)));
        service_row(
            s, busiest, &at, chosen, lit, index, cols, theme, lines, hits, images,
        );
    }
}

/// What made a service's requests, `made by PR board 20 · CI 10`.
fn made_by(s: &ServiceCount) -> String {
    if s.sources.is_empty() {
        String::new()
    } else {
        format!("made by {}", by_count(&s.sources))
    }
}

/// A service's protocols, `GraphQL · REST`.
fn protocols_of(s: &ServiceCount) -> String {
    s.protocols.keys().cloned().collect::<Vec<_>>().join(" · ")
}

/// A service row's columns from x 1, as board 45 lays them out: its name
/// and protocols at their natural widths, the sparkline and the share
/// meter stretching into the rest by their widths at 160 (24 and 20), and
/// the count at the right edge; on a narrow screen the protocols go first,
/// then the meter.
fn service_columns(name: usize, protocols: usize, cols: usize) -> Vec<Option<(usize, usize)>> {
    kit::columns(
        &[
            Col::Fixed {
                min: 4,
                natural: name,
                cap: 20,
            },
            Col::Fixed {
                min: 0,
                natural: protocols,
                cap: 36,
            },
            Col::Stretch {
                weight: 24,
                min: 24,
                max: None,
            },
            Col::Stretch {
                weight: 20,
                min: 20,
                max: None,
            },
            Col::Right { natural: 7, cap: 7 },
        ],
        cols.saturating_sub(2),
        2,
        &[1, 3],
    )
    .into_iter()
    .map(|c| c.map(|(x, w)| (x + 1, w)))
    .collect()
}

/// A service's two lines: its name, protocols, meter and count, then what
/// made its requests (with its protocols when their column is dropped),
/// its sparkline two rows tall beside both.
#[allow(clippy::too_many_arguments)]
fn service_row(
    s: &ServiceCount,
    busiest: usize,
    at: &[Option<(usize, usize)>],
    chosen: bool,
    lit: bool,
    index: Option<usize>,
    cols: usize,
    theme: &Theme,
    lines: &mut Vec<Line>,
    hits: &mut Vec<HitBox>,
    images: &mut Vec<Image>,
) {
    let y = lines.len();
    let accent = if chosen { Tone::Accent } else { Tone::Faint };
    let peak = s.by_time.iter().copied().max().unwrap_or(0).max(1);
    if let Some((x, w)) = at[2] {
        images.push(Image {
            name: format!("usage/spark/{}", s.service),
            x: x as u16,
            y: y as u16,
            width: w as u16,
            height: 2,
            shape: Shape::Series {
                bars: s
                    .by_time
                    .iter()
                    .map(|n| vec![(*n as f32, accent.fg(theme))])
                    .collect(),
                max: peak as f32,
                over: None,
                marks: Marks::default(),
            },
        });
    }
    if let Some((x, w)) = at[3] {
        images.push(Image {
            name: format!("usage/share/{}", s.service),
            x: x as u16,
            y: y as u16,
            width: w as u16,
            height: 1,
            shape: Shape::Meter {
                frac: s.total as f32 / busiest as f32,
                // An unchosen meter's fill in the dim text color, which
                // stands out from its track as the faint one doesn't.
                color: if chosen { Tone::Accent } else { Tone::Dim }.fg(theme),
                tick: None,
            },
        });
    }
    let name_style = if chosen {
        Tone::Text.bold(theme)
    } else {
        Tone::Text.style(theme)
    };
    let mut first = vec![Span::raw(" ")];
    let second = vec![Span::raw(" ")];
    if let Some(col) = at[0] {
        kit::place(
            &mut first,
            vec![Span::styled(s.service.clone(), name_style)],
            col,
        );
    }
    if let Some(col) = at[1] {
        kit::place(
            &mut first,
            vec![Span::styled(protocols_of(s), Tone::Dim.style(theme))],
            col,
        );
    }
    for col in [at[2], at[3]].into_iter().flatten() {
        kit::place(&mut first, Vec::new(), col);
    }
    if let Some(col) = at[4] {
        kit::place(
            &mut first,
            vec![Span::styled(
                format!("{:>7}", count(s.total)),
                Tone::Text.style(theme),
            )],
            col,
        );
    }
    // What made them, under the name and protocols.
    let mut made = Vec::new();
    if at[1].is_none() && !s.protocols.is_empty() {
        made.push(protocols_of(s));
    }
    if !s.sources.is_empty() {
        made.push(made_by(s));
    }
    let made = made.join(" · ");
    let mut rows = vec![first, second];
    if let Some((x, _)) = at[0] {
        let end = at[2].map_or(cols.saturating_sub(1), |(sx, _)| sx.saturating_sub(2));
        let room = end.saturating_sub(x);
        kit::place(
            &mut rows[1],
            vec![Span::styled(made.clone(), Tone::Faint.style(theme))],
            (x, room),
        );
        // Chosen, what made them in full under the row, wrapped at its
        // parts across the row, when it doesn't fit beside the sparkline.
        if chosen && width(&made) > room {
            let across = cols.saturating_sub(x + 1);
            let mut wrapped: Vec<String> = Vec::new();
            for part in made.split(" · ") {
                match wrapped.last_mut() {
                    Some(line) if width(line) + 3 + width(part) <= across => {
                        line.push_str(" · ");
                        line.push_str(part);
                    }
                    _ => wrapped.push(part.to_string()),
                }
            }
            for text in wrapped {
                let mut row = vec![Span::raw(" ")];
                kit::place(
                    &mut row,
                    vec![Span::styled(text, Tone::Dim.style(theme))],
                    (x, across),
                );
                rows.push(row);
            }
        }
    }
    for (dy, spans) in rows.into_iter().enumerate() {
        let spans = under_mouse(
            kit::row(kit::fit(spans, cols - 1, Style::new()), cols, chosen, theme),
            lit,
            theme,
        );
        if let Some(index) = index {
            hits.push(HitBox {
                y: (y + dy) as u16,
                x0: 0,
                x1: cols.saturating_sub(1) as u16,
                spot: Spot::Index(index as isize),
            });
        }
        lines.push(Line { spans, fill: None });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn more_bins_than_columns_are_merged_so_bars_labels_and_clicks_agree() {
        let span = SpanSpend {
            total: 73.0,
            start_ms: 0,
            bin_ms: 900_000,
            bins: vec![vec![1.0]; 73],
            tabs: vec![TabSpend::default()],
        };
        // At 100 columns the chart has 54.
        let fitted = fitted(&span, 100);
        assert_eq!(fitted.bins.len(), 37);
        assert_eq!(fitted.bin_ms, 1_800_000);
        assert_eq!(fitted.bins[0], vec![2.0]);
        assert_eq!(fitted.bins[36], vec![1.0]);
        assert_eq!(super::fitted(&span, 200).bins.len(), 73);
    }

    #[test]
    fn below_the_board_width_the_protocols_go_first_then_the_meter() {
        let at = service_columns(10, 30, 160);
        assert!(at.iter().all(Option::is_some), "{at:?}");
        // The count flush with the right edge.
        assert_eq!(at[4].map(|(x, w)| x + w), Some(159));
        let at = service_columns(10, 30, 90);
        assert!(at[1].is_none() && at[3].is_some(), "{at:?}");
        let at = service_columns(10, 30, 60);
        assert!(
            at[1].is_none() && at[3].is_none() && at[2].is_some(),
            "{at:?}"
        );
    }

    #[test]
    fn the_tabs_column_keeps_its_share_of_a_wide_screen() {
        assert_eq!(side_w(160), SIDE_W);
        assert_eq!(side_w(240), 81);
        assert_eq!(side_w(80), SIDE_MIN);
    }

    #[test]
    fn a_chosen_service_shows_every_source_when_they_dont_fit_beside_its_sparkline() {
        let sources: indexmap::IndexMap<String, usize> = (1..=8)
            .map(|n| (format!("a source named {n}"), n))
            .collect();
        let s = ServiceCount {
            service: "GitHub".into(),
            total: 36,
            protocols: [("REST".to_string(), 36)].into_iter().collect(),
            by_time: vec![1; 24],
            sources,
        };
        let at = service_columns(6, 4, 160);
        let draw = |chosen: bool| {
            let (mut lines, mut hits, mut images) = (Vec::new(), Vec::new(), Vec::new());
            service_row(
                &s,
                36,
                &at,
                chosen,
                false,
                Some(3),
                160,
                &Theme::DARK,
                &mut lines,
                &mut hits,
                &mut images,
            );
            let text: Vec<String> = lines.iter().map(|l| l.text()).collect();
            (text, hits.len())
        };
        let (text, hits) = draw(false);
        assert_eq!(text.len(), 2, "{text:?}");
        assert_eq!(hits, 2);
        let (text, hits) = draw(true);
        assert_eq!(text.len(), 4, "{text:?}");
        assert_eq!(hits, 4);
        let all = text[2..].join(" ");
        for n in 1..=8 {
            assert!(all.contains(&format!("a source named {n} {n}")), "{text:?}");
        }
        assert!(!all.contains('…'), "{text:?}");
    }
}
