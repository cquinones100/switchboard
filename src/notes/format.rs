//! The editing side of Notes' Format menu, and the typing it does for you,
//! ported from notes/lua/notes/format.lua.
//!
//! Paragraph styles change what a line starts with (render.rs has what each
//! looks like); applying the style a line already has makes it body text
//! again, as Notes does. Character styles wrap the selection, or the word
//! under the cursor, in their markers, and take them off again when they are
//! there already. Lists carry on when you press Enter in one, and stop when
//! you press it on an empty item; Tab and Shift-Tab indent and outdent an
//! item while you type.
//!
//! Each works on a `Doc`: the note's lines and the cursor of the window
//! showing them, as the Lua worked on a buffer and the current window.
//! Rows and byte columns count from 0.

use std::sync::LazyLock;

use regex::Regex;

use super::render::{self, Hl, Kind};
use super::table;
use super::text::Text;

/// Notes' "Move checked items to bottom": ticking an item moves it under
/// the ones still to do, so a list leads with what is left, a blank line
/// parting the two halves (`gap_before_checked`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Options {
    pub move_checked_to_bottom: bool,
    pub gap_before_checked: bool,
    /// The indent Tab gives an item (the config's `shiftwidth`).
    pub shiftwidth: usize,
}

impl Default for Options {
    fn default() -> Options {
        Options {
            move_checked_to_bottom: true,
            gap_before_checked: true,
            shiftwidth: 2,
        }
    }
}

/// An empty pair of markers put in for you to type into, taken out again
/// if you leave Insert mode without typing anything in it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pending {
    pub row: usize,
    pub col: usize,
    pub open: &'static str,
    pub close: &'static str,
}

/// A selection, for the styles that act on one: its first and last row,
/// the byte where it starts on the first and where it ends (exclusive) on
/// the last, and whether whole lines were selected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Selection {
    pub start: (usize, usize),
    pub end: (usize, usize),
    pub linewise: bool,
}

/// What Insert mode was asked for by an edit: at the cursor, or after the
/// line's end.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Insert {
    Here,
    End,
}

/// A note's lines and the cursor in them, as format, table and the editor
/// change them.
#[derive(Clone, Debug)]
pub struct Doc {
    pub text: Text,
    pub row: usize,
    pub col: usize,
    pub pending: Option<Pending>,
    /// Typing asked for by the edit just made.
    pub insert: Option<Insert>,
    pub opts: Options,
}

impl Doc {
    pub fn new(text: Text) -> Doc {
        Doc {
            text,
            row: 0,
            col: 0,
            pending: None,
            insert: None,
            opts: Options::default(),
        }
    }

    pub fn line(&self, row: usize) -> String {
        self.text.line(row)
    }

    pub fn set_line(&mut self, row: usize, text: &str) {
        self.text.set_line(row, text);
    }

    /// Asks for Insert mode: at the cursor, or after the line's end (Vim's
    /// `startinsert` and `startinsert!`).
    pub fn start_insert(&mut self, at_end: bool) {
        self.insert = Some(if at_end { Insert::End } else { Insert::Here });
    }
}

static MARKERS: LazyLock<[Regex; 6]> = LazyLock::new(|| {
    [
        Regex::new(r"^(#+\s+)").unwrap(),
        Regex::new(r"^([-*+]\s+\[[ xX]\]\s+)").unwrap(),
        Regex::new(r"^([-*+]\s+\[[ xX]\])$").unwrap(),
        Regex::new(r"^([-*+]\s+)").unwrap(),
        Regex::new(r"^(\d+[.)]\s+)").unwrap(),
        Regex::new(r"^(>\s?)").unwrap(),
    ]
});

/// A line's indent, the list or heading marker after it (with its space),
/// and the text after that.
pub fn split(line: &str) -> (&str, &str, &str) {
    let indent_len = render::indent_of(line);
    let (indent, rest) = line.split_at(indent_len);
    for pattern in MARKERS.iter() {
        if let Some(m) = pattern.find(rest) {
            let (marker, text) = rest.split_at(m.end());
            return (indent, marker, text);
        }
    }
    (indent, "", rest)
}

/// The paragraph styles, by name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Paragraph {
    Title,
    Heading,
    Subheading,
    Body,
    Bullet,
    Dash,
    Number,
    Check,
    Quote,
}

impl Paragraph {
    /// The marker each starts a line with.
    pub fn marker(self) -> &'static str {
        match self {
            Paragraph::Title => "# ",
            Paragraph::Heading => "## ",
            Paragraph::Subheading => "### ",
            Paragraph::Body => "",
            Paragraph::Bullet => "* ",
            Paragraph::Dash => "- ",
            Paragraph::Number => "1. ",
            Paragraph::Check => "- [ ] ",
            Paragraph::Quote => "> ",
        }
    }

    fn of(marker: &str) -> Paragraph {
        let b = marker.as_bytes();
        let hashes = b.iter().take_while(|c| **c == b'#').count();
        match hashes {
            1 => return Paragraph::Title,
            2 => return Paragraph::Heading,
            n if n >= 3 => return Paragraph::Subheading,
            _ => {}
        }
        if marker.contains("[ ]") || marker.contains("[x]") || marker.contains("[X]") {
            Paragraph::Check
        } else if matches!(b.first(), Some(b'*' | b'+')) {
            Paragraph::Bullet
        } else if b.first() == Some(&b'-') {
            Paragraph::Dash
        } else if b.first().is_some_and(u8::is_ascii_digit) {
            Paragraph::Number
        } else if b.first() == Some(&b'>') {
            Paragraph::Quote
        } else {
            Paragraph::Body
        }
    }
}

/// Gives lines `first..=last` a paragraph style, or body text when the
/// first has it already. A numbered list is numbered from 1; headings have
/// no indent. A cursor left on a marker goes to the text, so typing starts
/// there.
pub fn paragraph(doc: &mut Doc, first: usize, last: usize, style: Paragraph) {
    let first_line = doc.line(first);
    let style = if Paragraph::of(split(&first_line).1) == style {
        Paragraph::Body
    } else {
        style
    };
    for (n, row) in (first..=last).enumerate() {
        let line = doc.line(row);
        let (indent, _, text) = split(&line);
        let marker = if style == Paragraph::Number {
            format!("{}. ", n + 1)
        } else {
            style.marker().to_string()
        };
        let indent = if matches!(
            style,
            Paragraph::Title | Paragraph::Heading | Paragraph::Subheading
        ) {
            ""
        } else {
            indent
        };
        let new = format!("{indent}{marker}{text}");
        doc.set_line(row, &new);
    }
    if doc.row >= first && doc.row <= last {
        let line = doc.line(doc.row);
        let start = text_start(&line);
        if doc.col < start {
            doc.col = start.min(line.len().saturating_sub(1));
        }
    }
}

/// Where a line's text starts: after its indent and its list, checklist,
/// quote or heading marker.
pub fn text_start(line: &str) -> usize {
    let (indent, marker, _) = split(line);
    indent.len() + marker.len()
}

/// Where `i`, `a` or `I` starts typing on a line with a marker, which shows
/// as one symbol (☐, •, ▎) or not at all (#) but is several characters in
/// the file: never inside it. `None` for the key to do what it always does.
pub fn insert_point(line: &str, col: usize, key: char) -> Option<usize> {
    let start = text_start(line);
    if start == 0 {
        return None;
    }
    match key {
        'I' => Some(start),
        'i' if col < start => Some(start),
        'a' if col + 1 < start => Some(start),
        _ => None,
    }
}

/// Monospaced: the lines go between ``` fences, or come out of the block
/// they are in.
pub fn monospaced(doc: &mut Doc, first: usize, last: usize) {
    let lines = doc.text.lines();
    let kinds = render::kinds(&lines);
    if matches!(kinds[first], Kind::Code | Kind::Fence) {
        let mut open = first;
        while open > 0 && kinds[open] != Kind::Fence {
            open -= 1;
        }
        if kinds[first] == Kind::Fence && kinds.get(first + 1) == Some(&Kind::Code) {
            open = first;
        }
        let mut close = open + 1;
        while close < lines.len() && kinds[close] != Kind::Fence {
            close += 1;
        }
        if close < lines.len() {
            doc.text.set_lines(close, close + 1, &[] as &[&str]);
        }
        doc.text.set_lines(open, open + 1, &[] as &[&str]);
        return;
    }
    doc.text.set_lines(last + 1, last + 1, &["```"]);
    doc.text.set_lines(first, first, &["```"]);
}

// ── Character styles ────────────────────────────────────────────────────────

/// The character styles, each with its markers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Character {
    Bold,
    Italic,
    Underline,
    Strike,
    Highlight,
    Code,
}

impl Character {
    pub fn markers(self) -> (&'static str, &'static str) {
        match self {
            Character::Bold => ("**", "**"),
            Character::Italic => ("*", "*"),
            Character::Underline => ("<u>", "</u>"),
            Character::Strike => ("~~", "~~"),
            Character::Highlight => ("==", "=="),
            Character::Code => ("`", "`"),
        }
    }

    /// The highlight render.rs gives its text, to find it by.
    fn hl(self) -> Hl {
        match self {
            Character::Bold => Hl::Bold,
            Character::Italic => Hl::Italic,
            Character::Underline => Hl::Underline,
            Character::Strike => Hl::Strike,
            Character::Highlight => Hl::Highlight,
            Character::Code => Hl::Mono,
        }
    }
}

/// Wraps bytes `s..e` of a row in a style's markers, or unwraps them when
/// they are wrapped already; gives where the text starts then.
pub fn wrap(doc: &mut Doc, row: usize, s: usize, e: usize, style: Character) -> usize {
    let (open, close) = style.markers();
    let line = doc.line(row);
    let (s, e) = (s.min(line.len()), e.min(line.len()));
    let (before, inside, after) = (&line[..s], &line[s..e], &line[e..]);
    // A one-character marker that is part of a longer run, as the * of
    // **bold** is, isn't this style's.
    let (bb, ab) = (before.as_bytes(), after.as_bytes());
    let single = open.len() == 1
        && (bb.len() >= 2 && bb[bb.len() - 2] == open.as_bytes()[0]
            || ab.len() >= 2 && ab[1] == close.as_bytes()[0]);
    if before.ends_with(open) && after.starts_with(close) && !single {
        let new = format!(
            "{}{inside}{}",
            &before[..before.len() - open.len()],
            &after[close.len()..]
        );
        doc.set_line(row, &new);
        return s - open.len();
    }
    if inside.len() >= open.len() + close.len()
        && inside.starts_with(open)
        && inside.ends_with(close)
    {
        let new = format!(
            "{before}{}{after}",
            &inside[open.len()..inside.len() - close.len()]
        );
        doc.set_line(row, &new);
        return s;
    }
    doc.set_line(row, &format!("{before}{open}{inside}{close}{after}"));
    s + open.len()
}

fn is_word(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'_' | b'\'' | b'-')
}

/// The word around a byte, as columns, the end exclusive.
pub fn word_at(line: &str, col: usize) -> (usize, usize) {
    let b = line.as_bytes();
    let (mut s, mut e) = (col.min(b.len()), col.min(b.len()));
    while s > 0 && is_word(b[s - 1]) {
        s -= 1;
    }
    while e < b.len() && is_word(b[e]) {
        e += 1;
    }
    (s, e)
}

/// Takes the empty pair of markers out, if it is still there and empty.
pub fn drop_empty(doc: &mut Doc) {
    let Some(p) = doc.pending.take() else {
        return;
    };
    let line = doc.line(p.row);
    let pair = format!("{}{}", p.open, p.close);
    if line.get(p.col..p.col + pair.len()) == Some(pair.as_str()) {
        let new = format!("{}{}", &line[..p.col], &line[p.col + pair.len()..]);
        doc.set_line(p.row, &new);
    }
}

/// A character style, as Notes applies one:
/// - on a selection (charwise, or every line of a linewise one, each from
///   its text's start);
/// - with the cursor in text that has the style already, taking it off that
///   whole run of text;
/// - with the cursor on a word, on that word;
/// - anywhere else (a space, an empty line, just after a word typed), on
///   what you type next: its markers go in at the cursor, and Insert mode
///   starts between them.
///
/// `typing_col` is where you were typing when this came from Insert mode,
/// which then carries on in Insert mode.
pub fn character(
    doc: &mut Doc,
    style: Character,
    selection: Option<Selection>,
    typing_col: Option<usize>,
) {
    if let Some(sel) = selection {
        for row in sel.start.0..=sel.end.0 {
            let line = doc.line(row);
            let start = text_start(&line);
            let from = if !sel.linewise && row == sel.start.0 {
                sel.start.1
            } else {
                start
            };
            let to = if !sel.linewise && row == sel.end.0 {
                sel.end.1.min(line.len())
            } else {
                line.len()
            };
            if to > from {
                wrap(doc, row, from, to, style);
            }
        }
        return;
    }
    let row = doc.row;
    let col = doc.col;
    let line = doc.line(row);
    let caret = typing_col.unwrap_or(col);
    let carry_on = |doc: &mut Doc, at: usize| {
        doc.row = row;
        doc.col = at;
        if typing_col.is_some() {
            let len = doc.line(row).len();
            doc.start_insert(at >= len);
        }
    };
    // In text that has the style: off, for the whole run.
    for span in render::inline(&line, 0) {
        if span.hl != style.hl() || span.conceal.len() != 2 {
            continue;
        }
        let (open, close) = (span.conceal[0], span.conceal[1]);
        if caret >= open.0 && caret < close.1 {
            let new = format!(
                "{}{}{}",
                &line[..open.0],
                &line[open.1..close.0],
                &line[close.1..]
            );
            doc.set_line(row, &new);
            // The cursor stays on the same text: the opening marker is gone
            // from before it, and a cursor on a marker goes to the edge.
            let opened = open.1 - open.0;
            let at = if caret < open.1 {
                open.0
            } else if caret <= close.0 {
                caret - opened
            } else {
                close.0 - opened
            };
            return carry_on(doc, at);
        }
    }
    // On a word: that word. Typing, the cursor has to be inside it, not
    // just after it, since that is where the next letter goes.
    let b = line.as_bytes();
    let word_byte = |i: Option<usize>| i.and_then(|i| b.get(i)).is_some_and(|c| is_word(*c));
    let on_word = match typing_col {
        Some(_) => word_byte(caret.checked_sub(1)) && word_byte(Some(caret)),
        None => word_byte(Some(col)),
    };
    if on_word {
        let (s, e) = word_at(&line, if typing_col.is_some() { caret - 1 } else { col });
        let at = wrap(doc, row, s, e, style);
        let to = if typing_col.is_some() {
            caret + (at - s)
        } else {
            at
        };
        return carry_on(doc, to);
    }
    // Anywhere else: what you type next.
    let (open, close) = style.markers();
    let at = typing_col
        .unwrap_or(if line.is_empty() { 0 } else { col + 1 })
        .min(line.len());
    let new = format!("{}{open}{close}{}", &line[..at], &line[at..]);
    doc.set_line(row, &new);
    doc.pending = Some(Pending {
        row,
        col: at,
        open,
        close,
    });
    doc.row = row;
    doc.col = at + open.len();
    doc.start_insert(false);
}

/// A link to `url` on the selection or the word under the cursor; with
/// nothing under the cursor, the address is the link's text too.
pub fn link(doc: &mut Doc, selection: Option<Selection>, url: &str) {
    let (row, s, e) = match selection {
        Some(sel) => {
            let len = doc.line(sel.start.0).len();
            let e = if sel.start.0 == sel.end.0 && !sel.linewise {
                sel.end.1.min(len)
            } else {
                len
            };
            let s = if sel.linewise { 0 } else { sel.start.1 };
            (sel.start.0, s, e)
        }
        None => {
            let (s, e) = word_at(&doc.line(doc.row), doc.col);
            (doc.row, s, e)
        }
    };
    let line = doc.line(row);
    let text = if e > s { &line[s..e] } else { url };
    let new = format!("{}[{text}]({url}){}", &line[..s], &line[e..]);
    doc.set_line(row, &new);
}

// ── Checklists ──────────────────────────────────────────────────────────────

fn is_check(line: &str) -> bool {
    matches!(render::line_kind(line, false), Kind::Check { .. })
}

fn is_done(line: &str) -> bool {
    render::line_kind(line, false).done()
}

fn has_text(line: &str) -> bool {
    !line.trim().is_empty()
}

/// The rows of the checklist around `row`: the run of checklist items either
/// side of it, with whatever is indented under them (an item's details and
/// sub-items), starting and ending on an item. One blank line before ticked
/// items is the gap sorting leaves, and the list goes on across it.
fn checklist_bounds(lines: &[String], row: usize) -> Option<(usize, usize)> {
    let at = |r: isize| (r >= 0).then(|| lines.get(r as usize)).flatten();
    let part =
        |r: isize| at(r).is_some_and(|l| is_check(l) || (has_text(l) && render::indent_of(l) > 0));
    let blank = |r: isize| at(r).is_some_and(|l| !has_text(l));
    let ticked = |r: isize| at(r).is_some_and(|l| is_done(l));
    let row = row as isize;
    if !part(row) {
        return None;
    }
    let (mut first, mut last) = (row, row);
    let mut grew = true;
    while grew {
        grew = false;
        while part(first - 1) {
            first -= 1;
            grew = true;
        }
        while part(last + 1) {
            last += 1;
            grew = true;
        }
        if blank(last + 1) && ticked(last + 2) {
            last += 2;
            grew = true;
        }
        if ticked(first) && blank(first - 1) && part(first - 2) {
            first -= 2;
            grew = true;
        }
    }
    while first <= last && !is_check(&lines[first as usize]) {
        first += 1;
    }
    // What is indented under the last item (an image pasted under it, say)
    // is the item's, and moves with it.
    while last >= first
        && !is_check(&lines[last as usize])
        && render::indent_of(&lines[last as usize]) == 0
    {
        last -= 1;
    }
    (first <= last).then_some((first as usize, last as usize))
}

/// Lines of a list, ticked items under the rest at every level: each item
/// moves with everything indented under it, and each group keeps its order.
/// Lines before the first item at the shallowest level stay first.
fn sorted(lines: &[String], top: bool, gap: bool) -> Vec<String> {
    let lines: Vec<String> = if top {
        lines.iter().filter(|l| has_text(l)).cloned().collect()
    } else {
        lines.to_vec()
    };
    let Some(base) = lines
        .iter()
        .filter(|l| is_check(l))
        .map(|l| render::indent_of(l))
        .min()
    else {
        return lines;
    };
    let mut head = Vec::new();
    let mut items: Vec<Vec<String>> = Vec::new();
    for line in &lines {
        if is_check(line) && render::indent_of(line) <= base {
            items.push(vec![line.clone()]);
        } else if let Some(item) = items.last_mut() {
            item.push(line.clone());
        } else {
            head.push(line.clone());
        }
    }
    let (mut todo, mut done) = (Vec::new(), Vec::new());
    for item in items {
        let mut whole = vec![item[0].clone()];
        whole.extend(sorted(&item[1..], false, gap));
        if is_done(&item[0]) {
            done.push(whole);
        } else {
            todo.push(whole);
        }
    }
    let mut out = head;
    let both = !todo.is_empty() && !done.is_empty();
    out.extend(todo.into_iter().flatten());
    if top && gap && both {
        out.push(String::new());
    }
    out.extend(done.into_iter().flatten());
    out
}

/// Moves the ticked items of the checklist around `row` under the rest.
pub fn sort_checked(doc: &mut Doc, row: usize) {
    let lines = doc.text.lines();
    let Some((first, last)) = checklist_bounds(&lines, row) else {
        return;
    };
    let part = &lines[first..=last];
    let out = sorted(part, true, doc.opts.gap_before_checked);
    if out != part {
        doc.text.set_lines(first, last + 1, &out);
    }
}

/// Ticks or unticks the item on `row`; whether it was an item.
pub fn toggle_check(doc: &mut Doc, row: usize) -> bool {
    let line = doc.line(row);
    let Kind::Check { indent, .. } = render::line_kind(&line, false) else {
        return false;
    };
    let Some(open) = line[indent..].find('[').map(|i| i + indent) else {
        return false;
    };
    let mark = if line.as_bytes()[open + 1] == b' ' {
        "x"
    } else {
        " "
    };
    let new = format!("{}{mark}{}", &line[..open + 1], &line[open + 2..]);
    doc.set_line(row, &new);
    if doc.opts.move_checked_to_bottom {
        sort_checked(doc, row);
    }
    true
}

/// Takes out the blank lines sorting left before ticked items, for a change
/// that leaves no ticked items there.
fn close_gaps(doc: &mut Doc) {
    let lines = doc.text.lines();
    for row in (1..lines.len().saturating_sub(1)).rev() {
        let (above, below) = (&lines[row - 1], &lines[row + 1]);
        if !has_text(&lines[row])
            && is_done(below)
            && (is_check(above) || (has_text(above) && render::indent_of(above) > 0))
        {
            doc.text.set_lines(row, row + 1, &[] as &[&str]);
        }
    }
}

/// The rows that are checklist items, from the bottom up.
fn checks_from_bottom(doc: &Doc) -> Vec<(usize, Kind)> {
    let lines = doc.text.lines();
    let kinds = render::kinds(&lines);
    (0..lines.len())
        .rev()
        .filter(|&r| matches!(kinds[r], Kind::Check { .. }))
        .map(|r| (r, kinds[r]))
        .collect()
}

/// Notes' Uncheck All, for the whole note.
pub fn uncheck_all(doc: &mut Doc) {
    close_gaps(doc);
    for (row, kind) in checks_from_bottom(doc) {
        if let Kind::Check {
            indent, done: true, ..
        } = kind
        {
            let line = doc.line(row);
            if let Some(open) = line[indent..].find('[').map(|i| i + indent) {
                let new = format!("{} {}", &line[..open + 1], &line[open + 2..]);
                doc.set_line(row, &new);
            }
        }
    }
}

/// Notes' Delete Checked Items: each ticked item with what is indented
/// under it (an image pasted there, say), bottom up.
pub fn delete_checked(doc: &mut Doc) {
    close_gaps(doc);
    for (row, kind) in checks_from_bottom(doc) {
        if !kind.done() {
            continue;
        }
        let lines = doc.text.lines();
        let own = render::indent_of(&lines[row]);
        let under = lines[row + 1..]
            .iter()
            .take_while(|l| has_text(l) && render::indent_of(l) > own && !is_check(l))
            .count();
        doc.text.set_lines(row, row + 1 + under, &[] as &[&str]);
    }
    doc.row = doc.row.min(doc.text.len() - 1);
}

// ── Typing ──────────────────────────────────────────────────────────────────

/// Enter in Insert mode: in a list, a new item of the same kind (numbered
/// on, unticked), or, on an empty item, the end of the list. False when the
/// line isn't a list item, for Enter to do what it always does.
pub fn enter(doc: &mut Doc) -> bool {
    let (row, col) = (doc.row, doc.col);
    let line = doc.line(row);
    let kind = render::line_kind(&line, false);
    if !(kind.is_list() || matches!(kind, Kind::Quote { .. })) {
        return false;
    }
    let (indent, marker, text) = split(&line);
    if text.trim().is_empty() {
        doc.set_line(row, "");
        doc.col = 0;
        return true;
    }
    let next_marker = match kind {
        Kind::Check { .. } => marker.replace("[x]", "[ ]").replace("[X]", "[ ]"),
        Kind::Number { .. } => {
            let digits: String = marker.chars().take_while(char::is_ascii_digit).collect();
            let sep = marker[digits.len()..].chars().next().unwrap_or('.');
            format!("{}{sep} ", digits.parse::<u64>().unwrap_or(0) + 1)
        }
        _ => marker.to_string(),
    };
    let col = col.min(line.len());
    let (before, after) = line.split_at(col);
    let prefix = format!("{indent}{next_marker}");
    let new = [before.to_string(), format!("{prefix}{}", after.trim())];
    doc.text.set_lines(row, row + 1, &new);
    doc.row = row + 1;
    doc.col = prefix.len();
    if matches!(kind, Kind::Number { .. }) {
        renumber(doc, row + 1);
    }
    true
}

static LEADING_NUMBER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(\s*)\d+").unwrap());

/// Numbers the numbered list around `row` 1, 2, 3… at each indent.
pub fn renumber(doc: &mut Doc, row: usize) {
    let lines = doc.text.lines();
    let kinds = render::kinds(&lines);
    if !matches!(kinds[row], Kind::Number { .. }) {
        return;
    }
    let indented = |r: usize| {
        let l = &lines[r];
        render::indent_of(l) > 0 && has_text(l)
    };
    let mut first = row;
    while first > 0
        && (matches!(kinds[first - 1], Kind::Number { .. })
            || (indented(first - 1) && kinds[first - 1] != Kind::Body))
    {
        first -= 1;
    }
    let mut counts: Vec<(usize, usize)> = Vec::new();
    for r in first..lines.len() {
        let k = kinds[r];
        let keeps = indented(r) && !matches!(k, Kind::Body | Kind::Title { .. });
        if !matches!(k, Kind::Number { .. }) && !keeps {
            break;
        }
        if let Kind::Number { indent, .. } = k {
            counts.retain(|(i, _)| *i <= indent);
            let n = match counts.iter_mut().find(|(i, _)| *i == indent) {
                Some((_, n)) => {
                    *n += 1;
                    *n
                }
                None => {
                    counts.push((indent, 1));
                    1
                }
            };
            let new = LEADING_NUMBER
                .replace(&lines[r], format!("${{1}}{n}"))
                .into_owned();
            if new != lines[r] {
                doc.set_line(r, &new);
            }
        }
    }
}

/// Tab and Shift-Tab in Insert mode: a table's next or previous cell, or a
/// list item indented or outdented. False otherwise.
pub fn tab(doc: &mut Doc, step: i32) -> bool {
    let (row, col) = (doc.row, doc.col);
    let line = doc.line(row);
    let kind = render::line_kind(&line, false);
    if matches!(kind, Kind::Table { .. }) {
        return table::jump(doc, step);
    }
    if !kind.is_list() {
        return false;
    }
    let width = doc.opts.shiftwidth;
    if step > 0 {
        doc.set_line(row, &format!("{}{line}", " ".repeat(width)));
        doc.col = col + width;
    } else {
        let removed = width.min(render::indent_of(&line));
        if removed == 0 {
            return true;
        }
        doc.set_line(row, &line[removed..]);
        doc.col = col.saturating_sub(removed);
    }
    if matches!(kind, Kind::Number { .. }) {
        renumber(doc, row);
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(lines: &[&str]) -> Doc {
        Doc::new(Text::from_lines(lines))
    }

    #[test]
    fn paragraph_styles_replace_a_lines_marker_and_character_styles_wrap() {
        let mut d = doc(&["  - [ ] eggs", "milk", "bread"]);
        paragraph(&mut d, 0, 2, Paragraph::Number);
        assert_eq!(
            d.text.lines(),
            ["  1. eggs", "2. milk", "3. bread"],
            "numbered from 1, indent kept"
        );
        paragraph(&mut d, 0, 2, Paragraph::Number);
        assert_eq!(
            d.text.lines(),
            ["  eggs", "milk", "bread"],
            "the same style again is body text"
        );
        paragraph(&mut d, 1, 1, Paragraph::Heading);
        assert_eq!(d.line(1), "## milk");
        paragraph(&mut d, 2, 2, Paragraph::Quote);
        assert_eq!(d.line(2), "> bread");
        let mut d = doc(&["say hello world"]);
        d.col = 5;
        character(&mut d, Character::Bold, None, None);
        assert_eq!(
            d.line(0),
            "say **hello** world",
            "the word under the cursor"
        );
        d.col = 7;
        character(&mut d, Character::Bold, None, None);
        assert_eq!(d.line(0), "say hello world", "and off again");
        wrap(&mut d, 0, 10, 15, Character::Underline);
        assert_eq!(d.line(0), "say hello <u>world</u>");
        monospaced(&mut d, 0, 0);
        assert_eq!(d.text.lines(), ["```", "say hello <u>world</u>", "```"]);
        monospaced(&mut d, 1, 1);
        assert_eq!(
            d.text.lines(),
            ["say hello <u>world</u>"],
            "monospaced again takes the block out"
        );
    }

    #[test]
    fn a_character_style_works_as_in_notes() {
        let at = |col: usize, text: &str| {
            let mut d = doc(&[text]);
            d.col = col;
            d
        };
        let snap = |d: &Doc| (d.line(0), d.col, d.pending.as_ref().map(|p| p.col));
        let mut d = at(6, "alpha beta gamma");
        character(&mut d, Character::Bold, None, None);
        assert_eq!(snap(&d).0, "alpha **beta** gamma");
        assert_eq!(d.col, 8);
        let mut d = at(8, "alpha **beta** gamma");
        character(&mut d, Character::Bold, None, None);
        assert_eq!(
            (d.line(0), d.col),
            ("alpha beta gamma".into(), 6),
            "on bold text, Bold takes it off, the cursor staying on the word"
        );
        let mut d = at(7, "alpha **two words** x");
        character(&mut d, Character::Bold, None, None);
        assert_eq!(
            d.line(0),
            "alpha two words x",
            "for the whole run, not a word of it"
        );
        let mut d = at(8, "alpha **beta** gamma");
        character(&mut d, Character::Italic, None, None);
        assert_eq!(
            d.line(0),
            "alpha ***beta*** gamma",
            "italic on a bold word adds to it"
        );
        let mut d = at(5, "alpha beta");
        character(&mut d, Character::Strike, None, None);
        assert_eq!(
            snap(&d),
            ("alpha ~~~~beta".into(), 8, Some(6)),
            "markers after the space, the cursor between them"
        );
        assert_eq!(d.insert, Some(Insert::Here));
        let mut d = at(0, "");
        character(&mut d, Character::Underline, None, None);
        assert_eq!(snap(&d), ("<u></u>".into(), 3, Some(0)));
        drop_empty(&mut d);
        assert_eq!(d.line(0), "", "nothing typed in them: they go");
        let mut d = at(4, "type");
        character(&mut d, Character::Highlight, None, Some(4));
        assert_eq!(
            snap(&d),
            ("type====".into(), 6, Some(4)),
            "right after a word you typed, it is for what you type next"
        );
        let mut d = at(2, "type");
        character(&mut d, Character::Code, None, Some(2));
        assert_eq!(
            (d.line(0), d.col),
            ("`type`".into(), 3),
            "inside a word, the word, the cursor where it was in it"
        );
        let mut d = doc(&["one two"]);
        let sel = Selection {
            start: (0, 0),
            end: (0, 7),
            linewise: false,
        };
        character(&mut d, Character::Italic, Some(sel), None);
        assert_eq!(d.line(0), "*one two*", "the selection");
    }

    #[test]
    fn typing_starts_after_a_lines_marker() {
        let mut d = doc(&["", "milk"]);
        paragraph(&mut d, 0, 0, Paragraph::Check);
        let empty = (d.row, d.col);
        d.row = 1;
        d.col = 0;
        paragraph(&mut d, 1, 1, Paragraph::Check);
        assert_eq!(d.text.lines(), ["- [ ] ", "- [ ] milk"]);
        assert_eq!(
            empty,
            (0, 5),
            "on the item's last character, so a starts typing at its end"
        );
        assert_eq!((d.row, d.col), (1, 6), "on its text");
        assert_eq!(
            insert_point("- [ ] milk", 0, 'a'),
            Some(6),
            "a on the ☐ types after it"
        );
        assert_eq!(insert_point("- [ ] milk", 2, 'i'), Some(6));
        assert_eq!(
            insert_point("  * milk", 5, 'I'),
            Some(4),
            "I goes past the marker, not to the bullet"
        );
        assert_eq!(
            insert_point("- [ ] milk", 7, 'a'),
            None,
            "in the text, a is a as always"
        );
        assert_eq!(
            insert_point("milk", 0, 'I'),
            None,
            "and a line without a marker is left alone"
        );
        assert_eq!(insert_point("## Plan", 0, 'a'), Some(3));
    }

    #[test]
    fn checklists_tick_move_down_uncheck_all_and_delete_checked() {
        let mut d = doc(&[
            "# List",
            "- [ ] eggs",
            "- [ ] milk",
            "- [ ] bread",
            "",
            "after",
        ]);
        assert!(
            Options::default().move_checked_to_bottom,
            "a list leads with what is left unless turned off"
        );
        d.opts.move_checked_to_bottom = false;
        assert!(!toggle_check(&mut d, 0));
        toggle_check(&mut d, 1);
        assert_eq!(
            &d.text.lines()[1..4],
            ["- [x] eggs", "- [ ] milk", "- [ ] bread"]
        );
        d.opts.move_checked_to_bottom = true;
        toggle_check(&mut d, 2);
        assert_eq!(
            &d.text.lines()[1..5],
            ["- [ ] bread", "", "- [x] eggs", "- [x] milk"],
            "ticked items under the rest, in order, a blank line between"
        );
        uncheck_all(&mut d);
        assert_eq!(
            &d.text.lines()[1..5],
            ["- [ ] bread", "- [ ] eggs", "- [ ] milk", ""],
            "the gap goes with the ticks"
        );
        toggle_check(&mut d, 1);
        delete_checked(&mut d);
        assert_eq!(
            d.text.lines(),
            ["# List", "- [ ] eggs", "- [ ] milk", "", "after"]
        );
    }

    #[test]
    fn a_ticked_item_moves_with_its_details_and_sub_items() {
        let mut d = doc(&[
            "# Trip",
            "- [ ] book flights",
            "  compare two airlines",
            "  - [ ] outbound",
            "  - [ ] return",
            "- [ ] pack",
            "",
            "after",
        ]);
        toggle_check(&mut d, 3);
        assert_eq!(
            d.text.lines(),
            [
                "# Trip",
                "- [ ] book flights",
                "  compare two airlines",
                "  - [ ] return",
                "  - [x] outbound",
                "- [ ] pack",
                "",
                "after"
            ],
            "a sub-item sorts under its item, its details first"
        );
        toggle_check(&mut d, 1);
        assert_eq!(
            d.text.lines(),
            [
                "# Trip",
                "- [ ] pack",
                "",
                "- [x] book flights",
                "  compare two airlines",
                "  - [ ] return",
                "  - [x] outbound",
                "",
                "after"
            ],
            "an item moves with everything under it"
        );
    }

    #[test]
    fn the_gap_before_ticked_items_keeps_the_list_one_list() {
        let mut d = doc(&[
            "# List", "- [ ] a", "- [ ] b", "", "- [x] c", "  why c", "  - more", "", "after",
        ]);
        toggle_check(&mut d, 1);
        assert_eq!(
            d.text.lines(),
            [
                "# List", "- [ ] b", "", "- [x] a", "- [x] c", "  why c", "  - more", "", "after"
            ],
            "ticked across the gap joins the ticked half"
        );
    }

    #[test]
    fn an_image_under_an_item_moves_with_it_and_goes_when_it_is_deleted() {
        let mut d = doc(&[
            "- [ ] first",
            "- [ ] second",
            "  ![shot](.attachments/Bugs/shot.png)",
        ]);
        toggle_check(&mut d, 0);
        assert_eq!(
            d.text.lines(),
            [
                "- [ ] second",
                "  ![shot](.attachments/Bugs/shot.png)",
                "",
                "- [x] first"
            ],
            "the image stays under its item"
        );
        let mut d = doc(&[
            "- [ ] keep",
            "",
            "- [x] done",
            "  ![old](.attachments/Bugs/old.png)",
        ]);
        delete_checked(&mut d);
        assert_eq!(
            d.text.lines(),
            ["- [ ] keep"],
            "a ticked item deleted with its image line"
        );
    }

    #[test]
    fn enter_carries_a_list_on_and_tab_indents_and_renumbers() {
        let mut d = doc(&["- [x] eggs"]);
        d.col = 10;
        assert!(enter(&mut d));
        assert_eq!(
            d.text.lines(),
            ["- [x] eggs", "- [ ] "],
            "a new item, unticked"
        );
        assert_eq!((d.row, d.col), (1, 6));
        enter(&mut d);
        assert_eq!(
            d.text.lines(),
            ["- [x] eggs", ""],
            "Enter on an empty item ends the list"
        );
        let mut d = doc(&["1. one", "2. two"]);
        d.col = 5;
        enter(&mut d);
        assert_eq!(
            d.text.lines(),
            ["1. on", "2. e", "3. two"],
            "split at the cursor, and the rest renumbered"
        );
        d.row = 1;
        d.col = 3;
        tab(&mut d, 1);
        assert_eq!(
            d.text.lines(),
            ["1. on", "  1. e", "2. two"],
            "indented, it starts its own numbering"
        );
        let mut d = doc(&["plain"]);
        assert!(!enter(&mut d), "not a list: Enter does what it always does");
    }
}
