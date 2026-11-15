//! A client with no terminal: it attaches to a server as a terminal would
//! and reads what the server draws through a terminal emulator of its own,
//! so a script, a test or a Claude can see the whole screen (the panes, the
//! sidebar, mission control, the boxes over them) and type into it, where
//! `capture-pane` sees one pane and `send-keys` reaches only a pane.
//!
//! - `Screen`: the emulator (alacritty_terminal, as the panes have) fed with
//!   what the server sent, read back as lines of text, as text with its
//!   colors (ANSI), or as cells for a picture. It keeps kitty's graphics
//!   commands as Ghostty does (each image by id, placed by its placeholder
//!   cells or directly at the cursor), so the picture (`html`, drawn by
//!   Chrome) shows the server's images where the text forms leave them
//!   out. The integration tests' client (tests/common) reads through it
//!   too.
//! - `Client`: attached as Ghostty by default (`TERM`), with a window of
//!   the pixels the picture draws, so the server draws for it what it keeps
//!   for Ghostty: its images, truecolor, the half circles.
//! - `serve`: what `switchboard headless` leaves running: the client,
//!   attached at the size asked, and a socket of its own in the state
//!   directory, `headless-<id>.sock`, where `headless-keys` and
//!   `headless-screen` ask it (`ask`), one JSON line each way.
//!
//! A key is sent as a terminal sends it (`input::encode_key`, with no mode a
//! program asked for, since the server reads the client's keys itself), so
//! `M-m` reaches the server as it would from your keyboard.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use alacritty_terminal::event::VoidListener;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::{Config, Term};
use alacritty_terminal::vte::ansi::{Color, NamedColor, Processor};
use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Value, json};

use crate::mux::input::{Key, encode_key};
use crate::mux::proto::{self, ClientMsg, ServerMsg};

struct Size(usize, usize);

impl Dimensions for Size {
    fn total_lines(&self) -> usize {
        self.1
    }
    fn screen_lines(&self) -> usize {
        self.1
    }
    fn columns(&self) -> usize {
        self.0
    }
}

/// One cell as drawn: its character, its colors resolved to red, green and
/// blue, whether it is bold, whether its character takes two cells, and
/// the marks combined with it (an image placeholder's row and column).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cell {
    pub c: char,
    pub fg: (u8, u8, u8),
    pub bg: (u8, u8, u8),
    pub bold: bool,
    pub wide: bool,
    pub marks: Vec<char>,
}

/// An image a kitty graphics command sent: its PNG, base64'd as it came,
/// and the cells it fills when placed with placeholders.
#[derive(Clone, Debug)]
struct Image {
    data: String,
    cols: u16,
    rows: u16,
}

/// An image placed directly at a cell, offset by pixels (the box's shadow,
/// under the text).
#[derive(Clone, Debug)]
struct Placement {
    id: u32,
    line: usize,
    col: usize,
    dx: u32,
    dy: u32,
}

/// The images a terminal holds, as Ghostty keeps them from kitty's
/// graphics commands: by id, with the direct placements.
#[derive(Default)]
struct Images {
    by_id: HashMap<u32, Image>,
    placed: Vec<Placement>,
    /// A command split in chunks (`m=1`), its first keys and the data so far.
    loading: Option<(HashMap<String, String>, String)>,
    /// What came after the last whole command, kept until its end comes.
    pending: Vec<u8>,
}

/// A graphics command's keys (`a=T,i=3`) and payload, from its body.
fn command_parts(body: &[u8]) -> (HashMap<String, String>, String) {
    let body = String::from_utf8_lossy(body);
    let (keys, data) = body.split_once(';').unwrap_or((&body, ""));
    let keys = keys
        .split(',')
        .filter_map(|kv| kv.split_once('='))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    // Only base64 goes on into the page.
    let data = data
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '='))
        .collect();
    (keys, data)
}

impl Images {
    /// Takes one whole command, `ESC _ G` to `ESC \` left off, with the
    /// cursor where it came.
    fn take(&mut self, body: &[u8], cursor: (usize, usize)) {
        let (keys, data) = command_parts(body);
        let (keys, data) = match self.loading.take() {
            Some((first, mut so_far)) => {
                so_far.push_str(&data);
                if keys.get("m").map(String::as_str) == Some("1") {
                    self.loading = Some((first, so_far));
                    return;
                }
                (first, so_far)
            }
            None if keys.get("m").map(String::as_str) == Some("1") => {
                self.loading = Some((keys, data));
                return;
            }
            None => (keys, data),
        };
        let num = |k: &str| keys.get(k).and_then(|v| v.parse::<u32>().ok());
        let id = num("i").unwrap_or(0);
        match keys.get("a").map(String::as_str) {
            Some("d") => match keys.get("d").map(String::as_str) {
                Some("I" | "i") => {
                    self.by_id.remove(&id);
                    self.placed.retain(|p| p.id != id);
                }
                Some("a" | "A") | None => {
                    self.by_id.clear();
                    self.placed.clear();
                }
                _ => {}
            },
            Some("T") => {
                self.placed.retain(|p| p.id != id);
                self.by_id.insert(
                    id,
                    Image {
                        data,
                        cols: num("c").unwrap_or(0) as u16,
                        rows: num("r").unwrap_or(0) as u16,
                    },
                );
                if num("U") != Some(1) {
                    self.placed.push(Placement {
                        id,
                        line: cursor.0,
                        col: cursor.1,
                        dx: num("X").unwrap_or(0),
                        dy: num("Y").unwrap_or(0),
                    });
                }
            }
            _ => {}
        }
    }
}

/// The first `ESC _ G` in `bytes` at or after `from`.
fn find(bytes: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    bytes
        .get(from..)?
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|i| i + from)
}

/// The image id a placeholder's color carries.
fn id_of_color(c: (u8, u8, u8)) -> u32 {
    (c.0 as u32) << 16 | (c.1 as u32) << 8 | c.2 as u32
}

/// The row or column a placeholder's mark means.
fn mark_index(mark: Option<&char>) -> u16 {
    mark.and_then(|m| {
        crate::ui::images::MARKS
            .iter()
            .position(|&x| x == *m as u32)
    })
    .unwrap_or(0) as u16
}

/// A cell's text color, background and boldness, as the ANSI form sets
/// them.
type Pen = ((u8, u8, u8), (u8, u8, u8), bool);

/// The default text and background, where a cell names neither.
pub const DEFAULT_FG: (u8, u8, u8) = (0xd8, 0xd8, 0xd8);
pub const DEFAULT_BG: (u8, u8, u8) = (0x16, 0x16, 0x1d);

/// What a terminal of `cols` by `rows` shows, from the bytes it was sent.
pub struct Screen {
    term: Term<VoidListener>,
    parser: Processor,
    images: Images,
}

impl Screen {
    pub fn new(cols: u16, rows: u16) -> Screen {
        Screen {
            term: Term::new(
                Config::default(),
                &Size(cols as usize, rows as usize),
                VoidListener,
            ),
            parser: Processor::new(),
            images: Images::default(),
        }
    }

    /// Takes what the server sent: kitty's graphics commands are kept as
    /// Ghostty keeps them (`Images`), each with the cursor where it came,
    /// and the rest goes to the emulator.
    pub fn feed(&mut self, bytes: &[u8]) {
        let mut all = std::mem::take(&mut self.images.pending);
        all.extend_from_slice(bytes);
        let mut at = 0;
        while let Some(start) = find(&all, b"\x1b_G", at) {
            self.parser.advance(&mut self.term, &all[at..start]);
            let Some(end) = find(&all, b"\x1b\\", start + 3) else {
                // Its end is still to come; a command no end comes for in
                // 64 MB is dropped.
                if all.len() - start < 64 << 20 {
                    self.images.pending = all[start..].to_vec();
                }
                return;
            };
            let cursor = self.term.grid().cursor.point;
            self.images.take(
                &all[start + 3..end],
                (cursor.line.0.max(0) as usize, cursor.column.0),
            );
            at = end + 2;
        }
        // An ESC, or `ESC _`, at the very end may open a command.
        let keep = if all.ends_with(b"\x1b_") {
            2
        } else if all.ends_with(b"\x1b") {
            1
        } else {
            0
        };
        let cut = all.len() - keep;
        if at < cut {
            self.parser.advance(&mut self.term, &all[at..cut]);
        }
        self.images.pending = all[cut.max(at)..].to_vec();
    }

    pub fn resize(&mut self, cols: u16, rows: u16) {
        self.term.resize(Size(cols as usize, rows as usize));
    }

    pub fn size(&self) -> (usize, usize) {
        let grid = self.term.grid();
        (grid.columns(), grid.screen_lines())
    }

    /// The screen as lines of a character a cell, trailing blanks trimmed:
    /// a wide character's second cell is a space, so a column is a column.
    pub fn lines(&self) -> Vec<String> {
        self.rows(true)
    }

    /// The screen as text to read: a wide character's second cell left
    /// out, as a terminal draws the character over both.
    pub fn text(&self) -> Vec<String> {
        self.rows(false)
    }

    fn rows(&self, spacers: bool) -> Vec<String> {
        let grid = self.term.grid();
        (0..grid.screen_lines())
            .map(|y| {
                let row = &grid[Line(y as i32)];
                let line: String = (0..grid.columns())
                    .filter(|x| spacers || !row[Column(*x)].flags.contains(Flags::WIDE_CHAR_SPACER))
                    .map(|x| row[Column(x)].c)
                    .collect();
                line.trim_end().to_string()
            })
            .collect()
    }

    /// The cell at a line and column, as alacritty has it.
    pub fn raw_cell(&self, line: usize, col: usize) -> &alacritty_terminal::term::cell::Cell {
        &self.term.grid()[Line(line as i32)][Column(col)]
    }

    /// A cell's text color when it is given as red, green and blue.
    pub fn fg_rgb(&self, line: usize, col: usize) -> Option<(u8, u8, u8)> {
        match self.raw_cell(line, col).fg {
            Color::Spec(c) => Some((c.r, c.g, c.b)),
            _ => None,
        }
    }

    /// The first cell holding `c`, as (line, column).
    pub fn find_cell(&self, c: char) -> Option<(usize, usize)> {
        let grid = self.term.grid();
        (0..grid.screen_lines()).find_map(|y| {
            (0..grid.columns())
                .find(|x| grid[Line(y as i32)][Column(*x)].c == c)
                .map(|x| (y, x))
        })
    }

    /// Every cell, line by line, colors resolved; a wide character's
    /// second cell is left out, as a terminal draws the character over both.
    pub fn cells(&self) -> Vec<Vec<Cell>> {
        let grid = self.term.grid();
        (0..grid.screen_lines())
            .map(|y| {
                let row = &grid[Line(y as i32)];
                (0..grid.columns())
                    .filter(|x| !row[Column(*x)].flags.contains(Flags::WIDE_CHAR_SPACER))
                    .map(|x| {
                        let cell = &row[Column(x)];
                        let inverse = cell.flags.contains(Flags::INVERSE);
                        let (mut fg, mut bg) = (rgb(cell.fg, DEFAULT_FG), rgb(cell.bg, DEFAULT_BG));
                        if inverse {
                            std::mem::swap(&mut fg, &mut bg);
                        }
                        Cell {
                            c: cell.c,
                            fg,
                            bg,
                            bold: cell.flags.contains(Flags::BOLD),
                            wide: cell.flags.contains(Flags::WIDE_CHAR),
                            marks: cell.zerowidth().map(<[char]>::to_vec).unwrap_or_default(),
                        }
                    })
                    .collect()
            })
            .collect()
    }

    /// The screen as text with its colors, as 24-bit SGR sequences, each
    /// line reset at its end.
    pub fn ansi(&self) -> String {
        let mut out = String::new();
        for row in self.cells() {
            let mut last: Option<Pen> = None;
            let end = row
                .iter()
                .rposition(|c| c.c != ' ' || c.bg != DEFAULT_BG)
                .map_or(0, |i| i + 1);
            for cell in &row[..end] {
                let now = (cell.fg, cell.bg, cell.bold);
                if last != Some(now) {
                    out.push_str(&format!(
                        "\x1b[0;{}38;2;{};{};{};48;2;{};{};{}m",
                        if cell.bold { "1;" } else { "" },
                        cell.fg.0,
                        cell.fg.1,
                        cell.fg.2,
                        cell.bg.0,
                        cell.bg.1,
                        cell.bg.2
                    ));
                    last = Some(now);
                }
                out.push(cell.c);
            }
            out.push_str("\x1b[0m\n");
        }
        out
    }

    /// The screen as an HTML page for Chrome to draw as a picture
    /// (`switchboard headless-screen --png`), as Ghostty draws it: the
    /// cells' backgrounds, then the images (a direct placement under the
    /// text, a placeholder's cell of its image), then the text, a run of
    /// cells of one style as one span in a font sized to the cell less its
    /// line gap, a character beyond ASCII centered in its cell or cells,
    /// and the half circles and block elements Ghostty draws itself drawn
    /// as shapes, since no font is sure to fit them to the cell.
    pub fn html(&self, cell_width: u32, cell_height: u32) -> String {
        let (cw, ch) = (cell_width, cell_height);
        let hex = |c: (u8, u8, u8)| format!("#{:02x}{:02x}{:02x}", c.0, c.1, c.2);
        let (mut back, mut under, mut pics, mut text) =
            (String::new(), String::new(), String::new(), String::new());
        let mut shown = std::collections::BTreeSet::new();
        let mut width = 0;
        for (y, row) in self.cells().iter().enumerate() {
            let top = y as u32 * ch;
            let mut x = 0u32;
            let placed: Vec<(u32, &Cell)> = row
                .iter()
                .map(|c| {
                    let at = x;
                    x += if c.wide { 2 } else { 1 };
                    (at, c)
                })
                .collect();
            width = width.max(x);
            let mut i = 0;
            while i < placed.len() {
                let (start, bg) = (placed[i].0, placed[i].1.bg);
                let j = (i..placed.len())
                    .find(|&j| placed[j].1.bg != bg)
                    .unwrap_or(placed.len());
                let end = placed.get(j).map_or(x, |p| p.0);
                if bg != DEFAULT_BG {
                    back.push_str(&format!(
                        "<p style=\"left:{}px;top:{top}px;width:{}px;height:{ch}px;background:{}\"></p>",
                        start * cw,
                        (end - start) * cw,
                        hex(bg)
                    ));
                }
                i = j;
            }
            let mut i = 0;
            while i < placed.len() {
                let (at, cell) = placed[i];
                let left = at * cw;
                if cell.c == crate::ui::images::PLACEHOLDER {
                    let id = id_of_color(cell.fg);
                    if self.images.by_id.contains_key(&id) {
                        shown.insert(id);
                        let (r, c) = (
                            mark_index(cell.marks.first()),
                            mark_index(cell.marks.get(1)),
                        );
                        pics.push_str(&format!(
                            "<p class=g{id} style=\"left:{left}px;top:{top}px;background-position:-{}px -{}px\"></p>",
                            c as u32 * cw,
                            r as u32 * ch
                        ));
                    }
                    i += 1;
                    continue;
                }
                if let Some(shape) = shape(cell.c, cw as f32, ch as f32) {
                    text.push_str(&format!(
                        "<p style=\"left:{left}px;top:{top}px;{shape};background:{}\"></p>",
                        hex(cell.fg)
                    ));
                    i += 1;
                    continue;
                }
                let style = |cell: &Cell| {
                    format!(
                        "left:{left}px;top:{top}px;color:{}{}",
                        hex(cell.fg),
                        if cell.bold { ";font-weight:bold" } else { "" }
                    )
                };
                // ASCII and the box-drawing characters, which every monospace
                // font has at its advance.
                let in_run = |c: &Cell| {
                    c.c == ' '
                        || c.c == '\0'
                        || c.c.is_ascii_graphic()
                        || ('\u{2500}'..='\u{257F}').contains(&c.c)
                };
                if in_run(cell) {
                    let mut j = i;
                    let mut run = String::new();
                    while j < placed.len()
                        && in_run(placed[j].1)
                        && placed[j].1.fg == cell.fg
                        && placed[j].1.bold == cell.bold
                    {
                        run.push(placed[j].1.c);
                        j += 1;
                    }
                    if !run.trim_matches([' ', '\0']).is_empty() {
                        text.push_str(&format!(
                            "<p class=t style=\"{}\">{}</p>",
                            style(cell),
                            escape(run.trim_end_matches([' ', '\0']))
                        ));
                    }
                    i = j;
                    continue;
                }
                text.push_str(&format!(
                    "<p class=c style=\"{};width:{}px\">{}</p>",
                    style(cell),
                    if cell.wide { 2 * cw } else { cw },
                    escape(&cell.c.to_string())
                ));
                i += 1;
            }
        }
        for p in &self.images.placed {
            if let Some(image) = self.images.by_id.get(&p.id) {
                under.push_str(&format!(
                    "<img class=u src=\"data:image/png;base64,{}\" style=\"left:{}px;top:{}px\">",
                    image.data,
                    p.col as f32 * cw as f32 + p.dx as f32 / SCALE as f32,
                    p.line as f32 * ch as f32 + p.dy as f32 / SCALE as f32,
                ));
            }
        }
        let rows = self.size().1 as u32;
        let (page_w, page_h) = (width * cw, rows * ch);
        let mut css = String::new();
        for id in shown {
            let image = &self.images.by_id[&id];
            css.push_str(&format!(
                ".g{id}{{width:{cw}px;height:{ch}px;background-image:url(data:image/png;base64,{});background-size:{}px {}px}}",
                image.data,
                image.cols as u32 * cw,
                image.rows as u32 * ch
            ));
        }
        let font = cell_height as f32 * FONT_OF_CELL;
        format!(
            "<!doctype html><meta charset=utf-8><style>html,body{{margin:0;background:{}}}\
             body{{position:relative;width:{page_w}px;height:{page_h}px;overflow:hidden}}\
             p{{position:absolute;margin:0}}\
             .t,.c{{height:{ch}px;white-space:pre;font:{font:.2}px/{ch}px 'JetBrains Mono',Menlo,'DejaVu Sans Mono',monospace}}\
             .t{{letter-spacing:{:.3}px}}.c{{text-align:center;overflow:visible}}{css}\
             .u{{position:absolute;transform:scale({});transform-origin:0 0}}</style>\
             <body>{back}{under}{pics}{text}</body>",
            hex(DEFAULT_BG),
            cell_width as f32 - font * MONO_ADVANCE,
            1.0 / SCALE as f32,
        )
    }
}

/// The font's size as a share of the cell's height: the cell less
/// Ghostty's line gap, which with a monospace font's advance of 0.6 em
/// makes a letter as wide as the cell.
const FONT_OF_CELL: f32 = 5.0 / 6.0;

/// A monospace font's advance, in ems (JetBrains Mono's, Menlo's and
/// DejaVu Sans Mono's), which the letter spacing makes up to the cell.
const MONO_ADVANCE: f32 = 0.6021;

/// How many pixels the PNG draws for each CSS pixel, so text and the
/// server's images are as sharp as on a Retina screen; the server rasters
/// its images at the cell's size times this.
pub const SCALE: u32 = 2;

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('\0', " ")
}

/// The CSS that draws `c` as a shape in a cell of `w` by `h`, for the half
/// circles and block elements Ghostty draws itself.
fn shape(c: char, w: f32, h: f32) -> Option<String> {
    let block = |x: f32, y: f32, bw: f32, bh: f32| {
        Some(format!(
            "margin-left:{x:.2}px;margin-top:{y:.2}px;width:{bw:.2}px;height:{bh:.2}px"
        ))
    };
    let half = h / 2.0;
    match c as u32 {
        0xE0B6 => Some(format!(
            "width:{w}px;height:{h}px;border-radius:{w}px 0 0 {w}px/{half}px 0 0 {half}px"
        )),
        0xE0B4 => Some(format!(
            "width:{w}px;height:{h}px;border-radius:0 {w}px {w}px 0/0 {half}px {half}px 0"
        )),
        0x2580 => block(0.0, 0.0, w, half),
        n @ 0x2581..=0x2588 => {
            let bh = h * (n - 0x2580) as f32 / 8.0;
            block(0.0, h - bh, w, bh)
        }
        n @ 0x2589..=0x258F => block(0.0, 0.0, w * (0x2590 - n) as f32 / 8.0, h),
        0x2590 => block(w / 2.0, 0.0, w / 2.0, h),
        0x2594 => block(0.0, 0.0, w, h / 8.0),
        0x2595 => block(w * 7.0 / 8.0, 0.0, w / 8.0, h),
        _ => None,
    }
}

/// A color as red, green and blue: given so, one of the 256 in xterm's
/// palette, or one of the named sixteen; `default` for the terminal's own.
pub fn rgb(color: Color, default: (u8, u8, u8)) -> (u8, u8, u8) {
    match color {
        Color::Spec(c) => (c.r, c.g, c.b),
        Color::Indexed(i) => indexed(i),
        Color::Named(n) => match n {
            NamedColor::Foreground | NamedColor::BrightForeground => DEFAULT_FG,
            NamedColor::Background => DEFAULT_BG,
            NamedColor::DimForeground => (0x90, 0x90, 0x90),
            n if (n as usize) < 16 => indexed(n as u8),
            _ => default,
        },
    }
}

/// xterm's 256 colors.
fn indexed(i: u8) -> (u8, u8, u8) {
    const BASE: [(u8, u8, u8); 16] = [
        (0, 0, 0),
        (205, 49, 49),
        (13, 188, 121),
        (229, 229, 16),
        (36, 114, 200),
        (188, 63, 188),
        (17, 168, 205),
        (229, 229, 229),
        (102, 102, 102),
        (241, 76, 76),
        (35, 209, 139),
        (245, 245, 67),
        (59, 142, 234),
        (214, 112, 214),
        (41, 184, 219),
        (255, 255, 255),
    ];
    match i {
        0..=15 => BASE[i as usize],
        16..=231 => {
            let i = i - 16;
            let level = |n: u8| if n == 0 { 0 } else { 55 + n * 40 };
            (level(i / 36), level((i / 6) % 6), level(i % 6))
        }
        _ => {
            let v = 8 + (i - 232) * 10;
            (v, v, v)
        }
    }
}

/// The terminal a headless client attaches as by default: Ghostty, so the
/// server draws for it what it keeps for Ghostty (its images, truecolor, the
/// half circles).
pub const TERM: &str = "xterm-ghostty";

/// What a client attaching as `term` says it is (`TERM`, `TERM_PROGRAM`
/// and `COLORTERM`): Ghostty as Ghostty says it, anything else by its
/// name alone.
pub fn attached_as(term: &str) -> String {
    if term.contains("ghostty") {
        format!("{term} ghostty truecolor")
    } else {
        term.to_string()
    }
}

/// The bytes typing `words` sends: each a key's name (`M-m`, `Enter`, `]`,
/// `C-c`) sent as a terminal sends that key, else typed as text.
pub fn key_bytes(words: &[String]) -> Vec<u8> {
    let mut out = Vec::new();
    for word in words {
        match Key::parse(word) {
            Some(k) => out.extend(encode_key(&k, &Default::default())),
            None => out.extend_from_slice(word.as_bytes()),
        }
    }
    out
}

// ── The client left running ─────────────────────────────────────────────────

/// A headless client's socket, in the state directory.
pub fn socket_of(state: &Path, id: u32) -> PathBuf {
    state.join(format!("headless-{id}.sock"))
}

/// The first id with no client answering on its socket.
pub fn free_id(state: &Path) -> u32 {
    (1..)
        .find(|id| UnixStream::connect(socket_of(state, *id)).is_err())
        .unwrap_or(1)
}

struct Shared {
    screen: Screen,
    /// When the server last sent anything.
    last: Instant,
    ended: Option<String>,
}

/// A client attached to a server, reading what it draws into a `Screen` on
/// a thread of its own.
pub struct Client {
    stream: UnixStream,
    shared: Arc<Mutex<Shared>>,
}

impl Client {
    /// Attaches to the server on `server` at `cols` by `rows`, in
    /// `session` or the one the server picks, as the terminal `term`
    /// (`attached_as`), its window as many pixels as the PNG draws, so the
    /// server sizes its images as for a real window.
    pub fn attach(
        server: &Path,
        cols: u16,
        rows: u16,
        session: Option<String>,
        term: &str,
    ) -> Result<Client> {
        let mut stream = UnixStream::connect(server).context("no server answers there")?;
        let pixels = |n: u16, cell: u32| (n as u32 * cell * SCALE).min(u16::MAX as u32) as u16;
        stream.write_all(&proto::encode_client(&ClientMsg::Attach {
            session,
            cols,
            rows,
            term: attached_as(term),
            xpixel: pixels(cols, CELL.0),
            ypixel: pixels(rows, CELL.1),
            cwd: None,
        }))?;
        let shared = Arc::new(Mutex::new(Shared {
            screen: Screen::new(cols, rows),
            last: Instant::now(),
            ended: None,
        }));
        let mut reader = stream.try_clone()?;
        let fed = shared.clone();
        std::thread::spawn(move || {
            loop {
                let msg = proto::read_server(&mut reader);
                let mut s = fed.lock().unwrap_or_else(|e| e.into_inner());
                match msg {
                    Ok(Some(ServerMsg::Output(bytes))) => {
                        s.screen.feed(&bytes);
                        s.last = Instant::now();
                    }
                    Ok(Some(ServerMsg::Reply { .. })) => {}
                    Ok(Some(ServerMsg::Exit { message })) => {
                        s.ended =
                            Some(message.unwrap_or_else(|| "the server ended the client".into()));
                        break;
                    }
                    _ => {
                        s.ended = Some("the server went away".into());
                        break;
                    }
                }
            }
        });
        let client = Client { stream, shared };
        client.settle();
        Ok(client)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Shared> {
        self.shared.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Why the server ended this client, once it has.
    pub fn ended(&self) -> Option<String> {
        self.lock().ended.clone()
    }

    /// Types `words` (`key_bytes`), one at a time as fingers do, each
    /// followed by the next only once what it opened has drawn.
    pub fn keys(&mut self, words: &[String]) -> Result<()> {
        for word in words {
            let bytes = key_bytes(std::slice::from_ref(word));
            self.stream
                .write_all(&proto::encode_client(&ClientMsg::Input(bytes)))
                .map_err(|_| anyhow!("the server went away"))?;
            self.settle();
        }
        Ok(())
    }

    /// Waits until the server has sent nothing for a moment (a screen drawn
    /// whole), at most two seconds.
    pub fn settle(&self) {
        let deadline = Instant::now() + Duration::from_secs(2);
        std::thread::sleep(Duration::from_millis(60));
        while Instant::now() < deadline {
            if self.lock().last.elapsed() >= Duration::from_millis(150) {
                return;
            }
            std::thread::sleep(Duration::from_millis(30));
        }
    }

    /// Waits up to `wait` for a line of the screen to hold `text`.
    pub fn wait_text(&self, text: &str, wait: Duration) -> bool {
        let deadline = Instant::now() + wait;
        loop {
            if self.read(|s| s.lines().iter().any(|l| l.contains(text))) {
                return true;
            }
            if Instant::now() >= deadline || self.ended().is_some() {
                return false;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Reads the screen as it is now.
    pub fn read<T>(&self, f: impl FnOnce(&Screen) -> T) -> T {
        f(&self.lock().screen)
    }

    pub fn detach(&mut self) {
        let _ = self
            .stream
            .write_all(&proto::encode_client(&ClientMsg::Detach));
    }
}

/// The screen drawn as a PNG in `file` by headless Chrome: each cell
/// `CELL` CSS pixels, `SCALE` pixels to each, as Ghostty draws it
/// (`Screen::html`).
pub fn png(screen: &Screen, file: &Path) -> Result<()> {
    let chrome = crate::views::image::render::any_chrome().ok_or_else(|| {
        anyhow!("drawing a PNG needs Google Chrome or Chromium, and neither is installed")
    })?;
    let (cols, rows) = screen.size();
    // A folder no one can guess and only you can open, made fresh, so
    // nothing another account put in the temp folder first is written
    // through (as the picture views' own).
    let work = crate::views::pictures::private_dir(std::process::id());
    if !work.is_dir() {
        bail!("couldn't make a folder of its own for Chrome");
    }
    let page = work.join("screen.html");
    crate::util::write_atomic(&page, screen.html(CELL.0, CELL.1))?;
    let file = if file.is_absolute() {
        file.to_path_buf()
    } else {
        std::env::current_dir()?.join(file)
    };
    let status = std::process::Command::new(chrome)
        .args([
            "--headless",
            "--hide-scrollbars",
            "--no-first-run",
            "--no-default-browser-check",
            "--disable-extensions",
            "--use-mock-keychain",
            "--password-store=basic",
        ])
        .arg(format!("--force-device-scale-factor={SCALE}"))
        .arg(format!(
            "--user-data-dir={}",
            work.join("profile").display()
        ))
        .arg(format!(
            "--window-size={},{}",
            cols as u32 * CELL.0,
            rows as u32 * CELL.1
        ))
        .arg(format!("--screenshot={}", file.display()))
        .arg(format!("file://{}", page.display()))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    let _ = std::fs::remove_dir_all(&work);
    match status {
        Ok(s) if s.success() && file.is_file() => Ok(()),
        _ => bail!("Chrome didn't draw {}", file.display()),
    }
}

/// A cell in the PNG, in CSS pixels.
pub const CELL: (u32, u32) = (9, 18);

/// Attaches to the server on `server` (`Client::attach`), then answers on
/// `listen` until told to stop or the server ends the client. `ready` is
/// called once it listens.
pub fn serve(
    server: &Path,
    listen: &Path,
    cols: u16,
    rows: u16,
    session: Option<String>,
    term: &str,
    ready: impl FnOnce(),
) -> Result<()> {
    let mut client = Client::attach(server, cols, rows, session, term)?;
    let _ = std::fs::remove_file(listen);
    let listener = UnixListener::bind(listen)
        .with_context(|| format!("couldn't listen on {}", listen.display()))?;
    // Yours alone, as the server's own socket is: whoever connects types
    // into the workspace and reads it.
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(listen, std::fs::Permissions::from_mode(0o600))?;
    }
    // Once the server ends the client (it was cleaned up, killed), wake
    // the listener with a connection of its own, so it ends rather than
    // waiting on a socket no one may reach again.
    let shared = client.shared.clone();
    let wake = listen.to_path_buf();
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(Duration::from_millis(250));
            if shared
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .ended
                .is_some()
            {
                let _ = UnixStream::connect(&wake);
                return;
            }
        }
    });
    ready();
    for conn in listener.incoming() {
        let Ok(mut conn) = conn else { continue };
        let mut line = String::new();
        if BufReader::new(&conn).read_line(&mut line).is_err() {
            continue;
        }
        let request: Value = serde_json::from_str(&line).unwrap_or(Value::Null);
        let (answer, stop) = answer(&request, &mut client);
        let _ = writeln!(conn, "{answer}");
        if stop {
            client.detach();
            break;
        }
        if client.ended().is_some() {
            break;
        }
    }
    let _ = std::fs::remove_file(listen);
    Ok(())
}

fn answer(request: &Value, client: &mut Client) -> (Value, bool) {
    if let Some(ended) = client.ended() {
        return (json!({ "error": ended }), true);
    }
    if request["stop"] == true {
        return (json!({ "ok": true }), true);
    }
    if let Some(keys) = request["keys"].as_array() {
        let words: Vec<String> = keys
            .iter()
            .filter_map(|k| k.as_str().map(str::to_string))
            .collect();
        return match client.keys(&words) {
            Ok(()) => (json!({ "ok": true }), false),
            Err(e) => (json!({ "error": e.to_string() }), true),
        };
    }
    if let Some(form) = request["screen"].as_str() {
        client.settle();
        return (
            client.read(|s| match form {
                "ansi" => json!({ "screen": s.ansi() }),
                "png" => match request["file"].as_str() {
                    Some(file) => match png(s, Path::new(file)) {
                        Ok(()) => json!({ "ok": true }),
                        Err(e) => json!({ "error": e.to_string() }),
                    },
                    None => json!({ "error": "no file to write the PNG to" }),
                },
                _ => json!({ "screen": s.text().join("\n") }),
            }),
            false,
        );
    }
    (
        json!({ "error": "not a request a headless client answers" }),
        false,
    )
}

/// Asks the headless client `id` something, giving its answer.
pub fn ask(state: &Path, id: u32, request: &Value) -> Result<Value> {
    let socket = socket_of(state, id);
    let mut stream = UnixStream::connect(&socket)
        .map_err(|_| anyhow!("no headless client {id} answers (at {})", socket.display()))?;
    stream.set_read_timeout(Some(Duration::from_secs(60)))?;
    writeln!(stream, "{request}")?;
    let mut line = String::new();
    BufReader::new(&stream).read_line(&mut line)?;
    let answer: Value = serde_json::from_str(&line).context("the client's answer isn't JSON")?;
    if let Some(error) = answer["error"].as_str() {
        bail!("{error}");
    }
    Ok(answer)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_screen_reads_text_and_colors_back() {
        let mut s = Screen::new(20, 3);
        s.feed(b"plain \x1b[38;2;255;0;0mred\x1b[0m\r\n\x1b[1;32mgreen");
        assert_eq!(s.lines(), ["plain red", "green", ""]);
        assert_eq!(s.fg_rgb(0, 6), Some((255, 0, 0)));
        let cells = s.cells();
        assert_eq!(cells[1][0].fg, indexed(2));
        assert!(cells[1][0].bold);
        let ansi = s.ansi();
        assert!(ansi.contains("38;2;255;0;0"), "{ansi:?}");
        assert!(s.html(8, 16).contains("color:#ff0000"));
    }

    /// An image `id` sent for placeholders over `cols` by `rows`, then its
    /// top-left placeholder cell at the screen's third column.
    fn placed(id: u32, cols: u16, rows: u16) -> Vec<u8> {
        let mut out =
            format!("\x1b_Ga=T,U=1,f=100,t=d,q=2,i={id},c={cols},r={rows},m=1;iVBO\x1b\\")
                .into_bytes();
        out.extend_from_slice(b"\x1b_Gm=0;Rw==\x1b\\");
        let (r, g, b) = ((id >> 16) as u8, (id >> 8) as u8, id as u8);
        out.extend(
            format!(
                "ab\x1b[38;2;{r};{g};{b}m{}\x1b[0m",
                crate::ui::images::placeholder(0, 0)
            )
            .into_bytes(),
        );
        out
    }

    #[test]
    fn an_image_placed_by_placeholders_is_drawn_at_its_cell_until_deleted() {
        let mut s = Screen::new(20, 3);
        let id = 0x80_0001;
        // Split across reads, as a socket may split it.
        let bytes = placed(id, 4, 2);
        let (first, second) = bytes.split_at(9);
        s.feed(first);
        s.feed(second);
        assert_eq!(s.lines()[0].chars().take(2).collect::<String>(), "ab");
        let html = s.html(8, 16);
        assert!(
            html.contains(&format!(
                ".g{id}{{width:8px;height:16px;background-image:url(data:image/png;base64,iVBORw==);background-size:32px 32px}}"
            )),
            "{html}"
        );
        assert!(
            html.contains(&format!("<p class=g{id} style=\"left:16px;top:0px")),
            "{html}"
        );
        assert!(s.text()[0].starts_with("ab"), "text leaves images out");

        s.feed(format!("\x1b_Ga=d,d=I,i={id},q=2\x1b\\").as_bytes());
        assert!(!s.html(8, 16).contains("base64"));
    }

    #[test]
    fn an_image_placed_directly_sits_at_the_cursor_with_its_offset() {
        let mut s = Screen::new(20, 4);
        s.feed(b"\x1b7\x1b[3;5H\x1b_Ga=T,f=100,t=d,q=2,i=9,X=4,Y=6,z=-1,C=1;iVBORw==\x1b\\\x1b8");
        let html = s.html(8, 16);
        let left = 4.0 * 8.0 + 4.0 / SCALE as f32;
        let top = 2.0 * 16.0 + 6.0 / SCALE as f32;
        assert!(
            html.contains(&format!(
                "<img class=u src=\"data:image/png;base64,iVBORw==\" style=\"left:{left}px;top:{top}px\">"
            )),
            "{html}"
        );
    }

    #[test]
    fn a_run_of_one_style_is_one_span_and_shapes_are_drawn() {
        let mut s = Screen::new(30, 2);
        s.feed("hello world \x1b[1mbold\x1b[0m \u{E0B6}x\u{2584}".as_bytes());
        let html = s.html(8, 16);
        assert!(html.contains(">hello world</p>"), "{html}");
        assert!(html.contains("font-weight:bold\">bold</p>"), "{html}");
        assert!(
            html.contains("border-radius:8px 0 0 8px/8px 0 0 8px"),
            "{html}"
        );
        assert!(
            html.contains("margin-top:8.00px;width:8.00px;height:8.00px"),
            "the lower half block: {html}"
        );
    }

    #[test]
    fn ghostty_says_it_draws_truecolor_and_others_only_their_name() {
        assert_eq!(attached_as(TERM), "xterm-ghostty ghostty truecolor");
        assert_eq!(attached_as("xterm-256color"), "xterm-256color");
        assert!(crate::mux::graphics::truecolor(&attached_as(TERM)));
        assert!(crate::mux::graphics::speaks_kitty(&attached_as(TERM)));
    }

    #[test]
    fn key_names_are_sent_as_a_terminal_sends_them() {
        let words: Vec<String> = ["M-m", "Enter", "hi"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(key_bytes(&words), b"\x1bm\rhi");
    }
}
