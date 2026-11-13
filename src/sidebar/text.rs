//! Text in the sidebar and mission control, measured as a terminal draws it,
//! and the states' colors and marks (lib/sidebar/text.js). Every line is cut
//! or padded to exactly the columns it is given, so a frame overwrites the
//! last one without clearing.
//!
//! Width is counted by the code point, as text.js counted it: two columns for
//! East Asian wide characters and the symbols terminals draw as emoji (⏳ ✅
//! ❌ ⚡ ✨ ⭐ …), none for combining marks and joiners, and two for a symbol a
//! terminal would draw narrow when an emoji presentation selector (U+FE0F)
//! follows it, as in ⚠️. Counting any of these one short pushes the rest of
//! the line, and every border after it on the screen, a column right. The
//! sidebar writes its cells by this same count (`render::put`), so what is
//! measured and what is drawn never disagree.

use ratatui::style::Color;

/// East Asian wide characters and emoji, by code point (text.js's `WIDE`).
const WIDE: &[(u32, u32)] = &[
    (0x1100, 0x115f),
    (0x231a, 0x231b),
    (0x2329, 0x232a),
    (0x23e9, 0x23ec),
    (0x23f0, 0x23f0),
    (0x23f3, 0x23f3),
    (0x25fd, 0x25fe),
    (0x2614, 0x2615),
    (0x2648, 0x2653),
    (0x267f, 0x267f),
    (0x2693, 0x2693),
    (0x26a1, 0x26a1),
    (0x26aa, 0x26ab),
    (0x26bd, 0x26be),
    (0x26c4, 0x26c5),
    (0x26ce, 0x26ce),
    (0x26d4, 0x26d4),
    (0x26ea, 0x26ea),
    (0x26f2, 0x26f3),
    (0x26f5, 0x26f5),
    (0x26fa, 0x26fa),
    (0x26fd, 0x26fd),
    (0x2705, 0x2705),
    (0x270a, 0x270b),
    (0x2728, 0x2728),
    (0x274c, 0x274c),
    (0x274e, 0x274e),
    (0x2753, 0x2755),
    (0x2757, 0x2757),
    (0x2795, 0x2797),
    (0x27b0, 0x27b0),
    (0x27bf, 0x27bf),
    (0x2b1b, 0x2b1c),
    (0x2b50, 0x2b50),
    (0x2b55, 0x2b55),
    (0x2e80, 0xa4cf),
    (0xac00, 0xd7a3),
    (0xf900, 0xfaff),
    (0xfe30, 0xfe4f),
    (0xff00, 0xff60),
    (0xffe0, 0xffe6),
    (0x1f004, 0x1f004),
    (0x1f0cf, 0x1f0cf),
    (0x1f18e, 0x1f18e),
    (0x1f191, 0x1f19a),
    (0x1f200, 0x1f2ff),
    (0x1f300, 0x1f64f),
    (0x1f680, 0x1f6ff),
    (0x1f7e0, 0x1f7eb),
    (0x1f90c, 0x1f9ff),
    (0x1fa70, 0x1faff),
    (0x20000, 0x3fffd),
];

/// Combining marks, zero-width spaces and joiners, and the variation
/// selectors: drawn on the character before them, in no column of their own.
const ZERO: &[(u32, u32)] = &[
    (0x0300, 0x036f),
    (0x200b, 0x200f),
    (0xfe00, 0xfe0f),
    (0x20d0, 0x20ff),
];

/// The emoji presentation selector.
const VS16: char = '\u{fe0f}';

fn within(code: u32, ranges: &[(u32, u32)]) -> bool {
    ranges.iter().any(|&(from, to)| code >= from && code <= to)
}

/// The columns a character takes on its own.
pub fn char_width(c: char) -> usize {
    let code = c as u32;
    if code < 32 || within(code, ZERO) {
        0
    } else if within(code, WIDE) {
        2
    } else {
        1
    }
}

/// A character's width where it stands, after a character `before` columns
/// wide: an emoji presentation selector widens a narrow symbol before it.
pub fn width_after(c: char, before: usize) -> usize {
    if c == VS16 && before == 1 {
        1
    } else {
        char_width(c)
    }
}

/// The columns `text` takes.
pub fn width(text: &str) -> usize {
    let mut sum = 0;
    let mut before = 0;
    for c in text.chars() {
        let w = width_after(c, before);
        sum += w;
        before = w;
    }
    sum
}

/// `text` cut to `cols` columns, ending in … when anything was cut.
pub fn cut(text: &str, cols: usize) -> String {
    if width(text) <= cols {
        return text.to_string();
    }
    let mut out = String::new();
    let mut used = 0;
    let mut before = 0;
    for c in text.chars() {
        let w = width_after(c, before);
        if used + w + 1 > cols {
            break;
        }
        out.push(c);
        used += w;
        before = w;
    }
    if cols > 0 {
        out.push('…');
    }
    out
}

/// `text` cut to `cols` columns and padded with spaces to fill them.
pub fn pad(text: &str, cols: usize) -> String {
    let mut out = cut(text, cols);
    let w = width(&out);
    out.extend(std::iter::repeat_n(' ', cols.saturating_sub(w)));
    out
}

/// Plain text wrapped to `at_least` columns (four at the least), on word
/// boundaries where it can be, a word longer than a line split across as
/// many as it takes. Each line it has is wrapped on its own.
pub fn wrap(text: &str, at_least: usize) -> Vec<String> {
    let cols = at_least.max(4);
    let mut out = Vec::new();
    for paragraph in text.split('\n') {
        let mut lines: Vec<String> = Vec::new();
        let mut line = String::new();
        for word in paragraph.split_whitespace() {
            let mut word = word.to_string();
            while width(&word) > cols {
                let room = cols as isize - width(&line) as isize - isize::from(!line.is_empty());
                if room < 4 {
                    if !line.is_empty() {
                        lines.push(std::mem::take(&mut line));
                    }
                    continue;
                }
                let head: String = word.chars().take(room as usize).collect();
                lines.push(if line.is_empty() {
                    head.clone()
                } else {
                    format!("{line} {head}")
                });
                line.clear();
                word = word[head.len()..].to_string();
            }
            if line.is_empty() {
                line = word;
            } else if width(&line) + 1 + width(&word) <= cols {
                line.push(' ');
                line.push_str(&word);
            } else {
                lines.push(std::mem::replace(&mut line, word));
            }
        }
        lines.push(line);
        out.extend(lines);
    }
    out
}

/// A Markdown table's cells, its pipes escaped as `\|` kept in them.
fn cells(line: &str) -> Vec<String> {
    let trimmed = line.trim();
    let trimmed = trimmed.strip_prefix('|').unwrap_or(trimmed);
    let trimmed = match trimmed.strip_suffix('|') {
        Some(rest) if !rest.ends_with('\\') => rest,
        _ => trimmed,
    };
    let mut out = Vec::new();
    let mut cell = String::new();
    let mut chars = trimmed.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' if chars.peek() == Some(&'|') => {
                cell.push('|');
                chars.next();
            }
            '|' => out.push(std::mem::take(&mut cell).trim().to_string()),
            c => cell.push(c),
        }
    }
    out.push(cell.trim().to_string());
    out
}

fn is_table_row(line: &str) -> bool {
    let t = line.trim();
    t.len() >= 2 && t.starts_with('|') && t.ends_with('|')
}

/// A table's rule under its header: dashes, colons and pipes alone.
fn is_table_rule(line: &str) -> bool {
    let t = line.trim();
    t.contains('-')
        && t.chars().all(|c| matches!(c, '-' | ':' | '|' | ' ' | '\t'))
        && t.split('|')
            .map(str::trim)
            .filter(|cell| !cell.is_empty())
            .all(|cell| {
                let core = cell.trim_start_matches(':').trim_end_matches(':');
                !core.is_empty() && core.chars().all(|c| c == '-')
            })
}

/// The narrowest a table's column is drawn.
const MIN_COLUMN: usize = 4;

/// A table in `columns`: in a box, each column as wide as its longest cell,
/// or narrowed with its cells wrapped inside it when the table is too wide,
/// or one cell to a line under its header's name when even that can't fit.
fn table(rows: &[Vec<String>], columns: usize) -> Vec<String> {
    let count = rows.iter().map(Vec::len).max().unwrap_or(0);
    let full: Vec<Vec<String>> = rows
        .iter()
        .map(|row| {
            (0..count)
                .map(|i| row.get(i).cloned().unwrap_or_default())
                .collect()
        })
        .collect();
    let natural: Vec<usize> = (0..count)
        .map(|i| {
            full.iter()
                .map(|r| width(&r[i]))
                .max()
                .unwrap_or(0)
                .max(MIN_COLUMN)
        })
        .collect();
    let mut room = columns as isize - (count * 3 + 1) as isize;
    // Each column starts as wide as its longest word, where they all fit,
    // so no word is split; then what is left goes to the columns that want
    // more, an even share each, the ones wanting least first, so a short
    // column is never squeezed for a long one.
    let words: Vec<usize> = (0..count)
        .map(|i| {
            let longest = full
                .iter()
                .flat_map(|r| r[i].split_whitespace().map(width))
                .max()
                .unwrap_or(0)
                .max(MIN_COLUMN);
            natural[i].min(longest)
        })
        .collect();
    if words.iter().sum::<usize>() as isize > room {
        let header = &full[0];
        let mut out = Vec::new();
        for (r, row) in full.iter().skip(1).enumerate() {
            if r > 0 {
                out.push("─".repeat(columns.min(20)));
            }
            for (i, cell) in row.iter().enumerate() {
                if cell.is_empty() {
                    continue;
                }
                let named = if header[i].is_empty() {
                    cell.clone()
                } else {
                    format!("{}: {cell}", header[i])
                };
                out.extend(wrap(&named, columns));
            }
        }
        return out;
    }
    let mut widths = words.clone();
    room -= words.iter().sum::<usize>() as isize;
    let mut wanting: Vec<(usize, usize)> = natural
        .iter()
        .enumerate()
        .map(|(i, w)| (w - words[i], i))
        .collect();
    wanting.sort_by_key(|&(more, _)| more);
    let n = wanting.len();
    for (k, (more, i)) in wanting.into_iter().enumerate() {
        let share = (room.max(0) as usize) / (n - k);
        let given = more.min(share);
        widths[i] += given;
        room -= given as isize;
    }
    let rule = |left: &str, middle: &str, right: &str| {
        let parts: Vec<String> = widths.iter().map(|w| "─".repeat(w + 2)).collect();
        format!("{left}{}{right}", parts.join(middle))
    };
    let mut lines = vec![rule("┌", "┬", "┐")];
    for (r, row) in full.iter().enumerate() {
        if r > 0 {
            lines.push(rule("├", "┼", "┤"));
        }
        let wrapped: Vec<Vec<String>> = row
            .iter()
            .enumerate()
            .map(|(i, cell)| {
                if cell.is_empty() {
                    vec![String::new()]
                } else {
                    wrap(cell, widths[i])
                }
            })
            .collect();
        let height = wrapped.iter().map(Vec::len).max().unwrap_or(1);
        for l in 0..height {
            let cols: Vec<String> = wrapped
                .iter()
                .enumerate()
                .map(|(i, c)| format!(" {} ", pad(c.get(l).map_or("", String::as_str), widths[i])))
                .collect();
            lines.push(format!("│{}│", cols.join("│")));
        }
    }
    lines.push(rule("└", "┴", "┘"));
    lines
}

/// Text laid out in `columns` as Claude Code shows a reply: a Markdown table
/// drawn in a box (see `table`), everything else wrapped as `wrap` does,
/// blank lines kept.
pub fn layout(text: &str, columns: usize) -> Vec<String> {
    let lines: Vec<&str> = text.split('\n').collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if is_table_row(lines[i]) && lines.get(i + 1).is_some_and(|l| is_table_rule(l)) {
            let mut end = i + 2;
            while end < lines.len() && is_table_row(lines[end]) {
                end += 1;
            }
            let rows: Vec<Vec<String>> = std::iter::once(lines[i])
                .chain(lines[i + 2..end].iter().copied())
                .map(cells)
                .collect();
            out.extend(table(&rows, columns));
            i = end;
            continue;
        }
        if lines[i].trim().is_empty() {
            out.push(String::new());
        } else {
            out.extend(wrap(lines[i], columns));
        }
        i += 1;
    }
    out
}

/// Markdown read as plain text: fences, bold, code marks and heading marks
/// dropped.
pub fn plain(text: &str) -> String {
    use std::sync::LazyLock;
    static FENCE: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"```[^\n]*\n?").expect("a regex"));
    static BOLD: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"\*\*([^*]+)\*\*").expect("a regex"));
    static CODE: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"`([^`]+)`").expect("a regex"));
    static HEADING: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"(?m)^#{1,6}\s+").expect("a regex"));
    static BLANKS: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"\n{3,}").expect("a regex"));
    let text = FENCE.replace_all(text, "");
    let text = BOLD.replace_all(&text, "$1");
    let text = CODE.replace_all(&text, "$1");
    let text = HEADING.replace_all(&text, "");
    let text = BLANKS.replace_all(&text, "\n\n");
    text.trim().to_string()
}

/// How long ago a time in seconds was, shortly: `<1m`, `4m`, `2h`, `3d`;
/// empty for none.
pub fn ago(seconds: Option<i64>, now_secs: i64) -> String {
    let Some(at) = seconds.filter(|s| *s != 0) else {
        return String::new();
    };
    let s = (now_secs - at).max(0);
    if s < 60 {
        "<1m".into()
    } else if s < 3600 {
        format!("{}m", s / 60)
    } else if s < 86_400 {
        format!("{}h", s / 3600)
    } else {
        format!("{}d", s / 86_400)
    }
}

/// How long a stretch of seconds is, in its two largest units with no
/// space between them, a zero second unit left out: `<1m`, `12m`, `1h12m`,
/// `2h`, `2d3h`.
pub fn span(seconds: i64) -> String {
    let s = seconds.max(0);
    let two = |big: i64, big_unit: &str, small: i64, small_unit: &str| {
        if small == 0 {
            format!("{big}{big_unit}")
        } else {
            format!("{big}{big_unit}{small}{small_unit}")
        }
    };
    if s < 60 {
        "<1m".into()
    } else if s < 3600 {
        format!("{}m", s / 60)
    } else if s < 86_400 {
        two(s / 3600, "h", s % 3600 / 60, "m")
    } else {
        two(s / 86_400, "d", s % 86_400 / 3600, "h")
    }
}

/// How long until a time in seconds, for a reminder: `in 45m`, `in 2h 10m`,
/// `in 3d`.
pub fn until(at_secs: i64, now_ms: i64) -> String {
    let m = ((at_secs * 1000 - now_ms) as f64 / 60_000.0)
        .ceil()
        .max(1.0) as i64;
    if m < 60 {
        format!("in {m}m")
    } else if m < 1440 {
        let rest = if m % 60 != 0 {
            format!(" {}m", m % 60)
        } else {
            String::new()
        };
        format!("in {}h{rest}", m / 60)
    } else {
        format!("in {}d", (m as f64 / 1440.0).round() as i64)
    }
}

/// The spinner's frames, a turn every `SPINNER_MS`.
pub const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// How often the spinner turns.
pub const SPINNER_MS: u64 = 120;

/// What a tab's badge shows: Claude's state, the loop it runs, its PR's
/// place, a pin or a reminder (text.js's `STATES`). Each has a color (the
/// tmux.conf badge's, by the 256-color palette), the ink on it, a mark, and
/// words for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Badge {
    Waiting,
    Done,
    Working,
    /// Claude's turn ended with background tasks still running; it goes on
    /// when they end.
    Background,
    /// The background work is a loop: a review loop's Codex runs, a CI
    /// loop's watch on the checks, or queue-and-clean's watch on the queue.
    Review,
    Ci,
    Queue,
    /// A merged PR's tab waiting for its deploy, or testing it.
    Deploy,
    /// A self-paced /loop between its ticks.
    Scheduled,
    /// The background work is only Monitor watches.
    Monitor,
    Idle,
    /// Not Claude's: an idle tab whose PR is in its merge queue, or merged.
    Queued,
    Merged,
    /// Holding or waiting for the landing lock.
    Landing,
    /// An idle tab whose open PR has a failing check.
    Failing,
    /// Pinned by hand as needing you.
    Pinned,
    /// A reminder still to come, and one come due.
    Later,
    Reminder,
}

/// A badge's look.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BadgeLook {
    /// Its pill's color; none for idle, whose mark is faint.
    pub color: Option<u8>,
    pub ink: Option<u8>,
    pub mark: &'static str,
    pub words: &'static str,
}

/// Whether UX 2.0 is on, which draws every mark as a one-column symbol of
/// the terminal's font in place of an emoji (📌 ⏰ ⏳), so marks look alike
/// whatever emoji font the computer has, and take one column.
pub fn ux2() -> bool {
    crate::settings::is_on("ux2")
}

/// The mark for checks still running: ⏳, or ◌ with UX 2.0.
pub fn pending_mark() -> &'static str {
    if ux2() { "◌" } else { "⏳" }
}

impl Badge {
    pub fn look(self) -> BadgeLook {
        let l = |color: u8, ink: u8, mark, words| BadgeLook {
            color: Some(color),
            ink: Some(ink),
            mark,
            words,
        };
        match self {
            Badge::Waiting => l(220, 232, "!", "waiting on you"),
            Badge::Done => l(41, 232, "✓", "finished"),
            Badge::Working => l(67, 255, "⠋", "working"),
            Badge::Background => l(60, 255, "⠋", "working in background"),
            Badge::Review => l(97, 255, "⠋", "review loop"),
            Badge::Ci => l(30, 255, "⠋", "CI loop"),
            Badge::Queue => l(166, 255, "⠋", "queue-and-clean"),
            Badge::Deploy => l(133, 255, "⠋", "testing deploy"),
            Badge::Scheduled => l(31, 255, "↻", "looping"),
            Badge::Monitor => l(37, 232, "◉", "monitoring"),
            Badge::Idle => BadgeLook {
                color: None,
                ink: None,
                mark: "○",
                words: "",
            },
            Badge::Queued => l(173, 232, "»", "in merge queue"),
            Badge::Merged => l(135, 232, "◆", "merged"),
            Badge::Landing => l(179, 232, "⇣", "landing"),
            Badge::Failing => l(160, 255, "✗", "build failing"),
            Badge::Pinned => l(220, 232, if ux2() { "⚑" } else { "📌" }, "pinned"),
            Badge::Later => l(75, 232, if ux2() { "◷" } else { "⏰" }, "reminder"),
            Badge::Reminder => l(209, 232, if ux2() { "◷" } else { "⏰" }, "reminder"),
        }
    }

    /// Whether its mark is the spinner, turning while Claude works.
    pub fn spins(self) -> bool {
        matches!(
            self,
            Badge::Working
                | Badge::Background
                | Badge::Review
                | Badge::Ci
                | Badge::Queue
                | Badge::Deploy
        )
    }

    /// The pill's color, as a terminal color.
    pub fn color(self) -> Option<Color> {
        self.look().color.map(Color::Indexed)
    }

    /// The badge a loop's kind or a loop skill's name wears (`review`,
    /// `ci`, `queue`, `deploy`, `scheduled`); `None` for another name.
    pub fn of_loop(kind: &str) -> Option<Badge> {
        Some(match kind {
            "review" => Badge::Review,
            "ci" => Badge::Ci,
            "queue" => Badge::Queue,
            "deploy" => Badge::Deploy,
            "scheduled" => Badge::Scheduled,
            _ => return None,
        })
    }
}

/// `1st`, `2nd`, `3rd`, `11th`, as landing.js wrote a place in line.
pub fn ordinal(n: u32) -> String {
    let suffix = if (11..=13).contains(&(n % 100)) {
        "th"
    } else {
        match n % 10 {
            1 => "st",
            2 => "nd",
            3 => "rd",
            _ => "th",
        }
    };
    format!("{n}{suffix}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wide_characters_and_emoji_count_as_two_columns() {
        assert_eq!(width("abc"), 3);
        assert_eq!(width("⏳ ✅"), 5);
        assert_eq!(width("日本"), 4);
        assert_eq!(width("📌"), 2);
        assert_eq!(width("⏰ 3m"), 5);
        // A narrow symbol made emoji by U+FE0F is two, as terminals draw it.
        assert_eq!(width("⚠\u{fe0f}"), 2);
        assert_eq!(width("⚠"), 1);
        // Combining marks and joiners take none.
        assert_eq!(width("e\u{301}"), 1);
        assert_eq!(width("a\u{200d}b"), 2);
        // The marks the sidebar draws are one each.
        for mark in [
            "⠋", "✓", "✗", "◆", "»", "⇣", "○", "◉", "↻", "●", "═", "▌", "▏",
        ] {
            assert_eq!(width(mark), 1, "{mark}");
        }
    }

    #[test]
    fn cutting_ends_in_an_ellipsis_and_padding_fills_the_columns() {
        assert_eq!(cut("hello", 5), "hello");
        assert_eq!(cut("hello world", 6), "hello…");
        // A wide character that doesn't fit before the ellipsis is left out.
        assert_eq!(cut("ab日本", 4), "ab…");
        assert_eq!(cut("ab📌cd", 4), "ab…");
        assert_eq!(pad("ab", 4), "ab  ");
        assert_eq!(pad("abcdef", 4), "abc…");
        assert_eq!(width(&pad("日本語の名前", 7)), 7);
        assert_eq!(cut("abc", 0), "");
    }

    #[test]
    fn wrapping_breaks_between_words_and_splits_a_word_too_long() {
        assert_eq!(
            wrap("the quick brown fox", 10),
            vec!["the quick", "brown fox"]
        );
        assert_eq!(wrap("one\ntwo", 10), vec!["one", "two"]);
        assert_eq!(
            wrap("abcdefghijklmnop", 6),
            vec!["abcdef", "ghijkl", "mnop"]
        );
        assert_eq!(wrap("", 10), vec![""]);
        // Never narrower than four.
        assert_eq!(wrap("abcdef", 1), vec!["abcd", "ef"]);
    }

    #[test]
    fn a_markdown_table_is_drawn_in_a_box_or_a_cell_a_line_when_it_cant_fit() {
        let reply = "Today:\n\n| Time | Event | You |\n|---|:--|---|\n| 1:00–1:15 PM | Standup | not answered |\n| 1:15–2:00 PM | Community of Practicing Engineers | going |\n\nFree until 11.";
        assert_eq!(
            layout(reply, 80),
            vec![
                "Today:",
                "",
                "┌──────────────┬───────────────────────────────────┬──────────────┐",
                "│ Time         │ Event                             │ You          │",
                "├──────────────┼───────────────────────────────────┼──────────────┤",
                "│ 1:00–1:15 PM │ Standup                           │ not answered │",
                "├──────────────┼───────────────────────────────────┼──────────────┤",
                "│ 1:15–2:00 PM │ Community of Practicing Engineers │ going        │",
                "└──────────────┴───────────────────────────────────┴──────────────┘",
                "",
                "Free until 11.",
            ]
        );
        let narrow = layout(reply, 44);
        assert!(narrow.iter().all(|l| width(l) <= 44), "{narrow:#?}");
        assert!(narrow.contains(&"│ 1:15–2:00   │ Community of  │ going      │".to_string()));
        assert!(narrow.iter().any(|l| l.contains("│ Practicing ")));
        assert_eq!(
            layout(reply, 24)[2..6],
            [
                "Time: 1:00–1:15 PM",
                "Event: Standup",
                "You: not answered",
                "────────────────────"
            ]
        );
    }

    #[test]
    fn markdown_reads_as_plain_text() {
        assert_eq!(
            plain("# Title\n\n**bold** and `code`\n\n\n\n```sh\nls\n```"),
            "Title\n\nbold and code\n\nls"
        );
    }

    #[test]
    fn times_read_shortly() {
        let now = 1_000_000;
        assert_eq!(ago(None, now), "");
        assert_eq!(ago(Some(now - 10), now), "<1m");
        assert_eq!(ago(Some(now - 300), now), "5m");
        assert_eq!(ago(Some(now - 7200), now), "2h");
        assert_eq!(ago(Some(now - 3 * 86_400), now), "3d");
        assert_eq!(span(-5), "<1m");
        assert_eq!(span(59), "<1m");
        assert_eq!(span(12 * 60 + 30), "12m");
        assert_eq!(span(72 * 60), "1h12m");
        assert_eq!(span(2 * 3600 + 20), "2h");
        assert_eq!(span(2 * 86_400 + 3 * 3600 + 59), "2d3h");
        assert_eq!(span(86_400), "1d");
        let now_ms = now * 1000;
        assert_eq!(until(now + 45 * 60, now_ms), "in 45m");
        assert_eq!(until(now + 130 * 60, now_ms), "in 2h 10m");
        assert_eq!(until(now + 120 * 60, now_ms), "in 2h");
        assert_eq!(until(now + 3 * 86_400, now_ms), "in 3d");
        assert_eq!(until(now - 5, now_ms), "in 1m");
        assert_eq!(ordinal(1), "1st");
        assert_eq!(ordinal(2), "2nd");
        assert_eq!(ordinal(3), "3rd");
        assert_eq!(ordinal(11), "11th");
        assert_eq!(ordinal(22), "22nd");
    }
}
