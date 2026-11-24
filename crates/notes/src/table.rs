//! Tables, as Markdown writes them, ported from notes/lua/notes/table.lua:
//!
//! ```text
//!   | Item | Qty |
//!   | ---- | --- |
//!   | Eggs | 12  |
//! ```
//!
//! drawn with box lines (│ ├ ┼ ┤ ─), the first row bold, and lined up as
//! drawn: each cell padded on the screen to its column's width, measured as
//! the cell shows (markup hidden doesn't count, a wide character is two), so
//! a table written from outside with its cells unpadded reads aligned
//! without its text changing. The text is kept aligned too where you edit:
//! leaving Insert mode in a table, or any change that edits a table's rows
//! (`line_up_edited`), pads every cell of it, the cursor kept on its
//! character. In Insert mode, Tab moves to the next cell
//! (past the last, it adds a row) and Shift-Tab to the one before.

use super::format::Doc;
use super::render::{self, Hl, Look};
use switchboard_ui::draw::width;

pub fn is_row(line: Option<&str>) -> bool {
    line.is_some_and(|l| l.trim_start().starts_with('|'))
}

pub fn is_separator(line: &str) -> bool {
    let t = line.trim();
    t.starts_with('|')
        && t.len() > 1
        && t[1..]
            .chars()
            .all(|c| c.is_whitespace() || matches!(c, ':' | '|' | '-'))
        && t.contains('-')
}

/// The byte columns of a row's bars, an escaped one (`\|`) left out.
fn bars_of(line: &str) -> Vec<usize> {
    let b = line.as_bytes();
    (0..b.len())
        .filter(|&i| b[i] == b'|' && (i == 0 || b[i - 1] != b'\\'))
        .collect()
}

/// A row's cells, split on `|` that isn't escaped, without the outer bars.
pub fn cells(line: &str) -> Vec<String> {
    let t = line.trim_start();
    let t = t.strip_prefix('|').unwrap_or(t);
    let t = t.trim_end();
    let body = t.strip_suffix('|').unwrap_or(t);
    let mut cells = Vec::new();
    let mut current = String::new();
    let mut chars = body.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' && chars.peek() == Some(&'|') {
            current.push_str("\\|");
            chars.next();
        } else if c == '|' {
            cells.push(current.trim().to_string());
            current.clear();
        } else {
            current.push(c);
        }
    }
    cells.push(current.trim().to_string());
    cells
}

/// How wide a cell shows: its width less the markup hidden in it.
fn shown_width(cell: &str) -> usize {
    let hidden: usize = render::inline(cell, 0)
        .iter()
        .flat_map(|s| s.conceal.iter().map(|(a, b)| b - a))
        .sum();
    width(cell).saturating_sub(hidden)
}

/// The first and last row of the table around `row`, if it is in one.
pub fn bounds(lines: &[String], row: usize) -> Option<(usize, usize)> {
    let at = |r: usize| lines.get(r).map(String::as_str);
    if !is_row(at(row)) {
        return None;
    }
    let (mut first, mut last) = (row, row);
    while first > 0 && is_row(at(first - 1)) {
        first -= 1;
    }
    while is_row(at(last + 1)) {
        last += 1;
    }
    Some((first, last))
}

/// A table's rows, padded so every column lines up. Separator rows keep
/// their alignment colons.
pub fn aligned(rows: &[String]) -> Vec<String> {
    let indent: String = rows[0].chars().take_while(|c| c.is_whitespace()).collect();
    let parsed: Vec<(bool, Vec<String>)> =
        rows.iter().map(|l| (is_separator(l), cells(l))).collect();
    let columns = parsed.iter().map(|(_, c)| c.len()).max().unwrap_or(0);
    let mut widths = vec![3; columns];
    for (sep, cells) in &parsed {
        if !sep {
            for (c, cell) in cells.iter().enumerate() {
                widths[c] = widths[c].max(shown_width(cell));
            }
        }
    }
    parsed
        .iter()
        .map(|(sep, cells)| {
            let parts: Vec<String> = (0..columns)
                .map(|c| {
                    let cell = cells.get(c).map(String::as_str).unwrap_or("");
                    if *sep {
                        let (left, right) = (cell.starts_with(':'), cell.ends_with(':'));
                        let dashes = widths[c] - usize::from(left) - usize::from(right);
                        format!(
                            "{}{}{}",
                            if left { ":" } else { "" },
                            "-".repeat(dashes),
                            if right { ":" } else { "" }
                        )
                    } else {
                        format!("{cell}{}", " ".repeat(widths[c] - shown_width(cell)))
                    }
                })
                .collect();
            format!("{indent}| {} |", parts.join(" | "))
        })
        .collect()
}

/// Whether a change edited the table around `row` of `lines`: no table of
/// `before` has exactly its rows, so one a change only sat beside, or
/// moved whole, isn't.
pub fn edited(before: &[String], lines: &[String], row: usize) -> bool {
    let Some((first, last)) = bounds(lines, row) else {
        return false;
    };
    let rows = &lines[first..=last];
    let mut r = 0;
    while r < before.len() {
        match bounds(before, r) {
            Some((f, l)) => {
                if &before[f..=l] == rows {
                    return false;
                }
                r = l + 1;
            }
            None => r += 1,
        }
    }
    true
}

/// After a change made from `before`, every table it edited lined up,
/// wherever the cursor is (`:%s`, a linewise change begun above one, rows
/// deleted from a table's end). A table only beside the change stays as
/// written.
pub fn line_up_edited(doc: &mut Doc, before: &[String]) {
    let lines = doc.text.lines();
    let mut row = 0;
    while row < lines.len() {
        match bounds(&lines, row) {
            Some((_, last)) => {
                if edited(before, &lines, row) {
                    align(doc, row);
                }
                row = last + 1;
            }
            None => row += 1,
        }
    }
}

/// Aligns the table around `row`. Gives whether anything changed.
pub fn align(doc: &mut Doc, row: usize) -> bool {
    let lines = doc.text.lines();
    let Some((first, last)) = bounds(&lines, row) else {
        return false;
    };
    let rows = &lines[first..=last];
    let out = aligned(rows);
    if out == rows {
        return false;
    }
    if (first..=last).contains(&doc.row) {
        let at = doc.row - first;
        doc.col = carry(&rows[at], &out[at], doc.col);
    }
    doc.text.set_lines(first, last + 1, &out);
    true
}

/// Where byte `col` of a table row goes once the row is padded as `new`:
/// the same character of the same cell, or the same bar, so lining a table
/// up never moves the cursor to another character.
pub fn carry(old: &str, new: &str, col: usize) -> usize {
    let at = carried(old, new, col).min(new.len());
    // Always on a character of the new row.
    (0..=at)
        .rev()
        .find(|&b| new.is_char_boundary(b))
        .unwrap_or(0)
}

fn carried(old: &str, new: &str, col: usize) -> usize {
    let (bars, moved) = (bars_of(old), bars_of(new));
    let Some(&start) = bars.first() else {
        return col.min(new.len());
    };
    if col < start {
        return col.min(moved.first().copied().unwrap_or(0));
    }
    if let Some(k) = bars.iter().position(|&b| b == col) {
        return moved.get(k).copied().unwrap_or(col).min(new.len());
    }
    let k = bars.iter().filter(|&&b| b < col).count() - 1;
    let (Some(&from), Some(&to)) = (moved.get(k), moved.get(k + 1)) else {
        let end = bars[bars.len() - 1];
        let end_new = moved.get(bars.len() - 1).copied().unwrap_or(end);
        return (end_new + (col - end)).min(new.len());
    };
    let Some(&next) = bars.get(k + 1) else {
        // A last cell with no closing bar: through its text, as below.
        let start = |line: &str, bar: usize| {
            let cell = &line[bar + 1..];
            bar + 1 + (cell.len() - cell.trim_start().len())
        };
        let (s, s_new) = (start(old, bars[k]), start(new, from));
        return if col < s {
            (from + 1 + (col - bars[k] - 1)).min(s_new)
        } else {
            s_new + (col - s)
        }
        .min(new.len());
    };
    // The text of a cell, its spaces around it left out.
    let text = |line: &str, a: usize, b: usize| {
        let cell = &line[a + 1..b];
        let s = a + 1 + (cell.len() - cell.trim_start().len());
        (s, s + cell.trim().len())
    };
    let (s, e) = text(old, bars[k], next);
    let (s_new, e_new) = text(new, from, to);
    let at = if col < s {
        (from + 1 + (col - bars[k] - 1)).min(s_new)
    } else if col < e {
        s_new + (col - s)
    } else {
        e_new + (col - e)
    };
    at.min(to.saturating_sub(1)).max(from + 1)
}

/// `carry` for where typing goes in: typing at a bar with text right
/// before it is at the end of that text, which stays there.
pub fn carry_typing(old: &str, new: &str, col: usize) -> usize {
    let on_bar = bars_of(old).iter().skip(1).any(|&b| b == col);
    match old[..col.min(old.len())].char_indices().last() {
        Some((at, c)) if on_bar && !c.is_whitespace() => carry(old, new, at) + c.len_utf8(),
        _ => carry(old, new, col),
    }
}

/// A table line's look: its bars as box lines, a separator row as a line
/// across, and the header row bold.
pub fn draw(line: &str, header: bool, look: &mut Look) {
    let separator = is_separator(line);
    let bars = bars_of(line);
    for (n, &col) in bars.iter().enumerate() {
        let ch = if !separator {
            "│"
        } else if n == 0 {
            "├"
        } else if n == bars.len() - 1 {
            "┤"
        } else {
            "┼"
        };
        render::conceal_with(look, col, col + 1, ch, Hl::TableBorder, 210);
    }
    if separator {
        let b = line.as_bytes();
        for i in 0..b.len() {
            if (b[i] == b'-' || b[i] == b':') || (b[i] == b' ' && i > 0 && i + 1 < b.len()) {
                render::conceal_with(look, i, i + 1, "─", Hl::TableBorder, 205);
            }
        }
    } else if header && bars.len() > 1 {
        render::highlight(
            look,
            bars[0] + 1,
            bars[bars.len() - 1],
            Hl::TableHeader,
            155,
        );
    }
}

/// Pads a table's cells as they are drawn, `rows` the table's lines and
/// `looks` theirs: each column as wide as its widest cell shows (markup
/// hidden not counted, a wide character two), so the box lines line up
/// whatever spaces the text has, the text itself unchanged.
pub fn pad(rows: &[&str], looks: &mut [Look]) {
    let segments: Vec<(Vec<usize>, Vec<usize>)> = rows
        .iter()
        .zip(looks.iter())
        .map(|(line, look)| {
            let bars = bars_of(line);
            let shown = render::cells(line, look, false, &[], false);
            let widths = bars
                .windows(2)
                .map(|w| {
                    shown
                        .iter()
                        .filter(|c| c.col.is_some_and(|b| b > w[0] && b < w[1]))
                        .map(|c| c.width)
                        .sum()
                })
                .collect();
            (bars, widths)
        })
        .collect();
    let columns = segments.iter().map(|(_, w)| w.len()).max().unwrap_or(0);
    let mut widest = vec![0; columns];
    for (_, widths) in &segments {
        for (c, &w) in widths.iter().enumerate() {
            widest[c] = widest[c].max(w);
        }
    }
    for ((line, look), (bars, widths)) in rows.iter().zip(looks.iter_mut()).zip(&segments) {
        let fill = if is_separator(line) { "─" } else { " " };
        for (c, &w) in widths.iter().enumerate() {
            if widest[c] > w {
                look.pads.push((bars[c + 1], widest[c] - w, fill));
            }
        }
    }
}

// ── Editing ─────────────────────────────────────────────────────────────────

/// A new table under the cursor's line, two columns by two rows, the
/// cursor in its first cell, typing.
pub fn insert(doc: &mut Doc) {
    let row = doc.row;
    let current = doc.text.line(row);
    let mut lines = vec![
        "|     |     |".to_string(),
        "| --- | --- |".to_string(),
        "|     |     |".to_string(),
    ];
    let has_text = !current.trim().is_empty();
    let at = if has_text {
        lines.insert(0, String::new());
        row + 1
    } else {
        row
    };
    doc.text.set_lines(at, at, &lines);
    doc.row = at + usize::from(has_text);
    doc.col = 2;
    doc.start_insert(false);
}

/// The byte columns where each cell's text starts on a row: its first
/// character, or just past "| " in an empty cell.
fn cell_starts(line: &str) -> Vec<usize> {
    let bars = bars_of(line);
    let b = line.as_bytes();
    (0..bars.len().saturating_sub(1))
        .map(|n| {
            let mut s = bars[n] + 1;
            while s < bars[n + 1] && b[s] == b' ' {
                s += 1;
            }
            if s < bars[n + 1] { s } else { bars[n] + 2 }
        })
        .collect()
}

/// Which cell (counting from 1) a column is in, or 0 before the first bar.
fn cell_index(line: &str, col: usize) -> usize {
    let bars = bars_of(line);
    let index = bars.iter().filter(|&&bar| col >= bar).count();
    index.min(bars.len().saturating_sub(1))
}

/// Moves to the next (`step` 1) or previous (-1) cell, aligning first.
/// Past the last cell of the last row, adds a row.
pub fn jump(doc: &mut Doc, step: i32) -> bool {
    let row = doc.row;
    // The cell typed in, read before lining up: typing on a bar is at the
    // end of the cell before it.
    let bars = bars_of(&doc.text.line(row));
    let typed_in = bars
        .iter()
        .filter(|&&bar| bar < doc.col)
        .count()
        .min(cells(&doc.text.line(row)).len());
    align(doc, row);
    let mut lines = doc.text.lines();
    let Some((first, mut last)) = bounds(&lines, row) else {
        return false;
    };
    let mut r = row;
    let mut starts = cell_starts(&lines[r]);
    let mut index = typed_in as i64 + step as i64;
    loop {
        if index >= 1 && index as usize <= starts.len() && !is_separator(&lines[r]) {
            break;
        }
        if step < 0 {
            if r == first {
                return true;
            }
            r -= 1;
        } else {
            r += 1;
        }
        if r > last {
            add_row(doc, last);
            lines = doc.text.lines();
            last += 1;
        }
        starts = cell_starts(&lines[r]);
        index = if step > 0 { 1 } else { starts.len() as i64 };
    }
    doc.row = r;
    doc.col = starts[index as usize - 1];
    true
}

/// An empty row under `row`, with as many cells as the table has.
pub fn add_row(doc: &mut Doc, row: usize) {
    let line = doc.text.line(row);
    let count = cells(&line).len();
    let indent: String = line.chars().take_while(|c| c.is_whitespace()).collect();
    doc.text.set_lines(
        row + 1,
        row + 1,
        &[format!("{indent}|{}", "     |".repeat(count))],
    );
    align(doc, row);
}

fn joined(line: &str, cells: &[String]) -> String {
    let indent: String = line.chars().take_while(|c| c.is_whitespace()).collect();
    format!("{indent}| {} |", cells.join(" | "))
}

/// An empty column after the one the cursor is in.
pub fn add_column(doc: &mut Doc) {
    let lines = doc.text.lines();
    let Some((first, last)) = bounds(&lines, doc.row) else {
        return;
    };
    let at = cell_index(&lines[doc.row], doc.col);
    let out: Vec<String> = (first..=last)
        .map(|r| {
            let mut cells = cells(&lines[r]);
            let blank = if is_separator(&lines[r]) { "---" } else { "" };
            cells.insert(at.min(cells.len()), blank.to_string());
            joined(&lines[r], &cells)
        })
        .collect();
    doc.text.set_lines(first, last + 1, &aligned(&out));
}

pub fn delete_row(doc: &mut Doc) {
    let lines = doc.text.lines();
    let Some((first, last)) = bounds(&lines, doc.row) else {
        return;
    };
    if is_separator(&lines[doc.row]) {
        return;
    }
    doc.text.set_lines(doc.row, doc.row + 1, &[] as &[&str]);
    doc.row = doc.row.min(doc.text.len() - 1);
    // The rows left lined up, as adding or deleting a column leaves them,
    // whichever row went.
    if last > first {
        align(doc, first);
    }
}

pub fn delete_column(doc: &mut Doc) {
    let lines = doc.text.lines();
    let Some((first, last)) = bounds(&lines, doc.row) else {
        return;
    };
    let at = cell_index(&lines[doc.row], doc.col).max(1);
    let mut out = Vec::new();
    for line in &lines[first..=last] {
        let mut cells = cells(line);
        if at <= cells.len() {
            cells.remove(at - 1);
        }
        if cells.is_empty() {
            doc.text.set_lines(first, last + 1, &[] as &[&str]);
            doc.row = first.min(doc.text.len() - 1);
            return;
        }
        out.push(joined(line, &cells));
    }
    doc.text.set_lines(first, last + 1, &aligned(&out));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::Text;

    #[test]
    fn tables_line_up_as_they_show_and_tab_moves_between_cells() {
        let rows: Vec<String> = [
            "| Item | Qty |",
            "|:-|-:|",
            "| **Eggs** | 12 |",
            "| a \\| b | 1 |",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        assert_eq!(
            aligned(&rows),
            [
                "| Item   | Qty |",
                "| :----- | --: |",
                "| **Eggs**   | 12  |",
                "| a \\| b | 1   |"
            ],
            "bold markup doesn't count toward a cell's width"
        );
        let mut doc = Doc::new(Text::from_lines(&[
            "| A | B |",
            "| --- | --- |",
            "| x | y |",
        ]));
        doc.col = 2;
        let mut seen = Vec::new();
        for _ in 0..4 {
            jump(&mut doc, 1);
            seen.push((doc.row, doc.col));
        }
        assert_eq!(
            seen,
            [(0, 8), (2, 2), (2, 8), (3, 2)],
            "next cell, over the separator, then a new row"
        );
        assert_eq!(doc.text.len(), 4);
        doc.row = 0;
        doc.col = 2;
        add_column(&mut doc);
        assert_eq!(doc.text.line(0), "| A   |     | B   |");
        doc.col = 8;
        delete_column(&mut doc);
        assert_eq!(doc.text.line(0), "| A   | B   |");
        jump(&mut doc, -1);
        assert_eq!((doc.row, doc.col), (0, 2), "Shift-Tab goes back a cell");
        let mut doc = Doc::new(Text::from_lines(&["| a | b |", "|---|---|", "| x | y"]));
        doc.row = 2;
        doc.col = 7;
        jump(&mut doc, 1);
        assert_eq!(
            (doc.row, doc.text.len()),
            (3, 4),
            "past a last cell with no closing bar, a new row"
        );
        let mut doc = Doc::new(Text::from_lines(&[
            "| a | b |",
            "|---|---|",
            "| cccc | d |",
            "",
        ]));
        doc.row = 2;
        delete_row(&mut doc);
        assert_eq!(
            doc.text.lines()[..2],
            ["| a   | b   |", "| --- | --- |"],
            "the last row deleted, the rest lined up"
        );
        let mut doc = Doc::new(Text::from_lines(&["|abc|b|", "|-|-|", "|c|d|"]));
        doc.col = 4;
        jump(&mut doc, 1);
        assert_eq!(doc.text.line(0), "| abc | b   |");
        assert_eq!(
            (doc.row, doc.col),
            (0, 8),
            "typed up to a bar, Tab goes to the next cell"
        );
    }

    /// A table's lines as drawn, each cell with the byte it stands for.
    fn drawn(lines: &[&str]) -> Vec<Vec<render::Cell>> {
        let ks = render::kinds(lines);
        let looks = render::looks(
            lines,
            &ks,
            &render::Drawing {
                strike_checked: true,
                note_exists: &|_| true,
            },
        );
        lines
            .iter()
            .zip(&looks)
            .map(|(line, look)| render::cells(line, look, false, &[], false))
            .collect()
    }

    #[test]
    fn a_table_written_unpadded_draws_its_box_lines_in_line() {
        let lines = [
            "| a | bbbb |",
            "|---|---|",
            "| cccccc | **d** |",
            "| 日本 | e |",
        ];
        let rows = drawn(&lines);
        let text =
            |cells: &[render::Cell]| cells.iter().map(|c| c.text.clone()).collect::<String>();
        assert_eq!(
            rows.iter().map(|r| text(r)).collect::<Vec<_>>(),
            [
                "│ a      │ bbbb │",
                "├────────┼──────┤",
                "│ cccccc │ d    │",
                "│ 日本   │ e    │",
            ],
            "every column as wide as its widest cell shows"
        );
        let bars = |cells: &[render::Cell]| {
            let mut x = 0;
            let mut at = Vec::new();
            for c in cells {
                if matches!(c.text.as_str(), "│" | "├" | "┼" | "┤") {
                    at.push(x);
                }
                x += c.width;
            }
            at
        };
        for row in &rows {
            assert_eq!(bars(row), [0, 9, 16], "the box lines in the same columns");
        }
    }

    #[test]
    fn a_drawn_table_maps_each_byte_to_its_column_and_back() {
        let lines = ["| a | bbbb |", "| cccccc | d |"];
        let rows = drawn(&lines);
        for (line, cells) in lines.iter().zip(&rows) {
            // Each byte of the text to the column it is drawn at, and the
            // column back to the byte, padding standing for no byte.
            let mut x = 0;
            let mut seen = Vec::new();
            for c in cells {
                if let Some(b) = c.col {
                    seen.push(b);
                    let back = {
                        let mut at = 0;
                        cells.iter().find(|d| {
                            let hit = x < at + d.width;
                            at += d.width;
                            hit
                        })
                    };
                    assert_eq!(back.and_then(|d| d.col), Some(b), "{line}: byte {b} at {x}");
                }
                x += c.width;
            }
            assert_eq!(
                seen,
                (0..line.len()).collect::<Vec<_>>(),
                "{line}: every byte drawn once, in order"
            );
        }
    }

    #[test]
    fn lining_a_table_up_keeps_the_cursor_on_its_character() {
        let mut doc = Doc::new(Text::from_lines(&[
            "| a | bbbb |",
            "|---|---|",
            "| cccccc | d |",
        ]));
        doc.col = 7;
        assert!(align(&mut doc, 0));
        assert_eq!(doc.text.line(0), "| a      | bbbb |");
        assert_eq!(
            &doc.text.line(0)[doc.col..doc.col + 1],
            "b",
            "still on the second b"
        );
        assert_eq!(doc.col, 12);
        let mut doc = Doc::new(Text::from_lines(&[
            "| a | bbbb |",
            "|---|---|",
            "| cccccc | d |",
        ]));
        doc.col = 4;
        align(&mut doc, 0);
        assert_eq!(doc.col, 9, "on the bar it was on");
        let mut doc = Doc::new(Text::from_lines(&[
            "| a | bbbb |",
            "|---|---|",
            "| cccccc | d |",
        ]));
        doc.row = 2;
        doc.col = 11;
        align(&mut doc, 0);
        assert_eq!(
            &doc.text.line(2)[doc.col..doc.col + 1],
            "d",
            "a row of its own carried too"
        );
        assert_eq!(
            carry_typing("|a|日本|", "| a   | 日本 |", 9),
            14,
            "typing after the text, not inside a character"
        );
        assert_eq!(
            carry_typing("| a | b |", "| a | b |", 4),
            4,
            "after a space, still at the bar"
        );
        assert_eq!(
            carry("|   日本 | x |", "| 日本 | x   |", 3),
            2,
            "leading spaces carried to the text, never into it"
        );
    }

    #[test]
    fn a_table_shows_box_lines_and_a_bold_header() {
        let lines = ["| A   | B   |", "| --- | --- |", "| x   | y   |"];
        let ks = render::kinds(&lines);
        let looks = render::looks(
            &lines,
            &ks,
            &render::Drawing {
                strike_checked: true,
                note_exists: &|_| true,
            },
        );
        let shown = |i: usize| {
            render::cells(lines[i], &looks[i], false, &[], false)
                .iter()
                .map(|c| c.text.clone())
                .collect::<String>()
        };
        assert_eq!(shown(0), "│ A   │ B   │");
        assert_eq!(shown(1), "├─────┼─────┤");
        assert!(looks[0].has(Hl::TableHeader));
        assert!(!looks[2].has(Hl::TableHeader));
    }
}
