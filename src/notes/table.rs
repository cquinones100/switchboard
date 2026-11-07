//! Tables, as Markdown writes them, ported from notes/lua/notes/table.lua:
//!
//! ```text
//!   | Item | Qty |
//!   | ---- | --- |
//!   | Eggs | 12  |
//! ```
//!
//! drawn with box lines (│ ├ ┼ ┤ ─), the first row bold, and kept aligned:
//! leaving Insert mode, or any change in Normal mode, pads every cell of the
//! table the cursor is in to its column's width, measured as the cell shows
//! (markup hidden doesn't count). In Insert mode, Tab moves to the next cell
//! (past the last, it adds a row) and Shift-Tab to the one before.

use super::format::Doc;
use super::render::{self, Hl, Look};
use crate::ui::draw::width;

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
    doc.text.set_lines(first, last + 1, &out);
    true
}

/// Aligns every table in a note, for one just opened.
pub fn align_all(doc: &mut Doc) {
    let mut row = 0;
    while row < doc.text.len() {
        let lines = doc.text.lines();
        match bounds(&lines, row) {
            Some((_, last)) => {
                let rows = &lines[row..=last];
                let out = aligned(rows);
                if out != rows {
                    doc.text.set_lines(row, last + 1, &out);
                }
                row = last + 1;
            }
            None => row += 1,
        }
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
    align(doc, row);
    let mut lines = doc.text.lines();
    let Some((first, mut last)) = bounds(&lines, row) else {
        return false;
    };
    let mut r = row;
    let mut starts = cell_starts(&lines[r]);
    let mut index = cell_index(&lines[r], doc.col) as i64 + step as i64;
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
    if bounds(&lines, doc.row).is_none() || is_separator(&lines[doc.row]) {
        return;
    }
    doc.text.set_lines(doc.row, doc.row + 1, &[] as &[&str]);
    doc.row = doc.row.min(doc.text.len() - 1);
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
    use crate::notes::text::Text;

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
