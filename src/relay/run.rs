//! The relay running, and kept running (lib/slack-relay/run.js).
//!
//! `relay` is the relay itself: it takes the lock, turns the hooks' events
//! on, keeps the computer awake, opens the DM and says it is listening,
//! then hears Slack through Socket Mode and looks at the tabs every second
//! and whenever a hook writes an event, publishing the Home tab every ten
//! seconds when it changed and every minute regardless, and renewing the
//! offline note every two minutes.
//!
//! `supervise` keeps it running: it runs the relay as a child process and,
//! if that dies, starts it again and says so in the DM (the new run posts
//! that), or, after too many crashes in a row, says the relay is down. A
//! crash is anything but a clean exit: stop, Ctrl-C and closing the tab end
//! it for good. Waits grow between restarts, so a relay that can't reach
//! Slack doesn't spin.

use std::cell::{Cell, RefCell};
use std::future::Future;
use std::path::PathBuf;
use std::process::ExitCode;
use std::rc::Rc;
use std::time::Duration;

use anyhow::Result;
use indexmap::IndexSet;
use serde_json::Value;

use super::buttons::welcome_buttons;
use super::commands::handle;
use super::config::{MAX_LOG_BYTES, Paths, TAB_POLL_MS};
use super::dashboard::{DASHBOARD_MS, HEARTBEAT_MS, Refresh, refresh_dashboard};
use super::lifecycle::{clock, stop_relay};
use super::log::{DONT_RESTART, fail};
use super::post::{PostOpts, post, update};
use super::slack::{self, APP_TOKEN_SERVICE, BOT_TOKEN_SERVICE, Message, Slack};
use super::state::{Dm, number};
use super::{Cx, Memory, Relay, mux, presence, summaries};
use crate::util;

thread_local! {
    /// The work started in the background, so the tests can wait for it.
    static TASKS: RefCell<Vec<tokio::task::JoinHandle<()>>> = const { RefCell::new(Vec::new()) };
    /// The messages seen, so each is handled once however it arrived.
    static HANDLED: RefCell<IndexSet<String>> = RefCell::new(IndexSet::new());
}

/// Runs `work` beside everything else, on the relay's thread.
pub fn spawn(work: impl Future<Output = ()> + 'static) {
    let handle = tokio::task::spawn_local(work);
    TASKS.with(|t| {
        let mut tasks = t.borrow_mut();
        tasks.retain(|h| !h.is_finished());
        tasks.push(handle);
    });
}

/// Waits for the work started in the background, and the work it started.
pub async fn settle() {
    loop {
        let tasks: Vec<_> = TASKS.with(|t| t.borrow_mut().drain(..).collect());
        if tasks.is_empty() {
            return;
        }
        for t in tasks {
            let _ = t.await;
        }
    }
}

impl Relay {
    /// A relay with its files under `paths`, reaching Slack, the tabs and
    /// Haiku through what it is given.
    pub fn new(
        paths: Paths,
        transport: Box<dyn slack::Transport>,
        mux: Box<dyn mux::Mux>,
        haiku: Box<dyn summaries::Haiku>,
        presence: presence::Presence,
        echo: bool,
    ) -> Relay {
        let state = super::state::load(&paths);
        Relay {
            paths,
            state: RefCell::new(state),
            slack: Slack::new(transport),
            mux,
            haiku,
            presence,
            settings: Box::new(crate::settings::setting),
            log: super::log::Log::new(echo),
            life: RefCell::new(Default::default()),
            mem: RefCell::new(Memory::default()),
            homing: RefCell::new(Default::default()),
            started_at: util::now_ms(),
            cwd: std::env::current_dir().unwrap_or_else(|_| crate::paths::home()),
        }
    }
}

// ── The dispatcher ──────────────────────────────────────────────────────────

/// Handles what Slack sent or a catch-up found: each message once, however
/// it arrived, in ts order, one at a time. The cursor and threads advance
/// past everything seen, the relay's own posts included, before anything is
/// acted on, so a failure in one message doesn't replay the rest.
pub fn process_messages(cx: &Cx, messages: Vec<Message>) {
    let mut fresh: Vec<Message> = HANDLED.with(|h| {
        let mut handled = h.borrow_mut();
        let fresh: Vec<Message> = messages
            .into_iter()
            .filter(|m| !handled.contains(&m.ts))
            .collect();
        for m in &fresh {
            handled.insert(m.ts.clone());
        }
        // Only a catch-up overlapping live events repeats a message, so
        // recent ones are enough.
        while handled.len() > 500 {
            handled.shift_remove_index(0);
        }
        fresh
    });
    fresh.sort_by(|a, b| number(&a.ts).total_cmp(&number(&b.ts)));
    {
        let mut state = cx.state.borrow_mut();
        for m in &fresh {
            match &m.thread_ts {
                Some(thread) => {
                    state.newest_in(thread, &m.ts);
                    if let Some(handled) = state.threads.get_mut(thread)
                        && number(&m.ts) > number(handled)
                    {
                        *handled = m.ts.clone();
                    }
                }
                None => {
                    if number(&m.ts) > number(state.cursor.as_deref().unwrap_or("0")) {
                        state.cursor = Some(m.ts.clone());
                    }
                }
            }
        }
    }
    let mine: Vec<Message> = fresh.into_iter().filter(|m| !m.by_claude).collect();
    if !mine.is_empty() {
        cx.state.borrow_mut().last_activity = util::now_ms();
        // Writing from Slack counts as away for a while.
        let cx2 = cx.clone();
        spawn(async move { presence::from_slack(&cx2).await });
    }
    cx.save();
    for message in mine {
        let cx = cx.clone();
        spawn(async move {
            let turn = cx.mem_turn();
            let _turn = turn.lock().await;
            let thread = message.thread_ts.clone();
            if let Err(e) = handle(&cx, message).await {
                cx.log(&format!("failed: {e}"));
                let _ = post(
                    &cx,
                    &format!("🤖 That failed: {e}"),
                    thread.as_deref(),
                    PostOpts::default(),
                )
                .await;
            }
            cx.save();
        });
    }
}

thread_local! {
    static TURN: Rc<tokio::sync::Mutex<()>> = Rc::new(tokio::sync::Mutex::new(()));
}

impl Relay {
    /// The one message handled at a time, in order: tokio's lock is taken
    /// in the order asked for.
    fn mem_turn(&self) -> Rc<tokio::sync::Mutex<()>> {
        TURN.with(Rc::clone)
    }
}

// ── What Slack sends ────────────────────────────────────────────────────────

/// The app's ID, which the welcome message's Home link needs, comes only
/// with an event from Slack. The first one from the relay's workspace
/// records it, and the welcome message gets its Home button.
fn learn_app(cx: &Cx, app: Option<&str>, team: Option<&str>) {
    let Some(app) = app.filter(|a| !a.is_empty()) else {
        return;
    };
    let dm = cx.state.borrow().dm();
    if team != dm.team.as_deref() || dm.app_id.as_deref() == Some(app) {
        return;
    }
    if let Some(d) = cx.state.borrow_mut().dm.as_mut() {
        d.app_id = Some(app.to_string());
    }
    cx.save();
    cx.log(&format!("app {app}"));
    let welcome = cx.state.borrow().welcome.clone();
    if let Some(welcome) = welcome {
        let cx = cx.clone();
        spawn(async move {
            let text = cx.mem.borrow().welcome.clone();
            let dm = cx.state.borrow().dm();
            if let Err(e) = update(&cx, &welcome, &text, &welcome_buttons(&dm)).await {
                cx.log(&format!("couldn't add Home to the welcome: {e}"));
            }
        });
    }
}

fn quiet(cx: &Relay) -> bool {
    let life = cx.life.borrow();
    life.stopping || life.leaving
}

/// An event from Socket Mode: you opened the Home tab, or a message.
pub fn on_event(cx: &Cx, event: Value, team: Option<String>, app: Option<String>) {
    if quiet(cx) {
        return;
    }
    learn_app(cx, app.as_deref(), team.as_deref());
    let text = |k: &str| event.get(k).and_then(Value::as_str);
    let dm = cx.state.borrow().dm();
    // You opened the app's Home tab: show it as it is now.
    if text("type") == Some("app_home_opened")
        && text("user") == Some(dm.user.as_str())
        && text("tab") == Some("home")
    {
        let cx = cx.clone();
        spawn(async move {
            if let Err(e) = refresh_dashboard(&cx, Refresh::opened_quick()).await {
                cx.log(&format!("Home tab failed: {e}"));
            }
        });
        return;
    }
    if let Some(message) = slack::message_from(&dm, &event, team.as_deref(), |l| cx.log(l)) {
        process_messages(cx, vec![message]);
    }
}

/// A button you pressed, acted on as the reply it stands for, or a form
/// sent from one of the relay's screens. A screen you closed is no longer
/// kept up to date.
pub fn on_interactive(cx: &Cx, payload: Value) {
    if quiet(cx) {
        return;
    }
    let text = |p: &str| payload.pointer(p).and_then(Value::as_str);
    learn_app(
        cx,
        text("/api_app_id"),
        text("/team/id").or_else(|| text("/user/team_id")),
    );
    if text("/type") == Some("view_closed") {
        if let Some(view) = text("/view/id") {
            super::screens::forget_screen(cx, view);
        }
        return;
    }
    let dm = cx.state.borrow().dm();
    let press = slack::press_from(&dm, &payload, |l| cx.log(l));
    let form = if press.is_none() {
        slack::submission_from(&dm, &payload, |l| cx.log(l))
    } else {
        None
    };
    if press.is_none() && form.is_none() {
        return;
    }
    cx.state.borrow_mut().last_activity = util::now_ms();
    let cx2 = cx.clone();
    spawn(async move { presence::from_slack(&cx2).await });
    let cx = cx.clone();
    spawn(async move {
        let done = match (press, form) {
            (Some(press), _) => super::presses::handle_press(&cx, press).await,
            (None, Some(form)) => super::screens::handle_submission(&cx, &form).await,
            _ => Ok(()),
        };
        if let Err(e) = done {
            cx.log(&format!("press failed: {e}"));
        }
        cx.save();
    });
}

/// Every (re)connection catches up on what was sent while the relay
/// couldn't hear.
pub fn on_connect(cx: &Cx) {
    let cx = cx.clone();
    spawn(async move {
        match super::read::catch_up(&cx).await {
            Ok(messages) => process_messages(&cx, messages),
            Err(e) => cx.log(&format!("catching up failed: {e}")),
        }
    });
}

// ── The tabs ────────────────────────────────────────────────────────────────

/// Looks at the tabs and the hooks' events once for all the parts that
/// report on them; one pass at a time, and a pass asked for while one runs
/// runs right after it.
pub async fn tab_tick(cx: &Cx) {
    thread_local! {
        static BUSY: Cell<bool> = const { Cell::new(false) };
        static AGAIN: Cell<bool> = const { Cell::new(false) };
    }
    if quiet(cx) {
        return;
    }
    if BUSY.with(|b| b.replace(true)) {
        AGAIN.with(|a| a.set(true));
        return;
    }
    loop {
        let tabs = super::tabs::claude_tabs(cx);
        let mut events = super::followups::new_events(cx);
        super::followups::follow_ups(cx, &tabs, &mut events).await;
        // Looked at again after the follow-ups, which may have just reported
        // a turn whose end changed the tab's state.
        let now = super::tabs::claude_tabs(cx);
        super::alerts::alert_tabs(cx, &now, &events).await;
        super::homes::refresh_homes(cx, &now).await;
        super::milestones::milestones(cx, &tabs).await;
        super::stuck::stuck_tabs(cx, &tabs).await;
        super::screens::refresh_screens(cx, &now).await;
        super::outbox::deliver_outbox(cx).await;
        if !AGAIN.with(|a| a.replace(false)) || quiet(cx) {
            break;
        }
    }
    BUSY.with(|b| b.set(false));
}

// ── Starting ────────────────────────────────────────────────────────────────

pub fn welcome_text(restarted: &str) -> String {
    if restarted.is_empty() {
        format!(
            "🤖 **Relay listening** since {}\n\nEach tab has its own thread here; reply in it to talk to that tab. **Home** shows every tab at a glance, and Open on one gives its controls; **Help** lists every command.",
            clock(util::now_ms())
        )
    } else {
        format!(
            "🤖 **Relay restarted** after a crash, and still listening.\n\nWhat went wrong: {restarted}"
        )
    }
}

/// A start on purpose edits the last welcome back, when it is less than
/// this old, rather than posting another, so restarts don't fill the DM.
pub const WELCOME_REUSE_MS: i64 = 24 * 60 * 60_000;

/// Says the relay is listening, and gives where reading the DM starts
/// from: now, when the last welcome is reused, since everything before was
/// handled. A first start, an old welcome, or a crash posts a new one: a
/// crash is news. A link Slack won't take would fail the whole post, so if
/// it does, the welcome goes out without Home rather than not at all.
pub async fn say_listening(cx: &Cx, welcome: &str, restarted: &str) -> Result<String> {
    let dm = cx.state.borrow().dm();
    let last = cx.state.borrow().welcome.clone();
    if restarted.is_empty()
        && let Some(last) = last
        && util::now_ms() - ((number(&last) * 1000.0) as i64) < WELCOME_REUSE_MS
    {
        match update(cx, &last, welcome, &welcome_buttons(&dm)).await {
            Ok(_) => return Ok(format!("{:.6}", util::now_ms() as f64 / 1000.0)),
            Err(e) => cx.log(&format!("couldn't reuse the welcome: {e}")),
        }
    }
    let kickoff = match post(cx, welcome, None, PostOpts::buttons(welcome_buttons(&dm))).await {
        Ok(ts) => ts,
        Err(e) => {
            cx.log(&format!("welcome refused with Home: {e}"));
            let plain = Dm { app_id: None, ..dm };
            post(
                cx,
                welcome,
                None,
                PostOpts::buttons(welcome_buttons(&plain)),
            )
            .await?
        }
    };
    cx.state.borrow_mut().welcome = Some(kickoff.clone());
    Ok(kickoff)
}

/// A gap this long between one-second beats means the computer was asleep.
const SLEEP_GAP_MS: i64 = 30_000;

/// `slack-relay relay`: the relay itself, run by `supervise`.
pub fn relay() -> ExitCode {
    let paths = Paths::current();
    let server = mux::Server::current();
    if server.ask(&["list"]).is_err() {
        return fail(
            &paths,
            "no Switchboard server is running; start the workspace first, then the relay with `switchboard slack-relay`",
            DONT_RESTART,
        );
    }
    // A crash is logged, and the exit tells supervise to start it again.
    let log_paths = paths.clone();
    std::panic::set_hook(Box::new(move |info| {
        let at = info
            .location()
            .map(|l| format!(" at {}:{}", l.file(), l.line()))
            .unwrap_or_default();
        let why = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "a panic".into());
        let _ = fail(&log_paths, &format!("crashed: {why}{at}"), 1);
        std::process::exit(1);
    }));
    super::state::owner_only(&paths);
    if let Err(why) = super::lifecycle::take_lock(&paths) {
        return fail(&paths, &why, DONT_RESTART);
    }
    // Whose relay this is, and the app's tokens: without them it can't
    // start, and restarting won't help, so it says what to set up. The
    // owner first, from a file, so a home without one never asks the
    // keyring.
    let Some(owner) = slack::load_owner(&paths) else {
        let _ = std::fs::remove_file(paths.lock_file());
        return fail(
            &paths,
            &format!(
                "no owner in {}; it should hold {{ \"owner\": \"<your Slack member ID>\" }}",
                paths.config_file().display()
            ),
            DONT_RESTART,
        );
    };
    let (Some(bot), Some(app)) = (
        slack::token(BOT_TOKEN_SERVICE),
        slack::token(APP_TOKEN_SERVICE),
    ) else {
        let _ = std::fs::remove_file(paths.lock_file());
        return fail(
            &paths,
            &format!(
                "the Slack app's tokens aren't stored; store them with: switchboard platform secret-set {BOT_TOKEN_SERVICE} -a \"$USER\" (it reads the token from stdin), and the same for {APP_TOKEN_SERVICE}"
            ),
            DONT_RESTART,
        );
    };
    let https = match super::http::Https::new(slack::api_base(), bot) {
        Ok(h) => h,
        Err(e) => return fail(&paths, &e.to_string(), DONT_RESTART),
    };
    let haiku = summaries::ClaudeHaiku {
        dir: paths.state_dir.clone(),
    };
    let relay = Relay::new(
        paths.clone(),
        Box::new(https),
        Box::new(server),
        Box::new(haiku),
        presence::Presence::new(),
        true,
    );
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(r) => r,
        Err(e) => return fail(&paths, &format!("couldn't start: {e}"), 1),
    };
    let local = tokio::task::LocalSet::new();
    let cx = Rc::new(relay);
    let result = local.block_on(&runtime, run(cx.clone(), owner, app));
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => fail(&paths, &format!("crashed: {e}"), 1),
    }
}

/// The relay's life once it has what it needs: everything the module's
/// comment says, until it is stopped (which ends the process).
async fn run(cx: Cx, owner: String, app_token: String) -> Result<()> {
    // After a crash, why, from supervise; empty on a normal start.
    let restarted = std::env::var("SLACK_RELAY_RESTARTED").unwrap_or_default();
    // Turn the hooks on: they write events only while this file exists. One
    // left by a crashed relay is kept, so no turn that ended meanwhile is
    // lost.
    let events = cx.paths.events_file();
    let kept_events = events.exists();
    if !kept_events {
        util::write_private(&events, "", false)?;
    }
    // Mark this tab as the relay's, so `slack-relay` finds it whatever the
    // tab ends up named, and the relay never reports on itself, and its pane,
    // the one `slack-relay start` runs it in again once it has stopped.
    if let Ok(pane) = std::env::var("SWITCHBOARD_PANE")
        && !pane.is_empty()
    {
        let tab = cx
            .mux
            .windows()
            .unwrap_or_default()
            .into_iter()
            .find(|w| w.panes.iter().any(|p| p.id == pane))
            .map(|w| w.id);
        if let Some(tab) = tab {
            let _ = cx.mux.set_option(&tab, "@slack_relay", Some("1"));
            let _ = cx.mux.set_option(&pane, "@slack_relay", Some("1"));
        }
    }
    // The computer awake while the relay runs, and no longer. Not the
    // displays: they sleep as usual, which matters on a laptop with its lid
    // closed, which stays awake only while an external display is
    // connected.
    cx.life.borrow_mut().awake = crate::platform::keep_awake("The Slack relay is running");
    {
        let mut state = cx.state.borrow_mut();
        // When you last wrote, for the quick checks' pace: a fresh start
        // counts as now, so the relay is quick while you check it came up.
        state.last_activity = util::now_ms();
        if !kept_events {
            state.events_offset = 0;
        }
    }
    // Closing the relay's tab sends SIGHUP; that is a stop too, not a crash.
    for kind in [
        tokio::signal::unix::SignalKind::terminate(),
        tokio::signal::unix::SignalKind::interrupt(),
        tokio::signal::unix::SignalKind::hangup(),
    ] {
        if let Ok(mut signal) = tokio::signal::unix::signal(kind) {
            let cx = cx.clone();
            spawn(async move {
                if signal.recv().await.is_some() {
                    stop_relay(&cx).await;
                }
            });
        }
    }
    // State from before the relay was an app, or for another owner, belongs
    // to another conversation: its threads, alerts and homes are dropped.
    let known = cx.state.borrow().dm.clone();
    if !known.as_ref().is_some_and(|d| d.app && d.user == owner) {
        {
            let mut state = cx.state.borrow_mut();
            state.threads.clear();
            state.alerts.clear();
            state.homes.clear();
            state.following.clear();
            state.latest.clear();
            state.dashboard = None;
            state.cursor = None;
        }
        *cx.slack.owner.borrow_mut() = Some(owner.clone());
        let dm = slack::open_dm(&cx.slack, &owner).await?;
        cx.state.borrow_mut().dm = Some(dm);
        cx.save();
    }
    let dm = cx.state.borrow().dm();
    *cx.slack.channel.borrow_mut() = Some(dm.channel.clone());
    *cx.slack.owner.borrow_mut() = Some(owner);
    // The workspace's address, for links to threads; kept with the DM.
    if dm.url.is_none() {
        let url = cx
            .slack
            .api("auth.test", serde_json::json!({}))
            .await
            .ok()
            .and_then(|a| a.get("url").and_then(Value::as_str).map(str::to_string));
        if let Some(d) = cx.state.borrow_mut().dm.as_mut() {
            d.url = url;
        }
        cx.save();
    }
    cx.log(&format!("DM {}", dm.channel));
    // The Home tab is current from the start.
    if let Err(e) = refresh_dashboard(
        &cx,
        Refresh {
            quick: true,
            ..Refresh::default()
        },
    )
    .await
    {
        cx.log(&format!("dashboard failed: {e}"));
    }
    // Start from now: earlier messages were already handled, or were not
    // meant for the relay. After a crash, carry on from where the last run
    // got to instead, so a message sent during the restart is still
    // answered.
    let welcome = welcome_text(&restarted);
    cx.mem.borrow_mut().welcome = welcome.clone();
    let kickoff = say_listening(&cx, &welcome, &restarted).await?;
    {
        let mut state = cx.state.borrow_mut();
        if restarted.is_empty() || state.cursor.is_none() {
            state.cursor = Some(kickoff);
        }
    }
    // If Slack sent the offline note while the relay was gone, say it's
    // back; then keep the next one pushed ahead.
    if let Err(e) = super::offline::say_back_online(&cx).await {
        cx.log(&format!("back-online note failed: {e}"));
    }
    every(
        &cx,
        Duration::from_millis(super::offline::RENEW_MS),
        true,
        |cx| async move {
            if let Err(e) = super::offline::renew_offline_note(&cx).await {
                cx.log(&format!("offline note failed: {e}"));
            }
        },
    );
    cx.save();
    cx.log("listening; Ctrl-C or `stop` in the DM to end");

    // Slack: each message arrives as an event the moment it is sent.
    {
        let cx = cx.clone();
        spawn(async move { super::http::socket_mode(cx, app_token).await });
    }
    // Whether you're at the computer, for alerts about the tab in front.
    every(
        &cx,
        Duration::from_millis(presence::CHECK_MS),
        true,
        |cx| async move { presence::check(&cx).await },
    );
    // Images and videos you sent more than a week ago are deleted, with
    // their stills.
    every(&cx, Duration::from_secs(3600), true, |cx| async move {
        super::files::prune_files(&cx)
    });
    // A heartbeat that notices the computer has slept: the relay is frozen
    // while it sleeps, so a gap between beats is how long it was out.
    // Everything that judges by how long nothing has happened (stuck tabs)
    // waits a while after waking, since nothing could happen while it slept.
    {
        let beat = Rc::new(Cell::new(util::now_ms()));
        every(&cx, Duration::from_secs(1), false, move |cx| {
            let beat = beat.clone();
            async move {
                let now = util::now_ms();
                if now - beat.get() > SLEEP_GAP_MS {
                    cx.life.borrow_mut().woke_at = now;
                    cx.log(&format!(
                        "woke after {}m asleep",
                        ((now - beat.get()) as f64 / 60_000.0).round() as i64
                    ));
                    // Asleep long enough, Slack sent the offline note.
                    let back = super::offline::say_back_online(&cx).await;
                    let renewed = match back {
                        Ok(()) => super::offline::renew_offline_note(&cx).await,
                        Err(e) => Err(e),
                    };
                    if let Err(e) = renewed {
                        cx.log(&format!("offline note failed: {e}"));
                    }
                }
                beat.set(now);
            }
        });
    }
    every(
        &cx,
        Duration::from_millis(TAB_POLL_MS),
        true,
        |cx| async move { tab_tick(&cx).await },
    );
    // The Home tab, looked at every DASHBOARD_MS, or every HEARTBEAT_MS with
    // the poll_fast setting off; published at least every HEARTBEAT_MS for
    // its check-in time. The same failure every minute is logged once.
    {
        let looked = Rc::new(Cell::new(0i64));
        let beat_at = Rc::new(Cell::new(0i64));
        let last_error: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
        let busy = Rc::new(Cell::new(false));
        every(&cx, Duration::from_millis(DASHBOARD_MS), false, move |cx| {
            let (looked, beat_at, last_error, busy) = (
                looked.clone(),
                beat_at.clone(),
                last_error.clone(),
                busy.clone(),
            );
            async move {
                if busy.get() || quiet(&cx) {
                    return;
                }
                let due = |at: i64, every: u64| {
                    util::now_ms() - at >= every as i64 - DASHBOARD_MS as i64 / 2
                };
                if cx.setting_off("poll_fast") && !due(looked.get(), HEARTBEAT_MS) {
                    return;
                }
                busy.set(true);
                looked.set(util::now_ms());
                // Timed from when it starts, so a slow publish doesn't push
                // the next one past the minute.
                let heartbeat = due(beat_at.get(), HEARTBEAT_MS);
                if heartbeat {
                    beat_at.set(util::now_ms());
                }
                match refresh_dashboard(
                    &cx,
                    Refresh {
                        heartbeat,
                        ..Refresh::default()
                    },
                )
                .await
                {
                    Ok(()) => *last_error.borrow_mut() = None,
                    Err(e) => {
                        if heartbeat {
                            beat_at.set(0);
                        }
                        let why = e.to_string();
                        if last_error.borrow().as_deref() != Some(why.as_str()) {
                            cx.log(&format!("dashboard failed: {why}"));
                        }
                        *last_error.borrow_mut() = Some(why);
                    }
                }
                busy.set(false);
            }
        });
    }
    // A hook's event (a prompt taken, a turn ended) is acted on as it
    // lands: the events file is looked at four times a second, a stat each.
    {
        let seen = Rc::new(Cell::new(None::<u64>));
        every(&cx, Duration::from_millis(250), false, move |cx| {
            let seen = seen.clone();
            async move {
                let size = std::fs::metadata(cx.paths.events_file())
                    .map(|m| m.len())
                    .ok();
                if size != seen.replace(size) && size.is_some() {
                    tab_tick(&cx).await;
                }
            }
        });
    }
    // Runs until stopped, which ends the process.
    std::future::pending::<()>().await;
    Ok(())
}

/// Runs `work` every `period` (first at once, with `now`), skipping a run
/// that would start while the last is still going.
fn every<F, Fut>(cx: &Cx, period: Duration, now: bool, work: F)
where
    F: Fn(Cx) -> Fut + 'static,
    Fut: Future<Output = ()> + 'static,
{
    let cx = cx.clone();
    spawn(async move {
        let mut ticker = tokio::time::interval(period);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        if !now {
            ticker.tick().await;
        }
        loop {
            ticker.tick().await;
            if cx.life.borrow().stopping {
                return;
            }
            work(cx.clone()).await;
        }
    });
}

// ── Keeping it running ──────────────────────────────────────────────────────

/// After this many crashes within `CRASH_WINDOW_MS`, it gives up.
pub const MAX_CRASHES: usize = 5;
pub const CRASH_WINDOW_MS: i64 = 10 * 60_000;
pub const RESTART_DELAYS_MS: [u64; 5] = [2_000, 10_000, 30_000, 60_000, 120_000];

/// The reason for the last crash since `since` (ms), from the log the
/// relay wrote it to.
pub fn last_error(paths: &Paths, since: i64) -> Option<String> {
    let log = util::read_text(paths.log_file())?;
    let line = log.lines().rfind(|l| {
        l.contains(" Error: ")
            && l.split(' ')
                .next()
                .and_then(|t| t.parse::<jiff::Timestamp>().ok())
                .is_some_and(|t| t.as_millisecond() >= since)
    })?;
    let after = line.split_once(" Error: ")?.1;
    let after = after.strip_prefix("crashed: ").unwrap_or(after);
    Some(after.chars().take(300).collect())
}

/// Says in the DM that the relay is down, after too many crashes. The
/// offline note is cancelled first: this says it already.
pub async fn say_down(paths: &Paths, why: &str) {
    let state = super::state::load(paths);
    let Some(dm) = state.dm.clone().filter(|d| d.app) else {
        return;
    };
    let Some(bot) = slack::token(BOT_TOKEN_SERVICE) else {
        return;
    };
    let Ok(https) = super::http::Https::new(slack::api_base(), bot) else {
        return;
    };
    let relay = Relay::new(
        paths.clone(),
        Box::new(https),
        Box::new(mux::Server::current()),
        Box::new(summaries::ClaudeHaiku {
            dir: paths.state_dir.clone(),
        }),
        presence::Presence::new(),
        true,
    );
    *relay.slack.channel.borrow_mut() = Some(dm.channel.clone());
    *relay.slack.owner.borrow_mut() = Some(dm.user.clone());
    super::offline::cancel_offline_note(&relay).await;
    if let Err(e) = post(
        &relay,
        &format!(
            "🤖 **Relay down**\n\nIt crashed {MAX_CRASHES} times in ten minutes, most recently with: {why}\n\nIt needs starting again from the computer."
        ),
        None,
        PostOpts::default(),
    )
    .await
    {
        relay.log(&format!("could not say the relay is down: {e}"));
    }
}

/// How `watch` runs the relay: its command, the waits between restarts,
/// and what says it is down.
pub struct Watch {
    pub command: Vec<String>,
    pub delays: Vec<u64>,
    pub paths: Paths,
}

/// The restart loop: runs the relay until it ends on purpose, and gives the
/// code to exit with. After a crash it waits the next of `delays` and
/// starts it again, telling it why (`SLACK_RELAY_RESTARTED`); after
/// `MAX_CRASHES` within `CRASH_WINDOW_MS` it says the relay is down (with
/// `down`) and gives up. A SIGTERM or SIGHUP is passed to the relay, or,
/// between restarts, ends the wait; Ctrl-C reaches the relay directly, and
/// it stops cleanly.
pub async fn watch<D, Fut>(w: Watch, down: D) -> i32
where
    D: FnOnce(String) -> Fut,
    Fut: Future<Output = ()>,
{
    use tokio::signal::unix::{SignalKind, signal};
    let mut term = signal(SignalKind::terminate()).ok();
    let mut hup = signal(SignalKind::hangup()).ok();
    let _int = signal(SignalKind::interrupt()).ok();
    let mut crashes: Vec<i64> = Vec::new();
    let mut restarted = String::new();
    let mut down = Some(down);
    loop {
        let since = util::now_ms();
        let mut child = match tokio::process::Command::new(&w.command[0])
            .args(&w.command[1..])
            .env("SLACK_RELAY_RESTARTED", &restarted)
            .spawn()
        {
            Ok(c) => c,
            Err(e) => {
                super::log::fail(&w.paths, &format!("couldn't start the relay: {e}"), 1);
                return 1;
            }
        };
        let pid = child.id();
        let status = loop {
            tokio::select! {
                status = child.wait() => break status,
                Some(()) = recv(&mut term) => forward(pid, libc::SIGTERM),
                Some(()) = recv(&mut hup) => forward(pid, libc::SIGHUP),
            }
        };
        let Ok(status) = status else {
            return 1;
        };
        use std::os::unix::process::ExitStatusExt;
        let code = status.code();
        let signal = status.signal();
        if code == Some(0)
            || code == Some(DONT_RESTART as i32)
            || matches!(signal, Some(libc::SIGTERM | libc::SIGINT | libc::SIGHUP))
        {
            return code.unwrap_or(0);
        }
        let why = last_error(&w.paths, since).unwrap_or_else(|| match (signal, code) {
            (Some(s), _) => format!("killed by {}", signal_name(s)),
            (None, Some(c)) => format!("exit code {c}"),
            _ => "it ended".into(),
        });
        crashes.push(util::now_ms());
        crashes.retain(|at| *at >= util::now_ms() - CRASH_WINDOW_MS);
        let log = super::log::Log::new(true);
        if crashes.len() >= MAX_CRASHES {
            log.line(
                &w.paths,
                &format!(
                    "relay crashed {} times in ten minutes; giving up",
                    crashes.len()
                ),
            );
            if let Some(down) = down.take() {
                down(why).await;
            }
            // Nothing will read the hooks' events now.
            let _ = std::fs::remove_file(w.paths.events_file());
            return 1;
        }
        let delay = w
            .delays
            .get(crashes.len() - 1)
            .or(w.delays.last())
            .copied()
            .unwrap_or(0);
        log.line(
            &w.paths,
            &format!(
                "relay crashed ({why}); restarting in {}s",
                delay as f64 / 1000.0
            ),
        );
        restarted = why;
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_millis(delay)) => {}
            Some(()) = recv(&mut term) => return 0,
            Some(()) = recv(&mut hup) => return 0,
        }
    }
}

/// The next of a signal, or never when it couldn't be listened for.
async fn recv(signal: &mut Option<tokio::signal::unix::Signal>) -> Option<()> {
    match signal {
        Some(s) => s.recv().await,
        None => std::future::pending().await,
    }
}

fn forward(pid: Option<u32>, signal: libc::c_int) {
    if let Some(pid) = pid {
        // SAFETY: sending a signal to the child this process started.
        unsafe {
            libc::kill(pid as libc::pid_t, signal);
        }
    }
}

fn signal_name(signal: i32) -> String {
    match signal {
        libc::SIGKILL => "SIGKILL".into(),
        libc::SIGSEGV => "SIGSEGV".into(),
        libc::SIGABRT => "SIGABRT".into(),
        libc::SIGTERM => "SIGTERM".into(),
        other => format!("signal {other}"),
    }
}

/// `slack-relay run`: the relay, kept running.
pub fn supervise() -> ExitCode {
    let paths = Paths::current();
    if mux::Server::current().ask(&["list"]).is_err() {
        return fail(
            &paths,
            "no Switchboard server is running; start the workspace first, then the relay with `switchboard slack-relay`",
            1,
        );
    }
    super::state::owner_only(&paths);
    // Three old logs are kept, about six weeks, so an older problem can
    // still be traced.
    let log = paths.log_file();
    if std::fs::metadata(&log).is_ok_and(|m| m.len() > MAX_LOG_BYTES) {
        for n in [2, 1] {
            let from = PathBuf::from(format!("{}.{n}", log.display()));
            if from.exists() {
                let _ = std::fs::rename(&from, format!("{}.{}", log.display(), n + 1));
            }
        }
        let _ = std::fs::rename(&log, format!("{}.1", log.display()));
    }
    let exe = std::env::current_exe()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "switchboard".into());
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(r) => r,
        Err(e) => return fail(&paths, &format!("couldn't start: {e}"), 1),
    };
    let w = Watch {
        command: vec![exe, "slack-relay".into(), "relay".into()],
        delays: RESTART_DELAYS_MS.to_vec(),
        paths: paths.clone(),
    };
    let down_paths = paths.clone();
    let code = runtime.block_on(watch(
        w,
        |why| async move { say_down(&down_paths, &why).await },
    ));
    ExitCode::from(code.clamp(0, 255) as u8)
}
