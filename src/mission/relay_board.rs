//! Mission control's Slack relay view, board 53 ("Slack relay"): whether
//! the relay is running, in which tab and for how long, what it sent today
//! by itself (answers to what you sent it from Slack aren't counted, as
//! they go to you whatever the kinds say)
//! and how much of it notified you; the three presets (src/relay/sending.rs),
//! the one the settings match lit; a row for each kind of message the relay
//! sends with today's count, a choice switch (`kit::choice_switch`) of
//! Notify, Thread or Off and a "changed" chip where it differs from its
//! default, the chosen row opening to its whole description, today's count
//! by tab and its default; then autopilot's tabs and quiet hours as rows of
//! the same kind. New with the view, with no tmux-version file behind it.
//!
//! Its rows are the view's items (`items`), so the cursor, the filter,
//! Enter and Space work on them as on any list, and narrower than
//! `BOARD_AT` or without UX 2.0 the plain list draws them. Enter opens a
//! row's menu (`choices`): a setting's other values, a preset applied, the
//! relay started or stopped; a click on a switch's part sets it at once
//! (`part_value`). The counts are the relay's own file of what it sent
//! (`sending::read_sent`), which the server reads at most every few
//! seconds while the view is built, and the relay's state is the tab the
//! server holds marked `@slack_relay`, never a process.

use ratatui::style::{Modifier, Style};
use ratatui::text::Span;
use serde_json::{Value, json};

use super::views::{self, View};
use super::{HitBox, Mission, Spot};
use crate::items::{Item, State};
use crate::relay::sending::{self, Kind, Preset, Sent};
use crate::settings;
use crate::sidebar::render::Line;
use crate::sidebar::text::{cut, width, wrap};
use crate::ui::Theme;
use crate::ui::kit::{self, Col, Tone};

/// How wide the screen must be for the board; narrower, the plain list.
pub const BOARD_AT: usize = 80;
/// A label is cut past this many columns, and to this many at the least.
const LABEL_MAX: usize = 40;
const LABEL_MIN: usize = 24;
/// The today column.
const TODAY_W: usize = 5;
/// The "changed" chip, with its ends and the space after it.
const CHANGED_W: usize = 10;
/// A wrapped description at most.
const TEXT_MAX: usize = 100;

pub const STATUS: &str = "relay:status";
const PRESET: &str = "relay:preset:";

/// The relay as the server holds it: the tab marked `@slack_relay`, its
/// number and how long it has been open.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Running {
    pub tab: usize,
    pub age: Option<String>,
}

/// The settings below the kinds, in the order they show.
const AUTOPILOT_ROWS: [&str; 1] = [sending::AUTOPILOT];
const QUIET_ROWS: [&str; 4] = [
    sending::QUIET,
    sending::QUIET_FROM,
    sending::QUIET_TO,
    sending::QUIET_PROMPTS,
];

/// A setting's row: its label and description, its choices (on and off
/// for a switch), its value, its default and today's count when it is a
/// kind's.
fn setting_row(o: &settings::Setting, sent: &[Sent]) -> Item {
    let parts: Vec<Value> = match &o.choices {
        Some(list) => list
            .iter()
            .map(|c| {
                let short = c
                    .value
                    .as_str()
                    .and_then(sending::Send::of_value)
                    .map_or_else(|| c.label.clone(), |s| s.short().to_string());
                json!({ "value": c.value, "label": c.label, "short": short })
            })
            .collect(),
        None => vec![
            json!({ "value": true, "label": "On", "short": "On" }),
            json!({ "value": false, "label": "Off", "short": "Off" }),
        ],
    };
    let kind = Kind::ALL.into_iter().find(|k| k.key() == o.key);
    let mut data = json!({
        "relay_setting": o.key,
        "parts": parts,
        // The setting's own value; `value` is what the plain list shows
        // beside the label, its words and, for a kind, today's count.
        "raw": o.value,
        "value": o.value_words(),
        "words": o.value_words(),
        "default": o.default_words(),
        "changed": o.changed(),
        "search": format!("{} {}", o.key, o.value_words()),
    });
    if let Some(kind) = kind {
        let mine: Vec<&Sent> = sent
            .iter()
            .filter(|s| s.kind == kind && !s.notified)
            .collect();
        data["today"] = json!(mine.len());
        data["by_tab"] = json!(by_tab(&mine));
        data["value"] = json!(format!("{}, {} today", o.value_words(), mine.len()));
    }
    let off = o.value == Value::Bool(false) || o.value == "off";
    Item::new(format!("relay:setting:{}", o.key), &o.label)
        .detail(&o.description)
        .state(if off { State::Idle } else { State::Active })
        .data(data)
}

/// Today's count of a kind by tab, the most first: "tab 5 15 (autopilot) ·
/// tab 3 4", notices counted as "other programs".
fn by_tab(sent: &[&Sent]) -> String {
    let mut counts: Vec<(Option<i64>, bool, usize)> = Vec::new();
    for s in sent {
        let tab = s.tab.as_ref().map(|(i, _)| *i);
        match counts.iter_mut().find(|(t, _, _)| *t == tab) {
            Some(c) => {
                c.1 |= s.autopilot;
                c.2 += 1;
            }
            None => counts.push((tab, s.autopilot, 1)),
        }
    }
    counts.sort_by(|a, b| b.2.cmp(&a.2).then(a.0.cmp(&b.0)));
    counts
        .iter()
        .map(|(tab, pilot, n)| match tab {
            Some(t) if *pilot => format!("tab {t} {n} (autopilot)"),
            Some(t) => format!("tab {t} {n}"),
            None => format!("other programs {n}"),
        })
        .collect::<Vec<_>>()
        .join(" · ")
}

/// The view's rows, from the settings as last read, today's sent file and
/// the relay's tab.
pub fn items(sent: &[Sent], running: Option<&Running>) -> Vec<Item> {
    items_of(&settings::current_settings(), sent, running)
}

pub(crate) fn items_of(
    all: &[settings::Setting],
    sent: &[Sent],
    running: Option<&Running>,
) -> Vec<Item> {
    // A message is a line, and its line in the main DM, once posted, one
    // more marked notified.
    let notified = sent.iter().filter(|s| s.notified).count();
    let posted = sent.iter().filter(|s| !s.notified).count();
    let status = match running {
        Some(r) => Item::new(STATUS, "Running")
            .detail(match &r.age {
                Some(age) => format!("in tab {}, for {age}", r.tab),
                None => format!("in tab {}", r.tab),
            })
            .state(State::Active),
        None => Item::new(STATUS, "Not running")
            .detail("Enter starts it in a tab of its own")
            .state(State::Idle),
    }
    .data(json!({
        "running": running.is_some(),
        "sent": posted,
        "value": format!("{posted} sent by itself, {notified} notified you today"),
        "notified": notified,
        "search": "relay running stop start",
    }));
    let mut out = vec![status];
    let value_of = |key: &str| all.iter().find(|o| o.key == key).map(|o| o.value.clone());
    let lit = Preset::matching(&value_of);
    out.push(views::header("presets", "Presets", Preset::ALL.len()));
    for p in Preset::ALL {
        out.push(
            Item::new(format!("{PRESET}{}", p.slug()), p.label())
                .detail(p.about())
                .state(if lit == Some(p) {
                    State::Active
                } else {
                    State::Idle
                })
                .data(json!({
                    "preset": p.slug(),
                    "lit": lit == Some(p),
                    "search": format!("preset {}", p.slug()),
                })),
        );
    }
    let rows = |keys: Vec<String>, out: &mut Vec<Item>| {
        for key in keys {
            if let Some(o) = all.iter().find(|o| o.key == key) {
                out.push(setting_row(o, sent));
            }
        }
    };
    out.push(views::header("kinds", "What reaches you", Kind::ALL.len()));
    rows(Kind::ALL.iter().map(|k| k.key()).collect(), &mut out);
    out.push(views::header(
        "autopilot",
        "Autopilot's tabs",
        AUTOPILOT_ROWS.len(),
    ));
    rows(AUTOPILOT_ROWS.map(str::to_string).to_vec(), &mut out);
    out.push(views::header("quiet", "Quiet hours", QUIET_ROWS.len()));
    rows(QUIET_ROWS.map(str::to_string).to_vec(), &mut out);
    out
}

/// With no slack-relay.json naming its owner, the relay's row says it isn't
/// set up and how to set it up, rather than offering to start it.
pub fn not_set_up(items: &mut [Item]) {
    if let Some(status) = items.iter_mut().find(|i| i.id == STATUS) {
        status.title = "Not set up".into();
        status.detail = "it needs a Slack app and ~/.config/switchboard/slack-relay.json naming you: the guide's \"The Slack relay\" says how".into();
        status.data["set_up"] = json!(false);
    }
}

/// The settings key of a setting's row.
pub fn setting_of(item: &Item) -> Option<&str> {
    item.data.get("relay_setting").and_then(Value::as_str)
}

/// A preset's row's preset.
pub fn preset_of(item: &Item) -> Option<Preset> {
    item.data
        .get("preset")
        .and_then(Value::as_str)
        .and_then(Preset::of_slug)
}

/// A setting's row's choices: each one's value, its words in a menu and its
/// words on the switch.
pub fn parts(item: &Item) -> Vec<(Value, String, String)> {
    item.data
        .get("parts")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|p| {
            let text = |k: &str| p.get(k).and_then(Value::as_str).unwrap_or("").to_string();
            (p["value"].clone(), text("label"), text("short"))
        })
        .collect()
}

/// The setting and value a click on part `n` of a row's switch sets.
pub fn part_value(item: &Item, n: usize) -> Option<(String, Value, String)> {
    let key = setting_of(item)?.to_string();
    let (value, label, _) = parts(item).into_iter().nth(n)?;
    Some((key, value, label))
}

/// Whether a row draws a switch: two or three choices; more (an hour) show
/// its value's words.
fn switched(item: &Item) -> bool {
    let n = parts(item).len();
    (2..=3).contains(&n)
}

// ── Drawing ─────────────────────────────────────────────────────────────────

/// A line of the board, the row of the view it belongs to, and the parts
/// of its switch at their columns.
struct BoardLine {
    spans: Vec<Span<'static>>,
    row: Option<usize>,
    parts: Vec<(usize, usize, Spot)>,
}

impl BoardLine {
    fn plain(spans: Vec<Span<'static>>) -> BoardLine {
        BoardLine {
            spans,
            row: None,
            parts: Vec::new(),
        }
    }
}

/// The columns of a kind's row `w` wide: its label, today's count, its
/// switch and the "changed" chip at the right edge, the chip going first
/// where the width can't hold them all and then the label cut to
/// `LABEL_MIN`.
fn row_columns(label_w: usize, switch_w: usize, w: usize) -> [Option<(usize, usize)>; 4] {
    let gap = 2;
    let laid = |label_min: usize, changed: bool| {
        let mut cols = vec![
            Col::Stretch {
                weight: 1,
                min: label_min,
                max: None,
            },
            Col::Fixed {
                min: TODAY_W,
                natural: TODAY_W,
                cap: TODAY_W,
            },
            Col::Fixed {
                min: switch_w,
                natural: switch_w,
                cap: switch_w,
            },
        ];
        if changed {
            cols.push(Col::Right {
                natural: CHANGED_W,
                cap: CHANGED_W,
            });
        }
        let least = label_min + TODAY_W + switch_w + gap * 2 + if changed { CHANGED_W } else { 0 };
        (least <= w).then(|| kit::columns(&cols, w, gap, &[]))
    };
    let label_nat = label_w.min(LABEL_MAX);
    let laid = laid(label_nat, true)
        .or_else(|| laid(label_nat, false))
        .or_else(|| laid(LABEL_MIN, false))
        .unwrap_or_else(|| {
            kit::columns(
                &[
                    Col::Stretch {
                        weight: 1,
                        min: 1,
                        max: None,
                    },
                    Col::Fixed {
                        min: TODAY_W,
                        natural: TODAY_W,
                        cap: TODAY_W,
                    },
                    Col::Fixed {
                        min: switch_w,
                        natural: switch_w,
                        cap: switch_w,
                    },
                ],
                w,
                gap,
                &[],
            )
        });
    [
        laid.first().copied().flatten(),
        laid.get(1).copied().flatten(),
        laid.get(2).copied().flatten(),
        laid.get(3).copied().flatten(),
    ]
}

/// A row's spans put at `x` on a line built so far.
fn put_at(line: &mut Vec<Span<'static>>, spans: Vec<Span<'static>>, x: usize) {
    let used = kit::spans_width(line);
    if used < x {
        line.push(Span::raw(" ".repeat(x - used)));
    }
    line.extend(spans);
}

/// The status line: the relay's state, today's counts and a Start or Stop
/// chip at the right.
fn status_line(item: &Item, w: usize, chosen: bool, lit: bool, m: &Mission) -> Vec<Span<'static>> {
    let theme = &m.snap.theme;
    let running = item.data["running"] == true;
    let n = |k: &str| item.data.get(k).and_then(Value::as_u64).unwrap_or(0);
    let (mark, tone) = if running {
        ("●", Tone::Done)
    } else {
        ("○", Tone::Dim)
    };
    let left = vec![
        Span::raw(" "),
        Span::styled(mark, tone.style(theme)),
        Span::raw(" "),
        Span::styled(item.title.clone(), Tone::Text.bold(theme)),
        Span::styled(format!("  {}", item.detail), Tone::Dim.style(theme)),
    ];
    let counts = format!(
        "{} sent by itself today · {} notified you  ",
        n("sent"),
        n("notified")
    );
    let chip = if running {
        kit::chip("Stop", Tone::Failed, m.snap.caps, theme)
    } else {
        kit::chip("Start", Tone::Done, m.snap.caps, theme)
    };
    let right_w = width(&counts) + kit::spans_width(&chip);
    let room = w.saturating_sub(right_w + 1);
    let mut line = kit::fit(left, room, Style::new());
    line.push(Span::styled(counts, Tone::Dim.style(theme)));
    line.extend(chip);
    let line = kit::row(line, w, chosen, theme);
    if lit { lit_line(line, theme) } else { line }
}

/// A line under the mouse: on the Raised surface.
fn lit_line(spans: Vec<Span<'static>>, theme: &Theme) -> Vec<Span<'static>> {
    let bg = kit::Surface::Raised.color(theme);
    spans
        .into_iter()
        .map(|s| {
            let style = s.style.bg(bg);
            Span::styled(s.content, style)
        })
        .collect()
}

/// The presets' chips, wrapped to as many lines as the width needs: the lit
/// one in the accent, the chosen one (the cursor's, or the mouse's) bright
/// and reversed; "Custom" dim when the settings match none, then what the
/// chosen or lit one does.
fn preset_lines(items: &[Item], cursor: Option<usize>, w: usize, m: &Mission) -> Vec<BoardLine> {
    let theme = &m.snap.theme;
    let at: Vec<usize> = (0..items.len())
        .filter(|&i| preset_of(&items[i]).is_some())
        .collect();
    let mut lines: Vec<BoardLine> = Vec::new();
    let mut line: Vec<Span<'static>> = vec![Span::raw("  ")];
    let mut hits: Vec<(usize, usize, Spot)> = Vec::new();
    let mut any_lit = false;
    for &i in &at {
        let item = &items[i];
        let lit = item.data["lit"] == true;
        any_lit |= lit;
        let ring = cursor == Some(i) || m.hover == Some(Spot::Index(i as isize));
        let chip = if ring {
            vec![
                Span::styled(
                    format!(" {} ", item.title),
                    Style::new()
                        .fg(theme.bright)
                        .add_modifier(Modifier::BOLD | Modifier::REVERSED),
                ),
                Span::raw(" "),
            ]
        } else if lit {
            kit::chip(&item.title, Tone::Accent, m.snap.caps, theme)
        } else {
            kit::chip(&item.title, Tone::Dim, m.snap.caps, theme)
        };
        let cw = kit::spans_width(&chip);
        let x = kit::spans_width(&line);
        if x + cw > w && x > 2 {
            lines.push(BoardLine {
                spans: std::mem::take(&mut line),
                row: None,
                parts: std::mem::take(&mut hits),
            });
            line = vec![Span::raw("  ")];
        }
        let x = kit::spans_width(&line);
        hits.push((x, cw.saturating_sub(1), Spot::Index(i as isize)));
        line.extend(chip);
    }
    let about = cursor
        .filter(|c| at.contains(c))
        .or_else(|| at.iter().copied().find(|&i| items[i].data["lit"] == true))
        .map(|i| items[i].detail.clone());
    let mut tail: Vec<Span<'static>> = Vec::new();
    if !any_lit {
        tail.push(Span::styled("Custom  ", Tone::Faint.style(theme)));
    }
    if let Some(about) = about {
        tail.push(Span::styled(about, Tone::Dim.style(theme)));
    }
    let tail_w = kit::spans_width(&tail);
    if kit::spans_width(&line) + 1 + tail_w > w {
        lines.push(BoardLine {
            spans: std::mem::take(&mut line),
            row: None,
            parts: std::mem::take(&mut hits),
        });
        line = vec![Span::raw("  ")];
    } else {
        line.push(Span::raw(" "));
    }
    line.extend(tail);
    lines.push(BoardLine {
        spans: line,
        row: None,
        parts: hits,
    });
    lines
}

/// A setting's lines: its label bold, today's count, its switch (or its
/// value's words) and a "changed" chip; under it its description cut, or,
/// chosen, whole, with today's count by tab and its default.
#[allow(clippy::too_many_arguments)]
fn setting_lines(
    item: &Item,
    row: usize,
    chosen: bool,
    label_w: usize,
    slots: &[usize],
    w: usize,
    m: &Mission,
) -> Vec<BoardLine> {
    let theme = &m.snap.theme;
    let d = &item.data;
    let kind_row = d.get("today").is_some();
    let all = parts(item);
    let switch = switched(item);
    let slot_w: usize = slots.iter().sum::<usize>() + 2;
    let cols = row_columns(label_w, slot_w, w.saturating_sub(1));
    let lit_row = !chosen && m.hover == Some(Spot::Index(row as isize));
    let mut line: Vec<Span<'static>> = vec![Span::raw(" ")];
    let mut part_hits = Vec::new();
    if let Some((x, cw)) = cols[0] {
        let label = cut(&item.title, cw.min(LABEL_MAX));
        put_at(
            &mut line,
            vec![Span::styled(
                label,
                if chosen {
                    Tone::Text.bold(theme).fg(theme.bright)
                } else {
                    Tone::Text.bold(theme)
                },
            )],
            x + 1,
        );
    }
    if kind_row && let Some((x, cw)) = cols[1] {
        let n = d.get("today").and_then(Value::as_u64).unwrap_or(0);
        let text = n.to_string();
        let tone = if n > 0 { Tone::Text } else { Tone::Faint };
        put_at(
            &mut line,
            vec![Span::styled(text.clone(), tone.style(theme))],
            x + 1 + cw.saturating_sub(width(&text) + 1),
        );
    }
    if let Some((x, cw)) = cols[2] {
        // A switch, or a wider one's own width ending where the others end.
        if switch {
            let shorts: Vec<String> = all.iter().map(|p| p.2.clone()).collect();
            let shorts: Vec<&str> = shorts.iter().map(String::as_str).collect();
            let own_slots: Vec<usize> = if kind_row {
                slots.to_vec()
            } else {
                kit::switch_slots([shorts.as_slice()])
            };
            let own_w: usize = own_slots.iter().sum::<usize>() + 2;
            let chosen_part = all.iter().position(|p| p.0 == d["raw"]);
            let lit_part = (0..all.len()).find(|&n| m.hover == Some(Spot::Part(row as isize, n)));
            let (spans, places) = kit::choice_switch(
                &shorts,
                chosen_part,
                lit_part,
                &own_slots,
                m.snap.caps,
                theme,
            );
            let sx = (x + 1 + cw).saturating_sub(own_w);
            put_at(&mut line, spans, sx);
            // `kit::row` puts its bar before the line, a column.
            for (n, (px, pw)) in places.into_iter().enumerate() {
                part_hits.push((1 + sx + px, pw, Spot::Part(row as isize, n)));
            }
        } else {
            let words = d.get("words").and_then(Value::as_str).unwrap_or("");
            let words = cut(words, cw);
            put_at(
                &mut line,
                vec![Span::styled(words.clone(), Tone::Accent.style(theme))],
                x + 1 + cw.saturating_sub(width(&words)),
            );
        }
    }
    if d["changed"] == true
        && let Some((x, _)) = cols[3]
    {
        put_at(
            &mut line,
            kit::chip("changed", Tone::Needs, m.snap.caps, theme),
            x + 1,
        );
    }
    // The switch's parts keep their own fills on the chosen or lit row.
    let fills: Vec<Option<ratatui::style::Color>> = line.iter().map(|s| s.style.bg).collect();
    let first = kit::row(line, w, chosen, theme);
    let mut first = if lit_row {
        lit_line(first, theme)
    } else {
        first
    };
    // `kit::row` puts its bar first, a span.
    for (i, fill) in fills.into_iter().enumerate() {
        if let (Some(fill), Some(span)) = (fill, first.get_mut(i + 1)) {
            span.style = span.style.bg(fill);
        }
    }
    let mut out = vec![BoardLine {
        spans: first,
        row: Some(row),
        parts: part_hits,
    }];
    let words_w = w.saturating_sub(4).min(TEXT_MAX);
    let under = |spans: Vec<Span<'static>>| {
        let mut line = vec![Span::raw("   ")];
        line.extend(spans);
        kit::row(line, w, chosen, theme)
    };
    if chosen {
        for text in wrap(&item.detail, words_w) {
            out.push(BoardLine {
                spans: under(vec![Span::styled(text, Tone::Dim.style(theme))]),
                row: Some(row),
                parts: Vec::new(),
            });
        }
        if kind_row {
            let n = d.get("today").and_then(Value::as_u64).unwrap_or(0);
            let by = d.get("by_tab").and_then(Value::as_str).unwrap_or("");
            let text = if n == 0 {
                "None sent today".to_string()
            } else {
                format!("Today {n}: {by}")
            };
            out.push(BoardLine {
                spans: under(vec![Span::styled(
                    cut(&text, words_w),
                    Tone::Dim.style(theme),
                )]),
                row: Some(row),
                parts: Vec::new(),
            });
        }
        let default = d.get("default").and_then(Value::as_str).unwrap_or("");
        out.push(BoardLine {
            spans: under(vec![
                Span::styled("default ", Tone::Dim.style(theme)),
                Span::styled(default.to_string(), Tone::Text.style(theme)),
                Span::styled(
                    "  ·  Enter: its choices; a click on the switch sets one",
                    Tone::Dim.style(theme),
                ),
            ]),
            row: Some(row),
            parts: Vec::new(),
        });
    } else {
        let spans = under(vec![Span::styled(
            cut(&item.detail, words_w),
            Tone::Faint.style(theme),
        )]);
        out.push(BoardLine {
            spans: if lit_row {
                lit_line(spans, theme)
            } else {
                spans
            },
            row: Some(row),
            parts: Vec::new(),
        });
    }
    out
}

/// The board's lines from the row it starts at, and where a click on a
/// row, a preset or a switch's part lands.
pub fn board(
    view: &View,
    m: &Mission,
    cols: usize,
    rows: usize,
    lines: &mut Vec<Line>,
    hits: &mut Vec<HitBox>,
) {
    let theme = &m.snap.theme;
    let items = &view.data.items;
    let top = lines.len();
    let body = rows.saturating_sub(1 + top);
    let cursor = usize::try_from(m.cursor).ok().filter(|&c| c < items.len());
    let w = cols;
    // The kinds' switches share their slots, and their labels one column.
    let kind_parts: Vec<Vec<String>> = items
        .iter()
        .filter(|i| i.data.get("today").is_some())
        .map(|i| parts(i).into_iter().map(|p| p.2).collect())
        .collect();
    let kind_refs: Vec<Vec<&str>> = kind_parts
        .iter()
        .map(|p| p.iter().map(String::as_str).collect())
        .collect();
    let slots = kit::switch_slots(kind_refs.iter().map(Vec::as_slice));
    let label_w = kit::natural(
        items
            .iter()
            .filter(|i| setting_of(i).is_some())
            .map(|i| i.title.as_str()),
    );

    let mut out: Vec<BoardLine> = Vec::new();
    let mut presets_done = false;
    for (i, item) in items.iter().enumerate() {
        if item.id == STATUS {
            let lit = cursor != Some(i) && m.hover == Some(Spot::Index(i as isize));
            out.push(BoardLine {
                spans: status_line(item, w, cursor == Some(i), lit, m),
                row: Some(i),
                parts: Vec::new(),
            });
            continue;
        }
        if views::is_header(item) {
            out.push(BoardLine::plain(vec![Span::raw(" ".repeat(w))]));
            let today = item.id == "header:kinds";
            let mut label = kit::label(&item.title, Tone::Dim, None, w, theme);
            if today {
                let cols = row_columns(
                    label_w,
                    slots.iter().sum::<usize>() + 2,
                    w.saturating_sub(1),
                );
                if let Some((x, cw)) = cols[1] {
                    // Its words alone, so nothing is cut short before "today".
                    let mut words = vec![
                        Span::raw(" "),
                        Span::styled(item.title.to_uppercase(), Tone::Dim.bold(theme)),
                    ];
                    put_at(
                        &mut words,
                        vec![Span::styled("today", Tone::Faint.style(theme))],
                        x + 1 + cw.saturating_sub(5),
                    );
                    label = kit::fit(words, w, Style::new());
                }
            }
            out.push(BoardLine::plain(label));
            continue;
        }
        if preset_of(item).is_some() {
            if !presets_done {
                presets_done = true;
                out.extend(preset_lines(items, cursor, w, m));
            }
            continue;
        }
        if setting_of(item).is_some() {
            out.extend(setting_lines(
                item,
                i,
                cursor == Some(i),
                label_w,
                &slots,
                w,
                m,
            ));
        }
    }
    out.push(BoardLine::plain(vec![kit::rule(w, theme)]));
    out.push(BoardLine::plain(vec![
        Span::raw(" "),
        Span::styled("!", Tone::Failed.bold(theme)),
        Span::styled(
            "  Offline and crash messages always notify, and an answer to what you sent a tab from Slack always reaches you.",
            Tone::Dim.style(theme),
        ),
    ]));

    // Scrolled so the chosen row, or the presets it is on, shows whole.
    let chosen_end = cursor
        .and_then(|c| {
            if preset_of(&items[c]).is_some() {
                out.iter()
                    .rposition(|l| l.parts.iter().any(|p| p.2 == Spot::Index(c as isize)))
            } else {
                out.iter().rposition(|l| l.row == Some(c))
            }
        })
        .map_or(0, |e| e + 1);
    let skip = chosen_end.saturating_sub(body);
    for (n, line) in out.into_iter().skip(skip).take(body).enumerate() {
        let y = (top + n) as u16;
        // A switch's parts and a preset's chip first, so they win the
        // click over the row's box under them.
        for (x, pw, spot) in &line.parts {
            hits.push(HitBox {
                y,
                x0: *x as u16,
                x1: (x + pw).saturating_sub(1) as u16,
                spot: *spot,
            });
        }
        if let Some(r) = line.row {
            hits.push(HitBox {
                y,
                x0: 0,
                x1: cols.saturating_sub(1) as u16,
                spot: Spot::Index(r as isize),
            });
        }
        lines.push(Line {
            spans: kit::fit(line.spans, cols, Style::new()),
            fill: None,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::relay::sending::Send;

    fn sent(kind: Kind, tab: i64, pilot: bool, notified: bool) -> Sent {
        Sent {
            kind,
            tab: Some((tab, format!("t{tab}"))),
            autopilot: pilot,
            notified,
        }
    }

    fn view(items: Vec<Item>) -> View {
        View {
            id: views::ViewId::Relay,
            title: "Slack relay".into(),
            data: crate::items::ViewData {
                items,
                ..Default::default()
            },
            loaded: true,
        }
    }

    fn schema_with(pairs: &[(&str, &str)]) -> Vec<settings::Setting> {
        settings::schema()
            .into_iter()
            .map(|mut o| {
                if let Some((_, v)) = pairs.iter().find(|(k, _)| *k == o.key) {
                    o.value = Value::String(v.to_string());
                }
                o
            })
            .collect()
    }

    #[test]
    fn the_rows_carry_todays_counts_by_tab_and_the_preset_matched() {
        let log = vec![
            sent(Kind::Finished, 5, true, false),
            sent(Kind::Finished, 5, true, false),
            sent(Kind::Finished, 3, false, false),
            sent(Kind::Prompts, 3, false, false),
            sent(Kind::Prompts, 3, false, true),
        ];
        let pairs: Vec<(String, &str)> = Preset::Quiet.values();
        let pairs: Vec<(&str, &str)> = pairs.iter().map(|(k, v)| (k.as_str(), *v)).collect();
        let items = items_of(&schema_with(&pairs), &log, None);
        let finished = items
            .iter()
            .find(|i| setting_of(i) == Some("relay_send_finished"))
            .unwrap();
        assert_eq!(finished.data["today"], 3);
        // What the plain list shows beside each row.
        assert_eq!(finished.data["value"], "Thread only, 3 today");
        assert_eq!(finished.data["by_tab"], "tab 5 2 (autopilot) · tab 3 1");
        assert_eq!(finished.data["changed"], false);
        let quiet = items
            .iter()
            .find(|i| preset_of(i) == Some(Preset::Quiet))
            .unwrap();
        assert_eq!(quiet.data["lit"], true);
        assert_eq!(items[0].data["sent"], 4);
        assert_eq!(items[0].data["notified"], 1);
        assert_eq!(
            items[0].data["value"],
            "4 sent by itself, 1 notified you today"
        );
        // A prompt's switch has no Off.
        let prompts = items
            .iter()
            .find(|i| setting_of(i) == Some("relay_send_prompts"))
            .unwrap();
        assert_eq!(parts(prompts).len(), 2);
        let (key, value, label) = part_value(finished, 2).unwrap();
        assert_eq!(
            (key.as_str(), value, label.as_str()),
            (
                "relay_send_finished",
                Value::String(Send::Off.value().into()),
                "Don't send"
            )
        );
    }

    fn drawn(items: Vec<Item>, cols: usize, cursor: isize) -> (Vec<String>, Vec<HitBox>) {
        let mut m = Mission::new();
        m.snap.rules.ux2 = true;
        m.cursor = cursor;
        let (mut lines, mut hits) = (Vec::new(), Vec::new());
        board(&view(items), &m, cols, 60, &mut lines, &mut hits);
        let text = lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.to_string()).collect())
            .collect();
        (text, hits)
    }

    #[test]
    fn the_switches_line_up_and_the_changed_chips_keep_the_right_edge() {
        let items = items_of(
            &schema_with(&[
                ("relay_send_finished", "off"),
                ("relay_send_stuck", "thread"),
            ]),
            &[],
            Some(&Running {
                tab: 7,
                age: Some("1h12m".into()),
            }),
        );
        for cols in [120, 160, 240] {
            let (text, hits) = drawn(items.clone(), cols, 0);
            assert!(text[0].contains("Running") && text[0].contains("in tab 7, for 1h12m"));
            // Every kind's switch starts at the same column, a two-part one
            // as wide as the rest.
            let ends: Vec<u16> = (0..items.len())
                .filter(|&i| items[i].data.get("today").is_some())
                .filter_map(|i| {
                    hits.iter()
                        .filter(|h| matches!(h.spot, Spot::Part(r, _) if r == i as isize))
                        .map(|h| h.x0)
                        .min()
                })
                .collect();
            assert_eq!(ends.len(), Kind::ALL.len(), "{cols}");
            assert!(ends.windows(2).all(|p| p[0] == p[1]), "{cols}: {ends:?}");
            // A part's click lands on its words as drawn.
            let finished = items
                .iter()
                .position(|i| setting_of(i) == Some("relay_send_finished"))
                .unwrap();
            for (n, word) in ["Notify", "Thread", "Off"].iter().enumerate() {
                let hit = hits
                    .iter()
                    .find(|h| h.spot == Spot::Part(finished as isize, n))
                    .unwrap();
                let line: Vec<char> = text[hit.y as usize].chars().collect();
                let under: String = line[hit.x0 as usize..=hit.x1 as usize].iter().collect();
                assert_eq!(under.trim(), *word, "{cols}");
            }
            // The changed chips end at the right edge.
            for line in text.iter().filter(|l| l.contains("changed")) {
                assert!(line.trim_end().ends_with("changed"), "{cols}: {line}");
                assert!(width(line.trim_end()) >= cols - CHANGED_W, "{cols}: {line}");
            }
        }
    }

    #[test]
    fn a_narrow_board_drops_the_chips_before_cutting_the_labels() {
        let label_w = 41;
        let slots = 23;
        let wide = row_columns(label_w, slots, 119);
        assert!(wide[3].is_some());
        let narrow = row_columns(label_w, slots, 75);
        assert!(narrow[3].is_none());
        assert!(narrow[0].unwrap().1 >= LABEL_MAX);
        let narrower = row_columns(label_w, slots, 60);
        assert!(narrower[3].is_none());
        assert!(narrower[0].unwrap().1 >= LABEL_MIN);
    }

    #[test]
    fn the_chosen_kind_opens_to_its_counts_and_default() {
        let items = items_of(
            &settings::schema(),
            &[sent(Kind::Finished, 5, true, false)],
            None,
        );
        let at = items
            .iter()
            .position(|i| setting_of(i) == Some("relay_send_finished"))
            .unwrap();
        let (text, _) = drawn(items.clone(), 160, at as isize);
        let all = text.join("\n");
        assert!(all.contains("Today 1: tab 5 1 (autopilot)"), "{all}");
        assert!(all.contains("default Thread only"), "{all}");
        assert!(all.contains("Not running"));
        let mut unset = items_of(&settings::schema(), &[], None);
        not_set_up(&mut unset);
        assert_eq!(unset[0].title, "Not set up");
        assert!(unset[0].detail.contains("slack-relay.json"));
    }

    #[test]
    fn a_chosen_row_keeps_its_switchs_lit_part() {
        let items = items_of(&settings::schema(), &[], None);
        let at = items
            .iter()
            .position(|i| setting_of(i) == Some("relay_send_finished"))
            .unwrap();
        let mut m = Mission::new();
        m.snap.rules.ux2 = true;
        m.cursor = at as isize;
        let (mut lines, mut hits) = (Vec::new(), Vec::new());
        board(&view(items), &m, 160, 60, &mut lines, &mut hits);
        let row = lines
            .iter()
            .find(|l| l.spans.iter().any(|s| s.content.contains("Finished turns")))
            .unwrap();
        let chosen = kit::Surface::Chosen.color(&m.snap.theme);
        let thread = row
            .spans
            .iter()
            .find(|s| s.content.trim() == "Thread")
            .unwrap();
        assert_ne!(thread.style.bg, Some(chosen), "{:?}", thread.style);
    }
}
