//! The hints in the spare columns beside a lone pane (UX 2.0 board 77):
//! with Settings' `lone_pane_width` capping a tab's one pane, the columns
//! at its right list what the pane can open, centered in them, each a row
//! with its key as the kit's keycap and what it does: the tab's note, a
//! split to the right, the PR, the ticket (in Claude's place with UX 2.0
//! on, as their keys show them), a link read beside the pane, and under
//! them how to widen the pane. New with the lone pane's cap,
//! with no tmux-version file behind it.
//!
//! Each key is the one bound now (keys.json over the defaults); a thing
//! no key is bound to (a default keys.json unbinds) shows its name alone,
//! from where the keys start, so no blank key column reads as a key missing. The PR's and the ticket's
//! rows show only when the tab has one (the ticket as M-i finds it) and its
//! service is on. A row does what its key does, and is a
//! click, lit under the mouse, through the header's keys
//! (`header::Button::Beside`), so it goes where they go.
//!
//! It is drawn from what the server already holds, as everything on the
//! drawing path is: no file is read and no process started for it.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};

use crate::mux::Direction;
use crate::mux::keys::{Action, Bindings};
use crate::ui::Theme;
use crate::ui::kit::{self, Surface, Tone};

/// What a row opens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Beside {
    Note,
    Split,
    Pr,
    Ticket,
    Link,
}

impl Beside {
    /// What a click on it does, and the action whose key it shows.
    pub fn action(self) -> Action {
        match self {
            Beside::Note => Action::Notes(crate::notes::serve::Command::Beside),
            Beside::Split => Action::Split(Direction::Right),
            Beside::Pr => Action::View(crate::views::Command::PrView),
            Beside::Ticket => Action::View(crate::views::Command::TicketView),
            Beside::Link => Action::View(crate::views::Command::ReadLink),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Beside::Note => "Open the tab's note",
            Beside::Split => "Split to the right",
            Beside::Pr => "The PR",
            Beside::Ticket => "The ticket",
            Beside::Link => "Read a link",
        }
    }
}

/// The rows a tab gets: the PR's and the ticket's only when it has one.
pub fn rows(pr: bool, ticket: bool) -> Vec<Beside> {
    [
        Beside::Note,
        Beside::Split,
        Beside::Pr,
        Beside::Ticket,
        Beside::Link,
    ]
    .into_iter()
    .filter(|b| match b {
        Beside::Pr => pr,
        Beside::Ticket => ticket,
        _ => true,
    })
    .collect()
}

/// The shortest key bound to an action whose kind `matches`, as the
/// palette shows it: a root key, else the prefix and a key after it;
/// none when no key is.
fn bound(bindings: &Bindings, matches: impl Fn(&Action) -> bool) -> Option<String> {
    let shortest = |keys: Vec<String>| {
        keys.into_iter()
            .min_by(|a, b| (a.len(), a).cmp(&(b.len(), b)))
    };
    let root = bindings
        .root
        .iter()
        .filter(|(_, a)| matches(a))
        .map(|(k, _)| crate::ui::palette::shown_key(&k.notation()))
        .collect();
    shortest(root).or_else(|| {
        let prefix = bindings.prefix_key.notation();
        let after = bindings
            .prefix
            .iter()
            .filter(|(_, a)| matches(a))
            .map(|(k, _)| crate::ui::palette::shown_key(&format!("{prefix} {}", k.notation())))
            .collect();
        shortest(after)
    })
}

/// What the hints say: each row with its key, and the last line.
pub fn lines(bindings: &Bindings, rows: &[Beside]) -> (Vec<(Beside, Option<String>)>, String) {
    let keyed = rows
        .iter()
        .map(|&b| {
            let action = b.action();
            (b, bound(bindings, |a| *a == action))
        })
        .collect();
    let widen = bound(bindings, |a| {
        matches!(a, Action::Resize(Direction::Right, _))
    });
    let last = match widen {
        Some(key) => format!("Drag the edge or {key} to widen"),
        None => "Drag the edge to widen".to_string(),
    };
    (keyed, last)
}

/// The columns before what a row does: its key's, at least.
const KEY_COLS: usize = 7;

/// Draws the hints centered in `spare`, the columns right of the pane's
/// rule; gives where each row went, for a click. Nothing when they don't
/// fit, with a column of margin a side.
pub fn draw(
    buf: &mut Buffer,
    spare: Rect,
    keyed: &[(Beside, Option<String>)],
    last: &str,
    hover: Option<Beside>,
    caps: bool,
    theme: &Theme,
) -> Vec<(Rect, Beside)> {
    let mut hits = Vec::new();
    let widest = keyed
        .iter()
        .map(|(beside, key)| {
            let label = crate::ui::draw::width(beside.label());
            key.as_deref().map_or(label, |k| {
                (kit::spans_width(&cap(k, caps, theme)) + 1).max(KEY_COLS) + label
            })
        })
        .max()
        .unwrap_or(0)
        .max(crate::ui::draw::width(last));
    // A row is lit from two columns before its key (the bar, a space) to
    // two after its widest neighbor, so the list needs four more and a
    // margin a side.
    let height = keyed.len() * 2 + 3;
    if keyed.is_empty() || (spare.width as usize) < widest + 6 || (spare.height as usize) < height {
        return hits;
    }
    let sunken = Surface::Sunken.color(theme);
    let on_sunken = |spans: Vec<Span<'static>>| -> Line<'static> {
        Line::from(
            spans
                .into_iter()
                .map(|s| {
                    let style = if s.style.bg.is_none() {
                        s.style.bg(sunken)
                    } else {
                        s.style
                    };
                    Span::styled(s.content, style)
                })
                .collect::<Vec<_>>(),
        )
    };
    let x = spare.x + (spare.width - widest as u16) / 2;
    let top = spare.y + (spare.height - height as u16) / 2;
    let label = kit::label("Side panel", Tone::Dim, None, widest + 1, theme);
    crate::ui::draw::put_line(buf, x - 1, top, widest as u16 + 1, &on_sunken(label));
    for (i, (beside, key)) in keyed.iter().enumerate() {
        let y = top + 2 + 2 * i as u16;
        let lit = hover == Some(*beside);
        let mut spans = vec![Span::raw(" ")];
        if let Some(key) = key {
            let keycap = cap(key, caps, theme);
            let used = kit::spans_width(&keycap);
            spans.extend(keycap);
            spans.push(Span::raw(" ".repeat(KEY_COLS.saturating_sub(used).max(1))));
        }
        let text = if lit {
            Tone::Text.bold(theme)
        } else {
            Tone::Text.style(theme)
        };
        spans.push(Span::styled(beside.label(), text));
        let cols = widest + 4;
        let line = on_sunken(kit::row(spans, cols, lit, theme));
        let at = x - 2;
        crate::ui::draw::put_line(buf, at, y, cols as u16, &line);
        hits.push((Rect::new(at, y, cols as u16, 1), *beside));
    }
    let foot = vec![Span::styled(last.to_string(), Tone::Dim.style(theme))];
    let y = top + 2 + 2 * keyed.len() as u16;
    crate::ui::draw::put_line(buf, x, y, widest as u16, &on_sunken(foot));
    hits
}

/// A key as the kit's keycap, with nothing after it.
fn cap(key: &str, caps: bool, theme: &Theme) -> Vec<Span<'static>> {
    let mut spans = kit::keycap(key, "", false, caps, theme);
    spans.pop();
    spans
}

/// The spare columns right of a capped pane: right of its rule, to the
/// spread's edge; none when the pane fills the spread.
pub fn spare_of(spread: Rect, area: Rect) -> Option<Rect> {
    let left = area.right() + 1;
    (spread.right() > left).then(|| Rect::new(left, spread.y, spread.right() - left, spread.height))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pr_and_ticket_rows_show_only_when_the_tab_has_them() {
        assert_eq!(
            rows(false, false),
            [Beside::Note, Beside::Split, Beside::Link]
        );
        assert_eq!(rows(true, true).len(), 5);
    }

    #[test]
    fn each_row_shows_its_bound_key_and_a_rebound_one() {
        let mut bindings = Bindings::default();
        let (keyed, last) = lines(&bindings, &rows(false, false));
        assert_eq!(keyed[0].1.as_deref(), Some("M-e"));
        assert_eq!(keyed[2].1.as_deref(), Some("M-u"));
        assert!(last.contains("C-M-S-l"), "{last}");
        bindings.apply(&serde_json::json!({"root": {"M-e": null, "M-n": "notes-beside"}}));
        let (keyed, _) = lines(&bindings, &rows(false, false));
        assert_eq!(keyed[0].1.as_deref(), Some("M-n"));
        bindings
            .apply(&serde_json::json!({"root": {"M-n": null}, "prefix": {"e": "notes-beside"}}));
        let (keyed, _) = lines(&bindings, &rows(false, false));
        assert_eq!(keyed[0].1.as_deref(), Some("M-b e"));
    }

    #[test]
    fn a_row_with_no_key_starts_its_label_where_the_keys_start() {
        let bindings = Bindings::default();
        let (mut keyed, last) = lines(&bindings, &rows(false, false));
        keyed[2].1 = None;
        let mut buf = Buffer::empty(Rect::new(0, 0, 120, 40));
        let hits = draw(
            &mut buf,
            Rect::new(40, 0, 80, 40),
            &keyed,
            &last,
            None,
            true,
            &Theme::DARK,
        );
        let row = |i: usize| {
            let at = hits[i].0;
            (at.x..at.right())
                .map(|x| buf[(x, at.y)].symbol().to_string())
                .collect::<String>()
        };
        let note = row(0);
        let link = row(2);
        let keys_start = note.find(|c: char| c != ' ').unwrap();
        assert!(note[keys_start..].contains("M-e"), "{note}");
        assert_eq!(link.find("Read a link").unwrap(), keys_start, "{link}");
    }

    #[test]
    fn nothing_is_drawn_where_the_list_doesnt_fit() {
        let bindings = Bindings::default();
        let (keyed, last) = lines(&bindings, &rows(true, true));
        let mut buf = Buffer::empty(Rect::new(0, 0, 200, 40));
        let wide = draw(
            &mut buf,
            Rect::new(100, 0, 100, 40),
            &keyed,
            &last,
            None,
            true,
            &Theme::DARK,
        );
        assert_eq!(wide.len(), 5);
        let narrow = draw(
            &mut buf,
            Rect::new(180, 0, 20, 40),
            &keyed,
            &last,
            None,
            true,
            &Theme::DARK,
        );
        assert!(narrow.is_empty());
        let short = draw(
            &mut buf,
            Rect::new(100, 0, 100, 8),
            &keyed,
            &last,
            None,
            true,
            &Theme::DARK,
        );
        assert!(short.is_empty());
    }
}
