//! `switchboard slack-relay [start|stop|status|run|relay|notify]`
//! (bin/slack-relay and lib/slack-relay/cli.js).
//!
//! ```text
//! slack-relay              open the relay in a tab of its own, or go to it
//!                          if it is already running
//! slack-relay stop         stop it
//! slack-relay status       whether it runs, and since when it has listened
//! slack-relay run          the relay itself, restarted if it crashes;
//!                          `start` runs this in the new tab
//! slack-relay notify [--from <name>] [<text>]
//!                          post <text> (else what comes on stdin) in the DM
//!                          as the app, so Slack notifies you, through the
//!                          running relay; exits 0 once it is posted, 1 if
//!                          it wasn't (see outbox.rs)
//! ```
//!
//! `start` and `stop` take `--say`, for M-S and the palette: what goes
//! wrong (and for stop, that it is stopping) goes in a box on the screen,
//! since a key has nowhere to show output.
//!
//! The relay's tab is marked `@slack_relay` rather than known by its name,
//! which the tab's label can change. A tab left at a shell by a relay that
//! stopped is reused: its pane's shell is replaced by the relay, as the tab
//! first ran it (the server's `respawn-pane`), rather than the command
//! typed into a shell that may have had something half typed.

use std::io::Read;
use std::process::ExitCode;

use super::config::Paths;
use super::lifecycle::lock_holder;
use super::mux::{Mux, Server};
use crate::util;

/// The relay's tab, if one is open.
pub fn relay_window(server: &Server) -> Option<super::mux::Window> {
    server.windows()?.into_iter().find(|w| w.relay)
}

/// Whether a relay holds the lock and its process is alive.
pub fn running(paths: &Paths) -> bool {
    lock_holder(paths).is_some()
}

/// What the relay's tab runs: an interactive shell, so your rc puts
/// `claude` on the PATH for Haiku, which stays a shell if the relay stops.
fn tab_command() -> Vec<String> {
    let shell = crate::worktree::work_shell();
    let line = format!(
        "{} run; exec {} -i",
        crate::worktree::self_command("slack-relay"),
        crate::worktree::shell_quote(&shell)
    );
    vec![shell, "-i".into(), "-c".into(), line]
}

/// Goes to the relay's tab, or opens one running it. The running server
/// is used from anywhere (a Claude job, ssh), the new tab going in the
/// session used last; with no server running there is nowhere to put it.
pub fn start(server: &Server, paths: &Paths) -> Result<String, String> {
    if server.ask(&["list"]).is_err() {
        return Err("no Switchboard server is running; start the workspace first".into());
    }
    if let Some(existing) = relay_window(server) {
        if !running(paths)
            && let Some(pane) = existing.panes.first()
        {
            // Left at its shell when the relay stopped: started there again,
            // in the shell's place.
            let home = crate::paths::home();
            let home = home.to_string_lossy();
            let mut args = vec!["respawn-pane", "-k", "-t", &pane.id, "-c", &home, "--"];
            let command = tab_command();
            args.extend(command.iter().map(String::as_str));
            server.ask(&args).map_err(|e| e.to_string())?;
        }
        server.select_tab(&existing.id).map_err(|e| e.to_string())?;
        return Ok(String::new());
    }
    let dir = crate::paths::home();
    let (tab, _) = server
        .new_tab(&dir, Some("Slack relay"), Some(&tab_command()))
        .map_err(|e| e.to_string())?;
    let _ = server.set_option(&tab, "@slack_relay", Some("1"));
    server.select_tab(&tab).map_err(|e| e.to_string())?;
    Ok(String::new())
}

/// Stops the running relay, which says so in the DM first.
pub fn stop(paths: &Paths) -> Result<String, String> {
    let pid = util::read_text(paths.lock_file())
        .and_then(|t| {
            t.split_whitespace()
                .next()
                .and_then(|p| p.parse::<i64>().ok())
        })
        .unwrap_or(0);
    if pid <= 0 {
        return Err("the relay is not running".into());
    }
    // SAFETY: a signal to the process the lock names.
    let sent = unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) } == 0;
    if sent {
        Ok(format!("Stopping the relay (process {pid})."))
    } else {
        Err(format!("no process {pid}; the lock is stale"))
    }
}

/// Whether it runs, and where.
pub fn status(server: &Server, paths: &Paths) -> String {
    match lock_holder(paths) {
        Some((pid, _)) => {
            let tab = relay_window(server)
                .map(|w| format!(", in tab {} ({})", w.index, w.id))
                .unwrap_or_default();
            format!("The relay is running (process {pid}){tab}.")
        }
        None => "The relay is not running.".into(),
    }
}

/// Says a result in a box on the screen, through the server.
fn say(server: &Server, text: &str) {
    let _ = server.ask(&["say", text]);
}

pub fn main(args: Vec<String>) -> ExitCode {
    let paths = Paths::current();
    let server = Server::current();
    let saying = args.iter().any(|a| a == "--say");
    let args: Vec<String> = args.into_iter().filter(|a| a != "--say").collect();
    let command = args.first().map(String::as_str).unwrap_or("start");
    let outcome = |result: Result<String, String>, said: bool| -> ExitCode {
        match result {
            Ok(text) => {
                if !text.is_empty() {
                    if saying && said {
                        say(&server, &format!("slack-relay: {text}"));
                    } else {
                        println!("{text}");
                    }
                }
                ExitCode::SUCCESS
            }
            Err(why) => {
                if saying {
                    say(&server, &format!("slack-relay: {why}"));
                }
                eprintln!("Error: {why}");
                ExitCode::from(1)
            }
        }
    };
    match command {
        "start" => outcome(start(&server, &paths), false),
        "stop" => outcome(stop(&paths), true),
        "status" => {
            println!("{}", status(&server, &paths));
            ExitCode::SUCCESS
        }
        "run" => super::run::supervise(),
        "relay" => super::run::relay(),
        "notify" => {
            let mut rest: Vec<String> = args[1..].to_vec();
            let from = rest.iter().position(|a| a == "--from").map(|at| {
                let value = rest.get(at + 1).cloned().unwrap_or_default();
                rest.drain(at..(at + 2).min(rest.len()));
                value
            });
            let text = if rest.is_empty() {
                let mut text = String::new();
                let _ = std::io::stdin().read_to_string(&mut text);
                text
            } else {
                rest.join(" ")
            };
            match super::outbox::notify(&paths, &text, from.as_deref(), super::outbox::NOTIFY_WAIT)
            {
                Ok(ts) => {
                    println!("posted {ts}");
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("Error: {e}");
                    ExitCode::from(1)
                }
            }
        }
        "-h" | "--help" | "help" => {
            println!(
                "Usage: switchboard slack-relay [start|stop|status|run|notify [--from <name>] [<text>]]"
            );
            ExitCode::SUCCESS
        }
        other => {
            eprintln!("Error: unknown command \"{other}\"; use start, stop, status, run or notify");
            ExitCode::from(1)
        }
    }
}
