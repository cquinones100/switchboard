//! `switchboard slack-relay [start|stop|status|run|relay|notify|sends]`
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
//! slack-relay sends <needs-me|everything|quiet>
//!                          write the settings of one of the Slack relay
//!                          view's presets, which choose what reaches you,
//!                          and print what changed (see sending.rs)
//! ```
//!
//! `start` and `stop` take `--say`, for M-S and the palette: what goes
//! wrong (and for stop, that it is stopping) goes in a box on the screen,
//! since a key has nowhere to show output.
//!
//! The relay's tab is marked `@slack_relay` rather than known by its name,
//! which the tab's label can change. Stopping the relay closes its tab
//! (Settings' `relay_close_tab`); a tab left at a shell by a relay that
//! crashed, or with that setting off, is reused: its pane, marked
//! `@slack_relay` too, has its shell replaced by the relay, as the tab
//! first ran it (the server's `respawn-pane`), rather than the command
//! typed into a shell that may have had something half typed. A marked
//! tab with no marked pane (the relay's pane closed, a pane split beside it
//! keeping the tab) is unmarked and the relay opens in a new tab, so no
//! other program is replaced.

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

/// The pane in `tab` marked as the relay's, if it is still there.
fn relay_pane(server: &Server, tab: &str) -> Option<String> {
    let list: serde_json::Value = serde_json::from_str(&server.ask(&["list"]).ok()?).ok()?;
    list["sessions"]
        .as_array()?
        .iter()
        .flat_map(|s| s["tabs"].as_array().into_iter().flatten())
        .find(|t| t["id"] == tab)?["panes"]
        .as_array()?
        .iter()
        .find(|p| p["options"]["@slack_relay"] == "1")
        .and_then(|p| p["id"].as_str().map(String::from))
}

/// Whether a relay holds the lock and its process is alive.
pub fn running(paths: &Paths) -> bool {
    lock_holder(paths).is_some()
}

/// What the relay's tab runs: an interactive shell, so your rc puts
/// `claude` on the PATH for Haiku.
fn tab_command() -> Vec<String> {
    let shell = crate::worktree::work_shell();
    let line = run_command(
        &crate::worktree::self_command("slack-relay"),
        &crate::worktree::self_command("setting relay_close_tab"),
        &shell,
    );
    vec![shell, "-i".into(), "-c".into(), line]
}

/// The line the tab's shell runs: `<relay> run`, then, when it ended
/// cleanly (stopped from M-p, `slack-relay stop`, `stop` in the DM or
/// Ctrl-C) and `<setting>` prints `on`, nothing more, so the shell ends and
/// the tab closes; after a crash or a start the relay refused, or with the
/// setting off, the tab stays at a shell with what it said on the screen.
pub fn run_command(relay: &str, setting: &str, shell: &str) -> String {
    format!(
        "{relay} run && [ \"$({setting})\" = on ] && exit; exec {} -i",
        crate::worktree::shell_quote(shell)
    )
}

/// Goes to the relay's tab, or opens one running it. The running server
/// is used from anywhere (a Claude job, ssh), the new tab going in the
/// session used last; with no server running there is nowhere to put it.
pub fn start(server: &Server, paths: &Paths) -> Result<String, String> {
    if server.ask(&["list"]).is_err() {
        return Err("no Switchboard server is running; start the workspace first".into());
    }
    if let Some(existing) = relay_window(server) {
        if !running(paths) {
            let Some(pane) = relay_pane(server, &existing.id) else {
                // The relay's pane closed and a pane split beside it kept
                // the tab: no longer the relay's, its panes left alone.
                server
                    .set_option(&existing.id, "@slack_relay", None)
                    .map_err(|e| e.to_string())?;
                return start(server, paths);
            };
            // Left at its shell when the relay stopped: started there again,
            // in the shell's place.
            let home = crate::paths::home();
            let home = home.to_string_lossy();
            let mut args = vec!["respawn-pane", "-k", "-t", &pane, "-c", &home, "--"];
            let command = tab_command();
            args.extend(command.iter().map(String::as_str));
            server.ask(&args).map_err(|e| e.to_string())?;
        }
        server.select_tab(&existing.id).map_err(|e| e.to_string())?;
        return Ok(String::new());
    }
    let dir = crate::paths::home();
    let (tab, pane) = server
        .new_tab(&dir, Some("Slack relay"), Some(&tab_command()))
        .map_err(|e| e.to_string())?;
    let _ = server.set_option(&tab, "@slack_relay", Some("1"));
    let _ = server.set_option(&pane, "@slack_relay", Some("1"));
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
            // `--from` with nothing after it names no one, so the notice
            // goes out under `notice`, as it did.
            let from = rest.iter().position(|a| a == "--from").and_then(|at| {
                let value = rest.get(at + 1).cloned();
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
        "sends" => {
            let slugs = super::sending::Preset::ALL.map(|p| p.slug()).join(", ");
            let Some(preset) = args.get(1).and_then(|a| super::sending::Preset::of_slug(a)) else {
                eprintln!(
                    "Usage: switchboard slack-relay sends <{}>",
                    slugs.replace(", ", "|")
                );
                return ExitCode::from(2);
            };
            match super::sending::apply(preset) {
                Ok(changed) if changed.is_empty() => {
                    println!("{}: nothing to change", preset.label());
                    ExitCode::SUCCESS
                }
                Ok(changed) => {
                    println!("{}:", preset.label());
                    for (label, words) in changed {
                        println!("  {label}: {words}");
                    }
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
                "Usage: switchboard slack-relay [start|stop|status|run|notify [--from <name>] [<text>]|sends <needs-me|everything|quiet>]"
            );
            ExitCode::SUCCESS
        }
        // Said in the relay's log too, as bin/slack-relay's `fail` did.
        other => super::log::fail(
            &paths,
            &format!("unknown command \"{other}\"; use start, stop, status, run, notify or sends"),
            1,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::run_command;

    /// What the tab's line prints with `echo` standing in for the shell:
    /// `-i` when the shell is kept, nothing when the tab would close.
    fn after(relay: &str, setting: &str) -> String {
        let out = std::process::Command::new("sh")
            .args(["-c", &run_command(relay, setting, "echo")])
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    #[test]
    fn the_tab_closes_only_on_a_clean_stop_with_the_setting_on() {
        // `true run` and `false run`: the relay stopped, or crashed.
        assert_eq!(after("true", "echo on"), "");
        assert_eq!(after("false", "echo on"), "-i");
        assert_eq!(after("true", "echo off"), "-i");
        assert_eq!(after("true", "false"), "-i");
    }
}
