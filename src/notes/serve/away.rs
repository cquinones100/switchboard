//! What Notes' own tab shows while Notes is beside another tab's pane (M-e),
//! ported from tmux/notes-away.sh: which tab has it, kept up to date, so the
//! tab stays in the sidebar and says where Notes went. The server starts it
//! in the tab, beside Notes, as Notes moves out (`switchboard notes away`,
//! marked `@notes_placeholder`), and ends it when Notes comes back in its
//! place.
//!
//! Enter or a click goes to the tab that has Notes; M-e brings Notes back
//! here, as it does anywhere. If Notes has been quit, Enter or a click
//! starts it again here, in this pane, which is marked Notes' then.
//!
//! With UX 2.0, Notes' own tab has no header (`Session::headerless`), so
//! this draws the header's "⌘ Commands" at its top right, a click there
//! opening the palette as M-p does.

use std::io::{Read, Write};
use std::os::unix::process::CommandExt;
use std::process::ExitCode;
use std::sync::mpsc::{RecvTimeoutError, channel};
use std::time::Duration;

use serde_json::Value;

use crate::mux::input::{Input, KeyCode, MouseButton, MouseKind, Parser};
use crate::mux::socket;

/// How often where Notes is is read again, so a move or a rename shows.
const EVERY: Duration = Duration::from_secs(2);

const BOLD: &str = "\x1b[1m";
const DIM: &str = "\x1b[38;5;244m";
const RESET: &str = "\x1b[0m";

/// Where Notes is: its pane, and its tab's number and label.
struct Where {
    pane: String,
    index: i64,
    label: String,
}

/// Notes' pane, as the server lists it.
fn where_notes() -> Option<Where> {
    let list: Value = serde_json::from_str(&socket::ask_ok(&["list"]).ok()?).ok()?;
    let marked = |p: &Value| {
        p["options"]["@notes_pane"]
            .as_str()
            .is_some_and(|v| !v.is_empty() && v != "0")
    };
    list["sessions"]
        .as_array()?
        .iter()
        .flat_map(|s| s["tabs"].as_array().into_iter().flatten())
        .find_map(|t| {
            let pane = t["panes"].as_array()?.iter().find(|p| marked(p))?;
            let label = t["label"].as_str().filter(|l| !l.is_empty());
            Some(Where {
                pane: pane["id"].as_str()?.to_string(),
                index: t["index"].as_i64().unwrap_or(0),
                label: label.unwrap_or("its pane").to_string(),
            })
        })
}

/// "⌘ Commands", and the columns it takes on the top row, when Settings
/// draw it: UX 2.0 and palette_button on.
const COMMANDS: &str = "⌘ Commands";

fn commands_at(cols: u16) -> Option<(u16, u16)> {
    // Read afresh, not from the process's cache, so a change in Settings
    // shows on the next draw.
    let all = crate::settings::read_settings();
    let value = |key: &str| all.iter().find(|s| s.key == key).map(|s| s.value.clone());
    let on = value("ux2") == Some(serde_json::Value::Bool(true))
        && value("palette_button") != Some(serde_json::Value::Bool(false));
    let w = crate::ui::draw::width(COMMANDS) as u16;
    (on && cols > w + 2).then(|| (cols - w - 1, cols - 1))
}

fn draw(out: &mut impl Write, at: Option<&Where>) {
    let (cols, rows) = crossterm::terminal::size().unwrap_or((80, 24));
    let mut screen = String::from("\x1b[H\x1b[2J");
    if let Some((x, _)) = commands_at(cols) {
        screen.push_str(&format!("\x1b[1;{}H{DIM}{COMMANDS}{RESET}\x1b[H", x + 1));
    }
    for _ in 0..rows / 3 {
        screen.push_str("\r\n");
    }
    match at {
        Some(w) => screen.push_str(&format!(
            "  {BOLD}Notes is beside tab {}{RESET}, {}\r\n\r\n  {DIM}Enter: go there   M-e: bring it back here{RESET}",
            w.index, w.label
        )),
        None => screen.push_str(&format!(
            "  {BOLD}Notes isn't open.{RESET}\r\n\r\n  {DIM}Enter: open it here{RESET}"
        )),
    }
    let _ = out.write_all(screen.as_bytes());
    let _ = out.flush();
}

/// A click on "⌘ Commands".
fn commands(input: &Input) -> bool {
    let cols = crossterm::terminal::size().map_or(80, |(c, _)| c);
    match (input, commands_at(cols)) {
        (Input::Mouse(m), Some((from, to))) => {
            m.kind == MouseKind::Press
                && m.button == MouseButton::Left
                && m.row == 0
                && (from..to).contains(&m.col)
        }
        _ => false,
    }
}

/// Enter, or a click.
fn goes(input: &Input) -> bool {
    match input {
        Input::Key(k) => k.code == KeyCode::Enter && k.mods.is_empty(),
        Input::Mouse(m) => m.kind == MouseKind::Press && m.button == MouseButton::Left,
        _ => false,
    }
}

/// The terminal as it was, clicks no longer asked for.
fn restore(out: &mut impl Write) {
    let _ = out.write_all(b"\x1b[?1000l\x1b[?1006l\x1b[?25h\x1b[H\x1b[2J");
    let _ = out.flush();
    let _ = crossterm::terminal::disable_raw_mode();
}

/// `switchboard notes away`.
pub fn main() -> ExitCode {
    let _ = crossterm::terminal::enable_raw_mode();
    let mut out = std::io::stdout();
    // Clicks come as escape sequences once asked for.
    let _ = out.write_all(b"\x1b[?25l\x1b[?1000h\x1b[?1006h");
    let (tx, rx) = channel::<Vec<u8>>();
    std::thread::spawn(move || {
        let mut stdin = std::io::stdin();
        let mut buf = [0u8; 1024];
        loop {
            match stdin.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if tx.send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
            }
        }
    });
    let mut parser = Parser::new();
    loop {
        let at = where_notes();
        draw(&mut out, at.as_ref());
        let go = match rx.recv_timeout(EVERY) {
            Ok(bytes) => {
                let mut read = parser.feed(&bytes);
                if parser.pending() {
                    read.extend(parser.flush());
                }
                if read.iter().any(|p| commands(&p.input)) {
                    let _ = socket::ask(&["press", "palette"]);
                    continue;
                }
                read.iter().any(|p| goes(&p.input))
            }
            Err(RecvTimeoutError::Timeout) => false,
            Err(RecvTimeoutError::Disconnected) => break,
        };
        if !go {
            continue;
        }
        match at {
            Some(w) => {
                let _ = socket::ask(&["select-pane", "-t", &w.pane]);
            }
            None => return start_notes(&mut out),
        }
    }
    restore(&mut out);
    ExitCode::SUCCESS
}

/// Notes started here, in this pane, which is Notes' from now on.
fn start_notes(out: &mut impl Write) -> ExitCode {
    restore(out);
    if let Ok(pane) = std::env::var("SWITCHBOARD_PANE") {
        let _ = socket::ask(&["set", "-t", &pane, "-u", "@notes_placeholder"]);
        let _ = socket::ask(&["set", "-t", &pane, "@notes_pane", "1"]);
    }
    let command = super::notes_command(None, None, None);
    let err = std::process::Command::new(&command[0])
        .args(&command[1..])
        .exec();
    eprintln!("switchboard notes: {err}");
    ExitCode::FAILURE
}
