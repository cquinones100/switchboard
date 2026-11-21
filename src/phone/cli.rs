//! `switchboard phone [start|stop|status|test|run|pair <code>|devices|revoke <device>|preview]`.
//!
//! ```text
//! phone                  open the phone app in a tab of its own, or go to
//!                        it if it is already open; what stops it starting
//!                        (the setting off, phone.json, Tailscale) is said
//!                        first
//! phone stop             stop it
//! phone test             send a test notification to each phone that
//!                        turned notifications on, and say what came back
//! phone status           whether it runs, and where
//! phone run              the app itself, in the foreground; `start` runs
//!                        this in the new tab
//! phone pair <code>      pair the phone showing <code>, as typing it in
//!                        the workspace's box does
//! phone devices          the phones paired
//! phone revoke <device>  a phone's pairing taken away
//! phone preview [--port N] [--no-open]
//!                        the app from Switchboard's clone on 127.0.0.1
//!                        over made-up tabs, framed at a phone's size and
//!                        reloaded as its files are saved (`preview`), for
//!                        developing it; only in Switchboard's clone
//! ```
//!
//! `start`, `stop` and `test` take `--say`, for the palette: what goes wrong goes
//! in a box on the screen. Its tab is marked `@phone_app`.

use std::process::ExitCode;

use serde_json::Value;

use super::{config, pairing, tailnet};
use crate::mux::socket;
use crate::{platform, settings, util, worktree};

pub(crate) const MARK: &str = "@phone_app";

fn lock_file() -> std::path::PathBuf {
    config::state_dir().join("lock")
}

/// The app running, as its lock names it: its pid on the first line, then
/// the binary it runs from (`build_of_self`) and that build's version. A
/// lock written before the build was kept in it has the pid alone, which
/// reads as a build other than any.
pub(crate) struct Running {
    pub pid: i64,
    pub build: String,
    pub version: String,
}

/// The binary this process runs from, as `<path> <modified time in ns>`:
/// a build installed over the same path reads as another, which its
/// version alone wouldn't say of two builds of one commit.
pub(crate) fn build_of_self() -> String {
    let Ok(exe) = std::env::current_exe().and_then(|p| p.canonicalize()) else {
        return String::new();
    };
    let modified = std::fs::metadata(&exe)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_nanos());
    format!("{} {modified}", exe.to_string_lossy())
}

/// The app running, if one is.
pub(crate) fn running_app() -> Option<Running> {
    let text = util::read_text(lock_file())?;
    let mut lines = text.lines();
    let pid = lines.next()?.trim().parse::<i64>().ok()?;
    util::is_alive(pid).then(|| Running {
        pid,
        build: lines.next().unwrap_or("").to_string(),
        version: lines.next().unwrap_or("").to_string(),
    })
}

/// The pid of the app running, if one is.
fn running() -> Option<i64> {
    running_app().map(|r| r.pid)
}

/// What `status` says: whether it runs, and from which build.
pub(crate) fn status_words() -> String {
    match running_app() {
        Some(r) if r.version.is_empty() => {
            format!("The phone app is running (process {}).", r.pid)
        }
        Some(r) => {
            let exe = r.build.rsplit_once(' ').map_or(&*r.build, |(exe, _)| exe);
            format!(
                "The phone app is running (process {}, {} from {exe}).",
                r.pid, r.version
            )
        }
        None => "The phone app is not running.".into(),
    }
}

/// The command its tab runs: `phone run` from this binary, then the shell,
/// so the tab stays once it ends, showing why.
pub(crate) fn tab_command() -> Vec<String> {
    let shell = worktree::work_shell();
    let line = format!(
        "{} ; exec {} -i",
        worktree::self_command("phone run"),
        worktree::shell_quote(&shell)
    );
    vec![shell, "-i".into(), "-c".into(), line]
}

/// What has to be so before it starts: the setting on, phone.json, and
/// Tailscale running with certificates on. Gives what it serves on.
fn ready() -> Result<(config::Config, tailnet::Status), String> {
    if settings::setting("phone") != Some(Value::Bool(true)) {
        return Err("the phone app is off: turn on Settings' \"Phone app\" (phone)".into());
    }
    let config = config::read()?;
    let status = platform::tailscale_status().map_err(|e| format!("{e:#}"))?;
    let status = tailnet::status_from(&status)?;
    Ok((config, status))
}

fn tab_marked() -> Option<String> {
    let list: Value = serde_json::from_str(&socket::ask_ok(&["list"]).ok()?).ok()?;
    list["sessions"]
        .as_array()?
        .iter()
        .flat_map(|s| s["tabs"].as_array().into_iter().flatten())
        .find(|t| {
            t["panes"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|p| p["options"][MARK] == "1")
        })
        .and_then(|t| t["id"].as_str().map(str::to_string))
}

fn start() -> Result<String, String> {
    socket::ask_ok(&["list"])
        .map_err(|_| "no Switchboard server is running; start the workspace first".to_string())?;
    if running().is_some()
        && let Some(tab) = tab_marked()
    {
        socket::ask_ok(&["select-tab", "-t", &tab]).map_err(|e| e.to_string())?;
        return Ok(String::new());
    }
    ready()?;
    let home = crate::paths::home().to_string_lossy().into_owned();
    let command = tab_command();
    let mut args = vec!["new-tab", "-n", "Phone app", "-c", &home, "--"];
    args.extend(command.iter().map(String::as_str));
    let out = socket::ask_ok(&args).map_err(|e| e.to_string())?;
    let mut ids = out.split_whitespace();
    let (tab, pane) = (ids.next().unwrap_or(""), ids.next().unwrap_or(""));
    let _ = socket::ask_ok(&["set", "-t", pane, MARK, "1"]);
    let _ = socket::ask_ok(&["select-tab", "-t", tab]);
    Ok(String::new())
}

fn stop() -> Result<String, String> {
    let pid = running().ok_or("the phone app is not running")?;
    // SAFETY: a signal to the process the lock names, alive as just checked.
    unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
    Ok(format!("Stopping the phone app (process {pid})."))
}

#[cfg(feature = "net")]
fn run() -> ExitCode {
    let (config, status) = match ready() {
        Ok(ready) => ready,
        Err(why) => {
            super::log::line(&why);
            return ExitCode::from(1);
        }
    };
    if let Some(pid) = running() {
        super::log::line(&format!("already running (process {pid})"));
        return ExitCode::from(1);
    }
    if let Err(e) = util::make_dirs(config::state_dir()).and_then(|_| {
        let lock = format!(
            "{}\n{}\n{}\n",
            std::process::id(),
            build_of_self(),
            crate::update::version_line()
        );
        util::write_private(lock_file(), lock, false)
    }) {
        super::log::line(&format!("couldn't start: {e}"));
        return ExitCode::from(1);
    }
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(r) => r,
        Err(e) => {
            super::log::line(&format!("couldn't start: {e}"));
            return ExitCode::FAILURE;
        }
    };
    let code = runtime.block_on(async {
        let mut term =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).ok();
        let stopped = async {
            match term.as_mut() {
                Some(t) => {
                    t.recv().await;
                }
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            served = super::web::serve(status, config) => match served {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    super::log::line(&format!("stopped: {e:#}"));
                    ExitCode::from(1)
                }
            },
            _ = stopped => {
                super::log::line("stopped");
                ExitCode::SUCCESS
            }
            _ = tokio::signal::ctrl_c() => {
                super::log::line("stopped");
                ExitCode::SUCCESS
            }
        }
    });
    let _ = std::fs::remove_file(lock_file());
    // The listeners' tasks and the server's stream end with the process.
    runtime.shutdown_background();
    code
}

/// A test notification pushed to each listed subscription, from this
/// process (the key is the Keychain's, so `phone run` needn't be running):
/// what came of each, logged in phone.log as every push is.
#[cfg(feature = "net")]
fn test_push() -> Result<String, String> {
    use super::push::{self, Outcome};
    if push::listed_subscriptions().is_empty() {
        return Err("no phone has turned on notifications: open the app from its home screen icon and allow them".into());
    }
    let key = push::key().map_err(|e| format!("no push key: {e:#}"))?;
    let sent = util::block_on(push::send(
        &key,
        "Test notification from Switchboard",
        "",
        "",
    ));
    let words: Vec<String> = sent
        .iter()
        .map(|(device, outcome)| format!("{device}: {}", outcome.words()))
        .collect();
    let words = format!("Test notification: {}.", words.join("; "));
    if sent.iter().any(|(_, o)| matches!(o, Outcome::Sent(_))) {
        Ok(words)
    } else {
        Err(words)
    }
}

#[cfg(not(feature = "net"))]
fn test_push() -> Result<String, String> {
    Err("this build has no network support".into())
}

/// `phone preview`: the app's files in the clone this runs in, served on
/// 127.0.0.1 over made-up tabs until Ctrl-C, the frame opened in the
/// browser unless `--no-open`.
#[cfg(feature = "net")]
fn preview(args: &[String]) -> ExitCode {
    let fail = |why: String, code: u8| {
        eprintln!("switchboard phone preview: {why}");
        ExitCode::from(code)
    };
    let mut port = None;
    let mut open = true;
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--no-open" => open = false,
            "--port" => match args.next().and_then(|p| p.parse::<u16>().ok()) {
                Some(p) => port = Some(p),
                None => return fail("--port takes a port's number".into(), 2),
            },
            other => {
                return fail(
                    format!("unknown option \"{other}\"; use --port N or --no-open"),
                    2,
                );
            }
        }
    }
    let here = std::env::current_dir().unwrap_or_default();
    let dir = match super::preview::assets_dir(&here) {
        Ok(dir) => dir,
        Err(why) => return fail(why, 1),
    };
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(r) => r,
        Err(e) => return fail(format!("couldn't start: {e}"), 1),
    };
    let code = runtime.block_on(async {
        let listener = match super::preview::bind(port).await {
            Ok(l) => l,
            Err(e) => return fail(format!("{e:#}"), 1),
        };
        let url = match listener.local_addr() {
            Ok(at) => format!("http://{at}/preview"),
            Err(e) => return fail(e.to_string(), 1),
        };
        println!(
            "The phone preview of {} is at {url}; Ctrl-C stops it.",
            dir.display()
        );
        if open {
            println!("{}", crate::links::opening_words(&url));
            tokio::task::spawn_blocking(move || {
                if let Err(e) = crate::links::open_url_watched(&url) {
                    eprintln!("switchboard phone preview: couldn't open it: {e:#}");
                }
            });
        }
        tokio::select! {
            served = super::preview::serve(listener, dir) => match served {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => fail(format!("{e:#}"), 1),
            },
            _ = tokio::signal::ctrl_c() => ExitCode::SUCCESS,
        }
    });
    runtime.shutdown_background();
    code
}

#[cfg(not(feature = "net"))]
fn preview(_: &[String]) -> ExitCode {
    eprintln!("switchboard phone preview: this build has no network support");
    ExitCode::from(1)
}

#[cfg(not(feature = "net"))]
fn run() -> ExitCode {
    eprintln!("switchboard phone: this build has no network support");
    ExitCode::from(1)
}

fn say(text: &str) {
    let _ = socket::ask(&["say", text]);
}

pub fn main(args: Vec<String>) -> ExitCode {
    let saying = args.iter().any(|a| a == "--say");
    let args: Vec<String> = args.into_iter().filter(|a| a != "--say").collect();
    let outcome = |result: Result<String, String>| -> ExitCode {
        match result {
            Ok(text) => {
                if !text.is_empty() {
                    if saying {
                        say(&format!("Phone app: {text}"));
                    }
                    println!("{text}");
                }
                ExitCode::SUCCESS
            }
            Err(why) => {
                if saying {
                    say(&format!("Phone app: {why}"));
                }
                eprintln!("switchboard phone: {why}");
                ExitCode::from(1)
            }
        }
    };
    match args.first().map(String::as_str).unwrap_or("start") {
        "start" => outcome(start()),
        "stop" => outcome(stop()),
        "test" => outcome(test_push()),
        "status" => {
            println!("{}", status_words());
            ExitCode::SUCCESS
        }
        "run" => run(),
        "preview" => preview(&args[1..]),
        "pair" => match args.get(1) {
            Some(code) => match pairing::approve(code, util::now_ms()) {
                pairing::Approval::Paired(device) => outcome(Ok(format!(
                    "Paired {device}; it opens on your tabs in a moment."
                ))),
                other => outcome(Err(other.words())),
            },
            None => outcome(Err("pair takes the code your phone shows".into())),
        },
        "devices" => {
            let devices = pairing::devices();
            if devices.is_empty() {
                println!("No phone is paired.");
            }
            for d in devices {
                let when = util::local_at_ms(d.paired).strftime("%Y-%m-%d %H:%M");
                println!("{}\tpaired {when}", d.device);
            }
            ExitCode::SUCCESS
        }
        "revoke" => match args.get(1) {
            Some(device) => match pairing::revoke(device) {
                Ok(true) => outcome(Ok(format!("{device} is no longer paired."))),
                Ok(false) => outcome(Err(format!("{device} isn't paired"))),
                Err(e) => outcome(Err(e.to_string())),
            },
            None => outcome(Err("revoke takes a device's name".into())),
        },
        "-h" | "--help" | "help" => {
            println!(
                "Usage: switchboard phone [start|stop|status|test|run|pair <code>|devices|revoke <device>|preview [--port N] [--no-open]]"
            );
            ExitCode::SUCCESS
        }
        other => outcome(Err(format!(
            "unknown command \"{other}\"; use start, stop, status, test, run, pair, devices, revoke or preview"
        ))),
    }
}
