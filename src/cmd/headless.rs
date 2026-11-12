//! `switchboard headless`, `headless-keys` and `headless-screen`: a client
//! with no terminal, attached to a server as a terminal would be, for
//! driving and reading the whole screen (the panes, the sidebar, mission
//! control, the boxes) from a script or a Claude, which `send-keys` and
//! `capture-pane` can't, since they reach one pane. New here; the client is
//! src/ui/headless.rs.
//!
//!   switchboard headless [--size <cols>x<rows>] [--session <name>]
//!                        [--term <name>] [--yes]
//!       attaches a client, prints its id and leaves it running; it says it
//!       is Ghostty (xterm-ghostty) unless --term names another terminal,
//!       so the server draws its images and truecolor for it
//!   switchboard headless --stop <id>
//!   switchboard headless-keys <id> <key>...     keys by name: M-m ] j Enter
//!   switchboard headless-screen <id> [--ansi | --png <file>]
//!
//! It reaches the server the other commands do (`SWITCHBOARD`, else the
//! state directory's), so run it with a demo's or a try's prefix. The
//! workspace you are in is refused unless `--yes` is given, since a second
//! client there would take the size of its screen and see what you see.

use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use serde_json::json;

use crate::mux::socket;
use crate::paths;
use crate::ui::headless;

/// `<cols>x<rows>`.
pub fn parse_size(text: &str) -> Option<(u16, u16)> {
    let (c, r) = text.split_once('x')?;
    let (c, r) = (c.parse::<u16>().ok()?, r.parse::<u16>().ok()?);
    (c >= 20 && r >= 5).then_some((c, r))
}

fn fail(name: &str, e: anyhow::Error) -> ExitCode {
    eprintln!("switchboard {name}: {e:#}");
    ExitCode::FAILURE
}

/// `switchboard headless`.
pub fn main(args: Vec<String>) -> ExitCode {
    match start(&args) {
        Ok(code) => code,
        Err(e) => fail("headless", e),
    }
}

fn value<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .map(String::as_str)
}

fn start(args: &[String]) -> Result<ExitCode> {
    let state = paths::state_dir();
    if let Some(id) = value(args, "--stop") {
        let id: u32 = id
            .parse()
            .map_err(|_| anyhow!("{id} isn't a client's id"))?;
        headless::ask(&state, id, &json!({ "stop": true }))?;
        return Ok(ExitCode::SUCCESS);
    }
    let size = match value(args, "--size") {
        Some(s) => parse_size(s).ok_or_else(|| anyhow!("--size takes <cols>x<rows>"))?,
        None => (160, 45),
    };
    let session = value(args, "--session").map(str::to_string);
    let term = value(args, "--term").unwrap_or(headless::TERM).to_string();
    let server = socket::socket_path();
    if let Some(id) = value(args, "--serve") {
        let id: u32 = id
            .parse()
            .map_err(|_| anyhow!("{id} isn't a client's id"))?;
        headless::serve(
            &server,
            &headless::socket_of(&state, id),
            size.0,
            size.1,
            session,
            &term,
            || {},
        )?;
        return Ok(ExitCode::SUCCESS);
    }
    let in_workspace =
        std::env::var_os("SWITCHBOARD").is_some_and(|s| !s.is_empty() && Path::new(&s) == server);
    if in_workspace && !args.iter().any(|a| a == "--yes") {
        bail!(
            "that is the workspace you're in, where a second client would take its screen's size and see what you see; run it with a demo's or a try's prefix (switchboard demo --detached prints one), or give --yes"
        );
    }
    if UnixStream::connect(&server).is_err() {
        bail!("no server answers at {}", server.display());
    }
    std::fs::create_dir_all(&state)?;
    let id = headless::free_id(&state);
    let bin = std::env::current_exe().context("can't find the binary running")?;
    let mut c = Command::new(bin);
    c.args([
        "headless",
        "--serve",
        &id.to_string(),
        "--size",
        &format!("{}x{}", size.0, size.1),
        "--term",
        &term,
    ]);
    if let Some(s) = &session {
        c.args(["--session", s]);
    }
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(state.join(format!("headless-{id}.log")))?;
    c.stdin(Stdio::null()).stdout(log.try_clone()?).stderr(log);
    crate::demo::detach(&mut c);
    let mut child = c.spawn().context("couldn't start the client")?;
    let socket = headless::socket_of(&state, id);
    let deadline = Instant::now() + Duration::from_secs(10);
    while UnixStream::connect(&socket).is_err() {
        if let Ok(Some(_)) = child.try_wait() {
            bail!(
                "the client ended at once; {} says why",
                state.join(format!("headless-{id}.log")).display()
            );
        }
        if Instant::now() >= deadline {
            bail!("the client didn't start");
        }
        std::thread::sleep(Duration::from_millis(30));
    }
    // It runs on in a session of its own, waited for by no one here.
    std::thread::spawn(move || child.wait());
    println!("{id}");
    Ok(ExitCode::SUCCESS)
}

fn id_of(name: &str, args: &[String]) -> Result<u32, ExitCode> {
    match args.first().and_then(|a| a.parse().ok()) {
        Some(id) => Ok(id),
        None => {
            eprintln!("Usage: switchboard {name} <id> …");
            Err(ExitCode::from(2))
        }
    }
}

/// `switchboard headless-keys <id> <key>...`.
pub fn keys_main(args: Vec<String>) -> ExitCode {
    let id = match id_of("headless-keys", &args) {
        Ok(id) => id,
        Err(code) => return code,
    };
    match headless::ask(
        &paths::state_dir(),
        id,
        &json!({ "keys": args[1..].to_vec() }),
    ) {
        Ok(_) => ExitCode::SUCCESS,
        Err(e) => fail("headless-keys", e),
    }
}

/// `switchboard headless-screen <id> [--ansi | --png <file>]`.
pub fn screen_main(args: Vec<String>) -> ExitCode {
    let id = match id_of("headless-screen", &args) {
        Ok(id) => id,
        Err(code) => return code,
    };
    let rest = &args[1..];
    let request = if let Some(file) = value(rest, "--png") {
        let file = PathBuf::from(file);
        let file = if file.is_absolute() {
            file
        } else {
            std::env::current_dir().unwrap_or_default().join(file)
        };
        json!({ "screen": "png", "file": file.to_string_lossy() })
    } else if rest.iter().any(|a| a == "--ansi") {
        json!({ "screen": "ansi" })
    } else {
        json!({ "screen": "text" })
    };
    match headless::ask(&paths::state_dir(), id, &request) {
        Ok(answer) => {
            if let Some(text) = answer["screen"].as_str() {
                print!("{text}");
                if !text.ends_with('\n') {
                    println!();
                }
            }
            ExitCode::SUCCESS
        }
        Err(e) => fail("headless-screen", e),
    }
}
