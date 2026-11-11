//! With UX 2.0, the Resources view as a board (board 25 of the canvas,
//! new with the port; the tmux version drew it only as the table `list`
//! still draws below `BOARD_AT` columns): the whole computer at the top (its
//! CPU as a sparkline of the last two minutes, its memory as a meter, and
//! Switchboard's own CPU and memory), then a two-line row per tab with its
//! CPU, memory and process count, red past a core or 4 GB with a ✕ chip
//! saying why, its heaviest processes on the line under it, and its CPU and
//! memory over the last two minutes as sparklines at the right; the chosen
//! row opens to its heaviest processes as a tree.
//!
//! Everything comes from the view's data as `usage::resources` worked it
//! out off the server's task (`head`, and each item's `data`), so drawing
//! reads no process. The sparklines and the meter are image atoms
//! (`render::Image`), named by what they show so each reading replaces
//! them in place, and drawn as text where the terminal takes no images.

use ratatui::style::{Modifier, Style};
use ratatui::text::Span;
use serde_json::Value;

use super::render::{Image, Shape};
use super::views::View;
use super::{HitBox, Mission, Spot};
use crate::items::Item;
use crate::mux::TabId;
use crate::sidebar::render::Line;
use crate::sidebar::text::width;
use crate::ui::Theme;
use crate::ui::kit::{self, Surface, Tone};
use crate::usage::resources::{CPU_HEAVY, MEMORY_HEAVY, bytes};

/// The board needs this many columns; under it `list` draws the table.
pub const BOARD_AT: usize = 136;

/// What `s` sorts the tabs by.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Sort {
    #[default]
    Cpu,
    Memory,
}

impl Sort {
    pub fn next(self) -> Sort {
        match self {
            Sort::Cpu => Sort::Memory,
            Sort::Memory => Sort::Cpu,
        }
    }

    /// The items heaviest first by this; the order the view gave breaks a
    /// tie.
    pub fn sort(self, items: &mut [Item]) {
        let key = |i: &Item| match self {
            Sort::Cpu => i.data["cpu"].as_f64().unwrap_or(0.0),
            Sort::Memory => i.data["memory"].as_f64().unwrap_or(0.0),
        };
        items.sort_by(|a, b| key(b).total_cmp(&key(a)));
    }
}

/// The widest a tab's sparkline is; it holds the last two minutes.
const SPARK_W: usize = 20;
/// The whole computer's CPU sparkline.
const TOP_SPARK_W: usize = 40;
const TOP_METER_W: usize = 30;
/// Where the top's sparkline and its memory start.
const TOP_SPARK_AT: usize = 24;
const TOP_MEMORY_AT: usize = TOP_SPARK_AT + TOP_SPARK_W + 6;
/// The figures' columns.
const CPU_W: usize = 6;
const MEMORY_W: usize = 9;
const PROCS_W: usize = 6;
/// A tab's sparklines' tops: 160% CPU, 6 GB.
const CPU_MAX: f32 = 160.0;
const MEMORY_MAX: f32 = 6.0 * 1024.0 * 1024.0 * 1024.0;
/// The processes the chosen row's tree shows before "N more".
const TREE_SHOWN: usize = 4;
const NOTE: &str = "Memory is resident, so what two processes share counts in both. 100% CPU is one core, as top counts it.";

fn f(v: &Value, key: &str) -> f64 {
    v[key].as_f64().unwrap_or(0.0)
}

/// `38%`, `0.4%` under one.
fn percent(cpu: f64) -> String {
    if cpu < 1.0 && cpu > 0.0 {
        format!("{cpu:.1}%")
    } else {
        format!("{cpu:.0}%")
    }
}

/// `text` right-aligned in `cols`.
fn right(text: &str, cols: usize) -> String {
    format!("{}{text}", " ".repeat(cols.saturating_sub(width(text))))
}

/// The readings folded into at most `cols` values, each the highest of the
/// readings it stands for, so a sparkline narrower than the history still
/// spans all of it (the text form draws a value a column) and keeps a peak.
fn folded(values: &[f64], cols: usize) -> Vec<f64> {
    if values.len() <= cols || cols == 0 {
        return values.to_vec();
    }
    let per = values.len().div_ceil(cols);
    values
        .chunks(per)
        .map(|c| c.iter().copied().fold(0.0, f64::max))
        .collect()
}

/// The columns a row's figures and sparklines take after its name.
fn tail_w() -> usize {
    2 + CPU_W + 2 + MEMORY_W + 2 + PROCS_W + 3 + SPARK_W + 2 + SPARK_W + 1
}

/// A row's figures, each red past its limit: CPU, memory, processes.
fn figures(cpu: f64, memory: f64, procs: Option<u64>, theme: &Theme) -> Vec<Span<'static>> {
    let tone = |heavy: bool| {
        if heavy {
            Tone::Failed.bold(theme)
        } else {
            Tone::Text.style(theme)
        }
    };
    vec![
        Span::raw("  "),
        Span::styled(right(&percent(cpu), CPU_W), tone(cpu >= CPU_HEAVY)),
        Span::raw("  "),
        Span::styled(
            right(&bytes(memory as u64), MEMORY_W),
            tone(memory >= MEMORY_HEAVY as f64),
        ),
        Span::raw("  "),
        Span::styled(
            right(&procs.map(|n| n.to_string()).unwrap_or_default(), PROCS_W),
            Tone::Dim.style(theme),
        ),
    ]
}

/// What a line of the list is: a row's first line (with the item whose
/// sparklines it carries), or any other line, with what a click on it
/// chooses.
struct Entry {
    spans: Vec<Span<'static>>,
    spot: Option<Spot>,
    sparks: Option<usize>,
}

/// The chosen row's processes as a tree: the heaviest `TREE_SHOWN` in the
/// order the walk from its panes met them, each indented by its depth with
/// its CPU and memory in the columns, then how many more and their names.
fn tree_lines(item: &Item, name_w: usize, theme: &Theme) -> Vec<Vec<Span<'static>>> {
    let tree: Vec<&Value> = item.data["tree"]
        .as_array()
        .map(|t| t.iter().collect())
        .unwrap_or_default();
    let weight = |p: &Value| f(p, "cpu") / CPU_HEAVY + f(p, "memory") / MEMORY_HEAVY as f64;
    let mut heaviest: Vec<usize> = (0..tree.len()).collect();
    heaviest.sort_by(|&a, &b| weight(tree[b]).total_cmp(&weight(tree[a])));
    heaviest.truncate(TREE_SHOWN);
    heaviest.sort_unstable();
    let mut out = Vec::new();
    for &at in &heaviest {
        let p = tree[at];
        let depth = p["depth"].as_u64().unwrap_or(0) as usize;
        let name = p["name"].as_str().unwrap_or("?");
        let lead = format!("    {}├ ", "  ".repeat(depth.min(6)));
        let room = name_w.saturating_sub(width(&lead));
        let mut spans = vec![
            Span::styled(lead, Style::new().fg(kit::border(theme))),
            Span::styled(
                crate::sidebar::text::cut(name, room),
                Tone::Text.style(theme),
            ),
        ];
        spans = kit::fit(spans, name_w, Style::new());
        spans.extend(figures(f(p, "cpu"), f(p, "memory"), None, theme));
        out.push(spans);
    }
    if tree.len() > heaviest.len() {
        let mut rest: Vec<&str> = Vec::new();
        for (at, p) in tree.iter().enumerate() {
            let name = p["name"].as_str().unwrap_or("?");
            if !heaviest.contains(&at) && !rest.contains(&name) {
                rest.push(name);
            }
        }
        let names = rest.iter().take(3).copied().collect::<Vec<_>>().join(", ");
        let more = if rest.len() > 3 { ", …" } else { "" };
        out.push(vec![Span::styled(
            format!("    {} more: {names}{more}", tree.len() - heaviest.len()),
            Tone::Dim.style(theme),
        )]);
    }
    out
}

/// The board, from the bar down to the footer: `lines` gets its lines,
/// `hits` where a click on a row lands, and `images` the sparklines and the
/// meter to place over them.
pub fn board(
    view: &View,
    cols: usize,
    rows: usize,
    m: &Mission,
    lines: &mut Vec<Line>,
    hits: &mut Vec<HitBox>,
    images: &mut Vec<Image>,
) {
    let theme = &m.snap.theme;
    let line = |spans: Vec<Span<'static>>| Line {
        spans: kit::fit(spans, cols, Style::new()),
        fill: None,
    };
    let head = &view.data.head;
    // The whole computer.
    if head.is_object() {
        let top = lines.len();
        let cpu = head["cpu"].as_f64();
        let cores = head["cores"].as_u64().unwrap_or(1);
        let mut first = vec![
            Span::raw(" "),
            Span::styled("CPU ", Tone::Text.bold(theme)),
            Span::styled(
                cpu.map_or("…".into(), |c| format!("{c:.0}%")),
                Tone::Text.bold(theme),
            ),
            Span::styled(
                format!(" of {cores} core{}", if cores == 1 { "" } else { "s" }),
                Tone::Dim.style(theme),
            ),
        ];
        first = kit::fit(first, TOP_MEMORY_AT, Style::new());
        let used = f(head, "memoryUsed");
        let total = f(head, "memoryTotal");
        let memory_words = vec![
            Span::styled("Memory ", Tone::Text.bold(theme)),
            Span::styled(bytes(used as u64), Tone::Text.bold(theme)),
            Span::styled(
                format!(" of {}", bytes(total as u64)),
                Tone::Dim.style(theme),
            ),
        ];
        let memory_w = 32;
        first.extend(kit::fit(memory_words, memory_w, Style::new()));
        let mut second = kit::fit(vec![Span::raw("")], TOP_MEMORY_AT, Style::new());
        second.push(Span::styled(
            format!(
                "Switchboard itself: {} CPU, {}",
                percent(f(head, "ownCpu")),
                bytes(f(head, "ownMemory") as u64)
            ),
            Tone::Dim.style(theme),
        ));
        let mut third = vec![Span::raw(" ".repeat(TOP_SPARK_AT))];
        third.push(Span::styled("the last 2 minutes", Tone::Faint.style(theme)));
        lines.push(line(first));
        lines.push(line(second));
        lines.push(line(third));
        let bars: Vec<crate::ui::images::Bar> = head["cpuHistory"]
            .as_array()
            .map(|h| {
                h.iter()
                    .map(|v| vec![(v.as_f64().unwrap_or(0.0) as f32, Tone::Working.fg(theme))])
                    .collect()
            })
            .unwrap_or_default();
        images.push(Image {
            name: "resources/cpu".into(),
            x: TOP_SPARK_AT as u16,
            y: top as u16,
            width: TOP_SPARK_W as u16,
            height: 2,
            shape: Shape::Series {
                bars,
                max: 100.0,
                over: None,
            },
        });
        if total > 0.0 && cols >= TOP_MEMORY_AT + memory_w + TOP_METER_W {
            let frac = (used / total) as f32;
            let tone = if frac >= 0.9 {
                Tone::Failed
            } else if frac >= 0.75 {
                Tone::Needs
            } else {
                Tone::Done
            };
            images.push(Image {
                name: "resources/memory".into(),
                x: (TOP_MEMORY_AT + memory_w) as u16,
                y: top as u16,
                width: TOP_METER_W as u16,
                height: 1,
                shape: Shape::Meter {
                    frac,
                    color: tone.fg(theme),
                    tick: None,
                },
            });
        }
        lines.push(line(vec![kit::rule(cols, theme)]));
    }
    // The table's heads, the sorted column's lit.
    let name_w = cols.saturating_sub(tail_w() + 1);
    let lit = |on: bool| {
        if on {
            Tone::Accent.bold(theme)
        } else {
            Tone::Dim.bold(theme)
        }
    };
    let sort = m.resources_sort;
    let heads = vec![
        Span::raw(" "),
        Span::styled(format!("{:<name_w$}", "TAB"), lit(false)),
        Span::raw("  "),
        Span::styled(right("CPU", CPU_W), lit(sort == Sort::Cpu)),
        Span::raw("  "),
        Span::styled(right("MEMORY", MEMORY_W), lit(sort == Sort::Memory)),
        Span::raw("  "),
        Span::styled(right("PROCS", PROCS_W), lit(false)),
        Span::raw("   "),
        Span::styled(format!("{:<SPARK_W$}", "CPU, LAST 2 MINUTES"), lit(false)),
        Span::raw("  "),
        Span::styled("MEMORY", lit(false)),
    ];
    lines.push(line(heads));
    lines.push(line(vec![Span::raw(" "), kit::head_rule(cols - 2, theme)]));
    // The rows.
    let cursor = usize::try_from(m.cursor).ok();
    let tabs = if m.snap.all_tabs.is_empty() {
        &m.snap.tabs
    } else {
        &m.snap.all_tabs
    };
    let mut entries: Vec<Entry> = Vec::new();
    let mut chosen_end = 0;
    for (at, item) in view.data.items.iter().enumerate() {
        let chosen = cursor == Some(at);
        let spot = Some(Spot::Index(at as isize));
        let hover = m.hover == Some(Spot::Index(at as isize));
        let d = &item.data;
        let tab = match item.target {
            crate::items::Target::Tab(t) => tabs.iter().find(|x| x.id == t),
            _ => None,
        };
        let (number, name) = item
            .title
            .split_once(' ')
            .unwrap_or(("", item.title.as_str()));
        let color = tab
            .and_then(|t| t.color)
            .map(ratatui::style::Color::from)
            .unwrap_or(theme.bright);
        let mut left = vec![
            Span::styled(
                format!("{number} "),
                Style::new().fg(color).add_modifier(Modifier::BOLD),
            ),
            Span::styled(name.to_string(), Tone::Text.bold(theme)),
        ];
        left = kit::fit(left, name_w, Style::new());
        left.extend(figures(
            f(d, "cpu"),
            f(d, "memory"),
            d["processes"].as_u64(),
            theme,
        ));
        // The chip leads, so long process names are what gets cut.
        let mut under = vec![Span::raw("  ")];
        if let Some(why) = d["why"].as_str() {
            under.extend(kit::chip(
                &format!("✕ {why}"),
                Tone::Failed,
                m.snap.caps,
                theme,
            ));
            under.push(Span::raw(" "));
        }
        under.push(Span::styled(
            d["busiest"].as_str().unwrap_or("").to_string(),
            Tone::Dim.style(theme),
        ));
        let under = kit::fit(under, name_w + tail_w() - SPARK_W * 2 - 4, Style::new());
        let shade = |spans: Vec<Span<'static>>| -> Vec<Span<'static>> {
            let spans = kit::row(spans, cols - 1, chosen, theme);
            if hover && !chosen {
                let bg = Surface::Panel.color(theme);
                spans
                    .into_iter()
                    .map(|s| Span::styled(s.content, s.style.bg(bg)))
                    .collect()
            } else {
                spans
            }
        };
        entries.push(Entry {
            spans: shade(left),
            spot,
            sparks: Some(at),
        });
        entries.push(Entry {
            spans: shade(under),
            spot,
            sparks: None,
        });
        if chosen {
            for spans in tree_lines(item, name_w, theme) {
                entries.push(Entry {
                    spans: shade(spans),
                    spot,
                    sparks: None,
                });
            }
            chosen_end = entries.len();
        }
    }
    let top = lines.len();
    // Two lines for the note under the table.
    let room = rows.saturating_sub(1 + top + 2);
    let first = chosen_end.saturating_sub(room);
    let shown: Vec<&Entry> = entries.iter().skip(first).take(room).collect();
    for (i, e) in shown.iter().enumerate() {
        let y = top + i;
        if let Some(spot) = e.spot {
            hits.push(HitBox {
                y: y as u16,
                x0: 0,
                x1: cols.saturating_sub(1) as u16,
                spot,
            });
        }
        // A row's sparklines take its two lines, at the right.
        if let Some(at) = e.sparks
            && i + 1 < shown.len()
        {
            let item = &view.data.items[at];
            let d = &item.data;
            let series = |key: &str, tone: Tone| -> Vec<crate::ui::images::Bar> {
                let values: Vec<f64> = d[key]
                    .as_array()
                    .map(|h| h.iter().map(|v| v.as_f64().unwrap_or(0.0)).collect())
                    .unwrap_or_default();
                folded(&values, SPARK_W)
                    .into_iter()
                    .map(|v| vec![(v as f32, tone.fg(theme))])
                    .collect()
            };
            let x = 1 + name_w + 2 + CPU_W + 2 + MEMORY_W + 2 + PROCS_W + 3;
            let id = match item.target {
                crate::items::Target::Tab(TabId(n)) => n.to_string(),
                _ => item.id.clone(),
            };
            images.push(Image {
                name: format!("resources/{id}/cpu"),
                x: x as u16,
                y: y as u16,
                width: SPARK_W as u16,
                height: 2,
                shape: Shape::Series {
                    bars: series("cpuHistory", Tone::Working),
                    max: CPU_MAX,
                    over: Some(CPU_HEAVY as f32),
                },
            });
            images.push(Image {
                name: format!("resources/{id}/memory"),
                x: (x + SPARK_W + 2) as u16,
                y: y as u16,
                width: SPARK_W as u16,
                height: 2,
                shape: Shape::Series {
                    bars: series("memoryHistory", Tone::Landing),
                    max: MEMORY_MAX,
                    over: Some(MEMORY_HEAVY as f32),
                },
            });
        }
        lines.push(Line {
            spans: e.spans.clone(),
            fill: None,
        });
    }
    for _ in shown.len()..room {
        lines.push(line(Vec::new()));
    }
    lines.push(line(Vec::new()));
    lines.push(line(vec![Span::styled(
        format!(" {NOTE}"),
        Tone::Faint.style(theme),
    )]));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_minutes_fold_into_the_sparklines_columns_keeping_each_peak() {
        let mut readings = vec![0.0; 40];
        readings[1] = 90.0;
        let cols = folded(&readings, 20);
        assert_eq!(cols.len(), 20);
        assert_eq!(cols[0], 90.0, "a spike in the older minute stays");
        assert_eq!(folded(&[1.0, 2.0, 3.0], 20), vec![1.0, 2.0, 3.0]);
        assert_eq!(folded(&[1.0, 5.0, 2.0], 2), vec![5.0, 2.0]);
    }

    #[test]
    fn s_sorts_by_cpu_then_memory() {
        let item = |id: &str, cpu: f64, memory: u64| Item {
            id: id.into(),
            data: serde_json::json!({ "cpu": cpu, "memory": memory }),
            ..Item::default()
        };
        let mut items = vec![item("a", 1.0, 900), item("b", 50.0, 10)];
        Sort::Cpu.sort(&mut items);
        assert_eq!(items[0].id, "b");
        Sort::Cpu.next().sort(&mut items);
        assert_eq!(items[0].id, "a");
        assert_eq!(Sort::Memory.next(), Sort::Cpu);
    }
}
