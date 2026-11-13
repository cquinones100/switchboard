//! The server's side of the day: what the sidebar daemon's timers did for
//! it (lib/sidebar/daemon.js's `feed`, `slackLook`, `standup` and the
//! notifications), run from the server's tick. Its `todo`, `meetings` and
//! `keepUp` (the Calendar and Slack tabs) are starter routines now
//! (src/routines/starters.rs).
//! Each lookup starts in a background task, at most one of a kind at a
//! time, and answers with `Event::Day`; the views and notifications are
//! worked out on the tick from what was last answered and from small files,
//! and kept here for mission control and the sidebar to read.
//!
//! The calendar feed and the Slack look pause while you're away, but for
//! their first look after starting (`Server::away`), and both run on your
//! return; the standup carries on.
//!
//! The connectors down are asked about again here too, as soon as `/mcp`
//! writes Claude Code's config, and every minute while the Slack view shows
//! Slack missing (`connector::Watch`), so a reconnect shows in seconds.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;

use jiff::Zoned;
use serde_json::Value;

use super::calendar::{self, CalEvent, Feed, Fetched, SavedDay};
use super::{
    Claude, ClaudeAsk, Dirs, changes, log, new_notes, slack, slack_look, slack_thread, standup,
};
use crate::connector::Connection;
use crate::items::{Item, ViewData};
use crate::mux::server::{ClientId, Event, Server};
use crate::{connector, settings, util};

/// What the background tasks answer with.
#[derive(Debug)]
pub enum DayEvent {
    Feed(Fetched),
    SlackLooked(Result<String, String>),
    /// The threads a look found something new in, read.
    SlackThreadsRead(Result<slack_thread::Read, String>),
    /// The chosen Slack message's thread read, by its key.
    SlackThread(String, Result<(), String>),
    /// A Slack file read and kept, by its ID.
    SlackFile(String, Result<(), String>),
    /// `slack-threads.json` read, as it was at its modified time.
    SlackThreads(
        Option<std::time::SystemTime>,
        BTreeMap<String, slack_thread::Thread>,
    ),
    /// A kept Slack picture decoded, by its file's ID.
    SlackPicture(String, Option<image::RgbaImage>),
    /// A connector down asked about again at `at`, with what Claude Code
    /// said of it; `client` the one that asked by hand, else the watch did.
    Connector {
        service: String,
        at: i64,
        connection: Connection,
        client: Option<ClientId>,
    },
    /// A note's writing ended: what to log, and whether it should be tried
    /// again later.
    Noted {
        job: Job,
        log: Vec<String>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Job {
    Standup,
}

/// What the server keeps of your day.
#[derive(Default)]
pub struct Day {
    /// The Calendar view, as last worked out.
    pub calendar: ViewData,
    /// The Slack view, as last worked out.
    pub slack: ViewData,
    pub(crate) feed: Feed,
    saved: calendar::Saved,
    started: Option<i64>,
    away: bool,
    feed_asked: Option<i64>,
    feed_busy: bool,
    slack_asked: Option<i64>,
    slack_busy: bool,
    minute: Option<i64>,
    standup_tried: i64,
    busy: Vec<Job>,
    calendar_kept: bool,
    slack_kept: bool,
    /// The Slack threads kept (`slack-threads.json`), as last read.
    slack_threads: BTreeMap<String, slack_thread::Thread>,
    slack_threads_at: Option<std::time::SystemTime>,
    slack_threads_loading: bool,
    /// The threads and files asked for once the Slack view's message is
    /// chosen, and when, so one that fails waits `SLACK_RETRY_MS`; one of
    /// each at a time.
    slack_reading: HashMap<String, i64>,
    slack_fetching: HashMap<String, i64>,
    slack_thread_busy: bool,
    /// The threads a look found something new in being read.
    slack_wanted_busy: bool,
    slack_file_busy: bool,
    /// The pictures decoded for drawing, by file ID, the oldest let go
    /// past `SLACK_PICTURES`; and those being decoded.
    slack_pictures: HashMap<String, crate::mission::diagrams::Ready>,
    slack_picture_order: VecDeque<String>,
    slack_decoding: HashSet<String>,
    /// Kept files that couldn't be drawn (a video with no first frame, a
    /// damaged picture), and when, so each is tried again only after
    /// `SLACK_RETRY_MS`.
    slack_undrawable: HashMap<String, i64>,
    /// Why each thread that failed to read did, until it is read.
    slack_thread_failed: HashMap<String, (i64, String)>,
    /// The files that failed to read, until one is read.
    slack_file_failed: HashMap<String, String>,
    /// When to ask about the connectors down again, whether the watch's
    /// asking is under way, and when a view showing each service missing
    /// was last in front.
    connectors: connector::Watch,
    connectors_busy: usize,
    connector_shown: HashMap<String, i64>,
}

/// A Slack file `p` can show: its ID, its name, where it is kept, and
/// whether it is a video.
pub type Playable = (String, String, std::path::PathBuf, bool);

/// What the Slack board draws beyond its items for its chosen message
/// (`Server::slack_extras`).
#[derive(Default)]
pub struct SlackExtras {
    /// Its thread, once read.
    pub thread: Option<slack_thread::Thread>,
    /// Its thread's pictures decoded, by file ID.
    pub pictures: HashMap<String, crate::mission::diagrams::Ready>,
    /// Why its thread couldn't be read, when it couldn't.
    pub failed: Option<String>,
    /// Its thread's files that couldn't be read, by ID.
    pub unread: HashMap<String, String>,
    /// Its thread's files kept on this computer, by ID, which `p` shows.
    pub kept: HashSet<String>,
}

/// How long a thread or file that failed to read waits before it is asked
/// for again, and how many decoded pictures are held.
const SLACK_RETRY_MS: i64 = 2 * 60_000;
const SLACK_PICTURES: usize = 32;

const FEED_FIRST_MS: i64 = 3_000;
const SLACK_FIRST_MS: i64 = 11_000;
const MINUTE_MS: i64 = 60_000;
// Going to the Slack view asks for a look when the last is older than this.
const SLACK_NOW_MS: i64 = 60_000;
/// How long a view showing a connector missing counts as in front after
/// mission control last made it ready.
const SHOWN_FOR_MS: i64 = 10_000;

fn due(last: Option<i64>, every: i64, started: i64, first: i64, now: i64) -> bool {
    match last {
        Some(at) => now - at >= every,
        None => now - started >= first,
    }
}

impl Day {
    /// Today's events, from the feed or /today, for the parts that want them
    /// (the next meeting, the notifications).
    pub fn today(&mut self, dirs: &Dirs, now: &Zoned) -> Option<SavedDay> {
        let saved = self.saved.read(&dirs.state).cloned();
        calendar::read_day(&self.feed, saved.as_ref(), &util::ymd(now))
    }

    /// The feed's last error, when it is set up and failing.
    pub fn feed_error(&self) -> Option<&str> {
        self.feed
            .error
            .as_deref()
            .filter(|_| self.feed.configured())
    }
}

impl Server {
    /// The day's part of the tick: start what is due, then work out the
    /// views and notifications again.
    pub fn day_tick(&mut self) {
        if tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        let now = util::local_now();
        let ms = now.timestamp().as_millisecond();
        let dirs = Dirs::here();
        let started = *self.day.started.get_or_insert(ms);
        let away = self.away();
        if self.day.away && !away {
            log(&dirs.state, "back: catching up");
            self.day.feed_asked = None;
            self.day.slack_asked = None;
            self.day.started = Some(ms - SLACK_FIRST_MS);
        }
        self.day.away = away;
        let started = self.day.started.unwrap_or(started);
        // The first look after starting goes out with no one there too, as
        // the daemon's first timeouts weren't held while you were away.
        if (!away || self.day.feed_asked.is_none())
            && !self.day.feed_busy
            && due(
                self.day.feed_asked,
                calendar::FEED_POLL_MS,
                started,
                FEED_FIRST_MS,
                ms,
            )
        {
            self.read_feed();
        }
        if (!away || self.day.slack_asked.is_none())
            && settings::is_on("slack_background")
            && due(
                self.day.slack_asked,
                slack_look::SLACK_LOOK_MS,
                started,
                SLACK_FIRST_MS,
                ms,
            )
        {
            self.look_at_slack();
        }
        self.connectors_tick(ms);
        if due(self.day.minute, MINUTE_MS, started, 9_000, ms) {
            self.day.minute = Some(ms);
            self.day_minute(&dirs, &now);
        }
        self.day_views(&dirs, &now);
    }

    /// The day's events.
    pub fn day_event(&mut self, event: DayEvent) {
        let dirs = Dirs::here();
        match event {
            DayEvent::Feed(fetched) => {
                self.day.feed_busy = false;
                if let Fetched::Failed(why) = &fetched {
                    log(&dirs.state, why);
                }
                self.day.feed.take(fetched, util::now_ms());
            }
            DayEvent::SlackLooked(result) => {
                self.day.slack_busy = false;
                let looked = result.is_ok();
                match result {
                    Ok(line) => log(&dirs.state, &line),
                    Err(e) => log(&dirs.state, &format!("slack: {e}")),
                }
                if looked {
                    self.read_wanted_threads();
                }
            }
            DayEvent::SlackThreadsRead(result) => {
                self.day.slack_wanted_busy = false;
                match result {
                    Ok(read) => {
                        if read.read > 0 {
                            log(&dirs.state, &format!("slack: {} threads read", read.read));
                        }
                        // Said in the pane of the thread that failed.
                        let at = util::now_ms();
                        for (key, why) in read.failed {
                            log(&dirs.state, &format!("slack: thread {key}: {why}"));
                            // Choosing it waits as a look does.
                            self.day.slack_reading.insert(key.clone(), at);
                            self.day.slack_thread_failed.insert(key, (at, why));
                        }
                    }
                    Err(e) => log(&dirs.state, &format!("slack: threads not kept ({e})")),
                }
            }
            DayEvent::SlackThread(key, result) => {
                self.day.slack_thread_busy = false;
                match result {
                    Ok(()) => {
                        self.day.slack_reading.remove(&key);
                        self.day.slack_thread_failed.remove(&key);
                    }
                    Err(e) => {
                        log(&dirs.state, &format!("slack: thread {key}: {e}"));
                        // Its wait starts as it fails.
                        self.day.slack_reading.insert(key.clone(), util::now_ms());
                        self.day
                            .slack_thread_failed
                            .insert(key, (util::now_ms(), e));
                    }
                }
            }
            DayEvent::SlackFile(id, result) => {
                self.day.slack_file_busy = false;
                match result {
                    Ok(()) => {
                        self.day.slack_fetching.remove(&id);
                        self.day.slack_file_failed.remove(&id);
                    }
                    Err(e) => {
                        log(&dirs.state, &format!("slack: file {id}: {e}"));
                        // Its wait starts as it fails, so the files after it
                        // get their turn first.
                        self.day.slack_fetching.insert(id.clone(), util::now_ms());
                        self.day.slack_file_failed.insert(id, e);
                    }
                }
            }
            DayEvent::SlackThreads(at, threads) => {
                self.day.slack_threads_loading = false;
                self.day.slack_threads_at = at;
                // A thread read since it last failed has recovered.
                self.day
                    .slack_thread_failed
                    .retain(|key, (at, _)| threads.get(key).is_none_or(|t| t.read < *at));
                self.day.slack_threads = threads;
            }
            DayEvent::SlackPicture(id, img) => {
                self.day.slack_decoding.remove(&id);
                if img.is_none() {
                    self.day.slack_undrawable.insert(id.clone(), util::now_ms());
                }
                if let Some(img) = img {
                    let content = crate::ui::images::picture_key(&img);
                    self.day.slack_pictures.insert(
                        id.clone(),
                        crate::mission::diagrams::Ready {
                            image: Arc::new(img),
                            content,
                        },
                    );
                    self.day.slack_picture_order.push_back(id);
                    while self.day.slack_picture_order.len() > SLACK_PICTURES {
                        if let Some(old) = self.day.slack_picture_order.pop_front() {
                            self.day.slack_pictures.remove(&old);
                        }
                    }
                }
            }
            DayEvent::Connector {
                service,
                at,
                connection,
                client,
            } => {
                if client.is_none() {
                    self.day.connectors_busy = self.day.connectors_busy.saturating_sub(1);
                }
                connector::took(&service, at, connection);
                log(&dirs.state, &format!("connector: {service} {connection:?}"));
                match connection {
                    Connection::Connected => {
                        let words = format!("{service} is connected again.");
                        match client {
                            Some(c) => self.mission_note(c, words),
                            None => self.say(None, &words),
                        }
                        if service == "Slack" {
                            // The threads that failed for it are read
                            // again, rather than after their wait.
                            self.day.slack_thread_failed.clear();
                            self.day.slack_reading.clear();
                            if settings::is_on("slack_background") && !self.away() {
                                self.look_at_slack();
                            }
                        }
                    }
                    Connection::NotConnected => {
                        if let Some(c) = client {
                            self.mission_note(
                                c,
                                format!("{service} still isn't connected: run /mcp in Claude."),
                            );
                        }
                    }
                    Connection::Unknown => {
                        if let Some(c) = client {
                            self.mission_note(
                                c,
                                format!("Claude Code didn't say whether {service} is connected."),
                            );
                        }
                    }
                }
            }
            DayEvent::Noted { job, log: lines } => {
                self.day.busy.retain(|j| *j != job);
                for line in lines {
                    log(&dirs.state, &line);
                }
            }
        }
        self.day_views(&dirs, &util::local_now());
    }

    // ── What mission control calls ──────────────────────────────────────────

    /// Going to the Calendar view reads the feed again, at most a minute
    /// apart.
    pub fn calendar_opened(&mut self) {
        let ms = util::now_ms();
        if self.day.feed_asked.is_none_or(|at| ms - at > MINUTE_MS) {
            self.read_feed();
        }
    }

    /// The Calendar view looked at whole (a key pressed with it in front,
    /// nothing over it): today as it is now is seen, so its changes leave
    /// the notifications.
    pub fn calendar_looked_at(&mut self) {
        // Read there, every change the day had.
        let shown: Vec<String> = self
            .notices
            .get("calendar")
            .map(|items| items.iter().map(|i| i.id.clone()).collect())
            .unwrap_or_default();
        for id in shown {
            self.notice_history_mark(
                "calendar",
                &id,
                crate::sidebar::history::Sidebar::Read,
                false,
            );
        }
        let dirs = Dirs::here();
        let now = util::local_now();
        if let Some(day) = self.day.today(&dirs, &now) {
            changes::calendar_seen(&dirs.state, &day, now.time_zone());
        }
        self.day_views(&dirs, &now);
    }

    /// The calendar feed read again, after something changed it (an
    /// invitation answered from mission control's menu).
    pub fn calendar_reread(&mut self) {
        self.read_feed();
    }

    /// Going to the Slack view asks for a look now, when the last is over a
    /// minute old: the server's own look, or, with slack_background off, the
    /// Slack routine run now (or its tab, open, asked to look between runs).
    pub fn slack_opened(&mut self) {
        let dirs = Dirs::here();
        let ms = util::now_ms();
        if slack::read(&dirs.state).is_some_and(|d| ms - d.checked < SLACK_NOW_MS)
            || self
                .day
                .slack_asked
                .is_some_and(|at| ms - at < SLACK_NOW_MS)
        {
            return;
        }
        if settings::is_on("slack_background") {
            self.look_at_slack();
            return;
        }
        // The Slack routine, which keeps its own pause and setting; else a
        // tab named Slack that is no routine's (one the server opened looping
        // /slack-updates before the starters, beside which the starter was
        // made paused) is asked as before.
        if self.routine_run_starter(crate::routines::starters::Kind::Slack) == Some(true) {
            self.day.slack_asked = Some(ms);
            log(&dirs.state, "slack: ran the Slack routine");
            return;
        }
        if !self.day.slack_kept {
            return;
        }
        // The Slack tab's Claude, idle between its ticks (or with only its
        // loop's background work going): asked to look now, the command
        // typed and then Enter on its own, a moment later, as send-keys -l
        // and send-keys Enter did, so Enter never reaches Claude Code before
        // it has taken the command in.
        use crate::sidebar::model::TabState;
        let pane = self
            .sidebar_all_tabs()
            .into_iter()
            .filter(|t| self.tab_own_name(t.id).unwrap_or(&t.name) == "Slack")
            .filter(|t| {
                self.tabs
                    .get(&t.id)
                    .is_none_or(|tab| !tab.opts.contains_key(crate::routines::TAB_MARK))
            })
            .filter(|t| t.claude.is_some())
            .filter(|t| matches!(t.state, TabState::Idle | TabState::Background))
            .find_map(|t| t.claude_pane);
        let Some(pane) = pane else {
            return;
        };
        self.day.slack_asked = Some(ms);
        let enter = crate::mux::input::Key::new(
            crate::mux::input::KeyCode::Enter,
            crate::mux::input::Mods::NONE,
        );
        use crate::mission::answer::Press;
        self.type_into_claude(
            pane,
            vec![
                Press::Text("/slack-updates".into()),
                Press::Settle,
                Press::Key(enter),
            ],
        );
        log(&dirs.state, "slack: asked the Slack tab to look now");
    }

    /// Leaving the Slack view: what its look brought is seen, or with
    /// `only`, those messages alone (the board showed no others).
    pub fn slack_left(&mut self, only: Option<Vec<Item>>) {
        if let Some(items) = only {
            let dirs = Dirs::here();
            let now = util::now_ms();
            // Its threads read as far as the look that showed them, as
            // leaving the whole look would have them.
            let checked = slack::read(&dirs.state).map_or(now, |d| d.checked);
            for item in &items {
                let (_, id) = crate::mission::choices::notice_of(item);
                self.notice_history_mark(
                    "slack",
                    &id,
                    crate::sidebar::history::Sidebar::Read,
                    false,
                );
                if let Some(m) = item.data.get("message") {
                    let get = |k: &str| m.get(k).and_then(Value::as_str).unwrap_or("");
                    slack::mark_item_seen(&dirs.state, get("channel"), get("ts"), now);
                }
                if let Some(thread) = item.data.get("thread").and_then(Value::as_str) {
                    slack::mark_thread_read(&dirs.state, thread, checked);
                }
            }
            self.day_views(&dirs, &util::local_now());
            return;
        }
        // Read there, as the history says before their list lets them go.
        let shown: Vec<String> = self
            .notices
            .get("slack")
            .map(|items| items.iter().map(|i| i.id.clone()).collect())
            .unwrap_or_default();
        for id in shown {
            self.notice_history_mark("slack", &id, crate::sidebar::history::Sidebar::Read, false);
        }
        let dirs = Dirs::here();
        slack::mark_seen(&dirs.state);
        self.day_views(&dirs, &util::local_now());
    }

    /// A Slack message acted on from its menu (an item of the Slack view or
    /// of the "slack" notifications): seen, on its own.
    pub fn slack_item_acted(&mut self, item: &Item) {
        let (_, id) = crate::mission::choices::notice_of(item);
        self.notice_history_mark("slack", &id, crate::sidebar::history::Sidebar::Read, false);
        let dirs = Dirs::here();
        if let Some(m) = item.data.get("message") {
            let get = |k: &str| m.get(k).and_then(Value::as_str).unwrap_or("");
            slack::mark_item_seen(&dirs.state, get("channel"), get("ts"), util::now_ms());
        }
        self.day_views(&dirs, &util::local_now());
    }

    /// Dismisses Slack messages (items of the Slack view) until their
    /// threads get something newer: one, or every one shown.
    pub fn slack_dismiss(&mut self, items: &[Item]) {
        let dirs = Dirs::here();
        let refs: Vec<slack::Ref> = items
            .iter()
            .filter_map(|i| serde_json::from_value(i.data.get("message")?.clone()).ok())
            .collect();
        slack::dismiss(&dirs.state, &refs, util::now_ms());
        self.day_views(&dirs, &util::local_now());
        // Dismissed in the history too, however seeing them on the way
        // and their list changing left them there.
        for item in items {
            let (_, id) = crate::mission::choices::notice_of(item);
            self.notice_history_mark(
                "slack",
                &id,
                crate::sidebar::history::Sidebar::Dismissed,
                true,
            );
        }
    }

    /// Dismisses a notification this part gave, by its source ("meeting",
    /// "slack", "calendar", "new-notes"): the next meeting until another,
    /// a Slack message until its thread gets more, the calendar's changes by
    /// taking the day as seen, a new note until another is written.
    pub fn day_dismiss(&mut self, source: &str, item: &Item) {
        let dirs = Dirs::here();
        let now = util::local_now();
        match source {
            "meeting" => {
                if let Some(e) = item
                    .data
                    .get("event")
                    .and_then(|e| serde_json::from_value::<CalEvent>(e.clone()).ok())
                {
                    calendar::dismiss_meeting(&dirs.state, &e, now.time_zone());
                }
            }
            "slack" => {
                if let Some(r) = item
                    .data
                    .get("message")
                    .and_then(|m| serde_json::from_value::<slack::Ref>(m.clone()).ok())
                {
                    slack::dismiss(&dirs.state, &[r], util::now_ms());
                }
            }
            "calendar" => {
                // The whole day's changes go with it.
                self.notices_dismissed("calendar", None);
                if let Some(day) = self.day.today(&dirs, &now) {
                    changes::calendar_seen(&dirs.state, &day, now.time_zone());
                }
            }
            "new-notes" => {
                if let Some(path) = item.data.get("path").and_then(Value::as_str) {
                    new_notes::dismiss_note(&dirs.state, path);
                }
            }
            _ => {}
        }
        self.day_views(&dirs, &now);
    }

    /// A new note's notification acted on: the note opened in Notes, and its
    /// notification let go, as daemon.js's `openNote` did.
    pub fn new_note_open(&mut self, client: ClientId, rel: &str) {
        // Read, from the sidebar or a menu alike.
        self.notice_history_mark(
            "new-notes",
            &format!("note:{rel}"),
            crate::sidebar::history::Sidebar::Read,
            false,
        );
        self.open_note(client, rel);
        let dirs = Dirs::here();
        new_notes::dismiss_note(&dirs.state, rel);
        self.day_views(&dirs, &util::local_now());
    }

    // ── The lookups ─────────────────────────────────────────────────────────

    fn read_feed(&mut self) {
        if self.day.feed_busy || tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        self.day.feed_busy = true;
        self.day.feed_asked = Some(util::now_ms());
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let fetched = calendar::fetch_feed().await;
            let _ = tx.send(Event::Day(DayEvent::Feed(fetched)));
        });
    }

    /// Reads `slack-threads.json` again off the task once it has changed
    /// (the look, a thread read when chosen, or /slack-updates' save wrote
    /// it), which only a look at its modified time tells on the task.
    fn slack_threads_changed(&mut self, dirs: &Dirs) {
        let file = slack_thread::file(&dirs.state);
        let at = std::fs::metadata(&file).and_then(|m| m.modified()).ok();
        if at == self.day.slack_threads_at
            || self.day.slack_threads_loading
            || tokio::runtime::Handle::try_current().is_err()
        {
            return;
        }
        self.day.slack_threads_loading = true;
        let (state, tx) = (dirs.state.clone(), self.tx.clone());
        tokio::task::spawn_blocking(move || {
            let threads = slack_thread::read(&state);
            let _ = tx.send(Event::Day(DayEvent::SlackThreads(at, threads)));
        });
    }

    /// What the Slack board draws beyond its items for the chosen message
    /// (`item`): its thread, once read, the pictures of it decoded, and why
    /// its thread couldn't be read, when it couldn't, and its files that couldn't. A
    /// thread not kept is read now, then each of its pictures and videos
    /// not kept, one at a time, and those kept are decoded off the task.
    pub fn slack_extras(&mut self, item: Option<&Item>) -> SlackExtras {
        if !settings::is_on("slack_threads") {
            return Default::default();
        }
        let Some(item) = item else {
            return Default::default();
        };
        let get = |k: &str| item.data.get(k).and_then(Value::as_str).unwrap_or("");
        let key = get("thread").to_string();
        let thread = self.day.slack_threads.get(&key).cloned();
        let failed = self
            .day
            .slack_thread_failed
            .get(&key)
            .map(|(_, why)| why.clone());
        let now = util::now_ms();
        let due = |asked: Option<&i64>| asked.is_none_or(|at| now - at >= SLACK_RETRY_MS);
        // Reading pauses while you're away, as the looks do; what is kept
        // still draws.
        let running = tokio::runtime::Handle::try_current().is_ok() && !self.away();
        let stale = thread
            .as_ref()
            .is_none_or(|t| now - t.read >= slack_thread::STALE_MS);
        if stale
            && !key.is_empty()
            && running
            && !self.day.slack_thread_busy
            && due(self.day.slack_reading.get(&key))
            && let Some((channel, ts)) = key.split_once(':')
        {
            self.day.slack_thread_busy = true;
            self.day.slack_reading.insert(key.clone(), now);
            let (channel, ts, tx) = (channel.to_string(), ts.to_string(), self.tx.clone());
            tokio::spawn(async move {
                let state = Dirs::here().state;
                let result = match slack_thread::read_threads(
                    &state,
                    &Claude,
                    &[(channel, ts)],
                    now,
                )
                .await
                {
                    Ok(read) => match read.failed.into_iter().next() {
                        Some((_, why)) => Err(why),
                        None => Ok(()),
                    },
                    Err(e) => Err(e.to_string()),
                };
                let _ = tx.send(Event::Day(DayEvent::SlackThread(key, result)));
            });
        }
        let mut pictures = HashMap::new();
        let mut kept = HashSet::new();
        // Every file is read, the chosen message's first; the first of them
        // decoded, fewer than are held, so holding them never lets one go
        // that is asked for again.
        let ts = get("ts");
        let mut files: Vec<&slack_thread::SlackFile> = Vec::new();
        if let Some(t) = &thread {
            let own = t.messages.iter().filter(|m| m.ts == ts);
            let rest = t.messages.iter().filter(|m| m.ts != ts);
            files.extend(
                own.chain(rest)
                    .flat_map(|m| m.files.iter())
                    .filter(|f| f.drawn()),
            );
        }
        for (n, f) in files.into_iter().enumerate() {
            let on_disk = slack_thread::kept(f).is_some();
            // A picture whose file has gone (kept a week) is read again.
            if !on_disk && self.day.slack_pictures.remove(&f.id).is_some() {
                self.day.slack_picture_order.retain(|id| *id != f.id);
            }
            if let Some(ready) = self.day.slack_pictures.get(&f.id) {
                pictures.insert(f.id.clone(), ready.clone());
                kept.insert(f.id.clone());
                continue;
            }
            if on_disk {
                kept.insert(f.id.clone());
            }
            if !running {
                continue;
            }
            if on_disk {
                if n < SLACK_PICTURES / 2
                    && due(self.day.slack_undrawable.get(&f.id))
                    && self.day.slack_decoding.insert(f.id.clone())
                {
                    let (f, tx) = (f.clone(), self.tx.clone());
                    tokio::task::spawn_blocking(move || {
                        let img = slack_thread::load(&f);
                        let _ = tx.send(Event::Day(DayEvent::SlackPicture(f.id, img)));
                    });
                }
            } else if !self.day.slack_file_busy && due(self.day.slack_fetching.get(&f.id)) {
                self.day.slack_file_busy = true;
                self.day.slack_fetching.insert(f.id.clone(), now);
                let (f, tx) = (f.clone(), self.tx.clone());
                tokio::spawn(async move {
                    let result = slack_thread::fetch(&Claude, &f)
                        .await
                        .map(|_| ())
                        .map_err(|e| e.to_string());
                    let _ = tx.send(Event::Day(DayEvent::SlackFile(f.id, result)));
                });
            }
        }
        let unread = thread
            .iter()
            .flat_map(|t| t.files())
            .filter_map(|f| {
                let why = self.day.slack_file_failed.get(&f.id)?;
                Some((f.id.clone(), why.clone()))
            })
            .collect();
        SlackExtras {
            thread,
            pictures,
            failed,
            unread,
            kept,
        }
    }

    /// What `p` shows for the chosen Slack message, in the order it steps
    /// through them: the message's own pictures and videos (a video first),
    /// then the rest of its thread's, each as kept on this computer (its
    /// file's ID, name, path, and whether it is a video); and the ID of the
    /// one `clicked` (its place among `Thread::files`), when one was.
    pub fn slack_playable(
        &self,
        item: &Item,
        clicked: Option<usize>,
    ) -> (Vec<Playable>, Option<String>) {
        let get = |k: &str| item.data.get(k).and_then(Value::as_str);
        let (Some(key), Some(ts)) = (get("thread"), get("ts")) else {
            return Default::default();
        };
        let Some(thread) = self.day.slack_threads.get(key) else {
            return Default::default();
        };
        let mut own: Vec<&slack_thread::SlackFile> = thread
            .messages
            .iter()
            .filter(|m| m.ts == ts)
            .flat_map(|m| m.files.iter())
            .filter(|f| f.drawn())
            .collect();
        own.sort_by_key(|f| !f.is_video());
        let mut order = own.clone();
        order.extend(thread.files());
        // Each file once, where it first comes, so p steps through them all.
        let mut seen = HashSet::new();
        order.retain(|f| seen.insert(f.id.clone()));
        let kept = order
            .into_iter()
            .filter_map(|f| {
                let path = slack_thread::kept(f)?;
                Some((f.id.clone(), f.name.clone(), path, f.is_video()))
            })
            .collect();
        let clicked = clicked
            .and_then(|n| thread.files().nth(n))
            .map(|f| f.id.clone());
        (kept, clicked)
    }

    /// After a look, the threads it found something new in, for the board's
    /// pane and reply counts, read in a task of their own, one such read at
    /// a time; only while UX 2.0 draws the board, and not while you're
    /// away (the next look on your return reads them).
    fn read_wanted_threads(&mut self) {
        if !settings::is_on("slack_threads")
            || !settings::is_on("ux2")
            || self.away()
            || self.day.slack_wanted_busy
            || tokio::runtime::Handle::try_current().is_err()
        {
            return;
        }
        self.day.slack_wanted_busy = true;
        let tx = self.tx.clone();
        // Those that failed lately wait, as a chosen thread does.
        let now = util::now_ms();
        let waiting: HashSet<String> = self
            .day
            .slack_thread_failed
            .iter()
            .filter(|(_, (at, _))| now - at < SLACK_RETRY_MS)
            .map(|(key, _)| key.clone())
            .collect();
        tokio::spawn(async move {
            let result = slack_thread::read_wanted(&Dirs::here().state, &Claude, &waiting, now)
                .await
                .map_err(|e| e.to_string());
            let _ = tx.send(Event::Day(DayEvent::SlackThreadsRead(result)));
        });
    }

    /// A view showing `service`'s connector missing is in front, so while it
    /// stays it is asked about every `connector::SHOWN_CHECK_EVERY_MS`.
    pub fn connector_shown(&mut self, service: &str) {
        self.day
            .connector_shown
            .insert(service.to_string(), util::now_ms());
    }

    /// Asks Claude Code now whether `service` is connected, saying to
    /// `client` what it found.
    pub fn connector_check_now(&mut self, client: ClientId, service: &str) {
        if tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        self.ask_about_connector(service.to_string(), Some(client));
    }

    // The connectors down asked about again when `connector::Watch` says
    // so, one ask at a time, off the task; not while you are away, a change
    // to Claude Code's config kept for your return.
    fn connectors_tick(&mut self, now: i64) {
        if self.day.connectors_busy > 0 {
            return;
        }
        let config = std::fs::metadata(connector::claude_config())
            .and_then(|m| m.modified())
            .ok();
        if self.away() {
            self.day.connectors.first_sight(config);
            return;
        }
        self.day
            .connector_shown
            .retain(|_, at| now - *at < SHOWN_FOR_MS);
        let shown: Vec<&str> = self
            .day
            .connector_shown
            .keys()
            .map(String::as_str)
            .collect();
        let services = self
            .day
            .connectors
            .due(&connector::down(), config, &shown, now);
        if services.is_empty() {
            return;
        }
        // Each on its own, so one that stalls holds up none of the others.
        self.day.connectors_busy = services.len();
        for service in services {
            self.ask_about_connector(service, None);
        }
    }

    // Asks Claude Code about one connector off the task, its answer kept on
    // the task as it comes (`DayEvent::Connector`).
    fn ask_about_connector(&mut self, service: String, client: Option<ClientId>) {
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let at = util::now_ms();
            let connection = connector::check(&service).await;
            let _ = tx.send(Event::Day(DayEvent::Connector {
                service,
                at,
                connection,
                client,
            }));
        });
    }

    fn look_at_slack(&mut self) {
        self.day.slack_asked = Some(util::now_ms());
        if self.day.slack_busy || tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        self.day.slack_busy = true;
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let result = slack_look::look_slack(&Dirs::here(), &Claude, util::now_ms())
                .await
                .map_err(|e| e.to_string());
            let _ = tx.send(Event::Day(DayEvent::SlackLooked(result)));
        });
    }

    // The tabs' directories, each once.
    fn day_tab_dirs(&self) -> Vec<PathBuf> {
        let mut dirs: Vec<PathBuf> = Vec::new();
        for id in self.tabs.keys() {
            if let Some(p) = self.tab_path(*id)
                && !dirs.iter().any(|d| d == p)
            {
                dirs.push(p.to_path_buf());
            }
        }
        dirs
    }

    // Whether a tab keeps the Calendar or the Slack view (`name`): the tab
    // of that name with Claude in it, or any tab looping its skill
    // (calendar.js's `calendarLooping`, slack.js's `slackLooping`).
    fn keeps(&self, name: &str) -> bool {
        self.sidebar_all_tabs().iter().any(|t| {
            let called = self.tab_own_name(t.id).unwrap_or(&t.name);
            let claude = t.panes.iter().any(|p| self.runs_claude(*p));
            let looping = match &t.tab_loop {
                Some(crate::sidebar::model::TabLoop::Scheduled(s)) => Some(s.prompt.as_str()),
                _ => None,
            };
            if name == "Calendar" {
                calendar::keeps_calendar(called, claude, looping)
            } else {
                slack::keeps_slack(called, claude, looping)
            }
        })
    }

    // Once a minute: the standup when due, and whether a tab keeps the
    // Calendar and Slack views.
    fn day_minute(&mut self, dirs: &Dirs, now: &Zoned) {
        let ms = now.timestamp().as_millisecond();
        self.day.calendar_kept = self.keeps("Calendar");
        self.day.slack_kept = self.keeps("Slack");
        // The standup, each weekday morning.
        if settings::is_on("standup_note")
            && !self.day.busy.contains(&Job::Standup)
            && standup::standup_due(&dirs.notes, now, self.day.standup_tried)
        {
            self.day.standup_tried = ms;
            let parts = standup::Parts {
                github: settings::is_on("github"),
                linear: settings::is_on("linear"),
                summary: settings::is_on("standup_summary"),
            };
            let tab_dirs = self.day_tab_dirs();
            self.spawn_note(Job::Standup, async move {
                let (dirs, now) = (Dirs::here(), util::local_now());
                match standup::write_standup(&dirs, &tab_dirs, parts, &now, &ClaudeAsk).await {
                    Ok(line) => vec![line],
                    Err(e) => vec![format!("standup: {e}")],
                }
            });
        }
    }

    fn spawn_note(
        &mut self,
        job: Job,
        work: impl std::future::Future<Output = Vec<String>> + Send + 'static,
    ) {
        self.day.busy.push(job);
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let log = work.await;
            let _ = tx.send(Event::Day(DayEvent::Noted { job, log }));
        });
    }

    /// Whether the Calendar view needs a tab to keep it, as the starter
    /// Calendar routine is made (`starters::Was`): no feed (known to be
    /// none) and a day /today saved.
    pub(crate) fn calendar_needs_tab(&mut self) -> bool {
        let dirs = Dirs::here();
        self.day.feed.unconfigured() && self.day.saved.read(&dirs.state).is_some()
    }

    // ── The views and notifications ─────────────────────────────────────────

    // Works out the Calendar and Slack views and the notifications again,
    // drawing every client when a view changed.
    fn day_views(&mut self, dirs: &Dirs, now: &Zoned) {
        let today = util::ymd(now);
        let ms = now.timestamp().as_millisecond();
        let tz = now.time_zone().clone();
        let saved = self.day.saved.read(&dirs.state).cloned();
        // Where today's events started when first seen, for the view's
        // "moved" chip, while the view is on.
        let moves = match calendar::read_day(&self.day.feed, saved.as_ref(), &today) {
            Some(day) if settings::is_on("view_calendar") => {
                changes::moves(&dirs.state, &day, &today, &tz)
            }
            _ => Default::default(),
        };
        let calendar = calendar::calendar_view(&calendar::Look {
            feed: &self.day.feed,
            saved: saved.as_ref(),
            looping: self.day.calendar_kept,
            now,
            moves: &moves,
        });
        let background = settings::is_on("slack_background");
        let slack = slack::slack_view(&slack::Look {
            state: &dirs.state,
            background,
            looping: self.day.slack_kept,
            down: background
                .then(|| connector::connector_down("Slack"))
                .flatten(),
            now: ms,
            threads: &self.day.slack_threads,
        });
        if settings::is_on("slack_threads") {
            self.slack_threads_changed(dirs);
        }
        let changed = calendar != self.day.calendar || slack != self.day.slack;
        self.day.calendar = calendar;
        self.day.slack = slack;
        if changed {
            for c in self.clients.values_mut() {
                c.dirty = true;
            }
        }
        let on = settings::is_on("sidebar_notifications");
        let day = calendar::read_day(&self.day.feed, saved.as_ref(), &today);
        // The meeting about to start, or just started, not one declined
        // (those aren't kept).
        let meeting = if on && settings::is_on("next_meeting") {
            let gone = calendar::dismissed_meetings(&dirs.state);
            calendar::next_meeting(day.as_ref(), &gone, now)
                .map(|e| vec![calendar::meeting_notice(e, now)])
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        self.set_notices("meeting", meeting);
        let fresh = if on && settings::is_on("view_slack") {
            self.day
                .slack
                .items
                .iter()
                .filter(|i| i.data.get("old") == Some(&Value::Bool(false)))
                .cloned()
                .collect()
        } else {
            Vec::new()
        };
        self.set_notices("slack", fresh);
        // Only with the Calendar view on, which is where they are looked at.
        let changed = match day.as_ref() {
            Some(day)
                if on
                    && settings::is_on("calendar_changes")
                    && settings::is_on("view_calendar") =>
            {
                changes::change_notices(&changes::calendar_changes(&dirs.state, day, &today, &tz))
            }
            _ => Vec::new(),
        };
        self.set_notices("calendar", changed);
        let notes = if on && settings::is_on("new_notes") {
            new_notes::note_notices(&new_notes::new_notes(&dirs.state, &dirs.notes))
        } else {
            Vec::new()
        };
        self.set_notices("new-notes", notes);
    }
}
