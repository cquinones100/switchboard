//! With UX 2.0, the Keys view as a board (board 27 of the canvas, "Keys
//! with a heatmap", new with the port; the tmux version drew the keys only
//! as the list `list` still draws below `BOARD_AT` columns): a panel of the
//! groups at the left (the workspace's tags, then each app's own keys, then
//! the commands reached only through M-p), the group in front on the
//! highlight; at the top right the keyboard as a heatmap of the Alt keys
//! pressed this week with the group's keys outlined, and beside it the
//! chosen key's keycap, what it does and how often it was pressed; under a
//! rule, the group's keys as a table of the key, what it does, its tags and
//! whether it is the default or yours.
//!
//! Everything comes from the view's rows and head as `keymap` built them
//! from what the server holds (the bindings and the habits' counts it read
//! off its task), so drawing reads nothing. The keyboard is an image atom
//! (`render::Image`), drawn as text where the terminal takes no images.
//! The cursor stays the view's own, on a key's row: the group in front is
//! the chosen row's, so the filter and Enter work on the board as on the
//! list.

use ratatui::style::Style;
use ratatui::text::Span;

use super::keymap::{self, APPS, THROUGH, WORKSPACE};
use super::render::{Image, Shape};
use super::views::{View, is_header};
use super::{HitBox, Mission, Spot};
use crate::items::Item;
use crate::sidebar::render::Line;
use crate::sidebar::text::{cut, width, wrap};
use crate::ui::Theme;
use crate::ui::images::{self, Heat, Outlined};
use crate::ui::kit::{self, Surface, Tone};

/// The board needs this many columns (the panel at its narrowest and the
/// keyboard); under it `list` draws the list.
pub const BOARD_AT: usize = 100;
/// The groups' panel takes its widest name and count, between these.
const PANEL_MIN: usize = 18;
const PANEL_CAP: usize = 28;
/// The table's key and tags columns take their widest, up to these; what
/// a key does stretches, never under `WHAT_MIN`.
const KEY_CAP: usize = 18;
const TAGS_CAP: usize = 32;
const WHAT_MIN: usize = 24;
/// "default", or the "yours" chip.
const FROM_W: usize = 8;
/// The chosen key beside the keyboard: never under `BESIDE_MIN`, its
/// wrapped text never over `TEXT_CAP`.
const BESIDE_MIN: usize = 34;
const TEXT_CAP: usize = 100;
/// Description lines beside the keyboard.
const ABOUT_LINES: usize = 4;
const HABITS_OFF: &str = "Turn on Suggestions from how you work (habits) to see how often you press each key; it counts on this computer only.";

/// The board's widths at the screen's own width (board 45's rules: fixed
/// parts at their natural width up to a cap, the rest stretching). As it
/// narrows, the least important part gives way first, in this order: the
/// table's tags column (once what a key does would be under `WHAT_MIN`),
/// then the chosen key beside the keyboard (once it would be under
/// `BESIDE_MIN`), whose key and words stay on its row in the table.
struct Layout {
    panel_w: usize,
    right_at: usize,
    right_w: usize,
    /// Where the chosen key starts beside the keyboard, and how wide it is.
    beside: Option<(usize, usize)>,
    /// Whether the table has its tags column: it gives way at least as
    /// soon as the chosen key beside does, being the less important.
    tags: bool,
}

impl Layout {
    fn at(cols: usize, groups: &[Group]) -> Layout {
        let natural = groups
            .iter()
            .map(|g| width(&g.title) + width(&g.rows.len().to_string()) + 5)
            .chain([width("THROUGH M-P ONLY") + 2])
            .max()
            .unwrap_or(PANEL_MIN);
        let panel_w = natural.clamp(PANEL_MIN, PANEL_CAP);
        let right_at = panel_w + 2;
        let right_w = cols.saturating_sub(right_at + 1);
        let (kb_w, _) = images::keyboard_size();
        let beside_at = kb_w as usize + 3;
        let beside_w = right_w.saturating_sub(beside_at);
        Layout {
            panel_w,
            right_at,
            right_w,
            beside: (beside_w >= BESIDE_MIN).then_some((beside_at, beside_w.min(TEXT_CAP))),
            tags: beside_w >= BESIDE_MIN + 4,
        }
    }
}

/// A group of the view: its heading's place, its title, which part of
/// the list it is in, and its rows' places.
pub struct Group {
    pub at: usize,
    pub title: String,
    pub section: String,
    pub rows: Vec<usize>,
}

/// The view's groups in order, each with its rows.
pub fn groups(items: &[Item]) -> Vec<Group> {
    let mut out: Vec<Group> = Vec::new();
    for (at, item) in items.iter().enumerate() {
        if is_header(item) {
            out.push(Group {
                at,
                title: item.title.clone(),
                section: item.data["section"].as_str().unwrap_or(WORKSPACE).into(),
                rows: Vec::new(),
            });
        } else if let Some(g) = out.last_mut() {
            g.rows.push(at);
        }
    }
    out.retain(|g| !g.rows.is_empty());
    out
}

/// Whether the panel of groups fits in `rows` (mission control's whole
/// height): it doesn't scroll, so where it would be cut the list is drawn
/// instead, which reaches every group (its bar of two rows, a line of it,
/// and the footer around it).
pub fn fits(items: &[Item], rows: usize) -> bool {
    let groups = groups(items);
    let sections = [WORKSPACE, APPS, THROUGH]
        .iter()
        .filter(|s| groups.iter().any(|g| g.section == **s))
        .count();
    let panel = groups.len() + sections * 2 - sections.min(1);
    panel + 4 <= rows
}

/// The row the cursor is on, or the first row after a heading it is on.
pub fn chosen_row(items: &[Item], cursor: isize) -> Option<usize> {
    let from = usize::try_from(cursor).ok()?;
    (from..items.len()).find(|&i| !is_header(&items[i]))
}

/// The place of the group a row is in.
fn group_of(groups: &[Group], row: usize) -> Option<usize> {
    groups.iter().position(|g| g.rows.contains(&row))
}

/// Where `h` and `l` (`step` -1 or 1) take the cursor: the first row of
/// the group before or after the chosen one's, staying put at either end.
pub fn group_step(items: &[Item], cursor: isize, step: isize) -> Option<usize> {
    let groups = groups(items);
    let now = chosen_row(items, cursor).and_then(|r| group_of(&groups, r))?;
    let to = now.checked_add_signed(step)?;
    groups.get(to).map(|g| g.rows[0])
}

/// Where `j` and `k` take the cursor: the next or previous row of the
/// chosen one's group, staying put at its ends.
pub fn row_step(items: &[Item], cursor: isize, step: isize) -> Option<usize> {
    let groups = groups(items);
    let row = chosen_row(items, cursor)?;
    let g = &groups[group_of(&groups, row)?];
    let at = g.rows.iter().position(|&r| r == row)?;
    let to = at.checked_add_signed(step)?;
    g.rows.get(to).copied()
}

/// The letters of the Alt keys a row is bound to.
fn letters(item: &Item) -> impl Iterator<Item = char> + '_ {
    item.data["keys"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|k| k.as_str().and_then(keymap::alt_letter))
}

/// How a group is named in the caption: a tag as `#tabs`.
fn group_name(g: &Group) -> String {
    if g.section == WORKSPACE {
        format!("#{}", g.title)
    } else {
        g.title.clone()
    }
}

/// A line of the panel or the right side: its spans, and what a click on
/// it chooses.
type Part = (Vec<Span<'static>>, Option<Spot>);

/// The panel of groups, `panel_w` wide: a label over each part of the
/// list, each group with its count, the one in front chosen.
fn panel(
    groups: &[Group],
    front: Option<usize>,
    panel_w: usize,
    m: &Mission,
    theme: &Theme,
) -> Vec<Part> {
    let mut out: Vec<Part> = Vec::new();
    for (section, label) in [
        (WORKSPACE, "Workspace"),
        (APPS, "Each app's own"),
        (THROUGH, "Through M-p only"),
    ] {
        let here: Vec<(usize, &Group)> = groups
            .iter()
            .enumerate()
            .filter(|(_, g)| g.section == section)
            .collect();
        if here.is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push((Vec::new(), None));
        }
        out.push((kit::label(label, Tone::Dim, None, panel_w, theme), None));
        for (n, g) in here {
            let chosen = front == Some(n);
            let spot = Spot::Choose(g.rows[0] as isize);
            let name = if section == WORKSPACE {
                format!("#{}", g.title)
            } else {
                g.title.clone()
            };
            let count = g.rows.len().to_string();
            let room = panel_w.saturating_sub(width(&count) + 4);
            let name = cut(&name, room);
            let gap = panel_w.saturating_sub(width(&name) + width(&count) + 3);
            let style = if chosen {
                Tone::Text.bold(theme)
            } else {
                Tone::Text.style(theme)
            };
            let spans = vec![
                Span::raw(" "),
                Span::styled(name, style),
                Span::raw(" ".repeat(gap.max(1))),
                Span::styled(count, Tone::Dim.style(theme)),
            ];
            let spans = kit::row(spans, panel_w, chosen, theme);
            out.push((
                lit(spans, !chosen && m.hover == Some(spot), theme),
                Some(spot),
            ));
        }
    }
    out
}

/// A row on the panel surface while the mouse is over it.
fn lit(spans: Vec<Span<'static>>, on: bool, theme: &Theme) -> Vec<Span<'static>> {
    if !on {
        return spans;
    }
    let bg = Surface::Panel.color(theme);
    spans
        .into_iter()
        .map(|s| Span::styled(s.content, s.style.bg(bg)))
        .collect()
}

/// The chosen key beside the keyboard, `cols` wide: its keycap, what it
/// does at length, how often it was pressed this week and whether it is
/// the default or yours.
fn detail(item: &Item, most: u64, habits: bool, cols: usize, m: &Mission) -> Vec<Part> {
    let theme = &m.snap.theme;
    let key = keymap::key_of(item).unwrap_or("");
    let mut out: Vec<Part> = vec![(
        kit::keycap(key, &item.title, true, m.snap.caps, theme),
        None,
    )];
    out.push((Vec::new(), None));
    let about = keymap::action_of(item)
        .map(|a| a.detail())
        .filter(|d| !d.is_empty());
    let words = match (&about, keymap::app_of(item)) {
        (Some(d), _) => d.clone(),
        (None, Some(app)) => format!("{app}'s own key: only {app} takes it."),
        (None, None) => item.detail.clone(),
    };
    for w in wrap(&words, cols).into_iter().take(ABOUT_LINES) {
        out.push((vec![Span::styled(w, Tone::Dim.style(theme))], None));
    }
    out.push((Vec::new(), None));
    let mut last = Vec::new();
    if keymap::yours_of(item) {
        last.extend(kit::chip("yours", Tone::Needs, m.snap.caps, theme));
    } else {
        last.push(Span::styled("default  ", Tone::Dim.style(theme)));
    }
    let count = keymap::count_of(item);
    if habits && keymap::counted(item) {
        let words = match count {
            0 => "not pressed this week".to_string(),
            n if n == most => format!("{n} this week · your most used"),
            n => format!("{n} this week"),
        };
        last.push(Span::styled(words, Tone::Text.style(theme)));
    }
    out.push((last, None));
    out
}

/// The table's head, its dashed rule, and a line per key of the group in
/// front, `cols` wide.
/// `tags` false once the screen is too narrow for the tags column
/// (`Layout`).
fn table(view: &View, g: &Group, chosen: usize, cols: usize, tags: bool, m: &Mission) -> Vec<Part> {
    let theme = &m.snap.theme;
    let items = &view.data.items;
    let key_w = g
        .rows
        .iter()
        .filter_map(|&r| keymap::key_of(&items[r]))
        .map(width)
        .max()
        .unwrap_or(3)
        .clamp(3, KEY_CAP);
    let tags_natural = g
        .rows
        .iter()
        .map(|&r| width(&items[r].detail))
        .chain([width("TAGS")])
        .max()
        .unwrap_or(0)
        .min(TAGS_CAP);
    let fixed = 1 + key_w + 2 + 2 + 2 + FROM_W;
    // The tags column gives way first as the screen narrows.
    let tags_w =
        (tags && cols.saturating_sub(fixed + tags_natural + 2) >= WHAT_MIN).then_some(tags_natural);
    let what_w = cols.saturating_sub(fixed + tags_w.map_or(0, |t| t + 2));
    let pad = |s: &str, w: usize| {
        let s = cut(s, w);
        format!("{s}{}", " ".repeat(w.saturating_sub(width(&s))))
    };
    let head = Tone::Dim.bold(theme);
    let mut out: Vec<Part> = vec![(
        vec![
            Span::raw(" "),
            Span::styled(pad("KEY", key_w), head),
            Span::raw("  "),
            Span::styled(pad("WHAT IT DOES", what_w), head),
            Span::raw("  "),
        ]
        .into_iter()
        .chain(tags_w.map(|t| Span::styled(pad("TAGS", t + 2), head)))
        .chain([Span::styled("FROM", head)])
        .collect(),
        None,
    )];
    out.push((vec![kit::head_rule(cols, theme)], None));
    for &r in &g.rows {
        let item = &items[r];
        let is_chosen = r == chosen;
        let spot = Spot::Index(r as isize);
        let mut spans = vec![
            Span::styled(
                pad(keymap::key_of(item).unwrap_or(""), key_w),
                Tone::Accent.bold(theme),
            ),
            Span::raw("  "),
            Span::styled(
                pad(&item.title, what_w),
                if is_chosen {
                    Tone::Text.bold(theme)
                } else {
                    Tone::Text.style(theme)
                },
            ),
            Span::raw("  "),
        ];
        if let Some(t) = tags_w {
            spans.push(Span::styled(
                pad(&item.detail, t + 2),
                Tone::Faint.style(theme),
            ));
        }
        if keymap::yours_of(item) {
            spans.extend(kit::chip("yours", Tone::Needs, m.snap.caps, theme));
        } else {
            spans.push(Span::styled("default", Tone::Dim.style(theme)));
        }
        let spans = kit::row(spans, cols, is_chosen, theme);
        out.push((
            lit(spans, !is_chosen && m.hover == Some(spot), theme),
            Some(spot),
        ));
    }
    out
}

/// The board, from the bar down to the footer: `lines` gets its lines,
/// `hits` where a click lands (a group, a key on the keyboard, a row), and
/// `images` the keyboard to place over them.
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
    let items = &view.data.items;
    let groups = groups(items);
    let chosen = chosen_row(items, m.cursor);
    let front = chosen.and_then(|r| group_of(&groups, r));
    let top = lines.len();
    let room = rows.saturating_sub(1 + top);
    let layout = Layout::at(cols, &groups);
    let Layout {
        panel_w,
        right_at,
        right_w,
        beside,
        ..
    } = layout;
    let (kb_w, kb_h) = images::keyboard_size();
    let (kb_w, kb_h) = (kb_w as usize, kb_h as usize);
    let habits = view.data.head["habits"].as_bool() == Some(true);
    let heat: Heat = view.data.head["heat"]
        .as_object()
        .map(|o| {
            o.iter()
                .filter_map(|(k, v)| Some((k.chars().next()?, v.as_u64()?)))
                .collect()
        })
        .unwrap_or_default();
    let outlined: Outlined = front
        .map(|g| {
            groups[g]
                .rows
                .iter()
                .flat_map(|&r| letters(&items[r]))
                .collect()
        })
        .unwrap_or_default();
    // The right side: the keyboard (or why it isn't counted) with the
    // chosen key beside it, then the group's table.
    let mut right: Vec<Part> = Vec::new();
    let (beside_at, beside_w) = beside.unwrap_or((kb_w, 0));
    // A heading's count is its rows', not presses.
    let most = items
        .iter()
        .filter(|i| !is_header(i))
        .map(keymap::count_of)
        .max()
        .unwrap_or(0);
    let beside = chosen
        .filter(|_| beside_w > 0)
        .map(|c| detail(&items[c], most, habits, beside_w, m))
        .unwrap_or_default();
    let mut left_of: Vec<Vec<Span<'static>>> = Vec::new();
    if habits {
        left_of.extend((0..kb_h).map(|_| vec![Span::raw(" ".repeat(kb_w))]));
    } else {
        for w in wrap(HABITS_OFF, kb_w).into_iter().take(kb_h) {
            left_of.push(vec![Span::styled(w, Tone::Dim.style(theme))]);
        }
        while left_of.len() < kb_h {
            left_of.push(Vec::new());
        }
    }
    for (i, left) in left_of.into_iter().enumerate() {
        let mut spans = kit::fit(left, beside_at, Style::new());
        if let Some((b, _)) = beside.get(i) {
            spans.extend(kit::fit(b.clone(), beside_w, Style::new()));
        }
        right.push((spans, None));
    }
    let caption = match (habits, front) {
        (true, Some(g)) => format!(
            "Alt keys by how often you pressed them this week; the outlined ones are {}",
            group_name(&groups[g])
        ),
        (true, None) => "Alt keys by how often you pressed them this week".into(),
        (false, _) => String::new(),
    };
    right.push((vec![Span::styled(caption, Tone::Faint.style(theme))], None));
    right.push((vec![kit::rule(right_w, theme)], None));
    if let (Some(g), Some(c)) = (front, chosen) {
        let mut rows_of = table(view, &groups[g], c, right_w, layout.tags, m);
        // Scroll the table so the chosen row shows.
        let head = right.len() + 2;
        let fits = room.saturating_sub(head);
        let at = groups[g].rows.iter().position(|&r| r == c).unwrap_or(0);
        let skip = (at + 1).saturating_sub(fits);
        if skip > 0 {
            rows_of.drain(2..2 + skip.min(rows_of.len() - 2));
        }
        right.extend(rows_of);
    }
    let left = panel(&groups, front, panel_w, m, theme);
    for i in 0..room {
        let y = top + i;
        let (l, lspot) = left.get(i).cloned().unwrap_or_default();
        let (r, rspot) = right.get(i).cloned().unwrap_or_default();
        if let Some(spot) = lspot {
            hits.push(HitBox {
                y: y as u16,
                x0: 0,
                x1: (panel_w - 1) as u16,
                spot,
            });
        }
        if let Some(spot) = rspot {
            hits.push(HitBox {
                y: y as u16,
                x0: right_at as u16,
                x1: cols.saturating_sub(1) as u16,
                spot,
            });
        }
        let mut spans = kit::fit(l, panel_w, Style::new());
        spans.push(kit::rule_v(theme));
        spans.push(Span::raw(" "));
        spans.extend(kit::fit(r, right_w, Style::new()));
        lines.push(Line {
            spans: kit::fit(spans, cols, Style::new()),
            fill: None,
        });
    }
    if !habits || room < kb_h {
        return;
    }
    images.push(Image {
        name: "keys/keyboard".into(),
        x: right_at as u16,
        y: top as u16,
        width: kb_w as u16,
        height: kb_h as u16,
        shape: Shape::Keyboard { heat, outlined },
    });
    // A click on a key chooses its row: the group in front's first, else
    // the first anywhere.
    for y in 0..kb_h as u16 {
        for x in 0..kb_w as u16 {
            let Some(c) = images::key_at(x, y) else {
                continue;
            };
            let in_front = front
                .and_then(|g| {
                    groups[g]
                        .rows
                        .iter()
                        .find(|&&r| letters(&items[r]).any(|l| l == c))
                })
                .copied();
            let row = in_front.or_else(|| {
                (0..items.len())
                    .find(|&r| !is_header(&items[r]) && letters(&items[r]).any(|l| l == c))
            });
            if let Some(row) = row {
                let x = right_at as u16 + x;
                let y = (top as u16) + y;
                match hits.last_mut() {
                    Some(h)
                        if h.y == y && h.x1 + 1 == x && h.spot == Spot::Choose(row as isize) =>
                    {
                        h.x1 = x
                    }
                    _ => hits.push(HitBox {
                        y,
                        x0: x,
                        x1: x,
                        spot: Spot::Choose(row as isize),
                    }),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mission::keymap::{Shown, items};
    use crate::mux::keys::Bindings;
    use std::collections::HashMap;

    const ALL: Shown = Shown {
        github: true,
        linear: true,
        ask: true,
        easy: false,
    };

    #[test]
    fn h_and_l_go_between_groups_and_j_and_k_stay_in_one() {
        let items = items(&Bindings::default(), ALL, &HashMap::new());
        let groups = groups(&items);
        let first = groups[0].rows[0] as isize;
        let second = group_step(&items, first, 1).unwrap();
        assert_eq!(second, groups[1].rows[0]);
        assert_eq!(
            group_step(&items, second as isize, -1),
            Some(first as usize)
        );
        assert_eq!(group_step(&items, first, -1), None);
        let last_of_first = *groups[0].rows.last().unwrap();
        assert_eq!(row_step(&items, last_of_first as isize, 1), None);
        assert_eq!(row_step(&items, first, 1), Some(groups[0].rows[1]));
        // A cursor on a heading counts as on the row after it.
        assert_eq!(
            chosen_row(&items, groups[1].at as isize),
            Some(groups[1].rows[0])
        );
    }
}
