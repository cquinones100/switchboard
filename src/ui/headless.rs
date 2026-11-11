//! A client with no terminal: it attaches to a server as a terminal would
//! and reads what the server draws through a terminal emulator of its own,
//! so a script, a test or a Claude can see the whole screen (the panes, the
//! sidebar, mission control, the boxes over them) and type into it, where
//! `capture-pane` sees one pane and `send-keys` reaches only a pane.
//!
//! - `Screen`: the emulator (alacritty_terminal, as the panes have) fed with
//!   what the server sent, read back as lines of text, as text with its
//!   colors (ANSI), or as cells for a picture. The integration tests' client
//!   (tests/common) reads through it too.
//! - `serve`: what `switchboard headless` leaves running: the client,
//!   attached at the size asked, and a socket of its own in the state
//!   directory, `headless-<id>.sock`, where `headless-keys` and
//!   `headless-screen` ask it (`ask`), one JSON line each way.
//!
//! A key is sent as a terminal sends it (`input::encode_key`, with no mode a
//! program asked for, since the server reads the client's keys itself), so
//! `M-m` reaches the server as it would from your keyboard.

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
/// blue, and whether it is bold.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cell {
    pub c: char,
    pub fg: (u8, u8, u8),
    pub bg: (u8, u8, u8),
    pub bold: bool,
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
        }
    }

    pub fn feed(&mut self, bytes: &[u8]) {
        self.parser.advance(&mut self.term, bytes);
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

    /// The screen as an HTML page of its cells, each line a row of spans in
    /// a monospace font, for Chrome to draw as a picture
    /// (`switchboard headless-screen --png`).
    pub fn html(&self, cell_width: u32, cell_height: u32) -> String {
        let mut body = String::new();
        for row in self.cells() {
            body.push_str("<div class=r>");
            for cell in row {
                let c = match cell.c {
                    '<' => "&lt;".to_string(),
                    '>' => "&gt;".to_string(),
                    '&' => "&amp;".to_string(),
                    ' ' | '\0' => "&nbsp;".to_string(),
                    c => c.to_string(),
                };
                body.push_str(&format!(
                    "<span style=\"color:#{:02x}{:02x}{:02x};background:#{:02x}{:02x}{:02x}{}\">{c}</span>",
                    cell.fg.0,
                    cell.fg.1,
                    cell.fg.2,
                    cell.bg.0,
                    cell.bg.1,
                    cell.bg.2,
                    if cell.bold { ";font-weight:bold" } else { "" }
                ));
            }
            body.push_str("</div>");
        }
        format!(
            "<!doctype html><meta charset=utf-8><style>html,body{{margin:0;background:#{:02x}{:02x}{:02x}}}\
             .r{{height:{cell_height}px;line-height:{cell_height}px;white-space:pre;font:{}px Menlo,'DejaVu Sans Mono',monospace}}\
             .r span{{display:inline-block;width:{cell_width}px;height:{cell_height}px;overflow:visible;text-align:center}}</style><body>{body}</body>",
            DEFAULT_BG.0,
            DEFAULT_BG.1,
            DEFAULT_BG.2,
            cell_height * 3 / 4,
        )
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
    /// `session` or the one the server picks.
    pub fn attach(server: &Path, cols: u16, rows: u16, session: Option<String>) -> Result<Client> {
        let mut stream = UnixStream::connect(server).context("no server answers there")?;
        stream.write_all(&proto::encode_client(&ClientMsg::Attach {
            session,
            cols,
            rows,
            term: "xterm-256color".into(),
            xpixel: 0,
            ypixel: 0,
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
/// `CELL` CSS pixels, in its colors (`Screen::html`).
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
    ready: impl FnOnce(),
) -> Result<()> {
    let mut client = Client::attach(server, cols, rows, session)?;
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

    #[test]
    fn key_names_are_sent_as_a_terminal_sends_them() {
        let words: Vec<String> = ["M-m", "Enter", "hi"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(key_bytes(&words), b"\x1bm\rhi");
    }
}
