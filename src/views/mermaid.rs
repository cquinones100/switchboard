//! Mermaid diagrams drawn as text, for the Markdown views, where pr-view.js
//! ran mermaid-ascii: flowcharts (`flowchart` or `graph`, any direction) and
//! sequence diagrams, in box-drawing characters. Any other kind (a pie, a
//! class or state diagram, a Gantt chart) gives `None`, and the view keeps
//! its source with a line saying where it is shown properly.
//!
//! A flowchart is laid out in layers, as mermaid-ascii and Mermaid itself
//! lay one out: each node a layer below the longest chain of nodes leading
//! to it, an edge that loops back laid out the other way round with its
//! arrow at the top, an edge across several layers passing through a point
//! in each, and each layer's nodes ordered and placed under the nodes they
//! come from, so a chain draws straight and a fork branches either side.
//! Edges run from the middle of a box's edge, across the gap between two
//! layers on a lane of their own source's, to the middle of the next box's,
//! where the arrow is; a label sits on the edge just before its arrow.
//! Everything is laid out along the diagram's direction and across it, and
//! turned to the screen at the end, so top-down, bottom-up, left-to-right
//! and right-to-left are one layout. A left-to-right flowchart wider than
//! the pane is drawn top to bottom instead, as pr-view.js did, which is as
//! wide as its widest box; one still wider is left whole, for the pager to
//! cut at its edge.
//!
//! A sequence diagram is its participants in boxes along the top, a
//! lifeline down from each, and each message an arrow from one lifeline to
//! another under its text, dotted for a reply (`-->>`), a message to itself
//! a loop to the right; notes are boxes, and a loop's or an alternative's
//! start and end are lines of their own.

use std::collections::HashMap;
use std::sync::LazyLock;

use regex::Regex;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// The diagram drawn as text, or `None` for a kind that can't be.
/// `max_width` is the pane's text width, past which a left-to-right
/// flowchart is drawn top to bottom.
pub fn draw(source: &str, max_width: usize) -> Option<String> {
    let lines = statements(source);
    let (head, rest) = lines.split_first()?;
    let mut words = head.split_whitespace();
    match words.next()? {
        "flowchart" | "graph" => {
            let dir = match words.next().map(str::to_ascii_uppercase).as_deref() {
                Some("LR") => Dir::LeftRight,
                Some("RL") => Dir::RightLeft,
                Some("BT") => Dir::BottomUp,
                _ => Dir::TopDown,
            };
            let graph = Graph::parse(rest);
            if graph.nodes.is_empty() {
                return None;
            }
            let drawn = graph.draw(dir);
            let sideways = matches!(dir, Dir::LeftRight | Dir::RightLeft);
            if sideways && widest(&drawn) > max_width {
                return Some(graph.draw(Dir::TopDown));
            }
            Some(drawn)
        }
        "sequenceDiagram" => Sequence::parse(rest).map(|s| s.draw()),
        _ => None,
    }
}

fn width(text: &str) -> usize {
    UnicodeWidthStr::width(text)
}

fn widest(text: &str) -> usize {
    text.lines().map(width).max().unwrap_or(0)
}

/// The diagram's lines with comments, front matter and `%%{init}%%`
/// directives taken out, trimmed, blank ones dropped.
fn statements(source: &str) -> Vec<String> {
    let mut lines: Vec<&str> = source.lines().collect();
    // Front matter (`---` ... `---`) holds the diagram's title and config.
    if lines.first().is_some_and(|l| l.trim() == "---")
        && let Some(end) = lines.iter().skip(1).position(|l| l.trim() == "---")
    {
        lines.drain(..end + 2);
    }
    lines
        .into_iter()
        .map(|l| match l.find("%%") {
            Some(at) => &l[..at],
            None => l,
        })
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect()
}

/// A label's text: quotes off, `<br>` a new line, Markdown's bold and
/// italics marks off, a few entities decoded.
fn label_lines(text: &str) -> Vec<String> {
    static BREAK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)<br\s*/?>|\\n").unwrap());
    let text = text.trim();
    let text = text
        .strip_prefix('"')
        .and_then(|t| t.strip_suffix('"'))
        .unwrap_or(text);
    let text = text
        .strip_prefix('`')
        .and_then(|t| t.strip_suffix('`'))
        .unwrap_or(text);
    let text = text
        .replace("**", "")
        .replace("#quot;", "\"")
        .replace("&quot;", "\"")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">");
    BREAK
        .split(&text)
        .map(|l| l.trim().to_string())
        .collect::<Vec<_>>()
}

// ── The canvas ──────────────────────────────────────────────────────────────

const UP: u8 = 1;
const DOWN: u8 = 2;
const LEFT: u8 = 4;
const RIGHT: u8 = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stroke {
    Solid,
    Dotted,
    Thick,
}

#[derive(Default, Clone, Copy)]
struct LineCell {
    bits: u8,
    solid: bool,
    dotted: bool,
    thick: bool,
}

/// A grid of characters: lines drawn as which ways they run through each
/// cell, made box-drawing characters at the end, with boxes, arrows and
/// text over them.
#[derive(Default)]
struct Canvas {
    lines: HashMap<(usize, usize), LineCell>,
    fixed: HashMap<(usize, usize), String>,
}

impl Canvas {
    fn bit(&mut self, at: (usize, usize), bit: u8, stroke: Stroke) {
        let cell = self.lines.entry(at).or_default();
        cell.bits |= bit;
        match stroke {
            Stroke::Solid => cell.solid = true,
            Stroke::Dotted => cell.dotted = true,
            Stroke::Thick => cell.thick = true,
        }
    }

    /// A straight line from `a` to `b`, both ends included.
    fn segment(&mut self, a: (usize, usize), b: (usize, usize), stroke: Stroke) {
        let (mut x, mut y) = a;
        while (x, y) != b {
            let (nx, ny, out, back) = if x < b.0 {
                (x + 1, y, RIGHT, LEFT)
            } else if x > b.0 {
                (x - 1, y, LEFT, RIGHT)
            } else if y < b.1 {
                (x, y + 1, DOWN, UP)
            } else {
                (x, y - 1, UP, DOWN)
            };
            self.bit((x, y), out, stroke);
            self.bit((nx, ny), back, stroke);
            (x, y) = (nx, ny);
        }
        if a == b {
            self.lines.entry(a).or_default();
        }
    }

    fn put(&mut self, x: usize, y: usize, ch: &str) {
        self.fixed.insert((x, y), ch.to_string());
    }

    /// Text from `(x, y)` rightwards; a wide character takes the cell after
    /// it too.
    fn text(&mut self, x: usize, y: usize, text: &str) {
        let mut at = x;
        for g in text.graphemes(true) {
            let w = width(g).max(1);
            self.put(at, y, g);
            for extra in 1..w {
                self.put(at + extra, y, "");
            }
            at += w;
        }
    }

    /// A box with its corners square or round.
    fn rect(&mut self, x: usize, y: usize, w: usize, h: usize, round: bool) {
        if w < 2 || h < 2 {
            return;
        }
        let (tl, tr, bl, br) = if round {
            ("╭", "╮", "╰", "╯")
        } else {
            ("┌", "┐", "└", "┘")
        };
        for i in x + 1..x + w - 1 {
            self.put(i, y, "─");
            self.put(i, y + h - 1, "─");
            for j in y + 1..y + h - 1 {
                self.put(i, j, " ");
            }
        }
        for j in y + 1..y + h - 1 {
            self.put(x, j, "│");
            self.put(x + w - 1, j, "│");
        }
        self.put(x, y, tl);
        self.put(x + w - 1, y, tr);
        self.put(x, y + h - 1, bl);
        self.put(x + w - 1, y + h - 1, br);
    }

    fn render(&self) -> String {
        let cols = self
            .lines
            .keys()
            .chain(self.fixed.keys())
            .map(|(x, _)| x + 1)
            .max()
            .unwrap_or(0);
        let rows = self
            .lines
            .keys()
            .chain(self.fixed.keys())
            .map(|(_, y)| y + 1)
            .max()
            .unwrap_or(0);
        let mut out: Vec<String> = Vec::with_capacity(rows);
        for y in 0..rows {
            let mut row = String::new();
            for x in 0..cols {
                if let Some(ch) = self.fixed.get(&(x, y)) {
                    row.push_str(ch);
                } else if let Some(cell) = self.lines.get(&(x, y)) {
                    row.push_str(line_char(cell));
                } else {
                    row.push(' ');
                }
            }
            out.push(row.trim_end().to_string());
        }
        while out.last().is_some_and(String::is_empty) {
            out.pop();
        }
        out.join("\n")
    }
}

fn line_char(cell: &LineCell) -> &'static str {
    let only = |dotted: bool, thick: bool| {
        if cell.solid {
            Stroke::Solid
        } else if dotted && cell.dotted && !cell.thick {
            Stroke::Dotted
        } else if thick && cell.thick && !cell.dotted {
            Stroke::Thick
        } else {
            Stroke::Solid
        }
    };
    let vertical = cell.bits & (UP | DOWN) != 0;
    let horizontal = cell.bits & (LEFT | RIGHT) != 0;
    match (vertical, horizontal) {
        (true, false) => match only(true, true) {
            Stroke::Dotted => "┆",
            Stroke::Thick => "┃",
            Stroke::Solid => "│",
        },
        (false, true) => match only(true, true) {
            Stroke::Dotted => "┄",
            Stroke::Thick => "━",
            Stroke::Solid => "─",
        },
        (false, false) => " ",
        (true, true) => match cell.bits {
            b if b == DOWN | RIGHT => "┌",
            b if b == DOWN | LEFT => "┐",
            b if b == UP | RIGHT => "└",
            b if b == UP | LEFT => "┘",
            b if b == UP | DOWN | RIGHT => "├",
            b if b == UP | DOWN | LEFT => "┤",
            b if b == DOWN | LEFT | RIGHT => "┬",
            b if b == UP | LEFT | RIGHT => "┴",
            _ => "┼",
        },
    }
}

// ── Flowcharts ──────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Dir {
    TopDown,
    BottomUp,
    LeftRight,
    RightLeft,
}

#[derive(Clone, Debug)]
struct Node {
    id: String,
    label: Vec<String>,
    round: bool,
}

#[derive(Clone, Debug)]
struct Edge {
    from: usize,
    to: usize,
    label: String,
    arrow: bool,
    stroke: Stroke,
}

#[derive(Default, Debug)]
struct Graph {
    nodes: Vec<Node>,
    edges: Vec<Edge>,
}

static TEXT_LINK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^<?(--|==|-\.)\s+(.+?)\s*(-{2,}|={2,}|\.+-)(>)?").unwrap());
static PIPE_LINK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^<?(-{2,}|={2,}|-?\.+-?)(>)?\s*\|([^|]*)\|").unwrap());
static PLAIN_LINK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^<?(-{2,}|={2,}|-?\.+-?)(>)?").unwrap());

/// What opens a node's shape, the closer it needs, and whether its corners
/// are round; longest first, so `((` is read before `(`.
const SHAPES: &[(&str, &[&str], bool)] = &[
    ("(((", &[")))"], true),
    ("((", &["))"], true),
    ("([", &["])"], true),
    ("[[", &["]]"], false),
    ("[(", &[")]"], false),
    ("{{", &["}}"], false),
    ("[/", &["/]", "\\]"], false),
    ("[\\", &["\\]", "/]"], false),
    ("(", &[")"], true),
    ("[", &["]"], false),
    ("{", &["}"], false),
    (">", &["]"], false),
];

impl Graph {
    fn parse(lines: &[String]) -> Graph {
        let mut graph = Graph::default();
        for line in lines {
            for statement in split_statements(line) {
                let first = statement.split_whitespace().next().unwrap_or("");
                if [
                    "subgraph",
                    "end",
                    "classDef",
                    "class",
                    "style",
                    "linkStyle",
                    "click",
                    "direction",
                ]
                .contains(&first)
                {
                    continue;
                }
                graph.statement(&statement);
            }
        }
        graph
    }

    fn node(&mut self, id: &str) -> usize {
        match self.nodes.iter().position(|n| n.id == id) {
            Some(i) => i,
            None => {
                self.nodes.push(Node {
                    id: id.to_string(),
                    label: vec![id.to_string()],
                    round: false,
                });
                self.nodes.len() - 1
            }
        }
    }

    /// One statement: nodes joined by edges, `A --> B & C -- yes --> D`.
    fn statement(&mut self, text: &str) {
        let mut rest = text.trim();
        let mut previous: Option<Vec<usize>> = None;
        let mut edge: Option<(String, bool, Stroke)> = None;
        loop {
            let (group, after) = self.group(rest);
            if group.is_empty() {
                return;
            }
            if let (Some(froms), Some((label, arrow, stroke))) = (&previous, edge.take()) {
                for &from in froms {
                    for &to in &group {
                        self.edges.push(Edge {
                            from,
                            to,
                            label: label.clone(),
                            arrow,
                            stroke,
                        });
                    }
                }
            }
            previous = Some(group);
            rest = after.trim_start();
            if rest.is_empty() {
                return;
            }
            match link(rest) {
                Some((label, arrow, stroke, after)) => {
                    edge = Some((label, arrow, stroke));
                    rest = after.trim_start();
                }
                None => return,
            }
        }
    }

    /// Nodes joined by `&`, and what follows them.
    fn group<'a>(&mut self, text: &'a str) -> (Vec<usize>, &'a str) {
        let mut found = Vec::new();
        let mut rest = text;
        while let Some((id, label, round, after)) = node_ref(rest.trim_start()) {
            let i = self.node(&id);
            if let Some(label) = label {
                self.nodes[i].label = label_lines(&label);
                self.nodes[i].round = round;
            }
            found.push(i);
            rest = after;
            let trimmed = rest.trim_start();
            match trimmed.strip_prefix('&') {
                Some(more) => rest = more,
                None => break,
            }
        }
        (found, rest)
    }
}

/// A line's statements, split at `;` outside quotes and brackets.
fn split_statements(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut depth = 0i32;
    let mut quoted = false;
    for c in line.chars() {
        match c {
            '"' => quoted = !quoted,
            '[' | '(' | '{' if !quoted => depth += 1,
            ']' | ')' | '}' if !quoted => depth -= 1,
            ';' if !quoted && depth <= 0 => {
                out.push(std::mem::take(&mut current));
                continue;
            }
            _ => {}
        }
        current.push(c);
    }
    out.push(current);
    out.into_iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// A node at the start of `text`: its id, the label its shape gives, round
/// corners, and what follows it.
fn node_ref(text: &str) -> Option<(String, Option<String>, bool, &str)> {
    let mut end = 0;
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut i = 0;
    while i < chars.len() {
        let (at, c) = chars[i];
        let word = |c: char| c.is_alphanumeric() || c == '_';
        if word(c) {
            end = at + c.len_utf8();
            i += 1;
            continue;
        }
        // A dash or dot inside an id (`my-node`), not the start of a link.
        if (c == '-' || c == '.') && i > 0 && chars.get(i + 1).is_some_and(|(_, n)| word(*n)) {
            end = at + c.len_utf8();
            i += 1;
            continue;
        }
        break;
    }
    if end == 0 {
        return None;
    }
    let id = text[..end].to_string();
    let mut rest = &text[end..];
    // A class given in place (`A:::warn`) says nothing of the shape.
    if let Some(after) = rest.strip_prefix(":::") {
        let n = after
            .find(|c: char| !(c.is_alphanumeric() || c == '_' || c == '-'))
            .unwrap_or(after.len());
        rest = &after[n..];
    }
    for (open, closers, round) in SHAPES {
        let Some(inner) = rest.strip_prefix(open) else {
            continue;
        };
        // A quoted label may hold the closer itself.
        let (label, after) = if let Some(q) = inner.strip_prefix('"') {
            let close = q.find('"')?;
            let after = q[close + 1..].trim_start();
            let closer = closers.iter().find(|c| after.starts_with(**c))?;
            (q[..close].to_string(), &after[closer.len()..])
        } else {
            let (at, closer) = closers
                .iter()
                .filter_map(|c| inner.find(c).map(|at| (at, *c)))
                .min_by_key(|(at, _)| *at)?;
            (inner[..at].to_string(), &inner[at + closer.len()..])
        };
        let after = after.strip_prefix(":::").map_or(after, |a| {
            let n = a
                .find(|c: char| !(c.is_alphanumeric() || c == '_' || c == '-'))
                .unwrap_or(a.len());
            &a[n..]
        });
        return Some((id, Some(label), *round, after));
    }
    Some((id, None, false, rest))
}

/// A link at the start of `text`: its label, whether it ends in an arrow,
/// its stroke, and what follows it.
fn link(text: &str) -> Option<(String, bool, Stroke, &str)> {
    let stroke = |s: &str| {
        if s.contains('.') {
            Stroke::Dotted
        } else if s.contains('=') {
            Stroke::Thick
        } else {
            Stroke::Solid
        }
    };
    if let Some(c) = TEXT_LINK.captures(text) {
        let whole = c.get(0)?;
        let kind = format!("{}{}", &c[1], &c[3]);
        return Some((
            c[2].trim().to_string(),
            c.get(4).is_some(),
            stroke(&kind),
            &text[whole.end()..],
        ));
    }
    if let Some(c) = PIPE_LINK.captures(text) {
        let whole = c.get(0)?;
        return Some((
            label_lines(&c[3]).join(" "),
            c.get(2).is_some(),
            stroke(&c[1]),
            &text[whole.end()..],
        ));
    }
    let c = PLAIN_LINK.captures(text)?;
    let whole = c.get(0)?;
    let kind = &c[1];
    // `--` alone is no link, only the start of one with text.
    if kind == "--" && c.get(2).is_none() {
        return None;
    }
    Some((
        String::new(),
        c.get(2).is_some(),
        stroke(kind),
        &text[whole.end()..],
    ))
}

/// A point of the layout: a node, or where a long edge passes through a
/// layer.
#[derive(Clone, Debug)]
struct Point {
    node: Option<usize>,
    layer: usize,
    /// Along the diagram's direction and across it.
    main: usize,
    cross: usize,
    /// Where it starts across, laid out.
    at: i64,
}

/// A box's size along the diagram's direction and across it.
fn box_size(node: &Node, dir: Dir) -> (usize, usize) {
    let mut w = node.label.iter().map(|l| width(l)).max().unwrap_or(0) + 4;
    // An odd width has a middle column for the edges to meet, so a chain
    // draws straight down.
    if matches!(dir, Dir::TopDown | Dir::BottomUp) && w % 2 == 0 {
        w += 1;
    }
    let h = node.label.len() + 2;
    match dir {
        Dir::TopDown | Dir::BottomUp => (h, w),
        Dir::LeftRight | Dir::RightLeft => (w, h),
    }
}

impl Graph {
    fn draw(&self, dir: Dir) -> String {
        let n = self.nodes.len();
        let edges: Vec<&Edge> = self.edges.iter().filter(|e| e.from != e.to).collect();
        // Edges that loop back, found depth first, are laid out the other
        // way round, so what is left has no cycle to layer.
        let mut reversed = vec![false; edges.len()];
        let mut state = vec![0u8; n];
        fn visit(v: usize, edges: &[&Edge], state: &mut [u8], reversed: &mut [bool]) {
            state[v] = 1;
            for (i, e) in edges.iter().enumerate() {
                if e.from != v {
                    continue;
                }
                match state[e.to] {
                    0 => visit(e.to, edges, state, reversed),
                    1 => reversed[i] = true,
                    _ => {}
                }
            }
            state[v] = 2;
        }
        for v in 0..n {
            if state[v] == 0 {
                visit(v, &edges, &mut state, &mut reversed);
            }
        }
        let ends = |i: usize| {
            let e = edges[i];
            if reversed[i] {
                (e.to, e.from)
            } else {
                (e.from, e.to)
            }
        };
        // Each node a layer below the longest chain leading to it.
        let mut layer = vec![0usize; n];
        for _ in 0..n {
            let mut changed = false;
            for i in 0..edges.len() {
                let (a, b) = ends(i);
                if layer[b] < layer[a] + 1 {
                    layer[b] = layer[a] + 1;
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
        let layers_n = layer.iter().max().map_or(0, |m| m + 1);
        let mut points: Vec<Point> = (0..n)
            .map(|v| {
                let (main, cross) = box_size(&self.nodes[v], dir);
                Point {
                    node: Some(v),
                    layer: layer[v],
                    main,
                    cross,
                    at: 0,
                }
            })
            .collect();
        // Edges between the same two nodes (a question and its answer back)
        // are drawn as one line, with each one's arrow and label, rather
        // than one over the other.
        let mut groups: Vec<(usize, usize, Vec<usize>)> = Vec::new();
        for i in 0..edges.len() {
            let (a, b) = ends(i);
            match groups.iter_mut().find(|g| g.0 == a && g.1 == b) {
                Some(g) => g.2.push(i),
                None => groups.push((a, b, vec![i])),
            }
        }
        let group_label = |g: &[usize]| {
            g.iter()
                .map(|&i| edges[i].label.as_str())
                .filter(|l| !l.is_empty())
                .collect::<Vec<_>>()
                .join(" / ")
        };
        // Each group as the points it passes, top to bottom.
        let mut paths: Vec<Vec<usize>> = Vec::new();
        for (a, b, _) in &groups {
            let (a, b) = (*a, *b);
            let mut path = vec![a];
            for l in layer[a] + 1..layer[b] {
                points.push(Point {
                    node: None,
                    layer: l,
                    main: 0,
                    cross: 1,
                    at: 0,
                });
                path.push(points.len() - 1);
            }
            path.push(b);
            paths.push(path);
        }
        let mut layers: Vec<Vec<usize>> = vec![Vec::new(); layers_n];
        for (p, point) in points.iter().enumerate() {
            layers[point.layer].push(p);
        }
        let mut above: Vec<Vec<usize>> = vec![Vec::new(); points.len()];
        let mut below: Vec<Vec<usize>> = vec![Vec::new(); points.len()];
        for path in &paths {
            for pair in path.windows(2) {
                below[pair[0]].push(pair[1]);
                above[pair[1]].push(pair[0]);
            }
        }
        // Each layer ordered by where what it joins sits, a few times down
        // and up, so fewer edges cross.
        let mut order = vec![0usize; points.len()];
        let renumber = |layers: &[Vec<usize>], order: &mut [usize]| {
            for l in layers {
                for (i, p) in l.iter().enumerate() {
                    order[*p] = i;
                }
            }
        };
        renumber(&layers, &mut order);
        for _ in 0..4 {
            for l in 1..layers_n {
                sort_by_neighbours(&mut layers[l], &above, &order);
                renumber(&layers, &mut order);
            }
            for l in (0..layers_n.saturating_sub(1)).rev() {
                sort_by_neighbours(&mut layers[l], &below, &order);
                renumber(&layers, &mut order);
            }
        }
        // Across: each point under what it comes from, then over what it
        // leads to, kept apart by a gap.
        let gap: i64 = match dir {
            Dir::TopDown | Dir::BottomUp => 4,
            _ => 1,
        };
        for l in &layers {
            let mut at = 0i64;
            for &p in l {
                points[p].at = at;
                at += points[p].cross as i64 + gap;
            }
        }
        let center = |p: &Point| p.at + p.cross as i64 / 2;
        for pass in 0..3 {
            let (range, toward): (Vec<usize>, &Vec<Vec<usize>>) = if pass % 2 == 0 {
                ((1..layers_n).collect(), &above)
            } else {
                ((0..layers_n.saturating_sub(1)).rev().collect(), &below)
            };
            for l in range {
                let mut next_free = i64::MIN;
                for &p in &layers[l] {
                    let joined = &toward[p];
                    let want = if joined.is_empty() {
                        points[p].at
                    } else {
                        let sum: i64 = joined.iter().map(|q| center(&points[*q])).sum();
                        sum / joined.len() as i64 - points[p].cross as i64 / 2
                    };
                    let at = want.max(next_free);
                    points[p].at = at;
                    next_free = at + points[p].cross as i64 + gap;
                }
            }
        }
        let least = points.iter().map(|p| p.at).min().unwrap_or(0);
        for p in &mut points {
            p.at -= least;
        }
        // Along: each layer as deep as its deepest box, and between two
        // layers a lane for each point edges leave from, a row (or columns)
        // for labels, and the row the arrows are on.
        let thick: Vec<usize> = layers
            .iter()
            .map(|l| l.iter().map(|p| points[*p].main).max().unwrap_or(1).max(1))
            .collect();
        let mut lanes: Vec<HashMap<usize, usize>> = vec![HashMap::new(); layers_n];
        let mut label_room = vec![0usize; layers_n];
        for (g, path) in paths.iter().enumerate() {
            let label = group_label(&groups[g].2);
            for (k, pair) in path.windows(2).enumerate() {
                let l = points[pair[0]].layer;
                let count = lanes[l].len();
                lanes[l].entry(pair[0]).or_insert(count);
                let last = k + 2 == path.len();
                if last && !label.is_empty() {
                    let need = match dir {
                        Dir::TopDown | Dir::BottomUp => 1,
                        _ => width(&label) + 2,
                    };
                    label_room[l] = label_room[l].max(need);
                }
            }
        }
        // Lanes in the order their points sit across, so they cross less.
        for lane in &mut lanes {
            let mut keys: Vec<usize> = lane.keys().copied().collect();
            keys.sort_by_key(|p| order[*p]);
            *lane = keys.into_iter().enumerate().map(|(i, p)| (p, i)).collect();
        }
        // Across the screen an edge needs a cell of line before it turns,
        // or its arrow sits against the box it left; down it, a row is room
        // enough.
        let pad = match dir {
            Dir::TopDown | Dir::BottomUp => 0,
            _ => 1,
        };
        let mut start = vec![0usize; layers_n];
        let mut lane_at = vec![0usize; layers_n];
        let mut main_at = 0;
        for l in 0..layers_n {
            start[l] = main_at;
            lane_at[l] = main_at + thick[l] + pad;
            main_at = lane_at[l] + lanes[l].len().max(1) + label_room[l] + 1;
        }
        let total_main = start[layers_n - 1] + thick[layers_n - 1];
        // Turned to the screen.
        let to_screen = |main: usize, cross: usize| -> (usize, usize) {
            match dir {
                Dir::TopDown => (cross, main),
                Dir::BottomUp => (cross, total_main - 1 - main),
                Dir::LeftRight => (main, cross),
                Dir::RightLeft => (total_main - 1 - main, cross),
            }
        };
        let mut canvas = Canvas::default();
        // The edges first, the boxes over their ends.
        let mut marks: Vec<((usize, usize), &'static str)> = Vec::new();
        let mut labels: Vec<(usize, usize, usize, String)> = Vec::new();
        for (g, path) in paths.iter().enumerate() {
            let group = &groups[g].2;
            let stroke = edges[group[0]].stroke;
            let label = group_label(group);
            let mut route: Vec<(usize, usize)> = Vec::new();
            for (k, pair) in path.windows(2).enumerate() {
                let (a, b) = (&points[pair[0]], &points[pair[1]]);
                let l = a.layer;
                let ca = center(a) as usize;
                let cb = center(b) as usize;
                let from = match a.node {
                    Some(_) => start[l] + a.main,
                    None => start[l],
                };
                let lane = lane_at[l] + lanes[l][&pair[0]];
                let to = match b.node {
                    Some(_) => start[b.layer] - 1,
                    None => start[b.layer],
                };
                if k == 0 {
                    route.push((from, ca));
                }
                route.push((lane, ca));
                route.push((lane, cb));
                route.push((to, cb));
                if k + 2 == path.len() && !label.is_empty() {
                    labels.push((
                        lane_at[l] + lanes[l].len().max(1),
                        label_room[l],
                        cb,
                        label.clone(),
                    ));
                }
            }
            route.dedup();
            let screen: Vec<(usize, usize)> =
                route.iter().map(|(m, c)| to_screen(*m, *c)).collect();
            for pair in screen.windows(2) {
                canvas.segment(pair[0], pair[1], stroke);
            }
            if screen.len() == 1 {
                canvas.segment(screen[0], screen[0], stroke);
            }
            // Each end touches its box: an arrow into it, or a join on its
            // edge where the line leaves.
            let (first_m, first_c) = route[0];
            let (last_m, last_c) = *route.last().expect("a route");
            let top_border = to_screen(first_m - 1, first_c);
            let bottom_border = to_screen(last_m + 1, last_c);
            let head_bottom = group.iter().any(|&i| edges[i].arrow && !reversed[i]);
            let head_top = group.iter().any(|&i| edges[i].arrow && reversed[i]);
            let first = to_screen(first_m, first_c);
            let last = to_screen(last_m, last_c);
            canvas.bit(first, toward(first, top_border), stroke);
            canvas.bit(last, toward(last, bottom_border), stroke);
            if head_top {
                marks.push((first, arrow(first, top_border)));
            } else {
                marks.push((top_border, join(top_border, first)));
            }
            if head_bottom {
                marks.push((last, arrow(last, bottom_border)));
            } else {
                marks.push((bottom_border, join(bottom_border, last)));
            }
        }
        for point in &points {
            let Some(v) = point.node else {
                continue;
            };
            let node = &self.nodes[v];
            let m0 = start[point.layer];
            let c0 = point.at as usize;
            let a = to_screen(m0, c0);
            let b = to_screen(m0 + point.main - 1, c0 + point.cross - 1);
            let (x, y) = (a.0.min(b.0), a.1.min(b.1));
            let (w, h) = (a.0.max(b.0) - x + 1, a.1.max(b.1) - y + 1);
            canvas.rect(x, y, w, h, node.round);
            let inner = w.saturating_sub(4);
            for (j, line) in node.label.iter().enumerate() {
                let pad = inner.saturating_sub(width(line)) / 2;
                canvas.text(x + 2 + pad, y + 1 + j, line);
            }
        }
        for (at, ch) in marks {
            canvas.put(at.0, at.1, ch);
        }
        for (m, room, c, text) in labels {
            match dir {
                Dir::TopDown | Dir::BottomUp => {
                    let (x, y) = to_screen(m, c);
                    let x = x.saturating_sub(width(&text) / 2);
                    canvas.text(x, y, &text);
                }
                _ => {
                    let a = to_screen(m, c);
                    let b = to_screen(m + room - 1, c);
                    canvas.text(a.0.min(b.0) + 1, a.1, &text);
                }
            }
        }
        canvas.render()
    }
}

/// Orders a layer by the average place of what each point joins in the
/// layer beside it, a point joining nothing keeping its place.
fn sort_by_neighbours(layer: &mut [usize], joined: &[Vec<usize>], order: &[usize]) {
    let key: HashMap<usize, f64> = layer
        .iter()
        .map(|p| {
            let near = &joined[*p];
            let k = if near.is_empty() {
                order[*p] as f64
            } else {
                near.iter().map(|q| order[*q] as f64).sum::<f64>() / near.len() as f64
            };
            (*p, k)
        })
        .collect();
    layer.sort_by(|a, b| {
        key[a]
            .partial_cmp(&key[b])
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(order[*a].cmp(&order[*b]))
    });
}

/// The way from one cell to the next, as a line's bit.
fn toward(from: (usize, usize), to: (usize, usize)) -> u8 {
    if to.0 > from.0 {
        RIGHT
    } else if to.0 < from.0 {
        LEFT
    } else if to.1 > from.1 {
        DOWN
    } else {
        UP
    }
}

/// An arrowhead in `cell` pointing at `target`.
fn arrow(cell: (usize, usize), target: (usize, usize)) -> &'static str {
    match toward(cell, target) {
        RIGHT => "▶",
        LEFT => "◀",
        DOWN => "▼",
        _ => "▲",
    }
}

/// A box's edge with a line leaving it toward `out`.
fn join(border: (usize, usize), out: (usize, usize)) -> &'static str {
    match toward(border, out) {
        DOWN => "┬",
        UP => "┴",
        RIGHT => "├",
        _ => "┤",
    }
}

// ── Sequence diagrams ───────────────────────────────────────────────────────

#[derive(Debug)]
enum Step {
    Message {
        from: usize,
        to: usize,
        text: String,
        arrow: bool,
        dotted: bool,
    },
    Note {
        from: usize,
        to: usize,
        side: Side,
        text: String,
    },
    /// A loop's, an alternative's or a block's start, else or end.
    Mark(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Side {
    Over,
    Left,
    Right,
}

#[derive(Debug, Default)]
struct Sequence {
    ids: Vec<String>,
    labels: Vec<String>,
    steps: Vec<Step>,
}

static PARTICIPANT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?:participant|actor)\s+(.+?)(?:\s+as\s+(.+))?$").unwrap());
static MESSAGE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(.+?)\s*(-->>|->>|--x|-x|--\)|-\)|-->|->)\s*[+-]?\s*(.+?)\s*:(.*)$").unwrap()
});
static NOTE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^note\s+(over|left of|right of)\s+([^:,]+?)\s*(?:,\s*([^:]+?))?\s*:(.*)$")
        .unwrap()
});
static BLOCK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(loop|alt|else|opt|par|and|critical|option|break|rect)\b\s*(.*)$").unwrap()
});

impl Sequence {
    fn participant(&mut self, id: &str) -> usize {
        let id = id.trim();
        match self.ids.iter().position(|p| p == id) {
            Some(i) => i,
            None => {
                self.ids.push(id.to_string());
                self.labels.push(id.to_string());
                self.ids.len() - 1
            }
        }
    }

    fn parse(lines: &[String]) -> Option<Sequence> {
        let mut seq = Sequence::default();
        let mut numbered = false;
        let mut count = 0;
        for line in lines {
            if line == "autonumber" || line.starts_with("autonumber ") {
                numbered = true;
            } else if let Some(c) = PARTICIPANT.captures(line) {
                let i = seq.participant(&c[1]);
                if let Some(label) = c.get(2) {
                    seq.labels[i] = label_lines(label.as_str()).join(" ");
                }
            } else if let Some(c) = NOTE.captures(line) {
                let from = seq.participant(&c[2]);
                let to = c.get(3).map_or(from, |b| seq.participant(b.as_str()));
                let side = match c[1].to_ascii_lowercase().as_str() {
                    "left of" => Side::Left,
                    "right of" => Side::Right,
                    _ => Side::Over,
                };
                seq.steps.push(Step::Note {
                    from: from.min(to),
                    to: from.max(to),
                    side,
                    text: label_lines(&c[4]).join(" "),
                });
            } else if let Some(c) = BLOCK.captures(line) {
                seq.steps.push(Step::Mark(
                    format!("{} {}", &c[1], c[2].trim()).trim().to_string(),
                ));
            } else if line == "end" {
                seq.steps.push(Step::Mark("end".into()));
            } else if let Some(c) = MESSAGE.captures(line) {
                let from = seq.participant(&c[1]);
                let to = seq.participant(&c[3]);
                let kind = &c[2];
                count += 1;
                let text = label_lines(&c[4]).join(" ");
                let text = if numbered {
                    format!("{count}. {text}")
                } else {
                    text
                };
                seq.steps.push(Step::Message {
                    from,
                    to,
                    text,
                    arrow: kind != "->" && kind != "-->",
                    dotted: kind.starts_with("--"),
                });
            }
        }
        (!seq.ids.is_empty()).then_some(seq)
    }

    fn draw(&self) -> String {
        let n = self.ids.len();
        let boxes: Vec<usize> = self.labels.iter().map(|l| width(l) + 4).collect();
        // Where each lifeline runs: far enough apart for the boxes, every
        // message's text between its two, a loop's text beside its own, and
        // the notes beside theirs.
        let mut x: Vec<usize> = Vec::with_capacity(n);
        for i in 0..n {
            x.push(match i {
                0 => boxes[0] / 2,
                _ => x[i - 1] + (boxes[i - 1] - boxes[i - 1] / 2) + boxes[i] / 2 + 2,
            });
        }
        let mut needs: Vec<(usize, usize, usize)> = Vec::new();
        let mut right_room = boxes[n - 1] - boxes[n - 1] / 2;
        let mut left_room = boxes[0] / 2;
        for step in &self.steps {
            match step {
                Step::Message { from, to, text, .. } if from != to => {
                    needs.push(((*from).min(*to), (*from).max(*to), width(text) + 4));
                }
                Step::Message { from, text, .. } => {
                    let need = width(text) + 7;
                    if *from + 1 < n {
                        needs.push((*from, *from + 1, need));
                    } else {
                        right_room = right_room.max(need);
                    }
                }
                Step::Note {
                    from, side, text, ..
                } => {
                    let need = width(text) + 6;
                    match side {
                        Side::Right if *from + 1 < n => needs.push((*from, *from + 1, need)),
                        Side::Right => right_room = right_room.max(need),
                        Side::Left if *from > 0 => needs.push((*from - 1, *from, need)),
                        Side::Left => left_room = left_room.max(need),
                        Side::Over => right_room = right_room.max((width(text) + 4) / 2 + 1),
                    }
                }
                Step::Mark(_) => {}
            }
        }
        needs.sort_by_key(|(a, b, _)| b - a);
        for (a, b, need) in needs {
            let have = x[b] - x[a];
            if have < need {
                for xi in x.iter_mut().skip(b) {
                    *xi += need - have;
                }
            }
        }
        let shift = left_room.saturating_sub(x[0]);
        for xi in &mut x {
            *xi += shift;
        }
        let mut canvas = Canvas::default();
        for i in 0..n {
            let left = x[i] - boxes[i] / 2;
            canvas.rect(left, 0, boxes[i], 3, false);
            canvas.text(left + 2, 1, &self.labels[i]);
        }
        let mut row = 3;
        let mut fixed_rows: Vec<(usize, Vec<(usize, String)>)> = Vec::new();
        let mut notes: Vec<(usize, usize, usize, String)> = Vec::new();
        for step in &self.steps {
            match step {
                Step::Message {
                    from,
                    to,
                    text,
                    arrow,
                    dotted,
                } if from != to => {
                    let (a, b) = (x[*from].min(x[*to]), x[*from].max(x[*to]));
                    if !text.is_empty() {
                        let room = b - a - 1;
                        let at = a + 1 + room.saturating_sub(width(text)) / 2;
                        fixed_rows.push((row, vec![(at, text.clone())]));
                        row += 1;
                    }
                    let stroke = if *dotted { "┄" } else { "─" };
                    let mut cells = Vec::new();
                    for c in a + 1..b {
                        cells.push((c, stroke.to_string()));
                    }
                    let rightward = to > from;
                    if *arrow {
                        let head = if rightward {
                            (b - 1, "▶")
                        } else {
                            (a + 1, "◀")
                        };
                        cells.push((head.0, head.1.to_string()));
                    }
                    cells.push((x[*from], if rightward { "├" } else { "┤" }.to_string()));
                    fixed_rows.push((row, cells));
                    row += 1;
                }
                Step::Message {
                    from, text, arrow, ..
                } => {
                    let at = x[*from];
                    fixed_rows.push((
                        row,
                        vec![
                            (at, "├".into()),
                            (at + 1, "──┐".into()),
                            (at + 5, text.clone()),
                        ],
                    ));
                    fixed_rows.push((row + 1, vec![(at + 3, "│".into())]));
                    let head = if *arrow { "◀" } else { "─" };
                    fixed_rows.push((row + 2, vec![(at + 1, format!("{head}─┘"))]));
                    row += 3;
                }
                Step::Note {
                    from,
                    to,
                    side,
                    text,
                } => {
                    let w = width(text) + 4;
                    let left = match side {
                        Side::Right => x[*from] + 2,
                        Side::Left => x[*from].saturating_sub(w + 1),
                        Side::Over => ((x[*from] + x[*to]) / 2).saturating_sub(w / 2),
                    };
                    notes.push((left, row, w, text.clone()));
                    row += 3;
                }
                Step::Mark(text) => {
                    fixed_rows.push((row, vec![(0, format!("┄ {text} ┄"))]));
                    row += 1;
                }
            }
        }
        for (i, xi) in x.iter().enumerate() {
            let _ = i;
            canvas.segment((*xi, 3), (*xi, row), Stroke::Solid);
            canvas.bit((*xi, 3), UP, Stroke::Solid);
            canvas.put(*xi, 2, "┬");
        }
        for (r, cells) in fixed_rows {
            for (c, text) in cells {
                canvas.text(c, r, &text);
            }
        }
        for (left, r, w, text) in notes {
            canvas.rect(left, r, w, 3, false);
            canvas.text(left + 2, r + 1, &text);
        }
        canvas.render()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_flowchart_draws_its_boxes_and_arrows_top_down() {
        let drawn = draw(
            "flowchart TD\n  A[Start] --> B{Is it?}\n  B -->|Yes| C[Do it]\n  B -- No --> D(Skip)",
            80,
        )
        .unwrap();
        let lines: Vec<&str> = drawn.lines().collect();
        assert!(
            lines[0].starts_with("┌") || lines[0].trim_start().starts_with("┌"),
            "{drawn}"
        );
        assert!(drawn.contains("│ Start │"), "{drawn}");
        // An even label gets a space more on the right, so its box has a
        // middle column for the edges.
        assert!(drawn.contains("│ Is it?  │"), "{drawn}");
        assert!(drawn.contains("│ Do it │"), "{drawn}");
        assert!(drawn.contains("│ Skip  │"), "{drawn}");
        assert!(drawn.contains("╭"), "a round box for (Skip): {drawn}");
        assert!(drawn.contains("Yes") && drawn.contains("No"), "{drawn}");
        assert_eq!(drawn.matches('▼').count(), 3, "{drawn}");
        // Start is above the choice, the choice above both outcomes.
        let row = |text: &str| lines.iter().position(|l| l.contains(text)).unwrap();
        assert!(row("Start") < row("Is it?"));
        assert!(row("Is it?") < row("Do it"));
        assert_eq!(row("Do it"), row("Skip"), "the two outcomes side by side");
        // A chain draws straight down: the arrow under Start's middle.
        let start = lines[row("Start")];
        let middle = start.find("Start").unwrap() + 2;
        let below = lines[row("Start") + 1];
        assert_eq!(
            below.chars().nth(middle_col(start, middle)),
            Some('┬'),
            "{drawn}"
        );
    }

    fn middle_col(line: &str, byte: usize) -> usize {
        line[..byte].chars().count()
    }

    #[test]
    fn a_left_to_right_flowchart_reads_across_unless_too_wide() {
        let source = "graph LR\n  A[One] --> B[Two] --> C[Three]";
        let drawn = draw(source, 80).unwrap();
        let line = drawn.lines().find(|l| l.contains("One")).unwrap();
        assert!(line.contains("Two") && line.contains("Three"), "{drawn}");
        assert!(line.contains("▶"), "{drawn}");
        let narrow = draw(source, 15).unwrap();
        assert!(
            !narrow
                .lines()
                .any(|l| l.contains("One") && l.contains("Two")),
            "{narrow}"
        );
        assert!(narrow.contains("▼"), "drawn top to bottom: {narrow}");
    }

    #[test]
    fn a_loop_back_has_its_arrow_at_the_top() {
        let drawn = draw("flowchart TD\n  A --> B\n  B --> A", 80).unwrap();
        assert!(drawn.contains('▲') && drawn.contains('▼'), "{drawn}");
    }

    #[test]
    fn a_sequence_diagram_has_lifelines_and_messages() {
        let source = "sequenceDiagram\n  participant A as Alice\n  participant J as John\n  A->>J: Hello John\n  J-->>A: Great!\n  A->>A: Think";
        let drawn = draw(source, 80).unwrap();
        let lines: Vec<&str> = drawn.lines().collect();
        assert!(
            lines[1].contains("│ Alice │") && lines[1].contains("│ John │"),
            "{drawn}"
        );
        assert!(drawn.contains("Hello John"), "{drawn}");
        assert!(drawn.contains("─▶"), "{drawn}");
        assert!(drawn.contains("◀┄"), "a dotted reply: {drawn}");
        assert!(drawn.contains("──┐ Think"), "a message to itself: {drawn}");
        let hello = lines.iter().position(|l| l.contains("Hello")).unwrap();
        let great = lines.iter().position(|l| l.contains("Great")).unwrap();
        assert!(hello < great);
    }

    #[test]
    fn other_diagrams_are_not_drawn() {
        assert_eq!(draw("pie\n  \"a\": 1", 80), None);
        assert_eq!(draw("classDiagram\n  A <|-- B", 80), None);
        assert_eq!(draw("", 80), None);
    }

    #[test]
    fn statements_and_links_are_read_as_mermaid_writes_them() {
        let g = Graph::parse(&statements(
            "A[\"a [b]\"] --> B; B -.-> C & D\nC ==> D\nmy-node --- A",
        ));
        let ids: Vec<&str> = g.nodes.iter().map(|n| n.id.as_str()).collect();
        assert_eq!(ids, ["A", "B", "C", "D", "my-node"]);
        assert_eq!(g.nodes[0].label, ["a [b]"]);
        assert_eq!(g.edges.len(), 5);
        assert_eq!(g.edges[1].stroke, Stroke::Dotted);
        assert_eq!(g.edges[3].stroke, Stroke::Thick);
        assert!(!g.edges[4].arrow);
    }
}
