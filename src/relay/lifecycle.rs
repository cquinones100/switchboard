//! What the relay is doing as a whole (lib/slack-relay/lifecycle.js):
//! stopping, leaving (stopping on purpose, with a last post and Home tab
//! update first), keeping the computer awake while it runs, the lock that
//! lets one relay listen at a time, and when the computer last woke.
//!
//! Stopping on purpose (`stop`, Ctrl-C, closing the tab, `slack-relay
//! stop`) edits the welcome message to say it stopped, and when, rather
//! than posting a new one (a restart edits it back), marks the Home tab out
//! of date and cancels the offline note, giving them all `STOP_GRACE` to
//! finish, then shuts down.

use std::time::Duration;

use super::{Cx, Relay};
use crate::util;

pub const STOP_GRACE: Duration = Duration::from_secs(15);

/// The relay as a whole.
#[derive(Default)]
pub struct Life {
    /// Shutting down: nothing new is started.
    pub stopping: bool,
    /// Stopping on purpose, saying so first.
    pub leaving: bool,
    /// When the computer last woke from sleep (see run.rs's heartbeat).
    pub woke_at: i64,
    /// The computer kept awake while this is held.
    pub awake: Option<crate::platform::KeepAwake>,
    /// What ends the run; the tests note it instead.
    pub on_exit: Option<Box<dyn Fn()>>,
}

/// A time as `toLocaleTimeString([], { hour: "numeric", minute: "2-digit" })`.
pub fn clock(ms: i64) -> String {
    util::local_at_ms(ms).strftime("%-I:%M %p").to_string()
}

pub fn stopped_text() -> String {
    format!(
        "🤖 **Relay stopped** at {}. Start it again at the computer with `switchboard slack-relay` or M-S.",
        clock(util::now_ms())
    )
}

/// Stops on purpose, as the module's comment says.
pub async fn stop_relay(cx: &Cx) {
    if cx.life.borrow().leaving {
        return;
    }
    cx.life.borrow_mut().leaving = true;
    cx.log("stopping");
    let (dm, welcome) = {
        let state = cx.state.borrow();
        (state.dm.is_some(), state.welcome.clone())
    };
    let last = async {
        if !dm {
            return;
        }
        if let Some(welcome) = welcome
            && let Err(e) = super::post::update(cx, &welcome, &stopped_text(), &[]).await
        {
            cx.log(&format!("couldn't mark the welcome stopped: {e}"));
        }
        if let Err(e) = super::dashboard::refresh_dashboard(
            cx,
            super::dashboard::Refresh {
                quick: true,
                stopped: true,
                ..Default::default()
            },
        )
        .await
        {
            cx.log(&format!("dashboard failed: {e}"));
        }
        super::offline::cancel_offline_note(cx).await;
    };
    let _ = tokio::time::timeout(STOP_GRACE, last).await;
    shutdown(cx);
}

/// Shuts down: the computer may sleep again, the hooks stop writing
/// events, and the lock is let go.
pub fn shutdown(cx: &Relay) {
    if cx.life.borrow().stopping {
        return;
    }
    cx.life.borrow_mut().stopping = true;
    cx.life.borrow_mut().awake = None;
    let _ = std::fs::remove_file(cx.paths.events_file());
    let ours = util::read_text(cx.paths.lock_file())
        .and_then(|t| t.split_whitespace().next().map(str::to_string))
        == Some(std::process::id().to_string());
    if ours {
        let _ = std::fs::remove_file(cx.paths.lock_file());
    }
    cx.log("stopped");
    match &cx.life.borrow().on_exit {
        Some(exit) => exit(),
        None => std::process::exit(0),
    }
}

/// The process the lock names, if it is alive.
pub fn lock_holder(paths: &super::Paths) -> Option<(i64, String)> {
    let text = util::read_text(paths.lock_file())?;
    let mut words = text.split_whitespace();
    let pid: i64 = words.next()?.parse().ok()?;
    let who = words.next().unwrap_or("").to_string();
    util::is_alive(pid).then_some((pid, who))
}

/// Takes the lock, or says why not: another relay listens already.
pub fn take_lock(paths: &super::Paths) -> Result<(), String> {
    if let Some((pid, who)) = lock_holder(paths)
        && pid != std::process::id() as i64
    {
        return Err(format!(
            "another listener is running (process {pid}, {who}); stop it first"
        ));
    }
    let _ = std::fs::create_dir_all(&paths.state_dir);
    util::write_private(
        paths.lock_file(),
        format!("{} slack-relay\n", std::process::id()),
        false,
    )
    .map_err(|e| format!("couldn't take the lock: {e}"))
}
