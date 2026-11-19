//! The Story view as one HTML page, for `switchboard diff-story --html`
//! and the diff-artifact skill: the view drawn by its own code, as the
//! diff alone draws it (its header line over What runs, the outline and
//! the column, every file of tests unfolded), tall enough for the whole
//! column, and each cell written out with its colors. So the page looks
//! as the diff does and follows it as it changes, with nothing drawn a
//! second way. It is drawn in the dark theme and the light, the page
//! showing the one the reader's browser asks for.

use std::sync::mpsc::channel;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier};

use super::{App, Options};
use crate::git::Change;
use crate::story::Story;
use switchboard_ui::draw::width;
use switchboard_ui::theme::rgb;

/// The terminal's own background and text in each theme, which a cell
/// left at the terminal's colors takes (crates/ui's `SET_COLORS` for the
/// dark).
const DARK: ([u8; 3], [u8; 3]) = ([0x15, 0x16, 0x1e], [0xc9, 0xcd, 0xd9]);
const LIGHT: ([u8; 3], [u8; 3]) = ([0xff, 0xff, 0xff], [0x2b, 0x2f, 0x3a]);

/// The story's Story view `cols` columns wide as an HTML page titled
/// `title`, or `None` when the story isn't told in chapters.
pub fn story_page(
    opts: Options,
    cols: u16,
    change: Change,
    story: Story,
    title: &str,
) -> Option<String> {
    let dark = draw(&opts, false, cols, change.clone(), story.clone())?;
    let light = draw(&opts, true, cols, change, story)?;
    Some(format!(
        "<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
         <title>{}</title>\n<style>\n{}\n</style>\n</head>\n<body>\n\
         <main>\n<pre class=\"dark\">{dark}</pre>\n<pre class=\"light\">{light}</pre>\n</main>\n\
         </body>\n</html>\n",
        escape(title),
        style(),
    ))
}

/// The page's own colors as tokens, dark under the reader's dark mode,
/// the light drawing shown otherwise.
fn style() -> String {
    let hex = |c: [u8; 3]| format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2]);
    format!(
        ":root {{ --bg: {lb}; --fg: {lf}; color-scheme: light dark; }}\n\
         @media (prefers-color-scheme: dark) {{ :root:not([data-theme=\"light\"]) {{ --bg: {db}; --fg: {df}; }} }}\n\
         :root[data-theme=\"dark\"] {{ --bg: {db}; --fg: {df}; }}\n\
         body {{ margin: 0; background: var(--bg); color: var(--fg); }}\n\
         main {{ overflow-x: auto; padding: 16px; }}\n\
         pre {{ margin: 0; font: 13px/1.25 ui-monospace, SFMono-Regular, Menlo, Consolas, monospace; width: max-content; }}\n\
         pre.dark {{ display: none; }}\n\
         @media (prefers-color-scheme: dark) {{ :root:not([data-theme=\"light\"]) pre.dark {{ display: block; }} :root:not([data-theme=\"light\"]) pre.light {{ display: none; }} }}\n\
         :root[data-theme=\"dark\"] pre.dark {{ display: block; }}\n\
         :root[data-theme=\"dark\"] pre.light {{ display: none; }}",
        lb = hex(LIGHT.0),
        lf = hex(LIGHT.1),
        db = hex(DARK.0),
        df = hex(DARK.1),
    )
}

/// The view drawn in one theme, as the cells' HTML.
fn draw(opts: &Options, light: bool, cols: u16, change: Change, story: Story) -> Option<String> {
    let opts = Options {
        light,
        map: true,
        alone: true,
        keep: false,
        haiku: false,
        server: false,
        clipboard: false,
        pointer: false,
        easy: false,
        hover: false,
        scroll_bars: false,
        ..*opts
    };
    let (tx, _rx) = channel();
    let mut app = App::new(opts, cols, 40, tx);
    app.loaded(change);
    app.told(story);
    if !app.on_story() {
        return None;
    }
    let outline = app.items().len() + 4;
    let rows = app.column_whole().max(outline) as u16;
    let bottom = u16::from(app.has_footer());
    app.rows = rows + bottom;
    app.message = None;
    let lead = app.lead();
    let mut buf = Buffer::empty(Rect::new(0, 0, cols, app.rows + lead));
    app.draw(&mut buf);
    // The row of buttons over the view is the screen's alone: the header,
    // then the view.
    let header = u16::from(app.has_header());
    let shown: Vec<u16> = (0..header).chain(lead..lead + rows).collect();
    Some(cells(&buf, &shown, if light { LIGHT } else { DARK }))
}

/// The buffer's `rows` as HTML, a span for each run of cells
/// drawn alike.
fn cells(buf: &Buffer, rows: &[u16], (bg, fg): ([u8; 3], [u8; 3])) -> String {
    let mut out = String::new();
    for &y in rows {
        let mut x = 0;
        let mut run = String::new();
        let mut look: Option<String> = None;
        let mut line = String::new();
        while x < buf.area.width {
            let cell = &buf[(x, y)];
            let symbol = cell.symbol();
            let wide = width(symbol).max(1) as u16;
            let this = css(cell.fg, cell.bg, cell.modifier, bg, fg);
            if look.as_ref() != Some(&this) {
                flush(&mut line, &look, &run);
                run.clear();
                look = Some(this);
            }
            run.push_str(&escape(if symbol.is_empty() { " " } else { symbol }));
            x += wide;
        }
        flush(&mut line, &look, &run);
        out.push_str(line.trim_end_matches(' '));
        out.push('\n');
    }
    out
}

fn flush(line: &mut String, look: &Option<String>, run: &str) {
    if run.is_empty() {
        return;
    }
    match look.as_deref() {
        Some("") | None => line.push_str(run),
        Some(style) => line.push_str(&format!("<span style=\"{style}\">{run}</span>")),
    }
}

/// A cell's look as inline CSS, empty for the terminal's own colors.
fn css(fg: Color, bg: Color, modifier: Modifier, page_bg: [u8; 3], page_fg: [u8; 3]) -> String {
    let pixels = |c: Color, or: [u8; 3]| match c {
        Color::Reset => or,
        other => rgb(other),
    };
    let (mut f, mut b) = (pixels(fg, page_fg), pixels(bg, page_bg));
    if modifier.contains(Modifier::REVERSED) {
        std::mem::swap(&mut f, &mut b);
    }
    let hex = |c: [u8; 3]| format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2]);
    let mut style = String::new();
    if f != page_fg {
        style.push_str(&format!("color:{};", hex(f)));
    }
    if b != page_bg {
        style.push_str(&format!("background:{};", hex(b)));
    }
    if modifier.contains(Modifier::BOLD) {
        style.push_str("font-weight:bold;");
    }
    if modifier.contains(Modifier::ITALIC) {
        style.push_str("font-style:italic;");
    }
    if modifier.contains(Modifier::DIM) {
        style.push_str("opacity:.6;");
    }
    if modifier.contains(Modifier::UNDERLINED) {
        style.push_str("text-decoration:underline;");
    }
    style
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
