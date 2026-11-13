//! UX 2.0's Settings view as a board, board 26 of the UX 2.0 canvas
//! ("Settings in two panes"): the categories and your own files in a panel
//! at the left, each category carrying a chip with how many of its options
//! differ from their defaults, and the category in front as a pane beside
//! it, an option a row with its label, a "changed" chip, its key at the
//! right and a switch (src/ui/images.rs) or its value's words; the chosen
//! option opens to its whole description and its default. New with UX
//! 2.0, with no tmux-version file behind it.
//!
//! It draws the view's own rows (`views::settings_items`), so the cursor,
//! the filter, Enter and Space work on them as on the plain list: the
//! category in front is the heading over the chosen row, h and l move the
//! cursor to the first option of the category before or after it, and a
//! filter lists the matching options of every category in the pane, the
//! panel dimming the categories with none. The panel lists every category
//! whatever the filter leaves, from the settings as last read and
//! `OWN_FILES`, which drawing reads without a file.

use ratatui::style::{Color, Style};
use ratatui::text::Span;
use serde_json::Value;

use super::render::{Image, Shape};
use super::views::{self, View};
use super::{HitBox, Mission, Pane, Spot};
use crate::items::Item;
use crate::settings;
use crate::sidebar::render::Line;
use crate::sidebar::text::{cut, width, wrap};
use crate::ui::Theme;
use crate::ui::images::SWITCH_COLS;
use crate::ui::kit::{self, Surface, Tone};
use crate::usage::own::OWN_FILES;

/// How wide the screen must be for the board; narrower, the plain list.
pub const BOARD_AT: usize = 136;
/// The categories' panel at most and at least: as wide as its longest
/// name, its chip and count need, between the two.
const SIDE_MAX: usize = 34;
const SIDE_MIN: usize = 24;
/// A wrapped description at most, however wide the pane.
const TEXT_MAX: usize = 100;

/// A category or file of the panel: the heading's id in the view, its
/// name, how many options it has and how many changed.
struct Entry {
    id: String,
    name: String,
    count: usize,
    changed: usize,
}

/// Every category of the settings, then every file of yours, in the order
/// the view lists them; a category's counts from its heading in the view
/// where it has one, which a filter keeps whole.
fn entries(items: &[Item], ux2: bool) -> (Vec<Entry>, Vec<Entry>) {
    let mut categories: Vec<Entry> = Vec::new();
    for o in settings::current_settings() {
        let id = format!("header:{}", o.category);
        let at = match categories.iter().position(|e| e.id == id) {
            Some(at) => at,
            None => {
                categories.push(Entry {
                    id,
                    name: settings::category_name(&o.category, ux2).to_string(),
                    count: 0,
                    changed: 0,
                });
                categories.len() - 1
            }
        };
        categories[at].count += 1;
        categories[at].changed += usize::from(o.changed());
    }
    let mut files: Vec<Entry> = Vec::new();
    for f in OWN_FILES {
        let id = format!("header:{}", f.category);
        match files.iter_mut().find(|e| e.id == id) {
            Some(e) => e.count += f.rows.len(),
            None => files.push(Entry {
                id,
                name: f.category.to_string(),
                count: f.rows.len(),
                changed: 0,
            }),
        }
    }
    // Each repository's landing comes before your files, as the view
    // lists it (`Server::mission_own`), so h and l go in the panel's order.
    files.insert(
        0,
        Entry {
            id: format!("header:{}", crate::usage::own::LANDING),
            name: crate::usage::own::LANDING.to_string(),
            count: 0,
            changed: 0,
        },
    );
    for e in categories.iter_mut().chain(files.iter_mut()) {
        if let Some(head) = items.iter().find(|i| i.id == e.id) {
            let n = |k: &str| head.data.get(k).and_then(Value::as_u64).map(|n| n as usize);
            e.count = n("count").unwrap_or(e.count);
            e.changed = n("changed").unwrap_or(e.changed);
        }
    }
    (categories, files)
}

/// The heading over row `at`, by its place.
pub fn heading_of(items: &[Item], at: usize) -> Option<usize> {
    let last = items.len().checked_sub(1)?;
    (0..=at.min(last))
        .rev()
        .find(|&i| views::is_header(&items[i]))
}

/// Where h (`by` -1) or l (1) takes the cursor: the first row of the
/// category before or after the one in front; where it is at either end.
pub fn side_step(items: &[Item], cursor: isize, by: isize) -> isize {
    let at = cursor.max(0) as usize;
    let heads: Vec<usize> = (0..items.len())
        .filter(|&i| views::is_header(&items[i]) && first_row(items, i).is_some())
        .collect();
    let now = heading_of(items, at).and_then(|h| heads.iter().position(|&x| x == h));
    let next = match now {
        Some(n) => n as isize + by,
        None => 0,
    };
    match usize::try_from(next).ok().and_then(|n| heads.get(n)) {
        Some(&h) => first_row(items, h).map_or(cursor, |r| r as isize),
        None => cursor,
    }
}

/// The first row under heading `at`.
fn first_row(items: &[Item], at: usize) -> Option<usize> {
    let next = at + 1;
    items
        .get(next)
        .filter(|i| !views::is_header(i))
        .map(|_| next)
}

/// What file `e` opens for the chosen row: a file of yours for one of its
/// settings (`repos.json` for a repository's landing), else the settings
/// file.
pub fn file_of(item: Option<&Item>) -> Option<String> {
    let data = &item?.data;
    if data.get("repo_setting").is_some() {
        return Some("repos.json".into());
    }
    data.pointer("/own/file")
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn on_surface(spans: Vec<Span<'static>>, bg: Color) -> Vec<Span<'static>> {
    spans
        .into_iter()
        .map(|s| {
            let style = s.style.bg(bg);
            Span::styled(s.content, style)
        })
        .collect()
}

fn blank(cols: usize) -> Vec<Span<'static>> {
    vec![Span::raw(" ".repeat(cols))]
}

/// A line of the pane: its spans, the row of the view it belongs to, and
/// where its switch goes (a boolean option's first line).
struct PaneLine {
    spans: Vec<Span<'static>>,
    row: Option<usize>,
    switch: Option<(String, bool)>,
}

impl PaneLine {
    fn plain(spans: Vec<Span<'static>>) -> PaneLine {
        PaneLine {
            spans,
            row: None,
            switch: None,
        }
    }
}

/// What Enter does to the chosen option, after its default.
fn enter_words(item: &Item) -> String {
    let d = &item.data;
    let words = |k: &str| d.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    if d.get("repo_setting").is_some() {
        return "in ~/.config/switchboard/repos.json · Enter changes it".into();
    }
    if let Some(file) = d.pointer("/own/file").and_then(Value::as_str) {
        let what = match d.pointer("/own/kind").and_then(Value::as_str) {
            Some("file") => "Enter opens it",
            _ => "Enter changes it",
        };
        return format!("in ~/.config/switchboard/{file} · {what}");
    }
    let (default, next) = (words("default"), words("next"));
    if d["choice"] == true {
        return format!("default {default} · Enter lists its choices");
    }
    if d["changed"] == true {
        format!("default {default} · Enter turns it back {next}")
    } else {
        format!("default {default} · Enter turns it {next}")
    }
}

/// An option's lines in the pane, `w` wide: its label bold, a "changed"
/// chip, its key dim and right-aligned before its switch (or its value's
/// words), its description clipped under it; chosen, the whole
/// description and what Enter does, on the highlight.
fn option_lines(item: &Item, row: usize, chosen: bool, w: usize, m: &Mission) -> Vec<PaneLine> {
    let theme = &m.snap.theme;
    let caps = m.snap.caps;
    let d = &item.data;
    let is_setting = d.get("setting").is_some();
    let boolean = is_setting && d["choice"] != true;
    let key = d
        .get("setting")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            let field = d.pointer("/repo_setting/field")?.as_str()?;
            Some(format!("repos.json {field}"))
        })
        .or_else(|| {
            let o = d.get("own")?;
            let file = o.get("file")?.as_str()?;
            Some(match o.get("key").and_then(Value::as_str) {
                Some(k) => format!("{file} {k}"),
                None => file.to_string(),
            })
        })
        .unwrap_or_default();
    let value = d.get("value").and_then(Value::as_str).unwrap_or("");
    // The switch, or the value's words, at the right, a space from the edge.
    let right_w = if boolean {
        SWITCH_COLS as usize
    } else {
        width(value).min(16)
    };
    let mut left: Vec<Span<'static>> = vec![
        Span::raw(" "),
        Span::styled(
            item.title.clone(),
            if chosen {
                Tone::Text.bold(theme).fg(theme.bright)
            } else {
                Tone::Text.bold(theme)
            },
        ),
        Span::raw(" "),
    ];
    if d["changed"] == true {
        left.extend(kit::chip("changed", Tone::Needs, caps, theme));
    }
    let used: usize = left.iter().map(|s| width(&s.content)).sum();
    // `row` puts its bar first, a column.
    let inner = w.saturating_sub(1);
    let key_room = inner.saturating_sub(used + right_w + 3);
    let key = cut(&key, key_room);
    let gap = inner.saturating_sub(used + width(&key) + 2 + right_w + 1);
    left.push(Span::raw(" ".repeat(gap)));
    left.push(Span::styled(key, Tone::Faint.style(theme)));
    left.push(Span::raw("  "));
    if boolean {
        left.push(Span::raw(" ".repeat(right_w)));
    } else {
        let tone = if item.state == crate::items::State::Active {
            Tone::Accent
        } else {
            Tone::Dim
        };
        left.push(Span::styled(cut(value, right_w), tone.style(theme)));
    }
    let lit = !chosen
        && (m.hover == Some(Spot::Index(row as isize))
            || m.hover == Some(Spot::Menu(row as isize)));
    // With the categories focused (`C-w h`), the chosen option rests.
    let resting = chosen && m.pane_now() == Pane::Panel;
    let line = |spans: Vec<Span<'static>>| {
        if resting {
            kit::row_resting(spans, w, theme)
        } else {
            kit::row(spans, w, chosen, theme)
        }
    };
    let first = line(left);
    let first = if lit {
        on_surface(first, Surface::Raised.color(theme))
    } else {
        first
    };
    let setting = d
        .get("setting")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let mut out = vec![PaneLine {
        spans: first,
        row: Some(row),
        switch: boolean.then(|| (setting, item.state == crate::items::State::Active)),
    }];
    let words_w = inner.saturating_sub(2).min(TEXT_MAX);
    let under = |text: String, tone: Tone| {
        line(vec![Span::raw(" "), Span::styled(text, tone.style(theme))])
    };
    if chosen {
        for line in wrap(&item.detail, words_w) {
            out.push(PaneLine {
                spans: under(line, Tone::Dim),
                row: Some(row),
                switch: None,
            });
        }
        out.push(PaneLine {
            spans: under(enter_words(item), Tone::Accent),
            row: Some(row),
            switch: None,
        });
    } else {
        let spans = under(cut(&item.detail, words_w), Tone::Faint);
        out.push(PaneLine {
            spans: if lit {
                on_surface(spans, Surface::Raised.color(theme))
            } else {
                spans
            },
            row: Some(row),
            switch: None,
        });
    }
    out
}

/// The pane's heading: the category in front with how many options it has
/// and how many changed, or with a filter how many matched where.
fn pane_head(
    view: &View,
    front: Option<usize>,
    searching: Option<&str>,
    w: usize,
    theme: &Theme,
) -> Vec<Vec<Span<'static>>> {
    let items = &view.data.items;
    if let Some(text) = searching {
        let found = items.iter().filter(|i| !views::is_header(i)).count();
        let places = items.iter().filter(|i| views::is_header(i)).count();
        let facts = format!(
            "{found} {} in {places} {}",
            if found == 1 { "option" } else { "options" },
            if places == 1 {
                "category"
            } else {
                "categories"
            }
        );
        return kit::pane(
            &format!("Matching “{text}”"),
            vec![Span::styled(facts, Tone::Dim.style(theme))],
            w,
            theme,
        );
    }
    let Some(head) = front.and_then(|h| items.get(h)) else {
        return kit::pane("Settings", Vec::new(), w, theme);
    };
    let count = head.data.get("count").and_then(Value::as_u64).unwrap_or(0);
    let changed = head.data.get("changed").and_then(Value::as_u64);
    let facts = match changed {
        Some(0) => format!("{count} options · none changed"),
        Some(n) => format!("{count} options · {n} changed from its default"),
        // A category of your files may hold several, so the chosen row
        // names its own file (`enter_words`).
        None => format!(
            "{count} {} in ~/.config/switchboard",
            if count == 1 { "setting" } else { "settings" }
        ),
    };
    kit::pane(
        &head.title,
        vec![Span::styled(facts, Tone::Dim.style(theme))],
        w,
        theme,
    )
}

/// The board's lines from the row it starts at, its switches, and where a
/// click on a category, an option or a switch lands.
pub fn board(
    view: &View,
    m: &Mission,
    cols: usize,
    rows: usize,
    lines: &mut Vec<Line>,
    hits: &mut Vec<HitBox>,
    images: &mut Vec<Image>,
) {
    let theme = &m.snap.theme;
    let items = &view.data.items;
    let top = lines.len();
    let body = rows.saturating_sub(1 + top);
    let cursor = usize::try_from(m.cursor).ok().filter(|&c| c < items.len());
    let front = cursor.and_then(|c| heading_of(items, c));
    let searching = m
        .filter
        .as_ref()
        .map(|f| f.text.trim())
        .filter(|t| !t.is_empty());
    // The panel: the categories, then your files.
    let (categories, files) = entries(items, m.snap.rules.ux2);
    let side_w = categories
        .iter()
        .chain(&files)
        .map(|e| {
            let chip = if e.changed > 0 {
                e.changed.to_string().len() + 3
            } else {
                0
            };
            width(&e.name) + chip + e.count.to_string().len() + 4
        })
        .max()
        .unwrap_or(SIDE_MIN)
        .clamp(SIDE_MIN, SIDE_MAX);
    let w = cols.saturating_sub(side_w + 1);
    let panel_bg = Surface::Panel.color(theme);
    let mut panel: Vec<(Vec<Span<'static>>, Option<usize>)> = Vec::new();
    let section = |title: &str, list: &[Entry], panel: &mut Vec<_>| {
        panel.push((
            on_surface(kit::label(title, Tone::Dim, None, side_w, theme), panel_bg),
            None,
        ));
        for e in list {
            let at = items.iter().position(|i| i.id == e.id);
            let first = at.and_then(|h| first_row(items, h));
            let chosen = at.is_some() && at == front;
            let tone = if first.is_none() {
                Tone::Faint
            } else {
                Tone::Text
            };
            let count = e.count.to_string();
            let mut chip = if e.changed > 0 {
                kit::chip(&e.changed.to_string(), Tone::Needs, m.snap.caps, theme)
            } else {
                Vec::new()
            };
            let chip_w: usize = chip.iter().map(|s| width(&s.content)).sum();
            let name_w = side_w.saturating_sub(4 + chip_w + width(&count));
            let name = cut(&e.name, name_w);
            let mut spans = vec![
                Span::raw(" "),
                Span::styled(
                    name.clone(),
                    if chosen {
                        Tone::Text.bold(theme).fg(theme.bright)
                    } else {
                        tone.style(theme)
                    },
                ),
            ];
            let gap = side_w.saturating_sub(1 + 1 + width(&name) + chip_w + width(&count) + 1);
            spans.push(Span::raw(" ".repeat(gap)));
            spans.append(&mut chip);
            spans.push(Span::styled(count, Tone::Faint.style(theme)));
            let lit =
                !chosen && first.is_some() && m.hover == first.map(|r| Spot::Category(r as isize));
            // The category in front is chosen while the panel has the
            // focus, and rests while its options have it.
            let row = if chosen && m.pane_now() != Pane::Panel {
                kit::row_resting(spans, side_w, theme)
            } else {
                kit::row(spans, side_w, chosen, theme)
            };
            let bg = if lit {
                Surface::Raised.color(theme)
            } else {
                panel_bg
            };
            panel.push((if chosen { row } else { on_surface(row, bg) }, first));
        }
    };
    section("Categories", &categories, &mut panel);
    panel.push((on_surface(blank(side_w), panel_bg), None));
    section("Your files", &files, &mut panel);

    // The pane: the category in front's options, or every match by category.
    let mut pane: Vec<PaneLine> = pane_head(view, front, searching, w.saturating_sub(1), theme)
        .into_iter()
        .map(|mut line| {
            line.insert(0, Span::raw(" "));
            PaneLine::plain(line)
        })
        .collect();
    let head_lines = pane.len();
    let mut body_lines: Vec<PaneLine> = Vec::new();
    let shown: Vec<usize> = match (searching, front) {
        (Some(_), _) => (0..items.len()).collect(),
        (None, Some(h)) => (h + 1..items.len())
            .take_while(|&i| !views::is_header(&items[i]))
            .collect(),
        (None, None) => Vec::new(),
    };
    for i in shown {
        let item = &items[i];
        if views::is_header(item) {
            if !body_lines.is_empty() {
                body_lines.push(PaneLine::plain(blank(w)));
            }
            body_lines.push(PaneLine::plain(kit::label(
                &item.title,
                Tone::Dim,
                None,
                w,
                theme,
            )));
            continue;
        }
        body_lines.extend(option_lines(item, i, cursor == Some(i), w, m));
    }
    // Scrolled so the chosen option shows whole.
    let room = body.saturating_sub(head_lines);
    let chosen_end = body_lines
        .iter()
        .rposition(|l| l.row.is_some() && l.row == cursor)
        .map_or(0, |e| e + 1);
    let skip = chosen_end.saturating_sub(room);
    pane.extend(body_lines.into_iter().skip(skip).take(room));

    // The panel scrolled, on a short screen, so the category in front shows.
    let front_first = front.and_then(|h| first_row(items, h));
    let panel_skip = panel
        .iter()
        .position(|(_, r)| r.is_some() && *r == front_first)
        .map_or(0, |at| (at + 1).saturating_sub(body));

    let rule = kit::rule_v(theme);
    let x_pane = side_w + 1;
    for n in 0..body {
        let y = (top + n) as u16;
        let (left, left_row) = panel
            .get(n + panel_skip)
            .cloned()
            .unwrap_or_else(|| (on_surface(blank(side_w), panel_bg), None));
        if let Some(r) = left_row {
            hits.push(HitBox {
                y,
                x0: 0,
                x1: (side_w - 1) as u16,
                spot: Spot::Category(r as isize),
            });
        }
        let mut spans = left;
        spans.push(rule.clone());
        match pane.get(n) {
            Some(line) => {
                spans.extend(line.spans.clone());
                if let Some(r) = line.row {
                    let switch_x = cols.saturating_sub(SWITCH_COLS as usize + 1);
                    let switch_at = line.switch.is_some().then_some(switch_x);
                    hits.push(HitBox {
                        y,
                        x0: x_pane as u16,
                        x1: switch_at.unwrap_or(cols).saturating_sub(1) as u16,
                        spot: Spot::Index(r as isize),
                    });
                    if let (Some((key, on)), Some(x)) = (&line.switch, switch_at) {
                        hits.push(HitBox {
                            y,
                            x0: x as u16,
                            x1: (x + SWITCH_COLS as usize - 1) as u16,
                            spot: Spot::Menu(r as isize),
                        });
                        images.push(Image {
                            name: format!("settings/switch/{key}"),
                            x: x as u16,
                            y,
                            width: SWITCH_COLS,
                            height: 1,
                            shape: Shape::Switch { on: *on },
                        });
                    }
                }
            }
            None => spans.extend(blank(w)),
        }
        lines.push(Line {
            spans: kit::fit(spans, cols, Style::new()),
            fill: None,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows() -> Vec<Item> {
        views::settings_items(&[], true)
    }

    #[test]
    fn h_and_l_move_to_the_first_option_of_the_category_beside() {
        let items = rows();
        let second = (2..items.len())
            .find(|&i| views::is_header(&items[i]))
            .unwrap();
        assert_eq!(side_step(&items, 1, 1), second as isize + 1);
        assert_eq!(side_step(&items, second as isize + 2, -1), 1);
        // At the first category, h stays.
        assert_eq!(side_step(&items, 2, -1), 2);
        // A search that matched nothing leaves nothing to move to.
        assert_eq!(side_step(&[], 0, 1), 0);
        assert_eq!(heading_of(&[], 0), None);
    }

    #[test]
    fn a_short_screen_scrolls_the_panel_to_the_category_in_front() {
        let own = crate::usage::own::own_items(std::path::Path::new("/nonexistent/own"));
        let items = views::settings_items(&own, true);
        let last = items.iter().rposition(|i| !views::is_header(i)).unwrap();
        let view = View {
            id: views::ViewId::Settings,
            title: "Settings".into(),
            data: crate::items::ViewData {
                items: items.clone(),
                ..Default::default()
            },
            loaded: true,
        };
        let mut m = Mission::new();
        m.snap.rules.ux2 = true;
        m.cursor = last as isize;
        let (mut lines, mut hits, mut images) = (Vec::new(), Vec::new(), Vec::new());
        board(&view, &m, 140, 16, &mut lines, &mut hits, &mut images);
        let head = items[heading_of(&items, last).unwrap()].title.clone();
        let shown: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.to_string()))
            .collect();
        assert!(shown.contains(&format!("  {}", &head[..8])), "{shown}");
    }

    #[test]
    fn enter_says_what_it_does_from_the_default() {
        let setting = |changed: bool, value: &str, default: &str, next: &str| {
            Item::new("setting:x", "X").data(serde_json::json!({
                "setting": "x", "value": value, "default": default,
                "changed": changed, "choice": false, "next": next,
            }))
        };
        assert_eq!(
            enter_words(&setting(true, "off", "on", "on")),
            "default on · Enter turns it back on"
        );
        assert_eq!(
            enter_words(&setting(false, "on", "on", "off")),
            "default on · Enter turns it off"
        );
        let landing = Item::new("repo:a/b:landing", "a/b")
            .data(serde_json::json!({ "repo_setting": { "repo": "a/b", "field": "landing" } }));
        assert_eq!(file_of(Some(&landing)).as_deref(), Some("repos.json"));
        assert!(enter_words(&landing).contains("repos.json"));
    }
}
