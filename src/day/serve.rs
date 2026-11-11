//! The server's side of the day: what the sidebar daemon's timers did for
//! it (lib/sidebar/daemon.js's `feed`, `slackLook`, `standup`, `todo`,
//! `meetings`, `keepUp` and the notifications), run from the server's tick.
//! Each lookup starts in a background task, at most one of a kind at a
//! time, and answers with `Event::Day`; the views and notifications are
//! worked out on the tick from what was last answered and from small files,
//! and kept here for mission control and the sidebar to read.
//!
//! The calendar feed and the Slack look pause while you're away, but for
//! their first look after starting (`Server::away`), and both run on your
//! return; the notes (standup, meetings) carry on, and today's to-do list
//! waits for you, as before.

use std::collections::HashMap;
use std::path::PathBuf;

use jiff::Zoned;
use serde_json::Value;

use super::calendar::{self, CalEvent, Feed, Fetched, SavedDay};
use super::{
    Claude, ClaudeAsk, Dirs, changes, log, meetings, new_notes, slack, slack_look, standup, todo,
};
use crate::items::{Item, ViewData};
use crate::mux::server::{ClientId, Event, Server};
use crate::{connector, settings, util};

/// A view's last answer, as the sources report it for today's to-do list
/// (`Day::answered`): the day it answered, its lines, and whether it failed.
#[derive(Clone, Debug, Default)]
pub struct Answered {
    pub day: String,
    pub lines: Vec<String>,
    pub failed: bool,
}

/// What the background tasks answer with.
#[derive(Debug)]
pub enum DayEvent {
    Feed(Fetched),
    SlackLooked(Result<String, String>),
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
    Todo,
    Meetings,
}

/// What the server keeps of your day.
#[derive(Default)]
pub struct Day {
    /// The Calendar view, as last worked out.
    pub calendar: ViewData,
    /// The Slack view, as last worked out.
    pub slack: ViewData,
    /// The other views' last answers, by name ("prs", "ci", "tickets",
    /// "inbox"), which the sources report through `answered` for today's
    /// to-do list.
    pub views: HashMap<String, Answered>,
    feed: Feed,
    saved: calendar::Saved,
    started: Option<i64>,
    away: bool,
    feed_asked: Option<i64>,
    feed_busy: bool,
    /// The day the feed (or its absence) last answered.
    feed_answered: String,
    slack_asked: Option<i64>,
    slack_busy: bool,
    slack_looked: String,
    slack_failed: bool,
    minute: Option<i64>,
    standup_tried: i64,
    todo_tried: i64,
    todo_due_since: i64,
    meetings_tried: i64,
    busy: Vec<Job>,
    calendar_kept: bool,
    slack_kept: bool,
    opened: HashMap<&'static str, i64>,
}

const FEED_FIRST_MS: i64 = 3_000;
const SLACK_FIRST_MS: i64 = 11_000;
const MINUTE_MS: i64 = 60_000;
/// How long today's to-do list waits for a view that hasn't answered today,
/// before naming it as left out.
const TODO_WAIT_MS: i64 = 30 * 60_000;
// Going to the Slack view asks for a look when the last is older than this.
const SLACK_NOW_MS: i64 = 60_000;

fn due(last: Option<i64>, every: i64, started: i64, first: i64, now: i64) -> bool {
    match last {
        Some(at) => now - at >= every,
        None => now - started >= first,
    }
}

impl Day {
    /// A view's answer, for today's to-do list: `name` is "prs", "ci",
    /// "tickets" or "inbox"; `lines` what the list should know of it (each
    /// item's title and detail, and its link); `failed` when the last look
    /// failed, so it is named as left out rather than used.
    pub fn answered(&mut self, name: &str, lines: Vec<String>, failed: bool) {
        self.views.insert(
            name.to_string(),
            Answered {
                day: util::ymd(&util::local_now()),
                lines,
                failed,
            },
        );
    }

    /// Today's events, from the feed or /today, for the parts that want them
    /// (the meeting notes' timing).
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
                self.day.feed_answered = util::ymd(&util::local_now());
            }
            DayEvent::SlackLooked(result) => {
                self.day.slack_busy = false;
                self.day.slack_looked = util::ymd(&util::local_now());
                self.day.slack_failed = result.is_err();
                match result {
                    Ok(line) => log(&dirs.state, &line),
                    Err(e) => log(&dirs.state, &format!("slack: {e}")),
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
    /// minute old: the server's own look, or, with slack_background off, a
    /// Slack tab opened (or the one there asked to look between its ticks).
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
        if !self.day.slack_kept {
            self.open_loop_tab("Slack", "5m", "/slack-updates", &dirs);
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

    /// Leaving the Slack view: what its look brought is seen.
    pub fn slack_left(&mut self) {
        let dirs = Dirs::here();
        slack::mark_seen(&dirs.state);
        self.day_views(&dirs, &util::local_now());
    }

    /// A Slack message acted on from its menu (an item of the Slack view or
    /// of the "slack" notifications): seen, on its own.
    pub fn slack_item_acted(&mut self, item: &Item) {
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

    // Once a minute: the notes due, and the tabs that keep a view out of
    // date with no feed or look of its own.
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
        // Meeting notes, one look at a time, since a transcript can take
        // minutes.
        let events = self
            .day
            .today(dirs, now)
            .map(|d| d.events)
            .unwrap_or_default();
        if settings::is_on("meeting_notes")
            && !self.day.busy.contains(&Job::Meetings)
            && meetings::meetings_due(now, self.day.meetings_tried, &events)
        {
            self.day.meetings_tried = ms;
            self.spawn_note(Job::Meetings, async move {
                let (dirs, now) = (Dirs::here(), util::local_now());
                match meetings::write_meetings(&dirs, &now, &Claude).await {
                    Ok(lines) => lines,
                    Err(e) => vec![format!("meetings: {e}")],
                }
            });
        }
        if !self.day.away {
            self.todo_minute(dirs, now);
        }
        self.keep_up(dirs, now);
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

    // Today's to-do list, each weekday from the hour todo_note_time names,
    // with the todo_note setting on, once every view it reads from has
    // answered today (or `TODO_WAIT_MS` has passed, when it is named as
    // left out), so a note written once a day isn't written from nothing or
    // from yesterday.
    fn todo_minute(&mut self, dirs: &Dirs, now: &Zoned) {
        let ms = now.timestamp().as_millisecond();
        let hour = settings::cached("todo_note_time")
            .map(|v| util::js_string(&v))
            .and_then(|v| v.parse::<i8>().ok())
            .unwrap_or(9);
        if !settings::is_on("todo_note")
            || self.day.busy.contains(&Job::Todo)
            || !todo::todo_due(&dirs.notes, now, hour, self.day.todo_tried)
        {
            return;
        }
        let today = util::ymd(now);
        if util::ymd(&util::local_at_ms(self.day.todo_due_since)) != today {
            self.day.todo_due_since = ms;
        }
        let waited_out = ms - self.day.todo_due_since >= TODO_WAIT_MS;
        let (github, linear) = (settings::is_on("github"), settings::is_on("linear"));
        let mut sources = todo::Sources {
            github,
            ..todo::Sources::default()
        };
        let mut waiting = false;
        let views = [
            (
                "prs",
                github && settings::is_on("pr_board"),
                "your pull requests",
            ),
            ("ci", github && settings::is_on("view_ci"), "failing checks"),
            (
                "tickets",
                linear && settings::is_on("ticket_board"),
                "Linear tickets",
            ),
            (
                "inbox",
                linear && settings::is_on("view_inbox"),
                "the Linear inbox",
            ),
        ];
        for (name, on, words) in views {
            if !on {
                continue;
            }
            let lines = match self.day.views.get(name).filter(|a| a.day == today) {
                Some(a) if a.failed => {
                    sources.missing.push(words.into());
                    continue;
                }
                Some(a) => a.lines.clone(),
                None if waited_out => {
                    sources.missing.push(words.into());
                    continue;
                }
                None => {
                    waiting = true;
                    continue;
                }
            };
            match name {
                "prs" => sources.prs = lines,
                "ci" => sources.failing = lines,
                "tickets" => sources.tickets = lines,
                _ => sources.inbox = lines,
            }
        }
        let calendar_on = settings::is_on("view_calendar");
        let slack_on = settings::is_on("view_slack");
        let background = settings::is_on("slack_background");
        if calendar_on && self.day.feed_answered != today && !waited_out {
            waiting = true;
        }
        let saved_slack = slack::read(&dirs.state);
        let slack_today = saved_slack
            .as_ref()
            .is_some_and(|s| util::ymd(&util::local_at_ms(s.checked)) == today);
        if slack_on
            && !waited_out
            && ((background && self.day.slack_looked != today) || (!background && !slack_today))
        {
            waiting = true;
        }
        if waiting {
            return;
        }
        self.day.todo_tried = ms;
        let tz = now.time_zone();
        let day = calendar_on.then(|| self.day.today(dirs, now)).flatten();
        if let Some(day) = day.as_ref().filter(|d| d.date == today) {
            sources.calendar = day
                .events
                .iter()
                .map(|e| {
                    let when = if e.all_day {
                        "all day".to_string()
                    } else {
                        e.starts(tz)
                            .map(|t| {
                                let z = t.to_zoned(tz.clone());
                                let h = if z.hour() % 12 == 0 {
                                    12
                                } else {
                                    z.hour() % 12
                                };
                                format!(
                                    "{h}:{:02} {}",
                                    z.minute(),
                                    if z.hour() < 12 { "AM" } else { "PM" }
                                )
                            })
                            .unwrap_or_default()
                    };
                    let you = if e.going() {
                        String::new()
                    } else {
                        format!(" ({})", e.you)
                    };
                    format!("{when} {}{you}", e.title)
                })
                .collect();
        } else if calendar_on && self.day.feed_error().is_some() {
            sources.missing.push("today's calendar".into());
        }
        let slack_failed = self.day.slack.error.is_some() || (background && self.day.slack_failed);
        if slack_on {
            if slack_failed {
                sources.missing.push("Slack".into());
            } else {
                sources.slack = self
                    .day
                    .slack
                    .items
                    .iter()
                    .filter(|i| i.state == crate::items::State::Attention)
                    .map(|i| {
                        let mut line = [i.title.as_str(), i.detail.as_str()]
                            .iter()
                            .filter(|s| !s.is_empty())
                            .copied()
                            .collect::<Vec<_>>()
                            .join(" · ");
                        if let crate::items::Target::Url(u) = &i.target {
                            line.push(' ');
                            line.push_str(u);
                        }
                        line
                    })
                    .collect();
            }
        }
        let tab_dirs = self.day_tab_dirs();
        self.spawn_note(Job::Todo, async move {
            let (dirs, now) = (Dirs::here(), util::local_now());
            match todo::write_todo(&dirs, &tab_dirs, &sources, &now, &ClaudeAsk).await {
                Ok(line) => line.into_iter().collect(),
                Err(e) => vec![format!("todo: {e}")],
            }
        });
    }

    // The Calendar or Slack view out of date with no tab keeping it: its
    // tab, opened in the background, at most every `REOPEN_MS` each.
    fn keep_up(&mut self, dirs: &Dirs, now: &Zoned) {
        let today = util::ymd(now);
        let ms = now.timestamp().as_millisecond();
        let saved = self.day.saved.read(&dirs.state).cloned();
        if !self.day.calendar_kept
            && calendar::calendar_stale(&self.day.feed, saved.as_ref(), &today, ms)
        {
            self.open_loop_tab("Calendar", "30m", "/today", dirs);
        }
        if !self.day.slack_kept
            && !settings::is_on("slack_background")
            && slack::stale(slack::read(&dirs.state).as_ref(), ms)
        {
            self.open_loop_tab("Slack", "5m", "/slack-updates", dirs);
        }
    }

    // A tab running one skill on a loop, `claude "/loop <every> <skill>"` in
    // your home directory under the tab name `name`, opened without taking
    // you there (tmux/loop-tab.sh --background). Marked `@loop_tab`, so the
    // Slack relay alerts on its waits but not on each tick it finishes.
    fn open_loop_tab(&mut self, name: &'static str, every: &str, skill: &str, dirs: &Dirs) {
        let ms = util::now_ms();
        if self
            .day
            .opened
            .get(name)
            .is_some_and(|at| ms - at < calendar::REOPEN_MS)
        {
            return;
        }
        self.day.opened.insert(name, ms);
        let session = self
            .latest_client()
            .and_then(|c| self.clients.get(&c))
            .map(|c| c.session.clone())
            .or_else(|| self.sessions.first().map(|s| s.name.clone()));
        let Some(session) = session else {
            return;
        };
        log(
            &dirs.state,
            &format!("{name}: out of date with no tab keeping it; opening one"),
        );
        if let Err(e) = self.start_loop_tab(
            &session,
            name,
            every,
            skill,
            crate::mux::server::Opened::Background,
        ) {
            log(&dirs.state, &format!("{name}: couldn't open its tab: {e}"));
        }
    }

    /// The tab itself, in `session`, from an interactive shell of yours for
    /// your PATH, left at that shell when Claude exits; in front unless
    /// `background` (M-p's "Open a Calendar tab" and "Open a Slack tab"
    /// bring it there, as loop-tab.sh did without `--background`).
    pub(crate) fn start_loop_tab(
        &mut self,
        session: &str,
        name: &str,
        every: &str,
        skill: &str,
        opened: crate::mux::server::Opened,
    ) -> anyhow::Result<crate::mux::TabId> {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
        let command = vec![
            shell.clone(),
            "-i".into(),
            "-c".into(),
            format!("claude '/loop {every} {skill}'; exec {shell} -i"),
        ];
        let (tab, _) = self.new_tab(
            session,
            Some(crate::paths::home()),
            Some(command),
            Some(name.into()),
            opened,
        )?;
        if let Some(t) = self.tabs.get_mut(&tab) {
            t.opts.insert("@loop_tab".into(), "1".into());
        }
        Ok(tab)
    }

    // ── The views and notifications ─────────────────────────────────────────

    // Works out the Calendar and Slack views and the notifications again,
    // drawing every client when a view changed.
    fn day_views(&mut self, dirs: &Dirs, now: &Zoned) {
        let today = util::ymd(now);
        let ms = now.timestamp().as_millisecond();
        let tz = now.time_zone().clone();
        let saved = self.day.saved.read(&dirs.state).cloned();
        let calendar = calendar::calendar_view(&calendar::Look {
            feed: &self.day.feed,
            saved: saved.as_ref(),
            looping: self.day.calendar_kept,
            now,
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
        });
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
