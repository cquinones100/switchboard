//! Mission control on the server: what bin/tmux-sidebar's daemon did for a
//! mission client (lib/sidebar/daemon.js's `openMission`, `draw`,
//! `missionKey`, `filterKey`, `missionMouse`, `answerKey`, the menus'
//! `act`s, `cameTo`, `announceReminders`), done in the server task.
//!
//! Before a client with mission control open is drawn, and before each of
//! its keys, what it shows is put together again from the server's state
//! (`mission_prepare`) once anything may have changed it since, so a tab
//! closing or a lookup answering while it is open shows at once, and a key
//! always acts on what is on the screen. Every event but a pane's output, a
//! spinner's turn and mission control's own keys counts as such a change
//! (`MissionState::changes`); a key that only moves the cursor or Tab's
//! focus, the mouse lighting what it is over, a spinner's turn and a pane's
//! output leave the snapshot standing, and take again only the spinners'
//! frame and the screens of the panes that changed, each pane's kept with
//! its `version` (`mission_refresh`), so moving about with many tabs open
//! costs only the draw.
//! Bringing a view to the front counts it for the habits and tells its
//! source, as the daemon's `cameTo` did: the Inbox and CI views and the
//! Landing view's merge queues ask again, the Calendar reads its feed, Slack looks, the usage
//! views are kept up to date while in front.
//!
//! The keys and the mouse come here before the overlays see them
//! (`mission_input`), while mission control is the box on top; a menu,
//! question or line to type opened over it takes them as any box does.
//! What an option does runs here (`mission_do`); one that waits on
//! something (a browser, Linear, git) runs in a task of its own under the
//! busy box and answers with `Event::Mission`.

use crate::sidebar::history::Sidebar;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::Value;

use super::answer::{self, AnswerBox, Press};
use super::ask::Ask;
use super::busy::Busy;
use super::choices::{self, Choice, Ctx, Do};
use super::inbox_board;
use super::remind;
use super::views::{self, Scope, View, ViewId};
use super::{After, Focus, Jump, Mission, MissionEvent, Pane, Snapshot, Spot};
use crate::items::{Item, Target, ViewData};
use crate::mux::input::{self, Input, Key, KeyCode, Mods, Mouse, MouseButton, MouseKind, Parsed};
use crate::mux::server::{ClientId, Event, Server};
use crate::mux::{PaneId, TabId};
use crate::sidebar::model::{self, Rules};
use crate::sidebar::render::Filter;
use crate::sidebar::text::SPINNER_MS;
use crate::ui::{Confirm, Menu, MenuOption, Outcome, PickItem, Picker, Theme};
use crate::usage::ViewKind;
use crate::{paths, settings, util};

/// How often the worktrees of the tabs' repositories are read again while
/// Reopen (or M-j's picker) is wanted.
const WORKTREES_EVERY: Duration = Duration::from_secs(60);
/// The key Reopen's merged worktrees are shown under in a client's
/// `Folds::merged_open`, beside the Work map's repositories.
const REOPEN_MERGED: &str = "reopen";
/// How long `wt-revive --json` may take before the look is given up.
const REVIVED_TIMEOUT: Duration = Duration::from_secs(30);
/// How long your own files' Settings rows are kept before they are read
/// again.
const OWN_EVERY: Duration = Duration::from_secs(3);
/// How long before a repository's landing is worked out again after a look
/// that failed.
const LANDING_RETRY: Duration = Duration::from_secs(5 * 60);
/// How long after a tab opens its shell is typed into: it has drawn its
/// prompt by then.
const TYPE_AFTER: Duration = Duration::from_millis(500);
/// How long after an answer the tab's screen is read again, for the next
/// question of several.
const REANSWER_AFTER: Duration = Duration::from_millis(400);

/// How often the cards' spinners turn at most behind a box over mission
/// control (a menu, a question, the answer box), since the box is what is
/// being read (the daemon's `BOXED_SPIN_MS`); the busy box's own spinner
/// keeps its rate.
const BOXED_SPIN_MS: i64 = 500;

/// How long the note items found for a Calendar event stand before
/// they are read again.
const CALENDAR_NOTES_EVERY: Duration = Duration::from_secs(5 * 60);

/// The Inbox board's rows whose photos are asked for: this many around
/// the chosen one, more than a screen shows, so the rows a scroll
/// brings in have theirs.
const INBOX_PHOTOS: usize = 40;
/// How long after a ticket was asked for the Inbox asks for it again
/// while its copy is older than its notification, or the read failed.
const INBOX_TICKET_AGAIN: Duration = Duration::from_secs(60);
/// How long a ticket read for the Inbox stands before it is read again as
/// it is chosen, the copy shown meanwhile.
const INBOX_TICKET_EVERY: Duration = Duration::from_secs(5 * 60);

/// Mission control's keys, for `?` (render.js's `HELP.mission`), and its
/// Keys view.
pub(super) const KEYS: &[(&str, &str)] = &[
    (
        "h j k l",
        "choose a card or row; in the Calendar, j k walk the events and h l move a day",
    ),
    ("g G", "first, last"),
    (
        "J K",
        "scroll the pane beside a board or list (a PR's or a ticket's description, the Autopilot plan, a CI check's failing step) down, up; the wheel over it too",
    ),
    (
        "C-w h",
        "the focus to the view's pane at the left (the Settings board's categories, or the list from the pane beside it), as Vim's windows; j k there walk the categories, ↵ back to the options",
    ),
    (
        "C-w l",
        "the focus to the view's pane at the right (the pane beside a list, j k scrolling it a line, g G its top and end); a click in a pane gives it the focus too",
    ),
    (
        "C-w w",
        "the focus to the view's next pane round (C-w W the one before)",
    ),
    (
        "C-M-S-h C-M-S-l",
        "with the list of views in the sidebar's place and the keys on it, move the sidebar's edge; else widen, narrow the pane beside the PR, ticket or Autopilot board (the pane resize keys, as keys.json binds them without the prefix, which the footer names); a drag of its edge too; each board keeps its width",
    ),
    (
        "↵",
        "the chosen card's or row's actions, in every view, what ↵ did before them first (so ↵↵ does it: on a tab, go there)",
    ),
    (
        "m",
        "the view's own actions (a kind of notification, today or this week, a sort, which rows, finding a key, a day, autopilot's menu), then several views at once and this view's settings",
    ),
    (
        "⇧F10",
        "what a right click opens: the chosen item's actions, as ↵, or the view's, as m",
    ),
    (
        "↵ on a fold line",
        "Reopen: its merged worktrees' menu shows or hides them, or removes them after asking",
    ),
    (
        "i",
        "on a narrow screen, the Slack message's thread, the Inbox item's ticket or the landing's last output in the list's place (i or esc back)",
    ),
    ("/", "filter the cards and rows (↵ keep, esc clear)"),
    ("1-9", "choose that tab"),
    ("] [", "next, last view in its group"),
    ("M-] M-[", "the next, last group of views"),
    (
        "M-{ M-}",
        "back, forward through the tabs and views visited, closing it onto a tab",
    ),
    (
        "tab ⇧tab",
        "move focus through the groups' and views' names, the filter, each section of the view and the panes beside it, the footer's keys and easy mode's ×, round; ↵ presses it (with tab_focus off: next, last view)",
    ),
    ("v", "go to a view by typing its name (↵ go, esc cancel)"),
    ("esc q", "close"),
    ("M-w", "close, whatever is open over it"),
    (
        "M-1-9",
        "close and go to that tab, or a click on a tab in the sidebar",
    ),
    (
        "M-p",
        "the palette over it: a view picked shows here, anything else runs in the pane once closed",
    ),
    ("?", "these keys"),
];

/// The view menu's way to autopilot: its settings box
/// (`autopilot_menu`), else its menu.
/// Whether M-j's picker searches what the panes show for this filter.
fn searches_panes(m: &Mission, text: &str) -> bool {
    m.picker
        && m.scope == Some(Scope::Tabs)
        && !text.is_empty()
        && settings::is_on("tab_search_panes")
}

fn autopilot_label() -> &'static str {
    "Autopilot's settings…"
}

/// What an option of a view's menu (`m`) does.
enum ViewAct {
    /// What it does over all the view shows (`view_acts`).
    Whole(super::view_acts::Whole),
    /// The Notifications view narrowed to a kind, by its key (1 all).
    Kind(usize),
    /// The Usage view between today and this week.
    UsageWeek,
    /// The Resources view sorted by the other of CPU and memory.
    Sort,
    FindKey,
    /// A file opened in your editor: the Settings view's.
    EditFile(PathBuf, String),
    /// Autopilot's own menu.
    Autopilot,
    /// A change to autopilot as a whole, as its menu's option would make.
    Pilot(crate::autopilot::Act),
    /// The Reopen view's rows narrowed to tabs, to sessions, or both.
    ReopenKind(crate::usage::reopen::Kind),
    /// The Reopen view's chosen row's pane shown over the list, or put
    /// away, as `i` does.
    ReopenPane,
    /// The same for the Inbox board's ticket, the Routines board's
    /// routine, and the Reminders month's lists under it.
    InboxPane,
    RoutinesPane,
    RemindersLists,
    /// A day of the Notifications history unfolded.
    Unfold(String),
    /// The Calendar a day back or forward, or on today.
    Day(i64),
    Today,
    /// A reminder for a tab on a day of the Reminders view, or its today.
    RemindOn(jiff::civil::Date),
    RemindToday,
    /// The box for a new routine (src/routines).
    NewRoutine,
    /// Every notification the sidebar shows put away, as dismissing each
    /// alone would.
    DismissShown,
    /// A plugin's view asked again.
    AskAgain(String),
    ViewsAtOnce,
    /// The Settings view, filtered by the view's name.
    Settings(String),
    /// The sidebar's repository filter cleared, which the Tabs view follows.
    EveryRepo,
}

/// The file the Settings view opens for the chosen row: a file of yours
/// for one of its settings (`repos.json` for a repository's landing), else
/// the settings file, with the folder it is in.
fn settings_file_of(item: Option<&Item>) -> (PathBuf, String) {
    if let Some(file) = super::settings_board::file_of(item) {
        return (crate::usage::own::dir_of(&file), file);
    }
    // A relative SWITCHBOARD_SETTINGS is the server's, not the editor's,
    // which starts in the file's folder.
    let path = settings::settings_file();
    let path = std::path::absolute(&path).unwrap_or(path);
    let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
    let file = path
        .file_name()
        .map(|f| f.to_string_lossy().into_owned())
        .unwrap_or_else(|| "settings.json".into());
    (dir, file)
}

/// The columns "Answer: " takes before an option's label in a tab's menu,
/// which offers an option only where both show whole.
const ANSWER_PREFIX_W: usize = 8;

/// The notification a menu is for: its source, its item under its own id,
/// and whether to put it away after what is picked.
type Notice = Option<(String, Item, bool)>;

/// Where a tab's menu is opened from: mission control's leads with going
/// there; the sidebar's with peeking, chosen as it opens from m, Space or a
/// right click, or with going there chosen from Enter, so a second Enter
/// does what Enter did alone.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum TabMenuFrom {
    Mission,
    SidebarPeek,
    SidebarGo,
}

/// A kept notification's `Notice`: put away after only while the sidebar
/// still shows it.
fn kept_notice(item: &Item) -> Notice {
    notice_for(item, super::history::shown(item))
}

/// The notification a menu of the Notifications view is for, put away
/// after what is picked when `dismiss` says so and its kind is one
/// dismissing puts away (`choices::dismisses`): a suggestion is recorded
/// too, though its own choices say what becomes of it.
fn notice_for(item: &Item, dismiss: bool) -> Notice {
    let (source, id) = choices::notice_of(item);
    if source.is_empty() {
        return None;
    }
    let dismiss = dismiss && choices::dismisses(item).is_some();
    let mut own = item.clone();
    own.id = id;
    Some((source, own, dismiss))
}

/// Whether a row of the Notifications view's history is a notification,
/// not a day's heading or a fold.
fn is_a_notice(item: &Item) -> bool {
    super::history::notice_of(item).is_some()
}

/// How a client looks at the Notifications view, for its rows: the kind
/// its keycaps chose, the days it unfolded, and how many rows fit before a
/// day folds.
pub(crate) struct NoticeLook {
    kind: usize,
    unfolded: std::collections::HashSet<String>,
    screenful: usize,
}

/// What mission control opens on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Opening {
    /// The tabs (M-Space).
    Tabs,
    /// The Settings view (M-p's "Settings").
    Settings,
    /// A view by its name, lowercased with dashes for spaces (M-p's
    /// "Mission control: <view>"); the tabs when it is off.
    View(String),
    /// The Work map as a picker, scoped (M-j), or the Reopen view as one
    /// (M-o).
    Picker(Scope),
}

/// The shades mission control is drawn in for a client: the light ones
/// when the theme is light, the canvas's when its terminal draws truecolor
/// (crates/ui/src/theme.rs), as the sidebar's. Asked of the client itself,
/// since its snapshot is also made outside a draw (opening it, a key).
fn theme_for(truecolor: bool) -> Theme {
    crate::status::theme::drawing_for(truecolor, crate::status::theme::theme)
}

/// The lines a notch of the wheel over the pane beside a board scrolls it.
const SIDE_WHEEL: isize = 3;

fn plain(key: &Key) -> bool {
    key.mods.is_empty()
}

impl Server {
    // ── Opening and closing ─────────────────────────────────────────────────

    /// M-Space: mission control, or, when it is open, closed.
    pub fn toggle_mission(&mut self, client: ClientId) {
        if self.mission_open(client) {
            self.mission_close(client);
        } else {
            self.open_mission(client, Opening::Tabs);
        }
    }

    /// Whether a client has mission control open.
    pub fn mission_open(&self, client: ClientId) -> bool {
        self.clients
            .get(&client)
            .is_some_and(|c| c.overlays.find::<Mission>().is_some())
    }

    /// Opens mission control on a client (or brings the one open there to
    /// the view asked for).
    pub fn open_mission(&mut self, client: ClientId, how: Opening) {
        if !self.clients.contains_key(&client) {
            return;
        }
        let fresh = !self.mission_open(client);
        if fresh {
            let mut m = Mission::new();
            m.side_widths = super::side_widths();
            // Its first opening says how to get around it, in its own
            // footer (a tip's corner box would sit over its bar).
            if how == Opening::Tabs && settings::is_on("tips") && !self.tip_was_seen("mission") {
                m.note = Some(if settings::is_on("tab_focus") {
                    "Tip: [ ] switch views · Tab moves focus · Enter acts on it · m on the view · ? every key".into()
                } else {
                    "Tip: Tab and [ ] switch views · Enter acts on it · m on the view · ? every key"
                        .into()
                });
                self.tip_seen("mission");
            }
            self.open(client, Box::new(m), None);
        }
        let picker = matches!(how, Opening::Picker(_));
        self.mission_worktrees(picker);
        if let Opening::Picker(Scope::Untabbed) = how {
            self.mission_revived(true);
        }
        if let Opening::Picker(Scope::Reopen) = how {
            self.want_view(ViewKind::Reopen);
            self.want_past_sessions();
        }
        let Some(mut m) = self.take_mission(client) else {
            return;
        };
        // With several views shown, a view among them by name takes the
        // focus before anything is put together again, so the one going to
        // the back keeps the row it is on (`focus_cell`).
        let shown_by_name = match &how {
            _ if m.grid.is_none() => None,
            Opening::Tabs => Some(0),
            Opening::Settings => m.place_of("settings"),
            Opening::View(slug) => m
                .snap
                .views
                .iter()
                .position(|v| v.slug() == *slug)
                .map(|i| i + 1),
            Opening::Picker(_) => None,
        }
        .filter(|at| m.grid_shown().iter().any(|(_, shown)| shown == at));
        if let Some(at) = shown_by_name {
            m.slack_fresh = how == Opening::View("slack".into());
            show_view(&mut m, at);
            self.put_mission(client, m);
            self.mission_prepare(client);
            return;
        }
        m.scope = None;
        m.picker = false;
        m.cursor = 0;
        m.slack_fresh = how == Opening::View("slack".into());
        // The Landing view opens even with nothing landing, saying so.
        m.keep_landing = how == Opening::View("landing".into());
        m.keep_autopilot = how == Opening::View("autopilot".into());
        if let Opening::Picker(scope) = how {
            m.scope = Some(scope);
            m.picker = true;
            m.filter = Some(Filter {
                text: String::new(),
                typing: true,
            });
        }
        self.put_mission(client, m);
        self.mission_prepare(client);
        let Some(mut m) = self.take_mission(client) else {
            return;
        };
        let find = |m: &Mission, f: &dyn Fn(&View) -> bool| {
            m.snap.views.iter().position(f).map_or(0, |i| i + 1)
        };
        let to = match &how {
            Opening::Tabs => 0,
            Opening::Settings => find(&m, &|v| v.id == ViewId::Settings),
            Opening::View(slug) => find(&m, &|v| v.slug() == *slug),
            Opening::Picker(Scope::Reopen) => find(&m, &|v| v.id == ViewId::Reopen),
            Opening::Picker(_) => find(&m, &|v| v.id == ViewId::WorkMap),
        };
        // The view opened starts on its list.
        m.pane = None;
        // With several views at once, one shown takes the focus and
        // another shows alone.
        match how {
            Opening::Picker(_) => {
                m.grid = None;
                m.view = to;
                m.cursor = 0;
            }
            _ if m.grid.is_some() => show_view(&mut m, to),
            _ => m.view = to,
        }
        self.put_mission(client, m);
        self.mission_prepare(client);
    }

    /// Mission control on `view` with the cursor on its item `id`, as a
    /// notification's "Show it in" takes it there: a Slack message's
    /// thread row, which stands for it (`slack_board::members`), its
    /// thread beside it. The view's first row while it no longer lists it.
    fn show_in_view(&mut self, client: ClientId, view: &ViewId, id: &str) {
        // The Slack board opens on its first section before the cursor
        // reaches the item, which never showed it: what it had viewed
        // before stands, and only the item's section counts as shown.
        let viewed_before = self
            .mission_mut(client)
            .and_then(|m| m.slack_viewed.clone());
        self.open_mission(client, Opening::View(view.slug()));
        let Some(m) = self.mission_mut(client) else {
            return;
        };
        m.slack_viewed = viewed_before;
        let stands_for = |item: &Item| {
            item.id == id
                || super::slack_board::members(item)
                    .iter()
                    .any(|member| member.id == id)
        };
        let at = m
            .current()
            .filter(|v| v.id == *view)
            .and_then(|v| v.data.items.iter().position(stands_for));
        if let Some(at) = at {
            m.cursor = at as isize;
        }
        self.mission_prepare(client);
    }

    /// Closes mission control on a client, and whatever is open over it.
    pub fn mission_close(&mut self, client: ClientId) {
        let Some(c) = self.clients.get(&client) else {
            return;
        };
        let kinds = c.overlays.kinds();
        let Some(at) = kinds.iter().position(|k| *k == "mission") else {
            return;
        };
        let leaving_slack = c
            .overlays
            .find::<Mission>()
            .filter(|m| m.came.as_deref() == Some("Slack") || m.grid_has("slack"))
            .map(Mission::slack_seen);
        let shown = c
            .overlays
            .find::<Mission>()
            .filter(|m| m.walk_unsettled())
            .map(Mission::view_shown);
        self.settle_view_left(client, shown);
        let above: Vec<&'static str> = kinds[at + 1..].to_vec();
        for kind in above.into_iter().rev() {
            self.close_overlays(client, kind);
        }
        self.close_overlays(client, "mission");
        if let Some(seen) = leaving_slack {
            self.slack_left(seen);
        }
    }

    /// Mission control closing while the list of views' cursor hasn't
    /// rested (`Mission::nav_held`), `shown` the view on the screen: it is
    /// the place left, so M-Tab from the tab comes back to it, rather than
    /// the place from before the walk. Not when the session already went
    /// to a tab (its trail on no view), which visited the place it left.
    fn settle_view_left(&mut self, client: ClientId, shown: Option<String>) {
        let Some(shown) = shown else {
            return;
        };
        let Some(session) = self.clients.get(&client).map(|c| c.session.clone()) else {
            return;
        };
        let on_a_view = self
            .session(&session)
            .is_some_and(|s| s.trail.view.is_some());
        if on_a_view {
            self.trail_view_is(&session, Some(shown), true);
        }
    }

    /// The client's mission control, to change it.
    fn mission_mut(&mut self, client: ClientId) -> Option<&mut Mission> {
        self.clients
            .get_mut(&client)?
            .overlays
            .find_mut::<Mission>()
    }

    /// Takes a client's mission control out of its overlay, leaving a blank
    /// one in its place, so it can be changed with the whole server at hand;
    /// `put_mission` puts it back.
    fn take_mission(&mut self, client: ClientId) -> Option<Mission> {
        self.mission_mut(client).map(std::mem::take)
    }

    fn put_mission(&mut self, client: ClientId, m: Mission) {
        if let Some(slot) = self.mission_mut(client) {
            *slot = m;
        }
        if let Some(c) = self.clients.get_mut(&client) {
            c.dirty = true;
        }
    }

    /// What the last action did, in mission control's footer when it is
    /// open, else in a box.
    pub fn mission_note(&mut self, client: ClientId, note: impl Into<String>) {
        let note = note.into();
        if note.trim().is_empty() {
            return;
        }
        match self.mission_mut(client) {
            Some(m) => {
                m.note = Some(note);
                if let Some(c) = self.clients.get_mut(&client) {
                    c.dirty = true;
                }
            }
            None => self.say(Some(client), &note),
        }
    }

    // ── What it shows ───────────────────────────────────────────────────────

    /// Before a client is drawn: its mission control's snapshot taken again
    /// once anything has changed since, else only what changes without an
    /// event.
    pub fn mission_before_draw(&mut self, client: ClientId) {
        if !self.mission_open(client) {
            return;
        }
        if self.mission_stands(client) {
            self.mission_refresh(client);
        } else {
            self.mission_prepare(client);
        }
        self.mission_diagrams(client);
        self.work_map_count(client);
    }

    /// Asks, off the server's task, for the Mermaid diagrams of the
    /// description a client's board shows beside it, where the client draws
    /// them as pictures: once for each description chosen, and only those
    /// nothing is known of yet (`diagrams::Diagrams`). Each comes back as
    /// `MissionEvent::Diagram`, one Chrome at a time.
    fn mission_diagrams(&mut self, client: ClientId) {
        use std::hash::{Hash, Hasher};
        /// Held by the one task drawing, since each diagram is a run of
        /// Chrome.
        static ONE_AT_A_TIME: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let light = crate::status::theme::light();
        let Some(m) = self.mission_mut(client) else {
            return;
        };
        if m.snap.cell.is_none() {
            return;
        }
        let Some((text, place)) = super::render::description_beside(m) else {
            return;
        };
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        (text, light).hash(&mut hasher);
        let description = Some(hasher.finish());
        if m.diagrams_asked == description {
            return;
        }
        let sources = super::diagrams::sources(text, place);
        m.diagrams_asked = description;
        let sources = self.mission.diagrams.ask(sources, light);
        if sources.is_empty() {
            return;
        }
        // Fitted to a cell wider than any terminal's, not the asker's, since
        // every terminal shares what is kept and each picture is scaled to
        // its own cells as it is drawn (`ui::images::raster_picture`).
        let width = super::render::ABOUT_COLS as u32 * super::diagrams::WIDEST_CELL;
        let tx = self.tx.clone();
        tokio::task::spawn_blocking(move || {
            let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
            crate::views::pictures::draw_diagrams(&sources, light, |source, drawn| {
                let _ = tx.send(Event::Mission(MissionEvent::Diagram {
                    source: source.to_string(),
                    light,
                    image: drawn.ok().map(|img| super::diagrams::fitted(img, width)),
                }));
            });
        });
    }

    /// Something that may change what mission control shows has happened:
    /// every snapshot taken before it is put together again.
    pub(crate) fn mission_changed(&mut self) {
        self.mission.changes += 1;
    }

    /// Whether a client's snapshot was taken since the last change.
    fn mission_stands(&self, client: ClientId) -> bool {
        self.clients
            .get(&client)
            .and_then(|c| c.overlays.find::<Mission>())
            .is_some_and(|m| m.taken_at == Some(self.mission.changes))
    }

    /// What a standing snapshot takes again: the spinners' frame, and with
    /// UX 2.0 the screens of the panes that have changed. While a box is
    /// over it, both at most every `BOXED_SPIN_MS`, so a key typed in the
    /// box (the Slack reply box's) draws the box again rather than reading
    /// every tab's screen behind it.
    fn mission_refresh(&mut self, client: ClientId) {
        let boxed = self.mission_boxed(client);
        let Some(m) = self.mission_mut(client) else {
            return;
        };
        let was = (m.snap.frame, m.spun_at);
        m.snap.frame = spin_frame(m, boxed);
        let held = boxed && (m.snap.frame, m.spun_at) == was;
        if held {
            return;
        }
        let screens = self.mission_screens();
        if let Some(m) = self.mission_mut(client) {
            m.snap.screens = screens;
        }
    }

    /// Whether a box other than the busy box is over a client's mission
    /// control, which holds the spinners back.
    fn mission_boxed(&self, client: ClientId) -> bool {
        self.clients
            .get(&client)
            .is_some_and(|c| !matches!(c.overlays.top(), None | Some("mission" | "busy")))
    }

    /// Each tab's screen for UX 2.0's cards: the lines of its Claude's pane
    /// (else its active one) in the pane's own colors, from its emulator in
    /// memory, and the branch its directory was last named on. A pane is
    /// drawn again only when its `version` has changed since it was last.
    fn mission_screens(&mut self) -> std::collections::HashMap<TabId, super::Screen> {
        let mut cache = std::mem::take(&mut self.mission.screens);
        let mut kept = std::collections::HashMap::new();
        let mut out = std::collections::HashMap::new();
        for (&id, tab) in &self.tabs {
            let pane = self
                .sidebar
                .panes
                .iter()
                .find(|(p, r)| r.claude && tab.layout.panes().contains(p))
                .map_or(tab.active, |(p, _)| *p);
            let lines = match self.panes.get(&pane) {
                Some(slot) => {
                    let version = slot.pane.version();
                    let lines = match cache.remove(&pane) {
                        Some((drawn, lines)) if drawn == version => lines,
                        _ => slot.pane.styled_lines().into(),
                    };
                    kept.insert(pane, (version, super::Lines::clone(&lines)));
                    lines
                }
                None => super::Lines::default(),
            };
            let branch = self
                .tab_path(id)
                .and_then(|p| self.tab_names.names.branch(p))
                .map(str::to_string);
            // A waiting Claude's numbered options, as its screen shows them,
            // read from its own pane, parked behind a tool in its place too.
            let asking = self
                .tab_panes(id)
                .into_iter()
                .find(|p| self.agents.state(*p) == Some(crate::agent::State::Waiting));
            let (options, question, more) = match asking.and_then(|p| self.panes.get(&p)) {
                Some(slot) => answer::options_in(&slot.pane.text(60))
                    .map(|p| {
                        (
                            p.options.into_iter().map(|o| o.label).collect(),
                            p.question,
                            p.more,
                        )
                    })
                    .unwrap_or_default(),
                _ => Default::default(),
            };
            out.insert(
                id,
                super::Screen {
                    lines,
                    branch,
                    options,
                    question,
                    more,
                },
            );
        }
        self.mission.screens = kept;
        out
    }

    /// What the Calendar view in front draws beyond its items: the next
    /// working day after the day shown, the note items that name the chosen
    /// event or what it names (read off the task, kept a few minutes per
    /// event), and the photos of who is going to it, those loaded.
    fn calendar_extras(&mut self, m: &mut Mission) {
        let Some(view) = m.current().filter(|v| v.id == ViewId::Calendar) else {
            return;
        };
        let _ = view;
        let today = util::ymd(&util::local_now());
        let date = m.calendar_day.clone().unwrap_or(today);
        m.snap.calendar_next = crate::day::calendar::next_day(&self.day.feed, &date, &m.snap.tz);
        let Some(item) = m.chosen_item().cloned() else {
            return;
        };
        let Ok(event) = serde_json::from_value::<crate::day::calendar::CalEvent>(item.data.clone())
        else {
            return;
        };
        // An attendee's photo sends their address to Linear or Slack, so
        // only with its own setting on.
        if settings::is_on("calendar_photos") {
            for who in super::calendar::people(&event) {
                if let Some(photo) = self.photo(&who) {
                    m.snap.photos.insert(who, photo);
                }
            }
        }
        // The note items show in the pane beside the day.
        let fresh = self
            .mission
            .calendar_notes
            .get(&item.id)
            .filter(|(at, _)| at.elapsed() < CALENDAR_NOTES_EVERY);
        match fresh {
            Some((_, lines)) => m.snap.calendar_notes = lines.clone(),
            None => {
                if let Some((_, lines)) = self.mission.calendar_notes.get(&item.id) {
                    m.snap.calendar_notes = lines.clone();
                }
                self.read_calendar_notes(item.id.clone(), &event);
            }
        }
    }

    /// What the Slack board in front draws beyond its items: the chosen
    /// message's thread and its pictures (`Server::slack_extras`), and the
    /// photos of the people it shows.
    fn slack_board_extras(&mut self, m: &mut Mission) {
        // A Slack cell without the focus, among views shown at once, shows
        // its first section, which has been shown too.
        let cell = m
            .grid_shown()
            .into_iter()
            .filter(|(_, at)| *at != m.view)
            .filter_map(|(_, at)| at.checked_sub(1).and_then(|i| m.snap.views.get(i)))
            .find(|v| super::render::slack_board_shown(v))
            .cloned();
        // The workspace's own emoji in what the boards show: a cell's first
        // section, then the board in front's rows and thread.
        let mut emoji_names: Vec<String> = Vec::new();
        if let Some(v) = cell {
            let first = super::slack_board::shown(&v.data.items, -1);
            emoji_names.extend(
                first
                    .iter()
                    .flat_map(|i| super::slack_board::words_of(&v.data.items[*i]))
                    .flat_map(|text| {
                        crate::day::slack_emoji::custom_names(&text).collect::<Vec<_>>()
                    }),
            );
            m.slack_viewed.get_or_insert_default().extend(
                first
                    .into_iter()
                    .flat_map(|i| super::slack_board::members(&v.data.items[i]))
                    .map(|item| (item.id.clone(), item)),
            );
        }
        let Some(view) = m
            .current()
            .filter(|v| super::render::slack_board_shown(v))
            .cloned()
        else {
            // The board isn't in front, so it shows no thread: a Slack
            // cell without the focus, among views shown at once, has no
            // message chosen.
            self.slack_pane_shows(None);
            m.slack_thread_open = false;
            m.snap.slack_emoji = self.slack_emoji(emoji_names);
            return;
        };
        // What the board shows has been shown, every message of a thread's
        // row.
        let shown = slack_shown(m, &view);
        m.slack_viewed.get_or_insert_default().extend(
            shown
                .into_iter()
                .flat_map(|i| super::slack_board::members(&view.data.items[i]))
                .map(|item| (item.id.clone(), item)),
        );
        // Its thread beside the list, or in its place: opened once it has
        // shown a moment.
        let pane_shown = settings::is_on("slack_threads")
            && (super::slack_board::beside(m.laid().0 as usize) || m.slack_thread_open);
        self.slack_pane_shows(m.chosen_item().filter(|_| pane_shown));
        let extras = self.slack_extras(m.chosen_item());
        let (thread, pictures) = (extras.thread, extras.pictures);
        m.snap.slack_thread_failed = extras.failed;
        m.snap.slack_files_failed = extras.unread;
        m.snap.slack_files_kept = extras.kept;
        for who in super::slack_board::people(&view, thread.as_ref()) {
            if let Some(photo) = self.photo(&who) {
                m.snap.photos.insert(who, photo);
            }
        }
        emoji_names.extend(
            slack_shown(m, &view)
                .into_iter()
                .flat_map(|i| super::slack_board::words_of(&view.data.items[i]))
                .chain(
                    thread
                        .iter()
                        .flat_map(|t| t.messages.iter().map(|s| s.text.clone())),
                )
                .flat_map(|text| crate::day::slack_emoji::custom_names(&text).collect::<Vec<_>>()),
        );
        m.snap.slack_emoji = self.slack_emoji(emoji_names);
        m.snap.slack_thread = thread;
        m.snap.slack_pictures = pictures;
    }

    /// The Reopen view's chosen row's last screen, read from its small file
    /// once and kept while it stays chosen.
    fn reopen_extras(&mut self, m: &mut Mission) {
        let name = m
            .current()
            .filter(|v| v.id == ViewId::Reopen)
            .and_then(|_| m.chosen_item())
            .and_then(crate::usage::reopen::of)
            .and_then(|r| r["screen"].as_str())
            .map(str::to_string);
        let Some(name) = name else {
            m.snap.reopen_shot = None;
            return;
        };
        let kept = self
            .mission
            .reopen_shot
            .as_ref()
            .filter(|(n, _)| *n == name)
            .map(|(_, shot)| shot.clone());
        let shot = kept.or_else(|| {
            let shot = std::sync::Arc::new(crate::usage::shots::read(&paths::state_dir(), &name)?);
            self.mission.reopen_shot = Some((name, shot.clone()));
            Some(shot)
        });
        m.snap.reopen_shot = shot;
    }

    /// Reads, off the task, the note items that name an event or what it
    /// names, one read at a time; local alone, never sent.
    fn read_calendar_notes(&mut self, id: String, event: &crate::day::calendar::CalEvent) {
        if self.mission.calendar_notes_busy || tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        self.mission.calendar_notes_busy = true;
        let title = event.title.clone();
        // Each ticket or PR by its id and by its link, which a note may
        // name it by instead.
        let refs: Vec<String> = event
            .refs
            .iter()
            .flat_map(|r| std::iter::once(r.id.clone()).chain(r.url.clone()))
            .collect();
        let tx = self.tx.clone();
        tokio::task::spawn_blocking(move || {
            let dir = crate::notes::store::notes_dir();
            let lines = super::calendar::notes_naming(&dir, &title, &refs);
            let _ = tx.send(Event::Mission(MissionEvent::CalendarNotes(id, lines)));
        });
    }

    /// What the Inbox board in front draws beyond its items: the photos of
    /// who acted and of who wrote the comments it shows, those loaded; the
    /// commits naming its tickets; and the chosen notification's ticket,
    /// read off the task when there is no copy yet, the notification is
    /// newer than the copy, or the copy was read five minutes ago.
    fn inbox_extras(&mut self, m: &mut Mission) {
        let Some(view) = m.current().filter(|v| inbox_board::shown(v)) else {
            return;
        };
        let actors: Vec<_> = view
            .data
            .items
            .iter()
            .skip(
                usize::try_from(m.cursor)
                    .unwrap_or(0)
                    .saturating_sub(INBOX_PHOTOS / 2),
            )
            .take(INBOX_PHOTOS)
            .filter_map(inbox_board::actor_of)
            .collect();
        m.snap.inbox_links = self.sources.inbox_links().clone();
        let item = m.chosen_item().cloned();
        let mut people = actors;
        let Some(item) = item else {
            self.inbox_photos(m, people);
            return;
        };
        let ticket = inbox_board::ticket_of(&item).to_string();
        let held = self.mission.inbox_tickets.get(&ticket).cloned();
        if let Some(t) = &held {
            people.extend(inbox_board::people(t, &item));
        }
        self.inbox_photos(m, people);
        if ticket.is_empty() {
            return;
        }
        let updated = held
            .as_ref()
            .and_then(|t| crate::sources::ms_of(t.get("updatedAt").and_then(Value::as_str)));
        let stale = match (&held, updated) {
            (None, _) => true,
            (Some(t), _) if t.get("error").is_some() => true,
            (Some(_), Some(at)) => item.time.is_some_and(|t| t > at),
            (Some(_), None) => true,
        };
        let asked_lately = self
            .mission
            .inbox_asked
            .get(&ticket)
            .is_some_and(|at| at.elapsed() < INBOX_TICKET_AGAIN);
        // A copy stands only so long: the ticket may have changed since
        // with nothing new in the Inbox to say so.
        let read_long_ago = self
            .mission
            .inbox_asked
            .get(&ticket)
            .is_none_or(|at| at.elapsed() >= INBOX_TICKET_EVERY);
        // Paused while you're away, as the lookups are, the copy shown.
        if ((stale && !asked_lately) || read_long_ago) && !self.away() {
            self.read_inbox_ticket(ticket);
        }
        m.snap.inbox_ticket = held;
    }

    fn inbox_photos(&mut self, m: &mut Mission, people: Vec<crate::photos::Who>) {
        for who in people {
            if let Some(photo) = self.photo(&who) {
                m.snap.photos.insert(who, photo);
            }
        }
    }

    /// Reads an Inbox ticket off the task, one at a time: its cache file
    /// first (the ticket view's), shown at once, then Linear, whose answer
    /// is kept in the cache file too.
    fn read_inbox_ticket(&mut self, ticket: String) {
        if self.mission.inbox_ticket_busy || tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        self.mission.inbox_ticket_busy = true;
        self.mission
            .inbox_asked
            .insert(ticket.clone(), Instant::now());
        let cached = !self.mission.inbox_tickets.contains_key(&ticket);
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let file = crate::views::ticket::cache_file(&ticket);
            if cached {
                let read = file.clone();
                let copy = tokio::task::spawn_blocking(move || util::read_json(read))
                    .await
                    .ok()
                    .flatten();
                if copy.is_some() {
                    let _ = tx.send(Event::Mission(MissionEvent::InboxTicket {
                        ticket: ticket.clone(),
                        value: copy,
                        last: false,
                    }));
                }
            }
            let value = match crate::views::ticket::fetch(&ticket).await {
                Ok(issue) => {
                    let text = util::json_compact(&issue);
                    let _ = tokio::task::spawn_blocking(move || {
                        if let Some(dir) = file.parent() {
                            let _ = std::fs::create_dir_all(dir);
                        }
                        util::write_atomic(&file, text)
                    })
                    .await;
                    issue
                }
                Err(why) => serde_json::json!({ "error": why }),
            };
            let _ = tx.send(Event::Mission(MissionEvent::InboxTicket {
                ticket,
                value: Some(value),
                last: true,
            }));
        });
    }

    /// Enter on the Inbox board: the Tickets board's menu for its ticket,
    /// with no "Mark it read", since closing the menu marks it read in
    /// Linear, whatever was picked but dismissing it here, which leaves it
    /// unread there; it leaves the view once Linear answers.
    fn inbox_enter(&mut self, client: ClientId, item: Item) {
        let mut list: Vec<Choice> = self
            .item_choices(&ViewId::Inbox, &item, &[])
            .into_iter()
            .filter(|c| !matches!(c.act, Do::MarkRead(_)))
            .collect();
        list.push(Choice {
            key: 'x',
            label: "Dismiss it here: it stays unread in Linear".into(),
            act: Do::Dismiss {
                source: "inbox".into(),
                item: Box::new(item.clone()),
            },
        });
        let id = crate::sources::inbox::notification_of(&item).to_string();
        let options: Vec<MenuOption> = list
            .iter()
            .map(|c| MenuOption::new(c.label.clone()))
            .collect();
        let menu = Menu::new(item.title.clone(), options);
        self.choose(client, menu, move |server, client, chose| {
            let c = chose.and_then(|i| list.into_iter().nth(i));
            if !c.as_ref().is_some_and(|c| choices::puts_away(&c.act)) {
                server.inbox_mark_read(id);
            }
            if let Some(c) = c {
                if let Some(at) = server.clients.get_mut(&client) {
                    at.options_taken = at.options_taken.wrapping_add(1);
                }
                server.mission_pick(client, c, None, None);
            }
        });
    }

    /// Marks an Inbox notification read in Linear in the background.
    fn inbox_mark_read(&mut self, id: String) {
        if tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let error = crate::sources::inbox::mark_read(&id)
                .await
                .err()
                .map(|e| first_line(&e));
            let _ = tx.send(Event::Mission(MissionEvent::InboxRead { id, error }));
        });
    }

    /// Puts together what a client's mission control shows, and does what
    /// bringing a view to the front does.
    fn mission_prepare(&mut self, client: ClientId) {
        let Some((cols, rows)) = self.clients.get(&client).map(|c| (c.cols, c.rows)) else {
            return;
        };
        // It fills the session's main area, header included, leaving the
        // sidebar showing on its left.
        let left = self
            .clients
            .get(&client)
            .and_then(|c| self.session(&c.session))
            .map_or(0, |s| s.main_area().x)
            .min(cols.saturating_sub(1));
        let screens = self.mission_screens();
        let boxed = self.mission_boxed(client);
        let Some(mut m) = self.take_mission(client) else {
            return;
        };
        let text = m
            .filter
            .as_ref()
            .map(|f| f.text.trim().to_lowercase())
            .unwrap_or_default();
        let all_tabs = self.sidebar_all_tabs();
        // The Tabs view follows the session's sidebar's repository filter.
        let session = self
            .clients
            .get(&client)
            .map(|c| c.session.clone())
            .unwrap_or_default();
        let mut tabs = self.sidebar_all_marked(&session);
        let repo_filter = self.sidebar_repo(&session).map(|r| {
            r.file_name()
                .unwrap_or(r.as_os_str())
                .to_string_lossy()
                .into_owned()
        });
        // With several views at once the filter is the focused view's alone.
        let focused_id = m.current().map(|v| v.id.clone());
        let gridded = m.grid.is_some();
        if !text.is_empty() && (!gridded || m.view == 0) {
            let words = views::Words::new(&text);
            tabs.retain(|t| {
                let fields = [
                    t.name.to_lowercase(),
                    t.index.to_string(),
                    t.pr.map(|p| format!("#{p}")).unwrap_or_default(),
                    t.path
                        .as_ref()
                        .map(|p| p.to_string_lossy().to_lowercase())
                        .unwrap_or_default(),
                ];
                words.find(&fields.join(" "))
            });
        }
        // The Landing view stays while the client has it in front, or is
        // opening on it, so it doesn't vanish under you once the last
        // landing ends.
        let keep_landing = m.keep_landing
            || m.current().is_some_and(|v| v.id == ViewId::Landing)
            || m.grid_has("landing");
        let keep_autopilot = m.keep_autopilot
            || m.current().is_some_and(|v| v.id == ViewId::Autopilot)
            || m.grid_has("autopilot");
        let look = NoticeLook {
            kind: m.notice_kind,
            unfolded: m.unfolded.clone(),
            // A text filter looks through every day, folded or not.
            screenful: if text.is_empty() {
                (m.laid().1 as usize).saturating_sub(8).max(6)
            } else {
                usize::MAX
            },
        };
        let mut all = self.mission_views(m.scope.is_some(), keep_landing, keep_autopilot, &look);
        // What the Slack board's From your work reads: today's Calendar and
        // the Tickets as they are, whatever day the Calendar shows or the
        // filter leaves.
        let slack_work: Vec<View> = if m.current().is_some_and(|v| v.id == ViewId::Slack) {
            all.iter()
                .filter(|v| matches!(v.id, ViewId::Calendar | ViewId::Tickets))
                .cloned()
                .collect()
        } else {
            Vec::new()
        };
        // The Slack look whole, for what leaving the board marks seen,
        // kept while the view is turned off.
        if let Some(v) = all.iter().find(|v| v.id == ViewId::Slack) {
            m.slack_all = v.data.items.iter().map(|i| i.id.clone()).collect();
        }
        // The Calendar on another day than today, from the feed's days.
        if let Some(date) = m.calendar_day.clone() {
            let now_z = util::local_now();
            match crate::day::calendar::other_day_view(&self.day.feed, &date, &now_z) {
                Ok(data) if date != util::ymd(&now_z) => {
                    if let Some(v) = all.iter_mut().find(|v| v.id == ViewId::Calendar) {
                        v.loaded = true;
                        v.data = data;
                    }
                }
                _ => m.calendar_day = None,
            }
        }
        // A picker lists every repository's rows, whatever the map folds.
        let folds = if m.picker {
            views::Folds {
                folded: Default::default(),
                merged_open: m.folds.merged_open.clone(),
            }
        } else {
            m.folds.clone()
        };
        // M-j's picker with a filter finds tabs by what their panes show
        // too: what the search for this text found, or while it runs, what
        // the last one found that still holds it.
        let panes_searched = searches_panes(&m, &text).then(|| match &m.panes_found {
            Some((was, found)) if *was == text => found.clone(),
            Some((_, found)) => super::pane_search::narrowed(found, &text),
            None => std::collections::HashMap::new(),
        });
        let views: Vec<View> = all
            .iter()
            .map(|v| {
                if let Some(found) = panes_searched.as_ref().filter(|_| v.id == ViewId::WorkMap) {
                    let mut shown = views::shown(v, "", m.scope, &folds);
                    shown.data.items = super::pane_search::found_items(&v.data.items, &text, found);
                    return shown;
                }
                let filtered = !gridded || focused_id.as_ref() == Some(&v.id);
                views::shown(v, if filtered { &text } else { "" }, m.scope, &folds)
            })
            .map(|mut v| {
                // M-j's picker lists rows to pick, with no repository's
                // heading for Enter to land on.
                if m.picker && v.id == ViewId::WorkMap {
                    v.data.items.retain(|i| !views::is_repo_heading(i));
                }
                if v.id == ViewId::Resources {
                    m.resources_sort.sort(&mut v.data.items);
                }
                if v.id == ViewId::Reopen {
                    // A filter shows the merged worktrees it matches, as
                    // the Work map unfolded its matches while filtering.
                    let merged = m.folds.merged_open.contains(REOPEN_MERGED) || !text.is_empty();
                    crate::usage::reopen::fold_merged(&mut v.data.items, merged);
                    crate::usage::reopen::keep(&mut v.data.items, m.reopen_kind);
                    // M-o's picker lists rows to pick, with no day's
                    // heading for Enter to land on.
                    if m.picker {
                        v.data.items.retain(|i| !views::is_header(i));
                    }
                }
                // The Slack board lists its messages by section, so each
                // section's are together, oldest to newest.
                if v.id == ViewId::Slack {
                    super::slack_board::in_order(
                        &mut v.data.items,
                        settings::is_on("slack_threads"),
                    );
                }
                v
            })
            .collect();
        // A view turned on or off before the one in front (or coming and
        // going, as Landing does) leaves it in front, where it now is, and
        // Tab's focus on a view's name on that view.
        let was = m.current().map(|v| v.id.clone());
        // The Autopilot view orders its items by how far along they are, and
        // CI by whether their checks fail, run or pass, Resources by each
        // tab's CPU or memory, Usage its tabs by their spend, Reminders by
        // their time, Landing its waiters by their place in the queue, and
        // Reopen its rows by when, the past sessions arriving after the
        // closings, so a stage, a check, a reading, a reminder, J and K or a
        // row coming in moves them: the cursor follows the chosen one. So
        // does the Slack board, whose conversations move up as new messages
        // come and which shows only the chosen one's section.
        let follows = |id: &ViewId| {
            matches!(
                id,
                ViewId::Autopilot
                    | ViewId::Ci
                    | ViewId::Resources
                    | ViewId::Usage
                    | ViewId::Reminders
                    | ViewId::Routines
                    | ViewId::Landing
                    | ViewId::Reopen
            ) || *id == ViewId::Slack
        };
        // The Reminders view's chosen reminder, moved to another day, takes
        // the chosen day with it.
        let reminder_chosen = on_reminders(&m)
            && m.current()
                .and_then(|v| super::reminders::chosen(&m, v))
                .is_some_and(|i| i as isize == m.cursor);
        let followed = m.current().map(|v| v.id.clone()).filter(follows);
        let pilot_chosen = followed
            .as_ref()
            .and_then(|_| m.chosen_item())
            .map(|i| i.id.clone());
        // The Slack board's chosen row: its section, and whether it was
        // that section's newest.
        let slack_before = m.current().filter(|v| v.id == ViewId::Slack).and_then(|v| {
            let items = &v.data.items;
            super::slack_board::section_key_at(items, m.cursor)
                .map(|key| (key, super::slack_board::on_newest(items, m.cursor)))
        });
        if let Some(id) = was
            && views.get(m.view.wrapping_sub(1)).map(|v| &v.id) != Some(&id)
        {
            m.view = views.iter().position(|v| v.id == id).map_or(0, |i| i + 1);
        }
        if let Some(Focus::View(at)) = m.focus
            && at > 0
            && let Some(id) = m.snap.views.get(at - 1).map(|v| v.id.clone())
            && views.get(at - 1).map(|v| &v.id) != Some(&id)
        {
            m.focus = Some(Focus::View(
                views.iter().position(|v| v.id == id).map_or(0, |i| i + 1),
            ));
        }
        let now = util::now_ms();
        let frame = spin_frame(&mut m, boxed);
        m.snap = Snapshot {
            tabs,
            all_tabs,
            repo_filter,
            views,
            rules: Rules::now(),
            theme: theme_for(
                self.clients
                    .get(&client)
                    .is_some_and(crate::mux::server::Client::truecolor),
            ),
            tab_focus: settings::is_on("tab_focus"),
            nav_follow: settings::is_on("mission_nav_follow"),
            side_keys: super::side_keys(&self.bindings),
            close: settings::is_on("easy_keys") && settings::is_on("menu_mouse"),
            now_secs: now.div_euclid(1000),
            tz: jiff::tz::TimeZone::system(),
            frame,
            screens,
            // Not in M-j's and M-o's pickers.
            pilot: m
                .scope
                .is_none()
                .then(|| self.autopilot_summary())
                .flatten(),
            caps: self
                .clients
                .get(&client)
                .is_some_and(|c| crate::mux::graphics::round_ends(&c.term)),
            cell: self
                .clients
                .get(&client)
                .filter(|_| settings::is_on("view_images"))
                .and_then(|c| self.graphics_of(c))
                .map(|g| g.cell),
            diagrams: self.mission.diagrams.ready(crate::status::theme::light()),
            notice_counts: Self::notice_history_on().then(|| {
                super::history::counts(
                    &self.sidebar.history.newest_first(),
                    util::local_now().date(),
                    &jiff::tz::TimeZone::system(),
                )
            }),
            photos: Default::default(),
            calendar_next: None,
            calendar_notes: Vec::new(),
            inbox_ticket: None,
            inbox_links: Default::default(),
            slack_thread: None,
            slack_pictures: Default::default(),
            slack_emoji: Default::default(),
            reopen_shot: None,
            slack_work,
            slack_thread_failed: None,
            slack_files_failed: Default::default(),
            slack_files_kept: Default::default(),
        };
        m.left = left;
        // Its groups and views in a column of its own, beside the tabs
        // sidebar, or where there is no sidebar to list them in.
        m.own_column = settings::is_on("mission_keep_tabs_sidebar") || left <= 1;
        m.size = (cols.saturating_sub(m.view_left()), rows);
        if m.view > m.snap.views.len() {
            m.view = 0;
        }
        m.keep_landing = m.current().is_some_and(|v| v.id == ViewId::Landing);
        m.keep_autopilot = m.current().is_some_and(|v| v.id == ViewId::Autopilot);
        // A view shown at once stays as the one in front does.
        m.keep_landing |= m.grid_has("landing");
        m.keep_autopilot |= m.grid_has("autopilot");
        let mut chosen_found = false;
        if let Some(id) = pilot_chosen
            && let Some(at) = m
                .current()
                .filter(|v| Some(&v.id) == followed.as_ref())
                .and_then(|v| v.data.items.iter().position(|i| i.id == id))
        {
            m.cursor = at as isize;
            chosen_found = true;
        }
        // The Slack board opens its section at the newest row, as Slack
        // opens a conversation: brought to the front, or opened by name
        // while in front. A cursor on a section's newest row follows a new
        // message landing below it, and one whose row went (dismissed)
        // stays in its section, on its newest, or, with the section gone,
        // goes to the newest of the one now in its place. Rows arriving
        // where there were none open it too.
        let slack_to = m.current().filter(|v| v.id == ViewId::Slack).and_then(|v| {
            let items = &v.data.items;
            let opened = m.slack_fresh || !m.slack_was_shown;
            match &slack_before {
                _ if opened => Some(super::slack_board::section_step(items, m.cursor, 0)),
                None => Some(super::slack_board::section_step(items, m.cursor, 0)),
                // An older row gone leaves the cursor on the row after it,
                // in the same section.
                Some((key, newest)) if *newest || !chosen_found => {
                    match super::slack_board::newest_in(items, key) {
                        Some(at) if *newest => Some(at),
                        Some(_) => None,
                        // The last section gone: the one now last.
                        None => Some(super::slack_board::section_step(
                            items,
                            m.cursor.min(items.len() as isize - 1),
                            0,
                        )),
                    }
                }
                _ => None,
            }
        });
        if m.current().is_some_and(|v| v.id == ViewId::Slack) {
            m.slack_fresh = false;
        }
        if let Some(to) = slack_to {
            m.cursor = to;
        }
        m.settle_grid();
        fit_cursor(&mut m);
        if reminder_chosen {
            follow_reminder(&mut m);
        }
        self.calendar_extras(&mut m);
        self.inbox_extras(&mut m);
        self.slack_board_extras(&mut m);
        self.reopen_extras(&mut m);
        self.search_panes(client, &mut m, &text);
        m.taken_at = Some(self.mission.changes);
        // The view brought to the front: counted, and its source told.
        let now_view = m.current().map(|v| (v.id.clone(), base_title(v)));
        let title = now_view
            .as_ref()
            .map_or("Tabs".to_string(), |(_, t)| t.clone());
        let came = m.came.replace(title.clone());
        let cards_shown = m.view == 0 || m.grid_shown().iter().any(|(_, at)| *at == 0);
        let spins = m
            .snap
            .tabs
            .iter()
            .any(|t| crate::ui::kit::spins(model::badge_state(t, &m.snap.rules)))
            && cards_shown;
        let grid_ids: Vec<ViewId> = m
            .grid_shown()
            .iter()
            .filter_map(|(_, at)| at.checked_sub(1).and_then(|i| m.snap.views.get(i)))
            .map(|v| v.id.clone())
            .collect();
        let picker = m.picker;
        // Slack is left once it shows neither in front nor beside it, the
        // focus moving between views shown at once leaving nothing.
        let slack_now = title == "Slack" || m.grid_has("slack");
        let slack_left = std::mem::replace(&mut m.slack_was_shown, slack_now) && !slack_now;
        let slack_seen = m.slack_seen();
        if slack_left {
            m.slack_viewed = None;
        }
        self.put_mission(client, m);
        if slack_left {
            self.slack_left(slack_seen);
        }
        if came.as_deref() != Some(&title) {
            if !picker {
                self.count_view(&title);
            }
            if let Some((id, _)) = &now_view {
                self.mission_came_to(id);
            }
            // What going to it changed is taken up before it is drawn.
            if let Some(m) = self.mission_mut(client) {
                m.taken_at = None;
            }
        }
        // The views kept up to date only while one looks at them; Reopen's
        // closings whenever mission control is open, for its count on the
        // bar, its past sessions only while it shows (below) or M-o's
        // picker lists it.
        if settings::is_on("view_reopen") || picker {
            self.want_view(ViewKind::Reopen);
        }
        if picker
            && now_view
                .as_ref()
                .is_some_and(|(id, _)| *id == ViewId::Reopen)
        {
            self.want_past_sessions();
            self.mission_worktrees(false);
            self.mission_revived(false);
        }
        // A view shown beside the one in front is kept up to date as it is.
        let mut shown_ids = grid_ids;
        if let Some((id, _)) = now_view
            && !shown_ids.contains(&id)
        {
            shown_ids.push(id);
        }
        for id in shown_ids {
            match id {
                ViewId::Resources => self.want_view(ViewKind::Resources),
                ViewId::Usage => self.want_view(ViewKind::Usage),
                ViewId::Keys => self.want_key_counts(),
                ViewId::Reopen => {
                    self.want_past_sessions();
                    self.mission_worktrees(false);
                    self.mission_revived(false);
                }
                ViewId::Slack if settings::is_on("slack_background") => {
                    self.connector_shown("Slack")
                }
                ViewId::WorkMap => {
                    self.mission_worktrees(false);
                    self.mission_revived(false);
                }
                _ => {}
            }
        }
        if spins {
            self.mission_spin_later();
        }
    }

    /// Going to a view asks its source again, as the daemon's `cameTo` did.
    fn mission_came_to(&mut self, id: &ViewId) {
        match id {
            ViewId::Inbox => self.came_to_view(crate::sources::INBOX),
            ViewId::Ci => self.came_to_view(crate::sources::CI),
            ViewId::Landing => self.came_to_view(crate::sources::LANDING),
            ViewId::Calendar => self.calendar_opened(),
            ViewId::Settings => self.work_out_landing(),
            ViewId::Slack => self.slack_opened(),
            ViewId::WorkMap => {
                self.mission_worktrees(true);
                self.mission_revived(true);
                self.work_out_landing();
            }
            ViewId::Reopen => {
                self.mission_worktrees(true);
                self.mission_revived(true);
            }
            _ => {}
        }
    }

    /// Mission control's views as its bar groups them, for the ⊞ button's
    /// menu (board 60): each one that is on, by group, with its slug (none
    /// for the tabs) and how many rows it holds now, from what the server
    /// already has.
    pub(crate) fn mission_view_list(
        &mut self,
    ) -> Vec<(views::Group, String, Option<String>, usize)> {
        let look = NoticeLook {
            kind: 0,
            unfolded: Default::default(),
            screenful: usize::MAX,
        };
        let all = self.mission_views(false, false, false, &look);
        views::groups(&all)
            .into_iter()
            .flat_map(|(g, held)| held.into_iter().map(move |at| (g, at)))
            .map(|(g, at)| match at.checked_sub(1).and_then(|i| all.get(i)) {
                None => (g, "Tabs".to_string(), None, 0),
                Some(v) => {
                    let rows = v.data.items.iter().filter(|i| !views::is_header(i)).count();
                    (g, views::bar_title(v).to_string(), Some(v.slug()), rows)
                }
            })
            .collect()
    }

    /// Every view after the tabs, each only while its setting is on (the
    /// Work map for M-j's picker and Reopen for M-o's whatever their
    /// settings), Landing
    /// only while something lands or `keep_landing` (the client has it in
    /// front), Autopilot only while it is on or `keep_autopilot`.
    fn mission_views(
        &mut self,
        picker: bool,
        keep_landing: bool,
        keep_autopilot: bool,
        look: &NoticeLook,
    ) -> Vec<View> {
        let on = settings::is_on;
        let mut out: Vec<View> = Vec::new();
        let view = |id: ViewId, title: &str, data: ViewData, loaded: bool| View {
            id,
            title: title.to_string(),
            data,
            loaded,
        };
        if Self::notice_history_on() {
            let entries = self.sidebar.history.newest_first();
            let tz = jiff::tz::TimeZone::system();
            let today = util::local_now().date();
            let kind = crate::sidebar::history::Kind::of_key(look.kind);
            let items =
                super::history::rows(&entries, kind, &look.unfolded, today, &tz, look.screenful);
            let data = ViewData {
                items,
                empty: Some("No notifications in the last two weeks.".into()),
                at: Some(1),
                ..ViewData::default()
            };
            out.push(view(ViewId::Notifications, "Notifications", data, true));
        } else if on("sidebar_notifications") {
            // One by one, in the order of their kinds, as the sidebar and
            // the Notifications window have them.
            let me = &*self;
            let items: Vec<Item> = me
                .notice_lists()
                .into_iter()
                .flat_map(|(source, items)| {
                    items.iter().map(move |i| {
                        let mut item = me.notice_dated(source, i);
                        item.id = format!("{source}\t{}", i.id);
                        item
                    })
                })
                .collect();
            let data = ViewData {
                items,
                empty: Some("No notifications.".into()),
                at: Some(1),
                ..ViewData::default()
            };
            out.push(view(ViewId::Notifications, "Notifications", data, true));
        }
        // The Work map's gathering: M-j's picker lists its open tabs, and
        // Reopen its worktrees no tab has open.
        let reopen = on("view_reopen") || picker;
        let map = (picker || reopen).then(|| {
            let tabs = self.sidebar_all_tabs();
            let trees: Vec<views::Worktree> =
                self.mission.worktrees.values().flatten().cloned().collect();
            let pr_of = |p: &Path| self.sources.untabbed_pr(p).cloned();
            let facts = views::MapFacts {
                landings: self.mission.landings.clone(),
                counts: self.mission.counts.clone(),
                now_secs: util::now_ms() / 1000,
            };
            views::work_map(&tabs, &trees, &self.mission.revived, &pr_of, &facts)
        });
        if picker && let Some(items) = &map {
            let data = ViewData {
                items: items.clone(),
                at: Some(1),
                ..ViewData::default()
            };
            out.push(view(ViewId::WorkMap, "Work map", data, true));
        }
        if on("routines") {
            let data = ViewData {
                items: self.routines_items(),
                empty: Some(crate::routines::view::EMPTY.into()),
                at: Some(1),
                ..ViewData::default()
            };
            out.push(view(ViewId::Routines, "Routines", data, true));
        }
        if on("view_calendar") {
            let d = self.day.calendar.clone();
            let loaded = views::loaded(&d);
            out.push(view(ViewId::Calendar, "Calendar", d, loaded));
        }
        if on("reminders") {
            let rules = crate::sidebar::model::Rules::now();
            let data = ViewData {
                items: super::reminders::items(&self.sidebar_all_tabs(), &rules),
                empty: Some(super::reminders::EMPTY.into()),
                at: Some(1),
                ..ViewData::default()
            };
            out.push(view(ViewId::Reminders, "Reminders", data, true));
        }
        if on("view_slack") {
            let d = self.day.slack.clone();
            let loaded = views::loaded(&d);
            out.push(view(ViewId::Slack, "Slack", d, loaded));
        }
        if on("view_relay") {
            let (sent, set_up) = self.mission_relay_sent();
            let running = self.relay_running();
            let mut items = super::relay_board::items(&sent, running.as_ref());
            if !set_up && running.is_none() {
                super::relay_board::not_set_up(&mut items);
            }
            let data = ViewData {
                items,
                at: Some(1),
                ..ViewData::default()
            };
            out.push(view(ViewId::Relay, "Slack relay", data, true));
        }
        // M-o's picker lists it with the view turned off too.
        if reopen {
            let mut d = self.usage_view(ViewKind::Reopen);
            d.items.extend(crate::usage::reopen::worktrees(
                map.as_deref().unwrap_or(&[]),
            ));
            // The worktrees a tab has open, with its PR's address.
            let tabbed: Vec<(PathBuf, Option<String>)> = self
                .sidebar_all_tabs()
                .into_iter()
                .filter_map(|t| {
                    let path = t.path.as_ref()?;
                    let tree = self
                        .mission
                        .worktrees
                        .values()
                        .flatten()
                        .filter(|w| !w.main && path.starts_with(&w.path))
                        .max_by_key(|w| w.path.as_os_str().len())?;
                    let url = settings::is_on("github")
                        .then(|| {
                            let (n, repo) = (self.tab_pr(t.id)?, self.tab_pr_repo(t.id)?);
                            Some(format!("https://github.com/{repo}/pull/{n}"))
                        })
                        .flatten();
                    Some((tree.path.clone(), url))
                })
                .collect();
            let pr_of = |p: &Path| self.sources.untabbed_pr(p).map(|pr| pr.url.clone());
            crate::usage::reopen::mark_sessions(&mut d.items, &tabbed, &pr_of);
            out.push(view(ViewId::Reopen, "Reopen", d, true));
        }
        for (id, name, title) in [
            (ViewId::Prs, crate::sources::PRS, "PRs"),
            (ViewId::Ci, crate::sources::CI, "CI"),
            (ViewId::Landing, crate::sources::LANDING, "Landing"),
            (ViewId::Tickets, crate::sources::TICKETS, "Tickets"),
            (ViewId::Inbox, crate::sources::INBOX, "Inbox"),
        ] {
            if id == ViewId::Landing {
                let landing = self.landing_view(keep_landing);
                let landing_items = landing.as_ref().map(|d| d.items.clone());
                if let Some(d) = landing {
                    out.push(view(id, title, d, true));
                }
                // Autopilot's own view, after Landing.
                if let Some(items) =
                    self.autopilot_view(keep_autopilot, landing_items.as_deref().unwrap_or(&[]))
                {
                    let data = ViewData {
                        items,
                        at: Some(1),
                        ..ViewData::default()
                    };
                    out.push(view(ViewId::Autopilot, "Autopilot", data, true));
                }
                continue;
            }
            if let Some(d) = self.sources.view(name) {
                let mut d = d.clone();
                if id == ViewId::Ci {
                    d.items = views::ci_sections(&d.items);
                }
                let loaded = views::loaded(&d);
                out.push(view(id, title, d, loaded));
            }
        }
        for (id, kind) in [
            (ViewId::Resources, ViewKind::Resources),
            (ViewId::Usage, ViewKind::Usage),
        ] {
            if on(kind.setting()) {
                let mut d = self.usage_view(kind);
                if kind == ViewKind::Usage {
                    d.empty = Some(
                        "No Claude session has reported its usage yet today: each one does as its statusline draws."
                            .into(),
                    );
                }
                let loaded = views::loaded(&d);
                out.push(view(id, kind.title(), d, loaded));
            }
        }
        // Every key, from the bindings the server holds (M-/).
        if on("view_keys") {
            let shown = super::keymap::Shown {
                github: on("github"),
                linear: on("linear"),
                easy: on("easy_keys"),
            };
            let counts = self.keys_week();
            let data = ViewData {
                items: super::keymap::items(&self.bindings, shown, &counts),
                head: super::keymap::head(&counts, on("habits")),
                at: Some(1),
                ..ViewData::default()
            };
            out.push(view(ViewId::Keys, "Keys", data, true));
        }
        let own = self.mission_own();
        let data = ViewData {
            items: views::settings_items(&own),
            at: Some(1),
            ..ViewData::default()
        };
        out.push(view(ViewId::Settings, "Settings", data, true));
        for p in self.plugin_views() {
            out.push(view(ViewId::Plugin(p.name), &p.title, p.data, p.loaded));
        }
        out
    }

    /// Your own files' Settings rows, each repository's landing first (from
    /// repos.json), read again at most every `OWN_EVERY`.
    fn mission_own(&mut self) -> Vec<Item> {
        if let Some((at, items)) = &self.mission.own
            && at.elapsed() < OWN_EVERY
        {
            return items.clone();
        }
        let repos = self
            .tab_repos()
            .into_iter()
            .map(|(r, _)| r)
            .collect::<Vec<_>>();
        let mut items = crate::usage::own::landing_items(&repos);
        items.extend(crate::usage::own::own_items(&crate::usage::own::own_dir()));
        self.mission.own = Some((Instant::now(), items.clone()));
        items
    }

    /// What the Slack relay sent today (src/relay/sending.rs), read again at
    /// most every `OWN_EVERY`: a small file, a line a message.
    fn mission_relay_sent(&mut self) -> (Vec<crate::relay::sending::Sent>, bool) {
        if let Some((at, sent, set_up)) = &self.mission.relay_sent
            && at.elapsed() < OWN_EVERY
        {
            return (sent.clone(), *set_up);
        }
        let paths = crate::relay::Paths::current();
        let day = util::ymd(&util::local_now());
        let text =
            std::fs::read_to_string(crate::relay::sending::sent_file(&paths.state_dir, &day))
                .unwrap_or_default();
        let sent = crate::relay::sending::read_sent(&text);
        let set_up = crate::relay::slack::load_owner(&paths).is_some();
        self.mission.relay_sent = Some((Instant::now(), sent.clone(), set_up));
        (sent, set_up)
    }

    /// The Slack relay's tab, as the server holds it (marked
    /// `@slack_relay`), with its number and how long it has been open.
    fn relay_running(&self) -> Option<super::relay_board::Running> {
        let (id, tab) = self
            .tabs
            .iter()
            .find(|(_, t)| t.opts.get("@slack_relay").map(String::as_str) == Some("1"))?;
        Some(super::relay_board::Running {
            tab: self.tab_number(*id),
            age: crate::tabs::age(&tab.opts, util::now_secs()),
        })
    }

    /// The relay's tab, for its row's menu.
    fn relay_tab(&self) -> Option<TabId> {
        self.tabs
            .iter()
            .find(|(_, t)| t.opts.get("@slack_relay").map(String::as_str) == Some("1"))
            .map(|(id, _)| *id)
    }

    /// A Slack relay view row's menu: a setting's other values, a preset
    /// applied, or the relay started, stopped or gone to.
    fn relay_choices(&self, item: &Item) -> Vec<Choice> {
        use super::relay_board;
        let mut out = Vec::new();
        if item.id == relay_board::STATUS {
            match self.relay_tab() {
                Some(tab) => {
                    out.push(Choice {
                        key: 'g',
                        label: "Go to its tab".into(),
                        act: Do::GoTo(tab),
                    });
                    out.push(Choice {
                        key: 's',
                        label: "Stop the relay".into(),
                        act: Do::Press(crate::mux::keys::Action::StopSlackRelay),
                    });
                }
                None => out.push(Choice {
                    key: 's',
                    label: "Start the relay".into(),
                    act: Do::Press(crate::mux::keys::Action::SlackRelay),
                }),
            }
            return out;
        }
        if let Some(preset) = relay_board::preset_of(item) {
            out.push(Choice {
                key: 'a',
                label: format!("Use {}", preset.label()),
                act: Do::RelayPreset(preset),
            });
            return out;
        }
        let Some(key) = relay_board::setting_of(item) else {
            return out;
        };
        let keys = "123456789abcdefghijklmnopqrstuvwxyz";
        for (n, (value, label, _)) in relay_board::parts(item).into_iter().enumerate() {
            if value == item.data["raw"] {
                continue;
            }
            out.push(Choice {
                key: keys.chars().nth(n).unwrap_or(' '),
                label: label.clone(),
                act: Do::RelaySet {
                    key: key.to_string(),
                    value,
                    label,
                },
            });
        }
        out
    }

    /// A Slack relay setting written, taken up at once and said.
    fn relay_set(&mut self, client: ClientId, key: &str, value: Value, label: &str) {
        if let Err(e) = settings::write_setting(key, value) {
            self.say(Some(client), &format!("Couldn't save it: {e}"));
            return;
        }
        let name = settings::reload_settings()
            .into_iter()
            .find(|o| o.key == key)
            .map_or_else(|| key.to_string(), |o| o.label);
        self.mission.relay_sent = None;
        self.mission_prepare(client);
        self.mission_redraw();
        self.say(Some(client), &format!("{name}: {label}"));
    }

    /// A preset's settings written, and which it changed said.
    fn relay_preset(&mut self, client: ClientId, preset: crate::relay::sending::Preset) {
        let said = match crate::relay::sending::apply(preset) {
            Ok(changed) if changed.is_empty() => format!("{}: nothing to change", preset.label()),
            Ok(changed) => format!(
                "{}: {}",
                preset.label(),
                changed
                    .iter()
                    .map(|(label, words)| format!("{label} {words}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Err(e) => format!("Couldn't save it: {e}"),
        };
        settings::reload_settings();
        self.mission_prepare(client);
        self.mission_redraw();
        self.say(Some(client), &said);
    }

    /// A click on the `n`th after line of the chosen blocked card: to the
    /// item it names, as that item's choice in the card's menu goes.
    fn pilot_blocker(&mut self, client: ClientId, n: usize) {
        let Some(item) = self
            .mission_mut(client)
            .and_then(|m| m.chosen_item().cloned())
        else {
            return;
        };
        let list = self.item_choices(&ViewId::Autopilot, &item, &[]);
        match super::choices::blocker_act(&item, n, &list) {
            Some(c) => self.mission_do(client, c.act, &c.label),
            None => self.say(
                Some(client),
                "No tab is working that item and it has no plan to open.",
            ),
        }
    }

    /// A click on part `n` of the chosen row's switch: set at once.
    fn relay_part(&mut self, client: ClientId, n: usize) {
        let part = self
            .mission_mut(client)
            .and_then(|m| m.chosen_item().cloned())
            .and_then(|item| super::relay_board::part_value(&item, n));
        if let Some((key, value, label)) = part {
            self.relay_set(client, &key, value, &label);
        }
    }

    // ── The Work map's sources ──────────────────────────────────────────────

    /// Reads the worktrees of the tabs' repositories again, in the
    /// background, unless a read is under way or one was made within
    /// `WORKTREES_EVERY` (`now` asks regardless of the time).
    pub(crate) fn mission_worktrees(&mut self, now: bool) {
        if self.mission.worktrees_busy
            || (!now
                && self
                    .mission
                    .worktrees_at
                    .is_some_and(|at| at.elapsed() < WORKTREES_EVERY))
            || tokio::runtime::Handle::try_current().is_err()
        {
            return;
        }
        let mut mains: Vec<PathBuf> = Vec::new();
        for tab in self.sidebar_all_tabs() {
            let Some(path) = tab.path else {
                continue;
            };
            let Some((_, common)) = crate::sources::landing::git_of(&path) else {
                continue;
            };
            let main = if common.file_name().is_some_and(|n| n == ".git") {
                common.parent().map(Path::to_path_buf)
            } else {
                None
            };
            if let Some(main) = main.filter(|m| !mains.contains(m)) {
                mains.push(main);
            }
        }
        self.mission.worktrees_busy = true;
        self.mission.worktrees_at = Some(Instant::now());
        let tx = self.tx.clone();
        let github = settings::is_on("github");
        // Every repository the map heads: the tabs', and those only worktrees
        // with no tab are known in.
        let mut heads = mains.clone();
        for r in &self.mission.revived {
            if !r.repo.as_os_str().is_empty() && !heads.contains(&r.repo) {
                heads.push(r.repo.clone());
            }
        }
        tokio::task::spawn_blocking(move || {
            let mut found = std::collections::HashMap::new();
            let mut landings = std::collections::HashMap::new();
            for main in heads {
                // How it lands, as far as is known without asking: the
                // Settings view and coming to the map work out the rest.
                let lands = github
                    .then(|| crate::repos::landing_known(&main))
                    .flatten()
                    .map(|method| match method.as_str() {
                        "direct" => {
                            let default = crate::tabs::git::locate(&main)
                                .and_then(|p| crate::tabs::git::default_branch(&p.common_dir))
                                .unwrap_or_else(|| "main".into());
                            format!("lands straight on {default}")
                        }
                        _ => "lands through a PR".into(),
                    });
                if let Some(lands) = lands {
                    landings.insert(main, lands);
                }
            }
            for main in mains {
                let out = util::output_of(
                    std::process::Command::new("git")
                        .arg("-C")
                        .arg(&main)
                        .args(["worktree", "list", "--porcelain"]),
                );
                if let Ok(out) = out {
                    let text = String::from_utf8_lossy(&out.stdout);
                    // Never someone else's PR, which no Claude is started
                    // in (src/trust.rs), by its path alone.
                    let trees = views::parse_worktrees(&main, &text)
                        .into_iter()
                        .filter(|w| !crate::trust::in_review_dir(&w.path))
                        .collect();
                    found.insert(main.clone(), trees);
                }
            }
            let _ = tx.send(Event::Mission(MissionEvent::Worktrees(found, landings)));
        });
    }

    /// Asks for the worktrees no tab is in that are known elsewhere: what
    /// `switchboard wt-revive --json` prints, one JSON row a line, as the
    /// daemon read bin/wt-revive. Until that command gives rows, the map
    /// has the worktrees of the tabs' repositories alone.
    pub(crate) fn mission_revived(&mut self, now: bool) {
        if self.mission.revived_busy
            || (!now
                && self
                    .mission
                    .revived_at
                    .is_some_and(|at| at.elapsed() < WORKTREES_EVERY))
            || tokio::runtime::Handle::try_current().is_err()
        {
            return;
        }
        let Ok(exe) = crate::util::exe() else {
            return;
        };
        self.mission.revived_busy = true;
        self.mission.revived_at = Some(Instant::now());
        let tx = self.tx.clone();
        // A look that fails keeps the rows the last one gave, and one that
        // hangs is given up (and ended) after REVIVED_TIMEOUT, as the daemon
        // gave bin/wt-revive 30 seconds, so the next look can start.
        tokio::spawn(async move {
            let mut cmd = tokio::process::Command::new(exe);
            cmd.arg("wt-revive").arg("--json").arg(paths::home());
            let rows: Option<Vec<views::Revived>> =
                crate::util::run_async(&mut cmd, Some(REVIVED_TIMEOUT))
                    .await
                    .ok()
                    .map(|o| {
                        String::from_utf8_lossy(&o.stdout)
                            .lines()
                            .filter_map(|l| serde_json::from_str(l).ok())
                            .collect()
                    });
            let _ = tx.send(Event::Mission(MissionEvent::Revived(rows)));
        });
    }

    // ── Timers and events ───────────────────────────────────────────────────

    /// The spinners' next turn, timed once.
    fn mission_spin_later(&mut self) {
        if self.mission.spin_pending {
            return;
        }
        self.mission.spin_pending = true;
        self.send_later(
            Duration::from_millis(SPINNER_MS),
            Event::Mission(MissionEvent::Spin),
        );
    }

    /// What mission control's timers and background work say.
    pub fn mission_event(&mut self, event: MissionEvent) {
        match event {
            MissionEvent::Spin => {
                self.mission.spin_pending = false;
                let mut again = false;
                for c in self.clients.values_mut() {
                    let kinds = c.overlays.kinds();
                    let busy = kinds.contains(&"busy");
                    let cards = c.overlays.top() == Some("mission")
                        && c.overlays.find::<Mission>().is_some_and(|m| {
                            (m.view == 0 || m.grid_has(super::grid::TABS))
                                && m.snap.tabs.iter().any(|t| {
                                    crate::ui::kit::spins(model::badge_state(t, &m.snap.rules))
                                })
                        });
                    if busy || cards {
                        c.dirty = true;
                        again = true;
                    }
                }
                if again {
                    self.mission_spin_later();
                }
            }
            MissionEvent::Relabel { client, text } => {
                if let Some(c) = self.clients.get_mut(&client)
                    && let Some(busy) = c.overlays.find_mut::<Busy>()
                {
                    busy.set_text(text);
                    c.dirty = true;
                }
            }
            // On mission control's footer while its launch is still the one
            // shown last and the footer is empty or still says what the
            // choice said it was doing (`over`), never over a newer attempt's
            // words or what something since has said there; and in a box,
            // drawn over mission control and wrapped, so it holds the whole
            // of it. With the boxes off the footer alone, since the status
            // bar's line is under mission control.
            MissionEvent::SlackUnshown {
                client,
                launch,
                over,
                text,
            } => {
                let footer_is_its_own = self.mission_mut(client).is_some_and(|m| {
                    m.slack_launch == launch && m.note.as_deref().is_none_or(|said| said == over)
                });
                if footer_is_its_own {
                    self.mission_note(client, text.clone());
                }
                self.say(Some(client), &text);
            }
            MissionEvent::Done {
                client,
                note,
                after,
            } => {
                self.close_overlays(client, "busy");
                if let Some(after) = after {
                    self.mission_after(client, after);
                }
                if let Some(note) = note {
                    self.mission_note(client, note);
                }
            }
            MissionEvent::Press { pane, presses } => self.press(pane, presses),
            MissionEvent::Reanswer { client, tab, note } => {
                let found = self.prompt_of(tab);
                if found.is_some() && self.clients.contains_key(&client) {
                    self.open_answer(client, tab, Some(note));
                } else {
                    self.mission_note(client, note);
                }
            }
            MissionEvent::Worktrees(found, landings) => {
                self.mission.worktrees_busy = false;
                if found != self.mission.worktrees || landings != self.mission.landings {
                    self.mission.worktrees = found;
                    self.mission.landings = landings;
                    self.untabbed_prs();
                    self.mission_redraw();
                }
            }
            MissionEvent::Diagram {
                source,
                light,
                image,
            } => {
                if self.mission.diagrams.got(&source, light, image) {
                    self.mission_redraw();
                }
            }
            MissionEvent::InboxTicket {
                ticket,
                value,
                last,
            } => {
                if last {
                    self.mission.inbox_ticket_busy = false;
                    // A notification chosen meanwhile, whose read waited on
                    // this one, is asked for as the snapshot is taken again.
                    self.mission_changed();
                    self.mission_redraw();
                }
                // A failed read keeps the copy it had, but not an earlier
                // failure, which the latest replaces.
                let failed = value.as_ref().is_some_and(|v| v.get("error").is_some());
                let kept = failed
                    && self
                        .mission
                        .inbox_tickets
                        .get(&ticket)
                        .is_some_and(|t| t.get("error").is_none());
                if let Some(value) = value.filter(|_| !kept)
                    && self.mission.inbox_tickets.get(&ticket) != Some(&value)
                {
                    self.mission.inbox_tickets.insert(ticket, value);
                    self.mission_changed();
                    self.mission_redraw();
                }
            }
            MissionEvent::InboxRead { id, error } => match error {
                None => self.inbox_dropped(&id),
                Some(why) => self.say(None, &format!("Couldn't mark it read in Linear: {why}")),
            },
            MissionEvent::CalendarNotes(id, lines) => {
                self.mission.calendar_notes_busy = false;
                let changed = self
                    .mission
                    .calendar_notes
                    .get(&id)
                    .is_none_or(|(_, had)| *had != lines);
                self.mission
                    .calendar_notes
                    .insert(id, (Instant::now(), lines));
                if changed {
                    self.mission_changed();
                    self.mission_redraw();
                }
            }
            MissionEvent::Summary {
                session,
                turn_at,
                text,
            } => {
                if self.mission.summaries.got(&session, turn_at, text) {
                    self.mission_changed();
                    self.mission_redraw();
                }
            }
            MissionEvent::PanesSearched {
                client,
                text,
                found,
            } => {
                if let Some(c) = self.clients.get_mut(&client) {
                    c.dirty = true;
                }
                if let Some(m) = self.mission_mut(client) {
                    m.panes_searching = None;
                    m.panes_found = Some((text, found));
                    m.taken_at = None;
                }
            }
            MissionEvent::Counted(tree, counts) => {
                self.mission.counting = None;
                self.mission.counts.insert(tree, counts);
                self.mission_redraw();
            }
            MissionEvent::Revived(rows) => {
                self.mission.revived_busy = false;
                if let Some(rows) = rows
                    && rows != self.mission.revived
                {
                    self.mission.revived = rows;
                    self.untabbed_prs();
                    self.mission_redraw();
                }
            }
        }
    }

    pub(crate) fn mission_redraw(&mut self) {
        for c in self.clients.values_mut() {
            if c.overlays.find::<Mission>().is_some() {
                c.dirty = true;
            }
        }
    }

    /// Every tick: the reminders come due said, and every mission control
    /// drawn again, for the times it shows.
    pub fn mission_tick(&mut self) {
        self.announce_reminders();
        self.mission_redraw();
    }

    /// A reminder come due says so once: a banner (with the notifications
    /// setting on) and a box on the client used last, naming the tab and
    /// what it is for. Once said, the tab's `@remind_said` holds its time,
    /// so it isn't said again, and one that came due while nothing could
    /// show it is said when something can. Come due in the tab in front, it
    /// is done with once you have typed in that session since, as going to
    /// it would have done.
    fn announce_reminders(&mut self) {
        if !settings::is_on("reminders") {
            return;
        }
        let r = Rules::now();
        let tabs = self.sidebar_all_tabs();
        let notify = settings::is_on("notifications");
        let latest = self.latest_client();
        for t in tabs.iter().filter(|t| model::reminder_due(t, &r)) {
            let at = t.remind_at.unwrap_or(0);
            let said = self
                .tabs
                .get(&t.id)
                .and_then(|x| x.opts.get("@remind_said"))
                .and_then(|v| v.trim().parse::<i64>().ok());
            if said != Some(at) && (latest.is_some() || notify) {
                if let Some(x) = self.tabs.get_mut(&t.id) {
                    x.opts.insert("@remind_said".into(), at.to_string());
                }
                let note = t
                    .remind_note
                    .as_deref()
                    .filter(|n| !n.is_empty())
                    .map(|n| format!(" · {n}"))
                    .unwrap_or_default();
                let text = format!("Reminder: {} {}{note}", t.index, t.name);
                if notify && tokio::runtime::Handle::try_current().is_ok() {
                    let words = text.clone();
                    tokio::task::spawn_blocking(move || {
                        crate::platform::notify("Switchboard", &words)
                    });
                }
                if latest.is_some() {
                    self.say(latest, &text);
                }
            }
            // Typed in since it came due, with it in front: done with.
            let now = util::now_secs();
            let typed_since = self.clients.values().any(|c| {
                c.session == t.session && now - (c.activity.elapsed().as_secs() as i64) > at
            });
            if t.current && typed_since && said == Some(at) {
                if let Some(x) = self.tabs.get_mut(&t.id) {
                    for option in ["@remind_at", "@remind_note", "@remind_said"] {
                        x.opts.remove(option);
                    }
                }
                self.dirty_tab(t.id);
            }
        }
    }

    // ── Input ───────────────────────────────────────────────────────────────

    /// A client's input while mission control is open: M-w closes it
    /// whatever is over it; with it on top, it takes the keys, the mouse
    /// and pastes. Gives back the input it didn't take, for the overlays
    /// over it and the tab.
    pub fn mission_input(&mut self, client: ClientId, parsed: Parsed) -> Option<Parsed> {
        let Some(c) = self.clients.get(&client) else {
            return Some(parsed);
        };
        if c.overlays.find::<Mission>().is_none() {
            return Some(parsed);
        }
        // A tip over it closes on any key, which goes on to mission control
        // as if the tip hadn't been there.
        let kinds = c.overlays.kinds();
        let key = matches!(parsed.input, Input::Key(_));
        if key && kinds.ends_with(&["mission", "tip"]) {
            self.close_overlays(client, "tip");
        }
        let top = self
            .clients
            .get(&client)
            .is_some_and(|c| c.overlays.top() == Some("mission"));
        let close_key = Key::parse("M-w");
        // Find a key looks M-w up like any other key.
        let finding = self
            .clients
            .get(&client)
            .is_some_and(|c| c.overlays.top() == Some("find-key"));
        match &parsed.input {
            Input::Key(k) if Some(*k) == close_key && !finding => {
                self.mission_close(client);
                None
            }
            _ if !top => Some(parsed),
            Input::Key(k) => {
                self.mission_key(client, *k);
                None
            }
            Input::Mouse(m) => {
                self.mission_mouse(client, *m);
                None
            }
            Input::Paste(text) => {
                let text = text.replace(['\n', '\r'], " ");
                if let Some(m) = self.mission_mut(client) {
                    if let Some(f) = m.filter.as_mut().filter(|f| f.typing) {
                        f.text.push_str(&text);
                        m.cursor = 0;
                    } else if let Some(f) = m.nav_filter.as_mut().filter(|f| f.typing) {
                        f.text.push_str(&text);
                        nav_focus_listed(m);
                    } else if let Some(j) = m.jump.as_mut() {
                        j.text.push_str(&text);
                    }
                }
                self.mission_prepare(client);
                None
            }
            _ => Some(parsed),
        }
    }

    fn mission_key(&mut self, client: ClientId, key: Key) {
        if !self.mission_stands(client) {
            self.mission_prepare(client);
        }
        let Some(mut m) = self.take_mission(client) else {
            return;
        };
        // Any key puts out what the mouse lit.
        m.hover = None;
        // A key pressed with the Calendar view in front, whole and on
        // today, is the calendar looked at: what it shows is seen.
        // Its cell among several views leaves events out, so isn't the
        // whole day looked at.
        let looked = m.current().is_some_and(|v| v.id == ViewId::Calendar)
            && m.grid.is_none()
            && m.calendar_day.is_none()
            && m.filter.as_ref().is_none_or(|f| f.text.is_empty())
            && settings::is_on("calendar_changes")
            && self.notices.contains_key("calendar");
        if looked {
            self.calendar_looked_at();
        }
        let view = m.view;
        let only_moving = !looked && only_moves(&m, key);
        self.mission_key_in(client, &mut m, key);
        // Another view brought to the front starts on its list.
        if m.view != view {
            m.pane = None;
        }
        save_side(&mut m);
        // A move leaves the snapshot standing; anything else may have
        // changed what it shows.
        let stands = only_moving && m.view == view;
        if stands {
            fit_cursor(&mut m);
        }
        self.put_mission(client, m);
        if !stands {
            self.mission_prepare(client);
        }
        if let Some(after) = self.mission.after_key.take() {
            after(self);
        }
    }

    /// The context-menu action picked from the palette over mission
    /// control: what its key (S-F10) does there.
    pub fn mission_context_menu(&mut self, client: ClientId) {
        let Some(m) = self.mission_mut(client) else {
            return;
        };
        let key = context_menu_key(m);
        self.mission_key(client, key);
    }

    fn mission_key_in(&mut self, client: ClientId, m: &mut Mission, key: Key) {
        let context_menu =
            self.bindings.root.get(&key) == Some(&crate::mux::keys::Action::ContextMenu);
        let k = if context_menu {
            context_menu_key(m)
        } else {
            key.normalized()
        };
        // The workspace's own keys for going to a tab and the palette, as
        // keys.json binds them: a tab by its number closes mission control
        // and goes there, holding its digit for a tab past 9 as outside it; the palette opens over it. Before a filter or a
        // view's name being typed, which take no Alt keys.
        match self
            .bindings
            .root
            .get(&k)
            .or_else(|| self.bindings.root.get(&key))
        {
            Some(crate::mux::keys::Action::SelectTab(n)) => {
                let n = *n;
                self.mission_close_from(client, m);
                self.select_tab_key(client, n);
                return;
            }
            // Back, forward and M-Tab go from the view in front, once its
            // place is on the trail, after the key (mission control's state
            // is out of its overlay until then). With the history off, M-{
            // and M-} go to the tab on either side and M-Tab to the tab
            // before, closing it first.
            Some(
                a @ (crate::mux::keys::Action::TabBack
                | crate::mux::keys::Action::TabForward
                | crate::mux::keys::Action::LastTab),
            ) => {
                let a = a.clone();
                if !settings::is_on("tab_back_forward") {
                    self.mission_close_from(client, m);
                    self.act(client, a);
                    return;
                }
                // The view the list's cursor rests on is a place to come
                // back to, however briefly it rested there.
                let walked = m.nav_held.take().is_some();
                if let Some(session) = self.clients.get(&client).map(|c| c.session.clone()) {
                    self.trail_view_is(&session, m.trail_view(), walked);
                }
                self.mission.after_key = Some(Box::new(move |server: &mut Server| {
                    server.act(client, a);
                }));
                return;
            }
            // Its key for the notifications closes it and goes there, once
            // its state is put back, so a view the notifications open isn't
            // written over.
            // An Alt key alone, so a plain key bound to it in keys.json
            // stays mission control's.
            Some(crate::mux::keys::Action::Extra(crate::extras::Command::Notifications))
                if k.mods.alt =>
            {
                self.count_key(&key.notation());
                self.mission_close_from(client, m);
                self.mission.after_key = Some(Box::new(move |server: &mut Server| {
                    let action =
                        crate::mux::keys::Action::Extra(crate::extras::Command::Notifications);
                    if !server.sidebar_before_action(client, &action).0 {
                        server.go_to_notifications(client);
                    }
                }));
                return;
            }
            Some(crate::mux::keys::Action::Palette) => {
                // A view's name or a filter being typed ends, the filter's
                // words kept, so the keys after a pick act on what it shows.
                m.jump = None;
                if let Some(f) = m.filter.as_mut() {
                    f.typing = false;
                }
                super::nav::end_typing(m);
                self.open_palette(client);
                return;
            }
            _ => {}
        }
        // In a grid Tab goes to the next view, the filter being typed kept.
        let tabbing = matches!(k.code, KeyCode::Tab | KeyCode::BackTab);
        if m.grid.is_some()
            && tabbing
            && let Some(f) = m.filter.as_mut().filter(|f| f.typing)
        {
            f.typing = false;
            m.next_cell(if k.code == KeyCode::Tab { 1 } else { -1 });
            return;
        }
        if m.filter.as_ref().is_some_and(|f| f.typing) {
            self.filter_key(client, m, k);
            return;
        }
        if m.jump.is_some() {
            jump_key(m, k);
            return;
        }
        // With its list of views in the sidebar's place and the keys there,
        // a filter being typed or not,
        // the pane resize keys move the sidebar's edge, which the list
        // shares.
        if let Some(by) =
            nav_resize_key(&self.bindings, m, k).or_else(|| nav_resize_key(&self.bindings, m, key))
        {
            // It is a C-w's second key, which ends the C-w.
            m.window_key = None;
            let session = self.clients.get(&client).map(|c| c.session.clone());
            if let Some(session) = session {
                self.sidebar_resize_by(&session, by);
            }
            return;
        }
        // The list of views' filter being typed takes the keys, as the
        // view's own does.
        if m.nav_filter.as_ref().is_some_and(|f| f.typing) && nav_key(m, k) {
            return;
        }
        // What the last action did stays in the footer until the next key.
        m.note = None;
        // `C-w` and a direction move the focus between the view's panes,
        // as Vim's windows; any other second key, or one after the wait,
        // is taken as itself.
        let now = util::now_ms();
        let waited = m.window_waits(now);
        m.window_key = None;
        if waited && window_key(m, k) {
            return;
        }
        if k.code == KeyCode::Char('w') && k.mods == Mods::CTRL && settings::is_on("ctrl_w_panes") {
            m.window_key = Some(now);
            return;
        }
        // The list of views' keys, after a `C-w` waiting for its direction
        // has had it.
        if nav_key(m, k) {
            return;
        }
        // The pane resize keys, once a C-w waiting for its direction has
        // had it.
        if resize_key(&self.bindings, m, k) || resize_key(&self.bindings, m, key) {
            return;
        }
        let mut k = k;
        let press = matches!(k.code, KeyCode::Enter) || (k.code == KeyCode::Char(' ') && plain(&k));
        match m.focus {
            Some(Focus::Group(g)) if press => {
                open_group(m, g);
                return;
            }
            Some(Focus::View(v)) if press && v == m.view && follows(m) => {
                into_view(m);
                return;
            }
            Some(Focus::View(v)) if press => {
                show_view(m, v);
                return;
            }
            Some(Focus::Kind(key)) if press && notice_board(m) => {
                choose_kind(m, key);
                return;
            }
            Some(Focus::Filter) if press => k = Key::new(KeyCode::Char('/'), Mods::NONE),
            // A footer key with Tab's focus presses it while it is drawn; one
            // the view in front no longer shows (another view's) hands the
            // focus back to the chosen item, which Enter or Space presses.
            Some(Focus::Key(key)) if press && m.hits.iter().any(|h| h.spot == Spot::Key(key)) => {
                k = key
            }
            Some(Focus::Key(_)) if press => {
                m.focus = Some(Focus::Item);
                k = Key::new(KeyCode::Enter, Mods::NONE);
            }
            Some(Focus::Close) if press => {
                self.mission_close_from(client, m);
                return;
            }
            // Space presses the chosen item, as Enter does, whether Tab gave it
            // the focus or nothing has the focus yet.
            Some(Focus::Item) | None if k.code == KeyCode::Char(' ') && plain(&k) => {
                k = Key::new(KeyCode::Enter, Mods::NONE)
            }
            _ => {}
        }
        // A resize key pressed from the footer with Tab's focus on it.
        if resize_key(&self.bindings, m, k.normalized()) {
            return;
        }
        // The focused pane's own keys, typed or pressed from the footer;
        // a control Tab's focus is on (a view's name, the filter) took
        // Enter above.
        let on_items = matches!(m.focus, None | Some(Focus::Item | Focus::Key(_)));
        if on_items && pane_key(m, k) {
            return;
        }
        // `?`, typed or pressed from the footer with Tab's focus on it.
        if k.code == KeyCode::Char('?') && plain(&k) {
            let rows = KEYS
                .iter()
                .map(|(k, what)| (k.to_string(), what.to_string()))
                .collect();
            self.open(
                client,
                Box::new(crate::ui::keylist::KeyList::new(rows)),
                None,
            );
            return;
        }
        // Space changes a setting, as Enter does.
        if k.code == KeyCode::Char(' ')
            && plain(&k)
            && m.current()
                .is_some_and(|v| matches!(v.id, ViewId::Settings | ViewId::Relay))
        {
            k = Key::new(KeyCode::Enter, Mods::NONE);
        }
        // `m` is the view's own actions, in every view.
        if k.code == KeyCode::Char('m') && plain(&k) {
            self.view_menu(client, m);
            return;
        }
        if self.reminders_key(client, m, k) || self.routines_key(client, m, k) || ci_key(m, k) {
            return;
        }
        if keys_board_key(m, k) || self.inbox_key(client, m, k) {
            return;
        }
        if Self::slack_board_key(m, k) {
            return;
        }
        if let Some(by) = calendar_key(m, k) {
            self.calendar_day_step(client, m, by);
            return;
        }
        if landing_key(m, k) || reopen_key(m, k) {
            return;
        }
        let count = match m.current() {
            Some(v) => v.data.items.len(),
            None => m.order().len(),
        } as isize;
        let tab_focus = m.snap.tab_focus;
        let ctrl = k.mods == Mods::CTRL;
        let none = plain(&k);
        if m.grid.is_some() {
            match k.code {
                KeyCode::Tab => return m.next_cell(1),
                KeyCode::BackTab => return m.next_cell(-1),
                KeyCode::Esc if m.filter.is_none() => {
                    m.grid = None;
                    return;
                }
                _ => {}
            }
        }
        match k.code {
            KeyCode::Char('h') | KeyCode::Left if none => {
                let to = side_step(m, -1);
                move_to(m, to, count)
            }
            KeyCode::Char('l') | KeyCode::Right if none => {
                let to = side_step(m, 1);
                move_to(m, to, count)
            }
            KeyCode::Char('j') | KeyCode::Down if none => {
                let to = row_step(m, 1);
                move_to(m, to, count)
            }
            KeyCode::Char('k') | KeyCode::Up if none => {
                let to = row_step(m, -1);
                move_to(m, to, count)
            }
            KeyCode::Char('n') if ctrl => {
                let to = row_step(m, 1);
                move_to(m, to, count)
            }
            KeyCode::Char('p') if ctrl => {
                let to = row_step(m, -1);
                move_to(m, to, count)
            }
            KeyCode::Char('J') if none => {
                if let Some(last) = m.side_last() {
                    m.scroll_side(1, last);
                }
            }
            KeyCode::Char('K') if none => {
                if let Some(last) = m.side_last() {
                    m.scroll_side(-1, last);
                }
            }
            KeyCode::Char('g') | KeyCode::Home if none => move_to(m, 0, count),
            KeyCode::Char('G') | KeyCode::End => move_to(m, count - 1, count),
            KeyCode::Tab if tab_focus => move_focus(m, 1),
            KeyCode::BackTab if tab_focus => move_focus(m, -1),
            KeyCode::Tab => switch_view(m, 1, false),
            KeyCode::BackTab => switch_view(m, -1, false),
            KeyCode::Char(']') if none => switch_view(m, 1, true),
            KeyCode::Char('[') if none => switch_view(m, -1, true),
            // M-] and M-[ move between the groups.
            KeyCode::Char(']') if k.mods.alt && !k.mods.ctrl => switch_group(m, 1),
            KeyCode::Char('[') if k.mods.alt && !k.mods.ctrl => switch_group(m, -1),
            KeyCode::Esc if m.filter.is_some() => {
                // Esc clears a filter before it closes mission control.
                m.filter = None;
            }
            KeyCode::Esc => self.mission_close_from(client, m),
            KeyCode::Char('q') if none => self.mission_close_from(client, m),
            KeyCode::Char('c') if ctrl => self.mission_close_from(client, m),
            KeyCode::Char(' ') if k.mods == Mods::ALT => self.mission_close_from(client, m),
            KeyCode::Char('/') if none => {
                if m.current().and_then(View::columns).is_none() {
                    let text = m.filter.take().map(|f| f.text).unwrap_or_default();
                    m.filter = Some(Filter { text, typing: true });
                    // What the filter finds is the list's, which takes the focus.
                    m.pane = None;
                    m.cursor = 0;
                }
            }
            KeyCode::Char('v') if none => m.jump = Some(Jump::default()),
            KeyCode::Enter if notice_history_here(m) => self.notice_enter(client, m),
            // The shelf folded to its labels opens on one, and
            // Other repos on its own.
            KeyCode::Enter if shelf_label_chosen(m) || repos_label_chosen(m) => {
                // The cursor stays on the label's first tab as the rest
                // of the group comes out around it.
                let chosen = chosen_tab(m);
                // On a narrow screen Other repos is folded with the shelf
                // too, so its label opens both.
                m.other_repos_open |= repos_label_chosen(m);
                m.shelf_open |= shelf_label_chosen(m);
                if let Some(at) = chosen.and_then(|t| m.order().iter().position(|x| *x == t)) {
                    m.cursor = at as isize;
                }
            }
            KeyCode::Enter => self.mission_enter(client, m),
            KeyCode::Char(c @ '1'..='9') if none && m.view == 0 => {
                let n = c.to_digit(10).unwrap_or(0) as usize;
                let order = m.order();
                if let Some(i) = order
                    .iter()
                    .position(|t| m.snap.tabs.iter().any(|x| x.id == *t && x.index == n))
                {
                    move_to(m, i as isize, count);
                }
            }
            _ => {}
        }
    }

    /// The Inbox board's own keys: Enter its menu, marking it read; on a
    /// screen too narrow for the ticket beside, `i` showing it in the
    /// list's place, and `i` or Esc back. True when it took the key.
    fn inbox_key(&mut self, client: ClientId, m: &mut Mission, k: Key) -> bool {
        if !m.current().is_some_and(inbox_board::shown) {
            return false;
        }
        let Some(item) = m.chosen_item().cloned() else {
            return false;
        };
        let narrow = !inbox_board::beside(m.laid().0 as usize);
        match k.code {
            KeyCode::Enter => self.inbox_enter(client, item),
            KeyCode::Char('i') if plain(&k) && narrow => {
                m.inbox_pane = !m.inbox_pane;
                m.side = None;
            }
            KeyCode::Esc if narrow && m.inbox_pane => m.inbox_pane = false,
            _ => return false,
        }
        true
    }

    /// `h` and `l` in the Calendar: the day before or after the one shown,
    /// from the feed's days; past them, or with /today's day alone, says
    /// why not.
    /// The Slack board's own keys: `h` and `l` move the focus between the
    /// sections, the messages and the thread, as `C-w` and a direction do
    /// (with the sections folded into a line, or Settings' `ctrl_w_panes`
    /// off, they change the section);
    /// `j` and `k` in the messages stay within the section; `i` (or Esc)
    /// shows or hides the thread in the list's place on a screen too
    /// narrow for both. Enter opens the message's menu, as in every view,
    /// which plays its video or shows its picture. True when it took the
    /// key.
    fn slack_board_key(m: &mut Mission, k: Key) -> bool {
        let shown = m.current().is_some_and(super::render::slack_board_shown);
        // C-n and C-p move as j and k do.
        let ctrl_step = k.mods == Mods::CTRL && matches!(k.code, KeyCode::Char('n' | 'p'));
        if !shown || !(plain(&k) || k.code == KeyCode::Enter || ctrl_step) {
            return false;
        }
        // With the sections folded into a line, or no panes to move
        // between (Settings' `ctrl_w_panes` off), h and l change the
        // section.
        let folded =
            !super::slack_board::column(m.laid().0 as usize) || !m.panes().contains(&Pane::Panel);
        let by = match k.code {
            KeyCode::Char('h') | KeyCode::Left => Some(-1),
            KeyCode::Char('l') | KeyCode::Right => Some(1),
            _ => None,
        };
        if let Some(by) = by {
            if folded {
                if let Some(v) = m.current() {
                    m.cursor = super::slack_board::section_step(&v.data.items, m.cursor, by);
                }
            } else {
                let panes = m.panes();
                let at = panes.iter().position(|p| *p == m.pane_now()).unwrap_or(0);
                if let Some(to) = at.checked_add_signed(by).and_then(|i| panes.get(i)) {
                    m.pane = Some((m.view, *to));
                }
            }
            if m.focus.is_some() {
                m.focus = Some(Focus::Item);
            }
            return true;
        }
        let step = match k.code {
            KeyCode::Char('n') if ctrl_step => Some(1),
            KeyCode::Char('p') if ctrl_step => Some(-1),
            KeyCode::Char('j') | KeyCode::Down => Some(1),
            KeyCode::Char('k') | KeyCode::Up => Some(-1),
            _ => None,
        };
        if let Some(by) = step
            && m.pane_now() == Pane::List
        {
            if let Some(v) = m.current() {
                m.cursor = super::slack_board::row_step(&v.data.items, m.cursor, by);
            }
            if m.focus.is_some() {
                m.focus = Some(Focus::Item);
            }
            return true;
        }
        match k.code {
            KeyCode::Char('i') if !super::slack_board::beside(m.laid().0 as usize) => {
                m.slack_thread_open = !m.slack_thread_open;
                m.side = None;
                true
            }
            KeyCode::Esc if m.slack_thread_open && m.filter.is_none() => {
                m.slack_thread_open = false;
                true
            }
            _ => false,
        }
    }

    /// The chosen Slack message's video played or picture shown with Quick
    /// Look: the one clicked in its thread (`slack_clicked`), else, picked
    /// again from its menu, the thread's next after the one shown last, so
    /// each is reached from the keyboard too. Threads and pictures turned
    /// off in Settings show nothing.
    fn slack_play(&mut self, client: ClientId, m: &mut Mission, item: Option<Item>) {
        let clicked = m.slack_clicked.take();
        if !settings::is_on("slack_threads") {
            return;
        }
        let Some(item) = item.or_else(|| m.chosen_item().cloned()) else {
            return;
        };
        let (files, clicked) = self.slack_playable(&item, clicked);
        if files.is_empty() {
            self.say(
                Some(client),
                "No picture or video of it is on this computer yet: they are read once its thread is.",
            );
            return;
        }
        let at = match (&clicked, &m.slack_shown) {
            (Some(id), _) => files.iter().position(|f| f.0 == *id),
            (None, Some((shown_on, id))) if *shown_on == item.id => files
                .iter()
                .position(|f| f.0 == *id)
                .map(|i| (i + 1) % files.len()),
            _ => None,
        }
        .unwrap_or(0);
        let (id, name, file, video) = files[at].clone();
        m.slack_shown = Some((item.id.clone(), id.clone()));
        // Quick Look alone, never the app its type names: the file is
        // whatever a message's sender uploaded. One it couldn't show is
        // said (`MissionEvent::SlackUnshown`) and logged, since the choice was already
        // told it would show.
        let doing = if video { "Playing" } else { "Showing" };
        let note = if files.len() > 1 {
            format!(
                "{doing} {name} ({} of {}): its menu again for the next.",
                at + 1,
                files.len()
            )
        } else {
            format!("{doing} {name}.")
        };
        // The name is its sender's: in the failure and the log without
        // control characters and cut short, so it stays on its line and a
        // box wraps it.
        let shown: String = name.chars().filter(|c| !c.is_control()).collect();
        let shown = crate::ui::draw::cut(&shown, 60);
        let (tx, row, over) = (self.tx.clone(), item.id.clone(), note.clone());
        self.mission.slack_launches += 1;
        let launch = self.mission.slack_launches;
        m.slack_launch = launch;
        tokio::task::spawn_blocking(move || {
            if let Err(why) = crate::platform::preview(&file) {
                eprintln!(
                    "switchboard server: Slack file {id} chosen from {row}: showing {shown}: {why:#}"
                );
                let _ = tx.send(Event::Mission(MissionEvent::SlackUnshown {
                    client,
                    launch,
                    over,
                    text: format!("Couldn't show {shown}: {why:#}"),
                }));
            }
        });
        m.note = Some(note);
    }

    /// Runs `slack_play` once mission control is back in its overlay (a
    /// pick from a message's menu, `item`, or a click on a picture of the
    /// chosen one); from the Notifications window, with no mission control,
    /// on a mission of its own, what it did said in a box.
    fn slack_play_now(&mut self, client: ClientId, item: Option<Item>) {
        let (mut m, open) = match self.take_mission(client) {
            Some(m) => (m, true),
            None => {
                let mut m = Mission::new();
                m.slack_shown = self.mission.slack_shown.get(&client).cloned();
                (m, false)
            }
        };
        self.slack_play(client, &mut m, item);
        if open {
            self.put_mission(client, m);
            self.mission_redraw();
        } else {
            if let Some(shown) = m.slack_shown {
                self.mission.slack_shown.insert(client, shown);
            }
            if let Some(note) = m.note {
                self.say(Some(client), &note);
            }
        }
    }

    fn calendar_day_step(&mut self, client: ClientId, m: &mut Mission, by: i64) {
        let now = util::local_now();
        match calendar_step(&self.day.feed, m.calendar_day.as_deref(), by, &now) {
            Ok(day) => {
                m.calendar_day = day;
                m.cursor = 0;
                m.side = None;
                // A keycap's focus was for the event left behind.
                if matches!(m.focus, Some(Focus::Key(_))) {
                    m.focus = Some(Focus::Item);
                }
            }
            Err(why) => self.say(Some(client), &why),
        }
    }

    /// Counts, in the background, the branch of Reopen's chosen worktree
    /// against its default branch and its upstream, for its commits' chip
    /// and its pane: only with Reopen in front, once
    /// for each commit it is at (read from git's files, so moving back to
    /// a row counted starts nothing), one count at a time, so the cursor
    /// running down the rows counts the one it settles on.
    fn work_map_count(&mut self, client: ClientId) {
        if self.mission.counting.is_some() || tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        let Some(m) = self
            .clients
            .get(&client)
            .and_then(|c| c.overlays.find::<Mission>())
        else {
            return;
        };
        if m.current().is_none_or(|v| v.id != ViewId::Reopen) {
            return;
        }
        let Some(tree) = m
            .chosen_item()
            .filter(|i| crate::usage::reopen::kind_of(i) == crate::usage::reopen::WORKTREE)
            .and_then(crate::usage::reopen::dir_of)
            .map(Path::to_path_buf)
        else {
            return;
        };
        let Some(head) = crate::tabs::git::head_commit(&tree) else {
            return;
        };
        let upstream_at = |r: &str| watched_at(&tree, r);
        if !views::needs_count(&self.mission.counts, &tree, &head, &upstream_at) {
            return;
        }
        self.mission.counting = Some(tree.clone());
        let tx = self.tx.clone();
        tokio::task::spawn_blocking(move || {
            let counts = count_branch(&tree, head);
            let _ = tx.send(Event::Mission(MissionEvent::Counted(tree, counts)));
        });
    }

    /// Searches what the open tabs' panes show for M-j's picker's filter
    /// (`tab_search_panes`), off the server's task: each pane's screen and
    /// the last `tab_search_lines` of its scrollback copied as plain lines
    /// (a pane parked behind a tool left out),
    /// a copy kept while its pane's output stands. One search runs at a
    /// time; the filter typed on while it runs is searched for once it
    /// answers.
    fn search_panes(&mut self, client: ClientId, m: &mut Mission, text: &str) {
        if !searches_panes(m, text) {
            m.panes_found = None;
            m.pane_copies.clear();
            return;
        }
        let searched = m.panes_found.as_ref().is_some_and(|(was, _)| was == text);
        if searched || m.panes_searching.is_some() {
            return;
        }
        if tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        let lines =
            super::pane_search::lines_setting(settings::cached("tab_search_lines").as_ref());
        let mut copies = Vec::new();
        let mut kept = std::collections::HashMap::new();
        for (id, slot) in self.searchable_panes() {
            let version = slot.pane.version();
            let copied = match m.pane_copies.get(&id) {
                Some((v, n, copy)) if *v == version && *n == lines => copy.clone(),
                _ => {
                    std::sync::Arc::new(slot.pane.text(lines).lines().map(str::to_string).collect())
                }
            };
            kept.insert(id, (version, lines, copied.clone()));
            copies.push(super::pane_search::Copied {
                tab: slot.tab,
                pane: id,
                lines: copied,
            });
        }
        m.pane_copies = kept;
        m.panes_searching = Some(text.to_string());
        let text = text.to_string();
        let tx = self.tx.clone();
        tokio::task::spawn_blocking(move || {
            let found = super::pane_search::search(&copies, &text);
            let _ = tx.send(Event::Mission(MissionEvent::PanesSearched {
                client,
                text,
                found,
            }));
        });
    }

    /// The panes a search of what the tabs show reads (M-j's picker, M-F's
    /// Tabs): every open tab's, but one parked behind a tool in its place
    /// (M-d's diff), which can't be shown at its match.
    pub(crate) fn searchable_panes(
        &self,
    ) -> impl Iterator<Item = (PaneId, &crate::mux::server::PaneSlot)> {
        self.panes
            .iter()
            .filter(|(id, slot)| self.tabs.contains_key(&slot.tab) && !self.parked(**id))
            .map(|(id, slot)| (*id, slot))
    }

    /// Closes mission control from inside a key's handling, where its state
    /// is out of its overlay: Slack's view left is seen.
    fn mission_close_from(&mut self, client: ClientId, m: &Mission) {
        // Its state is out of the overlay, so the view on the screen is
        // read here.
        self.settle_view_left(client, m.walk_unsettled().then(|| m.view_shown()));
        // Shown among several views, it was looked at too.
        if m.came.as_deref() == Some("Slack") || m.grid_has("slack") {
            self.slack_left(m.slack_seen());
        }
        self.mission_close(client);
    }

    /// Keys while typing a `/` filter: Enter keeps it, Esc clears it, and
    /// the arrows move. In M-j's and M-o's picker, Enter opens the chosen
    /// row's menu, the filter typing on once it closes, and Esc closes.
    fn filter_key(&mut self, client: ClientId, m: &mut Mission, k: Key) {
        let moving = matches!(k.code, KeyCode::Up | KeyCode::Down)
            || (k.mods == Mods::CTRL && matches!(k.code, KeyCode::Char('p' | 'n')));
        let tabbing = matches!(k.code, KeyCode::Tab | KeyCode::BackTab) && m.snap.tab_focus;
        // In a picker, Tab and Shift-Tab go round its rows, still typing,
        // and past either end to easy mode's × (when it has one), which
        // Enter or Space presses, then on round; typing takes the focus
        // back to the rows.
        if m.picker && m.focus == Some(Focus::Close) {
            match k.code {
                KeyCode::Enter | KeyCode::Char(' ') => {
                    self.mission_close_from(client, m);
                    return;
                }
                _ => m.focus = None,
            }
            if tabbing {
                let end = if k.code == KeyCode::Tab {
                    KeyCode::Home
                } else {
                    KeyCode::End
                };
                self.picker_key(client, m, Key::new(end, Mods::NONE));
                return;
            }
        }
        if m.picker && tabbing {
            let forward = k.code == KeyCode::Tab;
            let was = m.cursor;
            let step = if forward { KeyCode::Down } else { KeyCode::Up };
            self.picker_key(client, m, Key::new(step, Mods::NONE));
            if m.cursor == was {
                if m.snap.close {
                    m.focus = Some(Focus::Close);
                } else {
                    let end = if forward { KeyCode::Home } else { KeyCode::End };
                    self.picker_key(client, m, Key::new(end, Mods::NONE));
                }
            }
            return;
        }
        if moving || (m.picker && k.code == KeyCode::Enter) {
            self.picker_key(client, m, k);
            return;
        }
        if m.picker
            && (k.code == KeyCode::Esc || (k.mods == Mods::CTRL && k.code == KeyCode::Char('c')))
        {
            self.mission_close_from(client, m);
            return;
        }
        // Tab and Shift-Tab leave the filter, kept, for the next or last stop.
        if tabbing {
            if let Some(f) = m.filter.as_mut() {
                f.typing = false;
            }
            m.focus = Some(Focus::Filter);
            self.mission_key_in(client, m, k);
            return;
        }
        let Some(f) = m.filter.as_mut() else {
            return;
        };
        match k.code {
            KeyCode::Enter => f.typing = false,
            KeyCode::Esc => m.filter = None,
            KeyCode::Char('c') if k.mods == Mods::CTRL => m.filter = None,
            KeyCode::Backspace => {
                f.text.pop();
            }
            KeyCode::Char('u') if k.mods == Mods::CTRL => f.text.clear(),
            KeyCode::Char(c) if !k.mods.ctrl && !k.mods.alt => f.text.push(c),
            _ => return,
        }
        m.cursor = 0;
    }

    /// A key taken while a filter is typed, as if it weren't, the filter
    /// typing on after it.
    fn picker_key(&mut self, client: ClientId, m: &mut Mission, k: Key) {
        if let Some(f) = m.filter.as_mut() {
            f.typing = false;
        }
        self.mission_key_in(client, m, k);
        if let Some(f) = m.filter.as_mut() {
            f.typing = true;
        }
    }

    // ── The mouse ───────────────────────────────────────────────────────────

    /// The mouse in mission control: the wheel moves to the next or last
    /// card or row, a click on a view's name switches to it, a click on a
    /// card or row chooses it and a second click on the chosen one does
    /// what Enter does; the mouse moving lights what a click would press.
    pub(crate) fn mission_mouse(&mut self, client: ClientId, mouse: Mouse) {
        let look = self
            .clients
            .get(&client)
            .map(|c| *c.overlays.look())
            .unwrap_or_default();
        let Some(m) = self.mission_mut(client) else {
            return;
        };
        // Its parts were drawn from the sidebar's edge (UX 2.0). Over the
        // sidebar a click closes it and does what it does there (a tab gone
        // to, a right click's peek over the workspace); the wheel and the
        // mouse moving stay the sidebar's, mission control left open.
        let dragging = m.side_drag.is_some() || m.pane_bar_drag.is_some();
        let on_nav = m.nav_area.is_some_and(|a| {
            mouse.col >= a.x
                && mouse.col < a.x + a.width
                && mouse.row >= a.y
                && mouse.row < a.y + a.height
        });
        if on_nav && !dragging {
            self.nav_mouse(client, mouse, look);
            return;
        }
        let beside = mouse.col >= m.left && m.col_in(mouse.col).is_none();
        // A drag begun in the view goes on, and ends, at its nearest column
        // while the mouse is beside it.
        let held = (beside && dragging)
            .then(|| (mouse.col.saturating_sub(m.view_left())).min(m.size.0.saturating_sub(1)));
        if beside && !dragging {
            // The spare columns beside a capped view take no clicks, and
            // put out what the mouse lit on its way there.
            if mouse.kind == MouseKind::Move {
                if m.hover.take().is_some() {
                    self.mission_dirty(client);
                }
                self.sidebar_mouse(client, mouse);
            }
            return;
        }
        let Some(col) = held.or_else(|| m.col_in(mouse.col)) else {
            // The ⊞ button stays where it is, lit: a click on it closes
            // mission control, and its right click opens its views' menu
            // over it.
            let on_mission_button = self.sidebar_under(client, mouse.col, mouse.row)
                == Some(Some(crate::sidebar::Owner::Mission));
            if on_mission_button && mouse.kind == MouseKind::Press {
                match mouse.button {
                    MouseButton::Left => {
                        if let Some(session) = self.clients.get(&client).map(|c| c.session.clone())
                        {
                            self.sidebar_leave(&session);
                        }
                        self.mission_close(client);
                    }
                    MouseButton::Right => {
                        self.sidebar_mouse(client, mouse);
                    }
                    _ => {}
                }
                return;
            }
            // So do ← and →: a step decides whether it closes (onto a
            // tab), and the trail menu opens over it, the trail as it is.
            let on_a_trail_button = matches!(
                self.sidebar_under(client, mouse.col, mouse.row),
                Some(Some(
                    crate::sidebar::Owner::Back | crate::sidebar::Owner::Forward
                ))
            );
            if mouse.kind == MouseKind::Press && !mouse.button.is_wheel() && !on_a_trail_button {
                self.mission_close(client);
            }
            self.sidebar_mouse(client, mouse);
            return;
        };
        // Moving off the sidebar puts out what it lit.
        if mouse.kind == MouseKind::Move {
            self.sidebar_mouse(client, mouse);
        }
        let Some(m) = self.mission_mut(client) else {
            return;
        };
        // The right button opens menus, so it follows Settings' "Click in
        // menus" as every click on a menu does.
        let menus_take_clicks = look.mouse;
        if mouse.kind == MouseKind::Press
            && mouse.button == MouseButton::Right
            && !menus_take_clicks
        {
            return;
        }
        // A press in the view ends the list's filter being typed, so the
        // keys it leads to (Enter, `m`) reach the view, and the wheel there
        // moves the view's items rather than the list's focus.
        if mouse.kind == MouseKind::Press {
            if !mouse.button.is_wheel() {
                super::nav::end_typing(m);
            }
            if mouse.button.is_wheel() && matches!(m.focus, Some(Focus::View(_))) {
                m.focus = Some(Focus::Item);
            }
        }
        let mouse = Mouse { col, ..mouse };
        let then = mouse_in(m, mouse, look.hover);
        save_side(m);
        match then {
            Then::Nothing => {}
            Then::Redraw => self.mission_dirty(client),
            Then::Close => self.mission_close(client),
            Then::Key(key) => {
                // The wheel moves as its key does, a filter being typed in a
                // picker typing on after it.
                let typing = self
                    .mission_mut(client)
                    .and_then(|m| m.filter.as_mut())
                    .map(|f| std::mem::replace(&mut f.typing, false))
                    .unwrap_or(false);
                // So does the list of views' filter.
                let nav_typing = self
                    .mission_mut(client)
                    .and_then(|m| m.nav_filter.as_mut())
                    .map(|f| std::mem::replace(&mut f.typing, false))
                    .unwrap_or(false);
                self.mission_key(client, key);
                if typing && let Some(f) = self.mission_mut(client).and_then(|m| m.filter.as_mut())
                {
                    f.typing = true;
                }
                if nav_typing
                    && let Some(f) = self.mission_mut(client).and_then(|m| m.nav_filter.as_mut())
                {
                    f.typing = true;
                }
            }
            Then::Enter => self.mission_key(client, Key::new(KeyCode::Enter, Mods::NONE)),
            Then::Press(key) => self.mission_key(client, key),
            Then::Prepare => {
                self.mission_prepare(client);
                self.mission_dirty(client);
            }
            Then::SlackPlay => self.slack_play_now(client, None),
            // A blocked card's after line on the Autopilot board, else a
            // part of the Slack relay view's switch.
            Then::Part(n) => {
                let on_autopilot = self
                    .mission_mut(client)
                    .is_some_and(|m| m.current().is_some_and(|v| v.id == ViewId::Autopilot));
                if on_autopilot {
                    self.pilot_blocker(client, n);
                } else {
                    self.relay_part(client, n);
                }
            }
        }
    }

    /// The mouse over the list of groups and views (src/mission/nav.rs): a
    /// click on a view's row brings it to the front, a right click opens
    /// its menu as `m` there does, a click on `‹ Tabs` goes back to the
    /// tabs, closing mission control, and on the filter starts typing one;
    /// the mouse moving lights what a click would press. The rest of the
    /// list takes no click.
    fn nav_mouse(&mut self, client: ClientId, mouse: Mouse, look: crate::ui::Look) {
        let Some(m) = self.mission_mut(client) else {
            return;
        };
        let hit = m
            .nav_hits
            .iter()
            .find(|h| h.y == mouse.row && mouse.col >= h.x0 && mouse.col <= h.x1)
            .map(|h| h.spot);
        // A press on the list ends whatever was being typed (either filter,
        // a view's name after `v`) and puts Tab's focus away, as a click in
        // the view does, so the keys after it reach what it chose.
        if mouse.kind == MouseKind::Press && !mouse.button.is_wheel() {
            m.focus = None;
            m.jump = None;
            if let Some(f) = m.filter.as_mut() {
                f.typing = false;
            }
            super::nav::end_typing(m);
        }
        let view = match (mouse.kind, mouse.button) {
            (MouseKind::Move, _) => {
                let lit = hit
                    .filter(|_| look.hover)
                    .filter(|s| *s != Spot::View(m.view));
                if lit != m.nav_hover || m.hover.is_some() {
                    m.nav_hover = lit;
                    m.hover = None;
                    self.mission_dirty(client);
                }
                return;
            }
            (MouseKind::Press, MouseButton::Left) => match hit {
                Some(Spot::View(v)) => v,
                Some(Spot::Close) => {
                    self.mission_close(client);
                    return;
                }
                Some(Spot::Filter) => {
                    let text = m.nav_filter.take().map(|f| f.text).unwrap_or_default();
                    m.nav_filter = Some(Filter { text, typing: true });
                    self.mission_dirty(client);
                    return;
                }
                _ => return,
            },
            // The wheel scrolls the list where it runs past its room.
            (MouseKind::Press, MouseButton::WheelUp | MouseButton::WheelDown) => {
                m.nav_top = if mouse.button == MouseButton::WheelUp {
                    m.nav_top.saturating_sub(3)
                } else {
                    m.nav_top + 3
                };
                m.nav_scrolled = true;
                self.mission_dirty(client);
                return;
            }
            (MouseKind::Press, MouseButton::Right) if look.mouse => match hit {
                Some(Spot::View(v)) => {
                    if v != m.view {
                        show_view(m, v);
                        self.mission_prepare(client);
                    }
                    self.mission_key(client, Key::new(KeyCode::Char('m'), Mods::NONE));
                    return;
                }
                _ => return,
            },
            _ => return,
        };
        m.nav_hover = None;
        show_view(m, view);
        self.mission_prepare(client);
        self.mission_dirty(client);
    }

    /// Draws a client's mission control again.
    fn mission_dirty(&mut self, client: ClientId) {
        if let Some(c) = self.clients.get_mut(&client) {
            c.dirty = true;
        }
    }

    // ── Enter ───────────────────────────────────────────────────────────────

    /// Enter on what is chosen: a tab's menu; on an item, its own menu, or
    /// what it does by itself (a heading opening, a setting changing, a
    /// plugin's item opening).
    /// A tab's menu, as Enter on its card opens it: going there, the
    /// options of the question it waits on, answering, its reminder, its
    /// place in the landing queue. `screen` is whether the card shows its
    /// screen in its place, on the Tabs view, which the menu
    /// offers to show or hide. `from` the sidebar puts peeking at it first,
    /// for the sidebar's right click, which peeked before it opened a menu,
    /// and chooses as the menu opens what the key that opened it did alone.
    pub(crate) fn tab_menu(
        &mut self,
        client: ClientId,
        tab: &crate::sidebar::model::TabModel,
        screen: Option<bool>,
        from: TabMenuFrom,
    ) {
        let all = self.sidebar_all_tabs();
        let waiter = self.sources.landing_waiter(tab.id).cloned();
        let mut list = choices::tab_choices(
            tab,
            &all,
            waiter.as_ref(),
            settings::is_on("reminders"),
            settings::is_on("tab_title"),
        );
        let (cols, rows) = self
            .clients
            .get(&client)
            .map_or((80, 24), |c| (c.cols as usize, c.rows as usize));
        // The menu's inside on this screen, at its least: its box two thirds
        // of the screen or 72 columns, never narrower than its keys' row
        // (crates/ui/src/menu.rs); a label has five columns less.
        let inner = ((cols * 2 / 3).min(72).saturating_sub(2))
            .max(crate::ui::draw::width(crate::ui::menu::KEYS) + 3)
            .min(cols.saturating_sub(2));
        // `screen` is there only for a UX 2.0 card, whose rows then bound
        // what counts as shown whole.
        let mut after_go = self.answer_choices(tab, inner.saturating_sub(5), screen.is_some());
        // What its answers answer, whole above them, wherever the menu is
        // opened (a card, a reminder): the request's kind and its words.
        let body = (!after_go.is_empty())
            .then_some(tab.ask.as_ref())
            .flatten()
            .map(|a| {
                let kind = if a.header.is_empty() {
                    &a.tool
                } else {
                    &a.header
                };
                format!("{kind}\n{}", a.words)
            });
        if let Some(shown) = screen.filter(|_| tab.claude.is_some()) {
            after_go.push(Choice {
                key: 's',
                label: if shown {
                    "Show its card again".into()
                } else {
                    "Show its screen in the card".into()
                },
                act: Do::CardScreen(tab.id),
            });
        }
        // A tab whose lone pane was given a width of its own (a drag of its
        // edge, the resize keys) can take the setting's again.
        if self.own_width(tab.id).is_some()
            && let Some(i) = list.iter().position(|c| matches!(c.act, Do::Rename(_)))
        {
            list.insert(
                i + 1,
                Choice {
                    key: 'w',
                    label: "Use the default width".into(),
                    act: Do::DefaultWidth(tab.id),
                },
            );
        }
        let at = 1.min(list.len());
        list.splice(at..at, after_go);
        if from != TabMenuFrom::Mission
            && let Some(i) = list.iter().position(|c| matches!(c.act, Do::Peek(_)))
        {
            let peek = list.remove(i);
            list.insert(0, peek);
        }
        // Answers only with the request whole above them: the menu shows its
        // options first and its text in the rows left (crates/ui/src/menu.rs).
        let room = rows.saturating_sub(6).max(1);
        let body_room = room.saturating_sub(list.len().min(room) + 1);
        // A command is shown character for character: answers only where
        // the menu lays it out unchanged, each of its lines whole, none
        // broken or trimmed.
        let shown_as_is = tab.ask.as_ref().is_none_or(|a| {
            a.kind != crate::agent::WaitKind::Permission
                || crate::ui::draw::wrap(&a.words, inner.saturating_sub(2))
                    == a.words.split('\n').collect::<Vec<_>>()
        });
        let body_fits = shown_as_is
            && body.as_ref().is_some_and(|b| {
                crate::ui::draw::wrap(b.trim(), inner.saturating_sub(2)).len() <= body_room
            });
        let body = if body_fits {
            body
        } else {
            list.retain(|c| !matches!(c.act, Do::AnswerWith { .. }));
            None
        };
        let at = match from {
            TabMenuFrom::SidebarGo => list
                .iter()
                .position(|c| matches!(c.act, Do::GoTo(_)))
                .unwrap_or(0),
            TabMenuFrom::Mission | TabMenuFrom::SidebarPeek => 0,
        };
        let title = format!("{} {}", tab.index, tab.name);
        self.about_tab(client, tab.id, move |s| {
            s.mission_menu(client, title, body, list, None, None, false, at);
        });
    }

    /// A waiting tab's question's options, each answering it as its number
    /// typed into the tab, only where what is answered is known whole: a
    /// request its card shows all of (with `on_card`, a UX 2.0 card's
    /// menu: `ask_fits`, given the options its screen shows as the card
    /// is; else one known whole, `ask_whole`: a question asking one thing,
    /// or a Bash command, never an edit whose change it doesn't show), a question of one choice, never the option that takes words of
    /// your own, one whose screen shows more than its label (`more`: words
    /// cut or lines under it), one wider than the menu shows whole, or one
    /// whose line reached the pane's edge, which may go on in a line the
    /// screen's reading leaves out. Anything else is answered in the tab, or
    /// with "Answer it here", which shows all of it.
    fn answer_choices(
        &self,
        tab: &crate::sidebar::model::TabModel,
        label_room: usize,
        on_card: bool,
    ) -> Vec<Choice> {
        if tab.state != crate::sidebar::model::TabState::Waiting {
            return Vec::new();
        }
        let Some(prompt) = self.prompt_of(tab.id) else {
            return Vec::new();
        };
        // From a UX 2.0 card, the request as the card lays it out, the
        // screen's own options (an answer of your own) among them, so the
        // menu answers only what the card shows whole; from anywhere else
        // (the sidebar, a reminder), where the
        // menu shows the request itself, one known whole.
        let whole = if on_card {
            let shown = super::Screen {
                options: prompt.options.iter().map(|o| o.label.clone()).collect(),
                question: prompt.question.clone(),
                more: prompt.more.clone(),
                ..Default::default()
            };
            super::render::ask_fits(tab, Some(&shown))
        } else {
            super::render::ask_whole(tab, None)
        };
        if prompt.multi || !whole {
            return Vec::new();
        }
        let pane_cols = self
            .claude_pane_of(tab.id)
            .and_then(|p| self.panes.get(&p))
            .map_or(0, |slot| slot.pane.size().0 as usize);
        prompt
            .options
            .iter()
            .filter(|o| prompt.free != Some(o.n) && !prompt.more.contains(&o.n))
            .filter(|o| {
                let w = crate::ui::draw::width(&o.label);
                w + ANSWER_PREFIX_W <= label_room && w + 12 < pane_cols
            })
            .map(|o| Choice {
                key: char::from_digit(o.n, 10).unwrap_or('9'),
                label: format!("Answer: {}", o.label),
                act: Do::AnswerWith {
                    tab: tab.id,
                    reply: o.n.to_string(),
                },
            })
            .collect()
    }

    fn mission_enter(&mut self, client: ClientId, m: &mut Mission) {
        let Some(view) = m.current().cloned() else {
            if m.cursor < 0 && m.snap.pilot.is_some() {
                self.autopilot_menu(client);
                return;
            }
            let order = m.order();
            let Some(tab) = usize::try_from(m.cursor)
                .ok()
                .and_then(|i| order.get(i))
                .and_then(|id| m.snap.tabs.iter().find(|t| t.id == *id))
                .cloned()
            else {
                return;
            };
            // Only a card has a screen to show in its place: a line on
            // the shelf has none.
            let screen = (!super::render::on_shelf_tab(&tab, &m.snap.rules))
                .then(|| m.card_screen == Some((tab.id, m.cursor)));
            self.tab_menu(client, &tab, screen, TabMenuFrom::Mission);
            return;
        };
        let Some(item) = m.chosen_item().cloned() else {
            return;
        };
        // A reminder's menu is its tab's, as its card's is.
        if m.current().is_some_and(|v| v.id == ViewId::Reminders)
            && let Target::Tab(id) = item.target
            && let Some(tab) = self.sidebar_all_tabs().into_iter().find(|t| t.id == id)
        {
            self.tab_menu(client, &tab, None, TabMenuFrom::Mission);
            return;
        }
        // Autopilot's view's heading opens its menu.
        if view.id == ViewId::Autopilot && item.id == crate::autopilot::view::HEADING {
            self.autopilot_menu(client);
            return;
        }
        if views::is_header(&item) {
            return;
        }
        // An app's own key only says so: a workspace key's menu presses it.
        if view.id == ViewId::Keys && super::keymap::press_of(&item).is_none() {
            self.say(Some(client), &super::keymap::only_there(&item));
            return;
        }
        if view.id == ViewId::Usage
            && item.data.get("tab_key").is_some()
            && !matches!(item.target, Target::Tab(t) if self.tabs.contains_key(&t))
        {
            self.say(Some(client), "That tab has closed.");
            return;
        }
        // The CI view's menu is for the failed check chosen under its PR.
        let item = match m.chosen_check() {
            Some(n) if view.id == ViewId::Ci => with_check(item, n),
            _ => item,
        };
        // The Slack board shows one section at a time, what its item's
        // menu is opened among.
        let shown: Vec<Item> = if super::render::slack_board_shown(&view) {
            slack_shown(m, &view)
                .into_iter()
                .map(|i| view.data.items[i].clone())
                .collect()
        } else {
            view.data.items.clone()
        };
        self.item_menu(client, &view.id, item, &shown);
    }

    /// An item's menu in a view, with what the view shows of the item above
    /// its choices (`choices::item_body`): the one box the Slack view, the
    /// Notifications view and the Notifications window draw for an item.
    fn item_menu(&mut self, client: ClientId, view: &ViewId, item: Item, shown: &[Item]) {
        let list = self.item_choices(view, &item, shown);
        if list.is_empty() {
            return;
        }
        let body = choices::item_body(view, &item);
        // Opened in the Slack view: read in Slack too, while that is on.
        if *view == ViewId::Slack {
            self.slack_item_opened(&item);
        }
        // A Slack message acted on, whatever was picked, is seen.
        let seen = choices::is_slack_message(view, &item);
        // A notification is dismissed after whatever is picked in its menu.
        let notice = (*view == ViewId::Notifications)
            .then(|| notice_for(&item, true))
            .flatten();
        let title = item.title.clone();
        // A calendar event's choices keep their keys, unshown (board 54).
        let keyed = *view == ViewId::Calendar;
        self.mission_menu(
            client,
            title,
            body,
            list,
            seen.then_some(item),
            notice,
            keyed,
            0,
        );
    }

    /// What an item's menu offers, with what the menus need to know of the
    /// tabs, the repositories and the suggestions.
    fn item_choices(&self, view: &ViewId, item: &Item, shown: &[Item]) -> Vec<Choice> {
        let repos_of = |p: &Path| {
            crate::sources::repo::remotes_of(p)
                .all()
                .map(|r| r.to_string())
                .collect()
        };
        let tabs = self.sidebar_all_tabs();
        let suggestions: Vec<(String, Vec<crate::usage::habits::Choice>)> = self
            .usage
            .suggestions
            .iter()
            .map(|s| (s.id.clone(), self.habit_choices(&s.id)))
            .collect();
        let habit = |id: &str| {
            suggestions
                .iter()
                .find(|(s, _)| s == id)
                .map(|(_, c)| c.clone())
                .unwrap_or_default()
        };
        let shows = |v: &ViewId| settings::is_on("notice_shows_in_view") && v.shows();
        let ctx = Ctx {
            tabs: &tabs,
            repos_of: &repos_of,
            home: paths::home(),
            habit: &habit,
            to_tab: settings::is_on("notice_to_tab"),
            to_tab_submits: settings::is_on("send_to_tab_submits"),
            slack_reply: settings::is_on("slack_reply"),
            shows: &shows,
        };
        if *view == ViewId::Settings {
            return self.setting_choices(item);
        }
        if *view == ViewId::Relay {
            return self.relay_choices(item);
        }
        // A notification's menu dismisses every one the sidebar shows, as
        // the view's own menu does, whatever the view lists beside it.
        let notices;
        let shown = if *view == ViewId::Notifications {
            notices = self.sidebar_notice_items();
            &notices[..]
        } else {
            shown
        };
        let mut list = choices::item_choices(view, item, shown, &ctx);
        // A message with a picture or video in its thread plays it, in the
        // Slack view and in its notification's menu alike, after opening it.
        let message = match view {
            ViewId::Slack => Some(item.clone()),
            ViewId::Notifications if choices::is_slack_message(view, item) => {
                let (source, id) = choices::notice_of(item);
                let mut own = item.clone();
                own.id = id;
                Some(crate::sidebar::notices::as_its_view_has_it(&source, &own))
            }
            _ => None,
        };
        let playable = message.filter(|m| {
            settings::is_on("slack_threads") && !self.slack_playable(m, None).0.is_empty()
        });
        if let Some(message) = playable {
            let at = 1.min(list.len());
            list.insert(
                at,
                Choice {
                    key: 'p',
                    label: "Play its video or show its picture".into(),
                    act: Do::SlackPlay(Box::new(message)),
                },
            );
        }
        list
    }

    /// `m`: the menu of the view's own actions, as board 55 draws it (what
    /// the view shows, how it is sorted, a day, finding a key), then
    /// mission control's: several views at once, and this view's settings.
    fn view_menu(&mut self, client: ClientId, m: &mut Mission) {
        let mut list: Vec<(String, ViewAct)> = Vec::new();
        let mut add = |label: &str, act: ViewAct| list.push((label.to_string(), act));
        let view = m.current().cloned();
        // What it does over all it shows first, then what it shows.
        let shown: Vec<Item> = match &view {
            Some(v) if super::render::slack_board_shown(v) => slack_shown(m, v)
                .into_iter()
                .map(|i| v.data.items[i].clone())
                .collect(),
            Some(v) => v.data.items.clone(),
            None => Vec::new(),
        };
        let items = view.as_ref().map_or(&[][..], |v| &v.data.items[..]);
        let facts = self.view_facts(client);
        for (label, act) in
            super::view_acts::of(view.as_ref().map(|v| &v.id), items, &shown, &facts)
        {
            add(&label, ViewAct::Whole(act));
        }
        match view.as_ref().map(|v| &v.id) {
            None => {
                if m.snap.pilot.is_some() {
                    add(autopilot_label(), ViewAct::Autopilot);
                }
                if m.snap.repo_filter.is_some() {
                    add("Every repository", ViewAct::EveryRepo);
                }
            }
            Some(ViewId::Notifications) => {
                use crate::sidebar::history::Kind;
                add("Dismiss every notification shown", ViewAct::DismissShown);
                if m.snap.notice_counts.is_none() {
                    // The view lists only what the sidebar shows: no kinds.
                } else {
                    for key in 1..=7 {
                        let name = Kind::of_key(key).map_or("All", Kind::name);
                        let label = if key == 1 {
                            "Show every kind".to_string()
                        } else {
                            format!("Show only {name}")
                        };
                        if key != m.notice_kind {
                            add(&label, ViewAct::Kind(key));
                        }
                    }
                }
            }
            Some(ViewId::Usage) => add(
                if m.usage_week {
                    "Show today"
                } else {
                    "Show this week"
                },
                ViewAct::UsageWeek,
            ),
            Some(ViewId::Resources) => add(
                match m.resources_sort {
                    super::resources_board::Sort::Cpu => "Sort by memory",
                    super::resources_board::Sort::Memory => "Sort by CPU",
                },
                ViewAct::Sort,
            ),
            Some(ViewId::Keys) => add("Find a key by pressing it", ViewAct::FindKey),
            Some(ViewId::Settings) => {
                let (dir, file) = settings_file_of(m.chosen_item());
                add(
                    &format!("Open {file} in your editor"),
                    ViewAct::EditFile(dir, file),
                );
            }
            Some(ViewId::Autopilot) => {
                use crate::autopilot::Act;
                add(autopilot_label(), ViewAct::Autopilot);
                // What changes autopilot as a whole, kept off its cards' menus.
                let mode = &self.autopilot.mode;
                if mode.on {
                    if mode.paused {
                        add("Resume", ViewAct::Pilot(Act::Resume));
                    } else {
                        add(
                            "Pause: open no more tabs for now",
                            ViewAct::Pilot(Act::Pause),
                        );
                    }
                    if mode.land_without_asking {
                        add("Ask before landing", ViewAct::Pilot(Act::AskBeforeLanding));
                    } else {
                        add(
                            "Land without asking",
                            ViewAct::Pilot(Act::LandWithoutAsking),
                        );
                    }
                    if mode.hands_free {
                        add("Ask me again: hands-free off", ViewAct::Pilot(Act::AskMe));
                    } else if settings::is_on("autopilot_handsfree") {
                        add("Hands-free: decide for me", ViewAct::Pilot(Act::HandsFree));
                    }
                    if mode.skipped_here().next().is_some() {
                        add(
                            "Take the skipped items again",
                            ViewAct::Pilot(Act::Unskip(None)),
                        );
                    }
                    add("Stop working the note", ViewAct::Pilot(Act::Stop));
                }
            }
            Some(ViewId::Calendar) if view.as_ref().is_some_and(|v| v.data.setup.is_none()) => {
                add("Go to today", ViewAct::Today);
                add("The day before", ViewAct::Day(-1));
                add("The day after", ViewAct::Day(1));
            }
            Some(ViewId::Reminders) if on_reminders(m) => {
                let day = super::reminders::chosen_day(m);
                add(
                    &format!(
                        "Remind me about a tab on {} {} {}",
                        remind::short_day(day.weekday()),
                        remind::short_month(day.month()),
                        day.day()
                    ),
                    ViewAct::RemindOn(day),
                );
                add("Go to today", ViewAct::RemindToday);
                // What `i` does on a screen too narrow for the lists beside.
                if (m.laid().0 as usize) < super::reminders::BESIDE_AT {
                    add(
                        if m.reminders_lists {
                            "Back to the chosen day"
                        } else {
                            "Show this month and later"
                        },
                        ViewAct::RemindersLists,
                    );
                }
            }
            Some(ViewId::Routines) => {
                add("New routine…", ViewAct::NewRoutine);
                // What `i` does on a screen too narrow for the pane.
                let narrow = (m.laid().0 as usize) < crate::routines::view::BESIDE_AT;
                if narrow && (m.chosen_item().is_some() || m.routines_pane) {
                    add(
                        if m.routines_pane {
                            "Back to the list"
                        } else {
                            "Show the chosen routine"
                        },
                        ViewAct::RoutinesPane,
                    );
                }
            }
            Some(ViewId::Inbox) => {
                // What `i` does on a screen too narrow for the ticket beside.
                let narrow = !inbox_board::beside(m.laid().0 as usize);
                let shown = view.as_ref().is_some_and(inbox_board::shown);
                if shown && narrow && (m.chosen_item().is_some() || m.inbox_pane) {
                    add(
                        if m.inbox_pane {
                            "Back to the list"
                        } else {
                            "Show its ticket"
                        },
                        ViewAct::InboxPane,
                    );
                }
            }
            Some(ViewId::Reopen) => {
                use crate::usage::reopen::Kind;
                for (label, kind) in [
                    ("Show only tabs", Kind::Tabs),
                    ("Show only sessions", Kind::Sessions),
                    ("Show both tabs and sessions", Kind::Both),
                ] {
                    if m.reopen_kind != kind {
                        add(label, ViewAct::ReopenKind(kind));
                    }
                }
                // What `i` does on a screen too narrow for the pane.
                let narrow = (m.laid().0 as usize) < super::reopen_board::PANE_AT;
                let on_a_row = m
                    .chosen_item()
                    .is_some_and(|i| crate::usage::reopen::of(i).is_some());
                if narrow && (on_a_row || m.reopen_pane) {
                    add(
                        if m.reopen_pane {
                            "Back to the list"
                        } else {
                            "Show what reopening it brings back"
                        },
                        ViewAct::ReopenPane,
                    );
                }
            }
            Some(ViewId::Plugin(name)) => add("Ask it again", ViewAct::AskAgain(name.clone())),
            _ => {}
        }
        add("Several views at once…", ViewAct::ViewsAtOnce);
        let title = view.as_ref().map_or("Tabs", views::bar_title).to_string();
        if view.as_ref().is_none_or(|v| v.id != ViewId::Settings) {
            add("This view's settings", ViewAct::Settings(title.clone()));
        }
        let options: Vec<MenuOption> = list.iter().map(|(l, _)| MenuOption::new(l)).collect();
        let menu = Menu::new(format!("{title}: view actions"), options);
        self.choose(client, menu, move |server, client, chose| {
            let Some((label, act)) = chose.and_then(|i| list.into_iter().nth(i)) else {
                return;
            };
            match act {
                ViewAct::Whole(whole) => server.whole_view_act(client, whole, &label),
                act => server.view_act(client, act),
            }
        });
    }

    /// What the view's `m` menu needs to know beyond the view
    /// (`view_acts::Facts`), from what the server holds and small files:
    /// the front pane's repository read from git's files, never by git.
    fn view_facts(&self, client: ClientId) -> super::view_acts::Facts {
        let rules = Rules::now();
        // Under the sidebar's repository filter, only the filtered
        // repository's, as the Tabs view shows them under Merged.
        let session = self
            .clients
            .get(&client)
            .map(|c| c.session.clone())
            .unwrap_or_default();
        let merged = self
            .sidebar_all_marked(&session)
            .iter()
            .filter(|t| !t.elsewhere && model::own_stage(t, &rules) == model::Stage::Merged)
            .map(|t| t.id)
            .collect();
        let dir = self
            .current_pane(client)
            .and_then(|p| self.pane_dir(p))
            .and_then(|d| crate::cmd::work::main_clone(&d));
        super::view_acts::Facts {
            github: settings::is_on("github"),
            linear: settings::is_on("linear"),
            slack_background: settings::is_on("slack_background"),
            slack_down: crate::connector::connector_down("Slack").is_some(),
            slack_mark_read: settings::is_on("slack_mark_read")
                && crate::day::slack_user::signed_in(&crate::day::Dirs::here().state).is_some(),
            merged,
            repo: dir
                .as_deref()
                .and_then(crate::sources::repo::origin_of)
                .map(|r| r.to_string()),
            tickets: dir.as_deref().and_then(crate::repos::tickets_known),
            dir,
        }
    }

    /// What was picked of a view's actions over all it shows.
    fn whole_view_act(&mut self, client: ClientId, act: super::view_acts::Whole, label: &str) {
        use super::view_acts::Whole;
        match act {
            Whole::Do(act) => self.mission_do(client, act, label),
            Whole::Refresh(view) => {
                self.refresh_sources(Some(view));
                self.mission_note(client, "Asked again.");
            }
            Whole::CheckSlack { down: true } => {
                self.mission_note(client, "Asking Claude Code about Slack…");
                self.connector_check_now(client, "Slack");
            }
            Whole::CheckSlack { down: false } => {
                self.slack_look_now();
                self.mission_note(client, "Looking at Slack again.");
            }
            Whole::SlackMarkRead(newest) => {
                for (channel, ts) in &newest {
                    self.slack_mark_read(channel, ts);
                }
                let n = newest.len();
                self.mission_note(
                    client,
                    format!(
                        "Marking {n} conversation{} read in Slack.",
                        if n == 1 { "" } else { "s" }
                    ),
                );
            }
            Whole::RerunMain(runs) => self.rerun_main(client, runs),
            Whole::Shape { dir, source } => self.shape_from_tickets(client, dir, source),
        }
    }

    /// The failed jobs of each failing default branch's run started again
    /// on GitHub, asked first, since it starts CI there.
    fn rerun_main(&mut self, client: ClientId, runs: Vec<(String, String)>) {
        let n = runs.len();
        let question = format!(
            "Run the failed jobs of {n} default branch run{} again on GitHub?",
            if n == 1 { "" } else { "s" }
        );
        let shown = runs
            .iter()
            .map(|(repo, run)| format!("{repo} run {run}"))
            .collect();
        self.ask(
            client,
            Confirm::new(question).list(shown),
            move |server, client, yes| {
                if !yes {
                    return;
                }
                server.mission_busy(client, "Starting the failed jobs again", None, async move {
                    use crate::integrations::github;
                    let mut failed = Vec::new();
                    for (repo, run) in &runs {
                        let path = format!("repos/{repo}/actions/runs/{run}/rerun-failed-jobs");
                        if let Err(e) = github::rest("POST", &path, None, "mission: CI view").await
                        {
                            failed.push(format!("{repo} run {run}: {e}"));
                        }
                    }
                    Some(if failed.is_empty() {
                        "Started the failed jobs again.".to_string()
                    } else {
                        format!("Couldn't start them all again: {}", failed.join("; "))
                    })
                });
            },
        );
    }

    /// The notifications the sidebar shows, each under the id the
    /// Notifications view gives it (`<source>\t<id>`), for dismissing every
    /// one shown (`choices::dismiss_all`) from the view's and the window's
    /// own menus, whichever list they were opened from.
    pub(crate) fn sidebar_notice_items(&self) -> Vec<Item> {
        self.notice_entries()
            .into_iter()
            .filter_map(|(owner, mut item)| match owner {
                crate::sidebar::Owner::Notice { source, .. } => {
                    item.id = format!("{source}\t{}", item.id);
                    Some(item)
                }
                _ => None,
            })
            .collect()
    }

    /// A menu of one choice over mission control, done as a view's is.
    fn small_view_menu(&mut self, client: ClientId, title: &str, label: &str, act: ViewAct) {
        let menu = Menu::new(title.to_string(), vec![MenuOption::new(label)]);
        self.choose(client, menu, move |server, client, chose| {
            if chose == Some(0) {
                server.view_act(client, act);
            }
        });
    }

    /// What was picked in a view's menu, done on mission control once it
    /// is back in its overlay.
    fn view_act(&mut self, client: ClientId, act: ViewAct) {
        match act {
            ViewAct::FindKey => return self.find_key(client),
            ViewAct::Autopilot => return self.autopilot_menu(client),
            ViewAct::Pilot(act) => return self.mission_do(client, Do::Autopilot(act), ""),
            ViewAct::EditFile(dir, file) => return self.edit_own_file(client, &dir, &file),
            ViewAct::RemindOn(day) => return self.remind_on_day(client, day),
            ViewAct::NewRoutine => return self.routine_new(client),
            ViewAct::DismissShown => {
                match choices::dismiss_all(&self.sidebar_notice_items()) {
                    Some(all) => self.mission_do(client, all, ""),
                    None => self.mission_note(client, "Nothing in the sidebar to dismiss."),
                }
                return;
            }
            ViewAct::AskAgain(name) => {
                if let Some(p) = self.usage.plugins.iter_mut().find(|p| p.name == name) {
                    p.fetched_at = None;
                }
                self.mission_note(client, "Asked it again.");
                return;
            }
            _ => {}
        }
        let Some(mut m) = self.take_mission(client) else {
            return;
        };
        match act {
            ViewAct::Kind(key) => choose_kind(&mut m, key),
            ViewAct::UsageWeek => {
                m.usage_week = !m.usage_week;
                settle(&mut m);
            }
            ViewAct::Sort => m.resources_sort = m.resources_sort.next(),
            ViewAct::ReopenKind(kind) => {
                m.reopen_kind = kind;
                m.cursor = 0;
            }
            ViewAct::ReopenPane => m.reopen_pane = !m.reopen_pane,
            ViewAct::InboxPane => {
                m.inbox_pane = !m.inbox_pane;
                m.side = None;
            }
            ViewAct::RoutinesPane => m.routines_pane = !m.routines_pane,
            ViewAct::RemindersLists => m.reminders_lists = !m.reminders_lists,
            ViewAct::Day(by) => self.calendar_day_step(client, &mut m, by),
            ViewAct::Today => {
                m.calendar_day = None;
                m.cursor = 0;
                m.side = None;
            }
            ViewAct::RemindToday => {
                let today = super::reminders::today(&m);
                choose_day(&mut m, today);
            }
            ViewAct::Unfold(day) => {
                // On its first notification, under its heading.
                m.unfolded.insert(day);
                m.cursor += 1;
            }
            ViewAct::ViewsAtOnce => {
                self.views_at_once(client, &mut m);
            }
            ViewAct::EveryRepo => {
                if let Some(session) = self.clients.get(&client).map(|c| c.session.clone()) {
                    self.sidebar_set_repo(&session, None);
                }
                m.other_repos_open = false;
            }
            ViewAct::Settings(title) => {
                if let Some(at) = m.place_of("settings") {
                    m.grid = None;
                    show_view(&mut m, at);
                    m.filter = Some(Filter {
                        text: title,
                        typing: false,
                    });
                    m.cursor = 0;
                    settle(&mut m);
                }
            }
            _ => {}
        }
        m.taken_at = None;
        self.put_mission(client, m);
        self.mission_prepare(client);
        self.mission_redraw();
    }

    /// Mission control on its Notifications view, the cursor on the
    /// notification `id` of `source` where the view lists it, and its menu
    /// open over it, as the sidebar leads to it with the Notifications
    /// window off: nothing picked yet, and never a view that doesn't show it
    /// (its history lists what the sidebar shows only once written).
    pub(crate) fn mission_on_notice(&mut self, client: ClientId, source: &str, id: &str) {
        self.open_mission(client, Opening::View("notifications".into()));
        let wanted = (source.to_string(), id.to_string());
        if let Some(m) = self.mission_mut(client)
            && let Some(at) = m
                .current()
                .filter(|v| v.id == ViewId::Notifications)
                .and_then(|v| {
                    v.data
                        .items
                        .iter()
                        .position(|i| choices::notice_of(i) == wanted)
                })
        {
            m.cursor = at as isize;
        }
        self.mission_redraw();
        // A kind's row names its first notification.
        let item = self
            .notices
            .get(source)
            .and_then(|items| items.iter().find(|i| i.id == id).or(items.first()))
            .cloned();
        if let Some(mut item) = item {
            item.id = format!("{source}\t{}", item.id);
            self.notice_menu(client, item);
        }
    }

    /// A notification's menu, over whatever is in front (the Notifications
    /// window's Enter), as its row in the Notifications view opens it.
    /// `item.id` is `<source>\t<id>`, as that view gives it.
    pub(crate) fn notice_menu(&mut self, client: ClientId, item: Item) {
        self.item_menu(client, &ViewId::Notifications, item, &[]);
    }

    /// Enter in the Notifications view's history: on a fold, its day
    /// unfolded, the cursor on its first notification; on a notification,
    /// its menu (`notice_menu_choices`), what Enter did before it first.
    fn notice_enter(&mut self, client: ClientId, m: &mut Mission) {
        let Some(item) = m.chosen_item().cloned() else {
            return;
        };
        if let Some(day) = super::history::fold_of(&item) {
            self.small_view_menu(
                client,
                &item.title,
                "Show them",
                ViewAct::Unfold(day.to_string()),
            );
            return;
        }
        if is_a_notice(&item) {
            self.notice_entry_menu(client, item);
        }
    }

    /// A kept notification's menu: its menu as the sidebar's has it, but
    /// one the sidebar no longer shows is neither put away nor offered to
    /// be.
    fn notice_menu_choices(&self, item: &Item) -> Vec<Choice> {
        let list = self.item_choices(&ViewId::Notifications, item, &[]);
        if super::history::shown(item) {
            return list;
        }
        list.into_iter()
            .filter(|c| !choices::puts_away(&c.act))
            .collect()
    }

    /// Whether a kept notification's service is turned off (`github` for
    /// a PR's news and a failing default branch, `linear` for the Inbox),
    /// saying so: its choices would reach it.
    fn notice_service_off(&mut self, client: ClientId, item: &Item) -> bool {
        let (source, _) = choices::notice_of(item);
        let (key, name) = match source.as_str() {
            "prs" | "ci" => ("github", "GitHub"),
            "inbox" => ("linear", "Linear"),
            _ => return false,
        };
        if settings::is_on(key) {
            return false;
        }
        self.say(Some(client), &format!("{name} is off in Settings."));
        true
    }

    /// A kept notification's whole menu, which Enter opens.
    fn notice_entry_menu(&mut self, client: ClientId, item: Item) {
        if self.notice_service_off(client, &item) {
            return;
        }
        let list = self.notice_menu_choices(&item);
        if list.is_empty() {
            return;
        }
        let body = choices::item_body(&ViewId::Notifications, &item);
        let seen = choices::is_slack_message(&ViewId::Notifications, &item);
        let notice = kept_notice(&item);
        let title = item.title.clone();
        self.mission_menu(
            client,
            title,
            body,
            list,
            seen.then_some(item),
            notice,
            false,
            0,
        );
    }

    /// A menu of choices over mission control; the one taken is done. Its
    /// options show no letter keys (board 54): ↑↓ or `/` move, Enter or a
    /// click takes; with `keyed`, each choice's own key takes it too,
    /// unshown. `seen` is a Slack message seen whatever
    /// is picked, and `notice` the notification it is the menu of, what
    /// is picked recorded as what it led to, and dismissed after it when
    /// its flag says so; `at` the choice chosen as it opens.
    #[allow(clippy::too_many_arguments)]
    fn mission_menu(
        &mut self,
        client: ClientId,
        title: String,
        body: Option<String>,
        list: Vec<Choice>,
        seen: Option<Item>,
        notice: Notice,
        keyed: bool,
        at: usize,
    ) {
        let options: Vec<MenuOption> = list
            .iter()
            .map(|c| {
                let option = MenuOption::new(c.label.clone());
                // A number takes the option with that number in every menu,
                // and j k g G q move or close it, so none of them is a key.
                if keyed && c.key.is_ascii_alphabetic() && !"jkgGq".contains(c.key) {
                    option.unshown_key(c.key)
                } else {
                    option
                }
            })
            .collect();
        let mut menu = Menu::new(title, options).at(at);
        if let Some(body) = body {
            menu = menu.body(body);
        }
        self.choose(client, menu, move |server, client, chose| {
            let Some(c) = chose.and_then(|i| list.into_iter().nth(i)) else {
                return;
            };
            if let Some(at) = server.clients.get_mut(&client) {
                at.options_taken = at.options_taken.wrapping_add(1);
            }
            server.mission_pick(client, c, seen, notice);
        });
    }

    /// A choice taken from an item's menu, done: `seen` a Slack message
    /// seen whatever it is; `notice` the notification it was picked for,
    /// what it led to recorded on it, and put away after it when its flag
    /// says so, unless the choice put it away itself.
    fn mission_pick(&mut self, client: ClientId, c: Choice, seen: Option<Item>, notice: Notice) {
        let label = c.label.clone();
        let dismissed = choices::puts_away(&c.act);
        // Playing its picture or video leaves it unseen and its notification
        // in place, as `p` did, so the list keeps its order and its next is
        // played from the same row.
        let playing = matches!(c.act, Do::SlackPlay(_));
        // The restart may yet refuse or fail its build, so the notification
        // that main is ahead stays until a build at or past main takes over.
        let restarting = matches!(c.act, Do::Restart);
        // Shown in the Slack view or the Inbox, an item stays listed there,
        // since dismissing either takes it out of the view it is being shown
        // in: a Slack message seen puts its notification away, an Inbox one
        // stays until it is dismissed from the view.
        let kept_in_its_view = matches!(
            c.act,
            Do::ShowIn {
                view: ViewId::Slack | ViewId::Inbox,
                ..
            }
        );
        if let Some(item) = seen.as_ref().filter(|_| !playing) {
            self.slack_item_acted(item);
        }
        if let Some((source, item, _)) = &notice {
            self.notice_led_to_by(source, item, &c.act, &label);
        }
        self.mission_do(client, c.act, &label);
        match notice {
            // Put away by the choice itself, it was dismissed, whatever
            // seeing it and its list changing made of it on the way.
            Some((source, item, _)) if dismissed => {
                self.notice_history_mark(&source, &item.id, Sidebar::Dismissed, true);
            }
            Some((source, item, true)) if !playing && !restarting && !kept_in_its_view => {
                self.notice_dismiss(&source, &item)
            }
            // A suggestion's choice says what becomes of it, which puts it
            // out of the sidebar as you dismissing it.
            Some((source, item, false)) if source == "habits" => {
                self.notice_history_mark(&source, &item.id, Sidebar::Dismissed, true);
            }
            _ => {}
        }
    }

    /// Puts a notification away, by the source that gave it: the next
    /// meeting until another, a Slack message until its thread gets more,
    /// the calendar's changes by taking the day as seen, a new note, an
    /// Inbox item (here alone), what autopilot said, a tab ready by taking
    /// its mark off.
    pub(crate) fn notice_dismiss(&mut self, source: &str, item: &Item) {
        self.notice_history_mark(source, &item.id, Sidebar::Dismissed, false);
        match source {
            "inbox" => self.inbox_dismiss(item),
            "ci" => self.main_ci_dismiss(item),
            "prs" => self.pr_notice_dismiss(item),
            "autopilot" => self.autopilot_dismiss(),
            "landed" => self.autopilot_dismiss_closed(Some(&item.id)),
            "autolanded" => self.autopilot_dismiss_autolanded(Some(&item.id)),
            "routines" => self.routines_dismiss(item),
            crate::agent::READY_SOURCE => self.ready_dismiss(item),

            "habits" => {}
            crate::mux::behind::SOURCE => self.behind_dismiss(item),
            _ => self.day_dismiss(source, item),
        }
    }

    /// What picking `act` from a notification's menu led to, recorded on
    /// it (`choices::led_to_words`).
    /// The occurrence is the history row's, when it was picked from one.
    pub(crate) fn notice_led_to_by(&mut self, source: &str, item: &Item, act: &Do, label: &str) {
        let tabs = self.sidebar_all_tabs();
        let number = |t: TabId| tabs.iter().find(|m| m.id == t).map(|m| m.index);
        if let Some(words) = choices::led_to_words(act, label, &number) {
            let occurrence = super::history::occurrence_of(item);
            self.led_to(source, &item.id, occurrence, &words);
        }
    }

    // ── Doing it ────────────────────────────────────────────────────────────

    /// Does what an option says. `label` names it in the busy box, for one
    /// that takes a moment.
    /// The Find a key box over the Keys view: Enter in it shows the key
    /// at its row in the list, its filter cleared.
    fn find_key(&mut self, client: ClientId) {
        let on = settings::is_on;
        let shown = super::keymap::Shown {
            github: on("github"),
            linear: on("linear"),
            easy: on("easy_keys"),
        };
        let counts = self.keys_week();
        let lookup = super::find_key::Lookup {
            rows: super::keymap::items(&self.bindings, shown, &counts),
            bindings: self.bindings.clone(),
            counts: on("habits").then_some(counts),
        };
        let caps = self
            .clients
            .get(&client)
            .is_some_and(|c| crate::mux::graphics::round_ends(&c.term));
        self.open(
            client,
            Box::new(super::find_key::FindKey::new(lookup, caps)),
            Some(Box::new(move |server: &mut Server, client, outcome| {
                let Outcome::Chose(row) = outcome else {
                    return;
                };
                if let Some(m) = server.mission_mut(client) {
                    m.filter = None;
                    m.cursor = row as isize;
                    // So Enter presses it, whatever Tab had focused.
                    if m.focus.is_some() {
                        m.focus = Some(Focus::Item);
                    }
                    m.taken_at = None;
                }
                server.mission_prepare(client);
                server.mission_redraw();
            })),
        );
    }

    /// "Several views at once…": the "Views at once" box, from the views shown now, else the
    /// choice kept last time (board 11's four the first time); its Enter
    /// shows them at once, kept for next time, and its Esc, or fewer than
    /// two picked, goes back to one view.
    fn views_at_once(&mut self, client: ClientId, m: &mut Mission) {
        let choice = match &m.grid {
            Some(g) => g.choice.clone(),
            None => super::grid::load(&paths::state_dir()).unwrap_or_default(),
        };
        let entries: Vec<super::views_box::Entry> = views::groups(&m.snap.views)
            .into_iter()
            .flat_map(|(group, held)| held.into_iter().map(move |at| (group, at)))
            .filter_map(|(group, at)| {
                let title = match at.checked_sub(1) {
                    None => "Tabs".to_string(),
                    Some(i) => views::bar_title(m.snap.views.get(i)?).to_string(),
                };
                Some(super::views_box::Entry {
                    group,
                    slug: m.slug_at(at)?,
                    title,
                })
            })
            .collect();
        let (b, picked) = super::views_box::ViewsBox::new(entries, &choice, m.snap.caps);
        self.open(
            client,
            Box::new(b),
            Some(Box::new(move |server: &mut Server, client, outcome| {
                let choice = picked.lock().ok().and_then(|mut p| p.take());
                server.show_views_at_once(client, outcome, choice);
            })),
        );
    }

    /// What the "Views at once" box ended with: the views picked shown at
    /// once and kept for next time; one view picked shown alone; Esc, or
    /// none, back to the view with the focus alone.
    fn show_views_at_once(
        &mut self,
        client: ClientId,
        outcome: Outcome,
        choice: Option<super::grid::Choice>,
    ) {
        let Some(mut m) = self.take_mission(client) else {
            return;
        };
        let mut came = Vec::new();
        // What Enter took is kept for the box's next opening, whatever it
        // shows.
        if let (Outcome::Yes, Some(choice)) = (&outcome, &choice)
            && let Err(e) = super::grid::save(&paths::state_dir(), choice)
        {
            eprintln!("mission control: couldn't keep the views at once: {e}");
        }
        match (outcome, choice) {
            (Outcome::Yes, Some(choice)) if choice.views.len() >= 2 => {
                came = choice
                    .views
                    .iter()
                    .filter_map(|slug| m.place_of(slug))
                    .filter_map(|at| at.checked_sub(1).and_then(|i| m.snap.views.get(i)))
                    .map(|v| v.id.clone())
                    .collect();
                m.grid = Some(super::grid::Grid::new(choice));
                m.focus = None;
                m.filter = None;
                m.pane = None;
                if let Some(at) = m.grid_shown().first().map(|(_, at)| *at) {
                    m.view = at;
                    m.cursor = 0;
                    m.slack_fresh = m.current().is_some_and(|v| v.id == ViewId::Slack);
                }
            }
            (Outcome::Yes, Some(choice)) if choice.views.len() == 1 => {
                m.grid = None;
                if let Some(at) = m.place_of(&choice.views[0]) {
                    show_view(&mut m, at);
                }
            }
            _ => m.grid = None,
        }
        m.taken_at = None;
        self.put_mission(client, m);
        // Each view shown asks its source again, as coming to it does.
        for id in &came {
            self.mission_came_to(id);
        }
        self.mission_prepare(client);
        self.mission_redraw();
    }

    pub(crate) fn mission_do(&mut self, client: ClientId, act: Do, label: &str) {
        let sends_to_tab = matches!(act, Do::ToTab(_));
        match act {
            Do::GoTo(tab) => self.mission_go(client, tab),
            Do::GoToMatch { tab, pane, text } => self.mission_go_to_match(client, tab, pane, &text),
            Do::Answer(tab) => self.open_answer(client, tab, None),
            Do::Peek(tab) => self.mission_peek(client, tab),
            Do::Restart => {
                self.mission_close(client);
                self.latest_asked(client);
            }
            Do::DefaultWidth(tab) => {
                self.set_own_width(tab, None);
                self.mission_note(client, "The tab takes the default width again.");
            }
            Do::TabTitle(tab) => self.show_tab_title(client, tab),
            Do::TabPrs(tab) => self.pick_from_tab_prs(client, tab),
            Do::Diff(tab) => self.tool_in_tab(client, tab, false),
            Do::Lazygit(tab) => self.tool_in_tab(client, tab, true),
            Do::ToolIn { dir, lazygit } => {
                let command = if lazygit {
                    if util::which("lazygit").is_none() {
                        self.mission_note(client, "lazygit isn't installed.");
                        return;
                    }
                    vec!["lazygit".to_string()]
                } else {
                    let exe = crate::util::exe()
                        .map(|e| e.to_string_lossy().into_owned())
                        .unwrap_or_else(|_| "switchboard".into());
                    let mut command = vec![exe, "diff".into()];
                    command.extend(self.diff_resize_flags());
                    command.push(dir.to_string_lossy().into_owned());
                    command
                };
                self.mission_close(client);
                self.mission_new_tab(client, &dir, Some(command), None);
            }
            Do::Rename(tab) => self.rename_tab(client, tab),
            Do::ChooseTicket(tab) => self.choose_ticket(client, tab, false),
            Do::Pin(tab) => self.toggle_pin(tab),
            Do::LandAlone(tab) => self.toggle_land(client, tab),
            Do::Remind(tab) => self.remind_tab(client, tab),
            Do::TabStatus(tab) => self.tab_status_menu(client, tab),
            Do::Palette(tab) => {
                self.mission_go(client, tab);
                self.open_palette(client);
            }
            Do::Land { root, way } => {
                let Ok(exe) = crate::util::exe() else {
                    return;
                };
                self.mission_busy(client, label, None, async move {
                    let out = tokio::task::spawn_blocking(move || {
                        util::output_of(
                            std::process::Command::new(exe)
                                .args(["land-lock", "move"])
                                .arg(&root)
                                .arg(way)
                                .current_dir(&root),
                        )
                    })
                    .await;
                    match out {
                        Ok(Ok(_)) => None,
                        Ok(Err(e)) => Some(format!("Couldn't move it: {e}")),
                        Err(e) => Some(format!("Couldn't move it: {e}")),
                    }
                });
            }
            Do::PrMove {
                common,
                number,
                way,
            } => {
                self.mission_busy(client, label, None, async move {
                    let moved = tokio::task::spawn_blocking(move || {
                        crate::merge_queue::change(&common, |q| {
                            crate::merge_queue::move_pr(q, number, way)
                        })
                    })
                    .await;
                    match moved {
                        Ok(Ok(true)) => None,
                        Ok(Ok(false)) => {
                            Some(format!("#{number} isn't waiting in the merge queue."))
                        }
                        Ok(Err(e)) => Some(format!("Couldn't move it: {e}")),
                        Err(e) => Some(format!("Couldn't move it: {e}")),
                    }
                });
            }
            Do::PrLeave { common, number } => {
                let label = label.to_string();
                self.ask(
                    client,
                    Confirm::new(format!("Take #{number} out of the merge queue?")),
                    move |server, client, yes| {
                        if !yes {
                            return;
                        }
                        server.mission_busy(client, &label, None, async move {
                            let out = tokio::task::spawn_blocking(move || {
                                crate::merge_queue::change(&common, |q| {
                                    crate::merge_queue::dequeue_pr(q, number)
                                })
                            })
                            .await;
                            Some(match out {
                                Ok(Ok(_)) => format!("Took #{number} out of the merge queue."),
                                Ok(Err(e)) => format!("Couldn't take it out: {e}"),
                                Err(e) => format!("Couldn't take it out: {e}"),
                            })
                        });
                    },
                );
            }
            Do::LeaveQueue(root) => {
                let Ok(exe) = crate::util::exe() else {
                    return;
                };
                let folder = root
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let label = label.to_string();
                self.ask(
                    client,
                    Confirm::new("Take it out of the landing queue?")
                        .list(vec![paths::tilde(&root)]),
                    move |server, client, yes| {
                        if !yes {
                            return;
                        }
                        server.mission_busy(client, &label, None, async move {
                            let out = tokio::task::spawn_blocking(move || {
                                util::output_of(
                                    std::process::Command::new(exe)
                                        .args(["land-lock", "leave"])
                                        .current_dir(&root),
                                )
                            })
                            .await;
                            Some(match out {
                                Ok(Ok(_)) => format!("Took {folder} out of the landing queue."),
                                Ok(Err(e)) => format!("Couldn't take it out: {e}"),
                                Err(e) => format!("Couldn't take it out: {e}"),
                            })
                        });
                    },
                );
            }
            Do::CloseTab(tab) => {
                let name = self
                    .sidebar_all_tabs()
                    .into_iter()
                    .find(|t| t.id == tab)
                    .map(|t| format!("{} {}", t.index, t.name))
                    .unwrap_or_default();
                self.ask(
                    client,
                    Confirm::new(format!("Close tab {name}?")),
                    move |server, client, yes| {
                        if yes {
                            server.kill_tab(tab);
                            server.mission_note(client, format!("Closed tab {name}."));
                        }
                    },
                );
            }
            Do::StopSlot { pid, command } => {
                let words = crate::slots::program_words(&command);
                self.ask(
                    client,
                    Confirm::new(format!("Stop {words}?")),
                    move |server, client, yes| {
                        if !yes {
                            return;
                        }
                        // Only the command still reporting the same words:
                        // its own process while it holds the slot, the
                        // stand-in while it waits.
                        let Some(target) = server
                            .slots
                            .get(pid)
                            .filter(|u| u.command == command)
                            .map(|u| u.child.unwrap_or(u.pid))
                        else {
                            server.mission_note(client, format!("{words} had already ended."));
                            return;
                        };
                        // The command and everything it started (a build
                        // script, a shell's jobs), read off the task.
                        let label = format!("Stopping {words}");
                        server.mission_busy(client, &label, None, async move {
                            let stopped = tokio::task::spawn_blocking(move || {
                                let list: Vec<(u32, u32)> = crate::mux::procinfo::all_usage()
                                    .iter()
                                    .map(|p| (p.pid, p.ppid))
                                    .collect();
                                let mut tree = crate::slots::descendants(&list, target);
                                tree.push(target);
                                let mut sent = 0;
                                for pid in tree {
                                    let Ok(p) = libc::pid_t::try_from(pid) else {
                                        continue;
                                    };
                                    // SAFETY: kill only sends a signal, to the
                                    // command the stand-in reported just now
                                    // and the processes under it.
                                    if unsafe { libc::kill(p, libc::SIGTERM) } == 0 {
                                        sent += 1;
                                    }
                                }
                                if sent > 0 {
                                    format!("Stopped {words}.")
                                } else {
                                    format!("{words} had already ended.")
                                }
                            })
                            .await;
                            Some(stopped.unwrap_or_else(|e| format!("Couldn't stop it: {e}")))
                        });
                    },
                );
            }
            Do::EndProcess { tab, pid, name } => {
                let index = self
                    .sidebar_all_tabs()
                    .into_iter()
                    .find(|t| t.id == tab)
                    .map_or(0, |t| t.index);
                let panes: Vec<u32> = self
                    .panes_with_parked(tab)
                    .iter()
                    .filter_map(|p| self.panes.get(p)?.pane.pid())
                    .collect();
                let label = format!("Ending {name}");
                self.ask(
                    client,
                    Confirm::new(format!("End {name} in tab {index}?")),
                    move |server, client, yes| {
                        if !yes {
                            return;
                        }
                        server.mission_busy(client, &label, None, async move {
                            let ended = tokio::task::spawn_blocking(move || {
                                let list = crate::mux::procinfo::all_usage();
                                if !crate::usage::resources::still_under(&list, &panes, pid, &name)
                                {
                                    return format!("{name} had already ended.");
                                }
                                let Ok(signaled) = libc::pid_t::try_from(pid) else {
                                    return format!("{name} had already ended.");
                                };
                                // SAFETY: kill only sends a signal; the pid was
                                // checked just now to be the process offered.
                                if unsafe { libc::kill(signaled, libc::SIGTERM) } == 0 {
                                    format!("Ended {name}.")
                                } else {
                                    format!(
                                        "Couldn't end {name}: {}",
                                        std::io::Error::last_os_error()
                                    )
                                }
                            })
                            .await;
                            Some(ended.unwrap_or_else(|e| format!("Couldn't end it: {e}")))
                        });
                    },
                );
            }
            Do::CloseMerged(list) => self.close_merged(client, list),
            Do::Open(url) => {
                // The busy box says where it is opening as soon as that is
                // read, and lasts until the browser comes to the front.
                let tx = self.tx.clone();
                self.mission_busy(client, label, None, open_under_busy(tx, client, url));
            }
            Do::ReadHere(url) => {
                self.mission_close(client);
                match self.current_pane(client) {
                    Some(pane) => self.read_beside(pane, &url),
                    None => self.say(Some(client), "No pane to read it beside."),
                }
            }
            Do::Copy(text) => {
                self.to_clipboard(Some(client), &text);
                self.mission_note(client, format!("Copied {text}"));
            }
            Do::StartIn { dir, work } => {
                self.mission_close(client);
                self.mission_new_tab(client, &dir, Some(work.command()), None);
            }
            Do::TypeInto { tab, pane, text } => {
                self.type_into_claude(
                    pane,
                    vec![
                        Press::Text(text),
                        Press::Key(Key::new(KeyCode::Enter, Mods::NONE)),
                    ],
                );
                self.mission_go(client, tab);
            }
            Do::CloseTicket { ticket, title } => {
                // Which state it would move to, from Linear, then asked about
                // on the server, since it changes Linear for everyone.
                self.mission_busy_with(client, label, async move {
                    match crate::sources::tickets::done_state_of(&ticket).await {
                        Err(e) => (
                            Some(format!("Couldn't read the ticket: {}", first_line(&e))),
                            None,
                        ),
                        Ok(None) => (
                            Some(format!("{ticket}'s team has no completed state.")),
                            None,
                        ),
                        Ok(Some((_, true, _, _))) => {
                            (Some(format!("{ticket} is already closed.")), None)
                        }
                        Ok(Some((issue, false, state, name))) => (
                            None,
                            Some(After::CloseTicket {
                                ticket,
                                title,
                                issue,
                                state,
                                name,
                            }),
                        ),
                    }
                });
            }
            Do::MarkRead(id) => {
                self.mission_busy_with(client, label, async move {
                    match crate::sources::inbox::mark_read(&id).await {
                        Ok(()) => (Some("Marked read.".into()), Some(After::DropInbox(id))),
                        Err(e) => (
                            Some(format!("Couldn't mark it read: {}", first_line(&e))),
                            None,
                        ),
                    }
                });
            }
            // The connector takes several seconds, which the busy box shows,
            // as the daemon's menu waited on its answer.
            Do::Rsvp(event, answer) => {
                self.mission_busy_with(client, label, async move {
                    let said = crate::day::calendar::rsvp(&event, answer).await;
                    (Some(said), Some(After::Calendar))
                });
            }
            Do::TabFor(h) => {
                self.mission_close(client);
                // As the daemon opened new-tab.zsh with NEW_TAB_PROMPT and
                // NEW_TAB_NAME: Claude run from your shell, which the tab is
                // left at when it exits, the tab at the end and in front, in
                // a directory asked for as M-t asks, or a failing default
                // branch's repository's main clone, read from git's files.
                let open = move |server: &mut Server, client: ClientId, dir: PathBuf| {
                    // Never in a worktree of someone else's PR, a typed path
                    // included, as M-t refuses one (src/trust.rs).
                    if crate::trust::in_review_dir(&dir) {
                        let refusal = crate::trust::refusal(&dir);
                        server.say(Some(client), &refusal);
                        return;
                    }
                    let command =
                        crate::worktree::tab_command("claude \"$@\"", "claude", &[h.prompt]);
                    if let Some(tab) =
                        server.work_tab(client, &dir, command, crate::mux::server::Opened::Front)
                    {
                        if let Some(t) = server.tabs.get_mut(&tab) {
                            t.opts.insert("@tab_name".into(), h.name);
                        }
                        server.name_tab(tab);
                        server.front(client, &[tab]);
                    }
                };
                // A checkout (a submodule its own), never a git directory.
                let main_clone =
                    |d: &Path| crate::cmd::work::main_clone(d).unwrap_or(d.to_path_buf());
                match h.dir.as_deref().map(main_clone) {
                    Some(dir) => open(self, client, dir),
                    None => {
                        self.hold_view_left(client);
                        self.pick_dir_for(client, "A tab for it", open);
                    }
                }
            }
            Do::ToTab(h) | Do::PasteToTab(h) => {
                self.mission_close(client);
                // Only a tab's Claude takes it, never a shell, which would
                // run its lines, someone else's words, as commands.
                let items: Vec<PickItem> = self
                    .sidebar_all_tabs()
                    .iter()
                    .filter(|t| t.claude_pane.is_some())
                    .map(|t| {
                        let label = match t.pr {
                            Some(pr) => format!("#{pr} {}", t.name),
                            None => t.name.clone(),
                        };
                        PickItem {
                            text: format!("{}  {label}", t.index),
                            tags: t.session.clone(),
                            value: t.id.0.to_string(),
                            ..PickItem::default()
                        }
                    })
                    .collect();
                if items.is_empty() {
                    self.say(Some(client), "No tab runs Claude to send it to.");
                    return;
                }
                // Choosing the tab is the confirmation, so the picker says
                // what choosing one does.
                let (title, says) = if sends_to_tab {
                    ("Send to", "Sends it as written: choosing a tab sends it")
                } else {
                    ("Paste into", "Pastes it without sending, to add to first")
                };
                let picker = Picker::new(format!("{title}: {}…", h.name), items)
                    .named("send-to-tab")
                    .prompt("send to> ")
                    .header(says);
                self.hold_view_left(client);
                self.pick(client, picker, move |server, client, picked| {
                    let Some(tab) = picked.and_then(|p| p.value.parse().ok()).map(TabId) else {
                        return;
                    };
                    let runs_claude = server
                        .sidebar_all_tabs()
                        .iter()
                        .any(|t| t.id == tab && t.claude_pane.is_some());
                    if !runs_claude {
                        server.say(
                            Some(client),
                            "That tab's Claude has ended, so nothing was sent.",
                        );
                        return;
                    }
                    server.paste_into_tab(client, tab, &h.text, sends_to_tab);
                });
            }
            Do::SlackReply(reply) => self.slack_reply_box(client, *reply, ""),
            // It changes your reminders in Slack, so it asks first.
            Do::SlackReminderDone { id, text } => {
                let first = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
                let quoted = format!("\"{}\"", crate::ui::draw::cut(first, 60));
                let question =
                    Confirm::new("Mark this reminder complete in Slack?").list(vec![quoted]);
                self.ask(client, question, move |server, client, yes| {
                    if !yes {
                        return;
                    }
                    server.mission_busy_with(client, "Marking it complete", async move {
                        match crate::day::slack_user::complete(&id).await {
                            Ok(()) => (
                                Some("Marked complete in Slack.".into()),
                                Some(After::SlackReminderDone(id)),
                            ),
                            Err(e) => (Some(format!("Couldn't mark it complete: {e}")), None),
                        }
                    });
                });
            }
            Do::SlackDismiss(items) => {
                self.slack_dismiss(&items);
                self.mission_note(client, "Dismissed.");
            }
            Do::Reopen(key) => match self.reopen_closed(client, &key) {
                Ok((_, note)) => {
                    self.mission_close(client);
                    if let Some(note) = note {
                        self.say(Some(client), &note);
                    }
                }
                Err(e) => self.mission_note(client, e),
            },
            Do::ResumeSession(id) => match self.resume_session(client, &id) {
                Ok(_) => self.mission_close(client),
                Err(e) => self.mission_note(client, e),
            },
            Do::RemoveClosed(key) => {
                let name = self
                    .clients
                    .get(&client)
                    .and_then(|c| c.overlays.find::<Mission>())
                    .and_then(|m| m.chosen_item())
                    .map(|i| i.title.clone())
                    .unwrap_or_default();
                self.ask(
                    client,
                    Confirm::new("Forget it? Its directory and session stay.").list(vec![name]),
                    move |server, _, yes| {
                        if yes {
                            server.remove_closed(&key);
                        }
                    },
                );
            }
            Do::OpenTabNote(note) => {
                self.mission_close(client);
                self.open_note(client, &note);
            }
            Do::ShowMerged => {
                let Some(mut m) = self.take_mission(client) else {
                    return;
                };
                m.folds.merged(REOPEN_MERGED);
                m.taken_at = None;
                self.put_mission(client, m);
                self.mission_prepare(client);
                self.mission_redraw();
            }
            Do::RemoveWorktrees(dirs) => {
                let n = dirs.len();
                let shown: Vec<String> = dirs.iter().map(paths::tilde).collect();
                let question = format!(
                    "Remove {n} merged worktree{}? One with changes not committed is kept.",
                    if n == 1 { "" } else { "s" }
                );
                self.ask(
                    client,
                    Confirm::new(question).list(shown),
                    move |server, client, yes| {
                        if yes {
                            server.remove_worktrees(client, dirs);
                        }
                    },
                );
            }
            Do::OpenHere(dir) => {
                self.mission_close(client);
                // The same handoff wtr makes: the worktree's last Claude
                // session resumed, or a new one when it has none.
                let exe = crate::util::exe()
                    .map(|e| e.to_string_lossy().replace('\'', "'\\''"))
                    .unwrap_or_else(|_| "switchboard".into());
                self.start_in(client, &dir, None, &format!("'{exe}' claude-here"));
            }
            Do::RemoveWorktree(dir) => {
                let shown = paths::tilde(&dir);
                self.ask(
                    client,
                    Confirm::new("Remove this worktree? One with changes not committed is kept.")
                        .list(vec![shown]),
                    move |server, client, yes| {
                        if yes {
                            server.remove_worktrees(client, vec![dir]);
                        }
                    },
                );
            }
            Do::Habit { id, outcome } => match self.habit_outcome(&id, outcome) {
                Ok(Some(said)) if id == format!("on:{}", crate::setup::way::SETTING) => {
                    self.claude_plugin_setup(client, said)
                }
                Ok(Some(said)) => self.mission_note(client, said),
                Ok(None) => {}
                Err(e) => self.mission_note(client, e),
            },
            Do::Dismiss { source, item } => {
                self.notice_dismiss(&source, &item);
                self.mission_note(client, "Dismissed.");
            }
            Do::DismissAll(notices) => {
                for (source, item) in &notices {
                    self.notice_dismiss(source, item);
                }
                // Dismissed in the history too, though one put away before
                // it took it out of its list on the way (a Slack thread's
                // older message), as `slack_dismiss` marks its own.
                for (source, item) in &notices {
                    self.notice_history_mark(source, &item.id, Sidebar::Dismissed, true);
                }
                self.mission_note(
                    client,
                    format!("Dismissed {} notifications.", notices.len()),
                );
            }
            Do::Autopilot(act) => self.autopilot_act(client, act),
            Do::ReadLog { job, name, line } => self.read_ci_log(client, &job, &name, line),
            Do::ReadOutput(file) => {
                if !file.is_file() {
                    self.mission_note(client, "Its last output is no longer kept.");
                    return;
                }
                self.page_beside(client, "@landing_output", &file, None);
            }
            Do::OpenNote(rel) => {
                self.mission_close(client);
                self.new_note_open(client, &rel);
            }
            Do::OpenView(slug) => self.open_mission(client, Opening::View(slug)),
            Do::ShowIn { view, id } => self.show_in_view(client, &view, &id),
            Do::AnswerWith { tab, reply } => self.answer_tab(client, tab, &reply),
            Do::SlackPlay(item) => self.slack_play_now(client, Some(*item)),
            Do::EditFile { dir, file } => self.edit_own_file(client, &dir, &file),
            Do::CardScreen(tab) => {
                let Some(mut m) = self.take_mission(client) else {
                    return;
                };
                let shown = m.card_screen.is_some_and(|(t, _)| t == tab);
                m.card_screen = (!shown && chosen_tab(&m) == Some(tab)).then_some((tab, m.cursor));
                self.put_mission(client, m);
                self.mission_redraw();
            }
            Do::RelaySet { key, value, label } => self.relay_set(client, &key, value, &label),
            Do::RelayPreset(preset) => self.relay_preset(client, preset),
            Do::Change(item) => {
                let Some(mut m) = self.take_mission(client) else {
                    return;
                };
                self.change_item(client, &mut m, &item);
                m.taken_at = None;
                self.put_mission(client, m);
                self.mission_prepare(client);
                self.mission_redraw();
            }
            Do::SetSetting { key, value } => self.set_setting_shown(client, &key, value),
            Do::TypeSetting(key) => self.type_setting(client, key),
            Do::Press(action) => {
                self.mission_close(client);
                self.act(client, action);
            }
            Do::Routine { id, act } => {
                use crate::routines::Act;
                let note = match act {
                    Act::RunNow => self.routine_run_now(client, &id),
                    Act::OpenSession => {
                        self.routine_open_session(client, &id);
                        None
                    }
                    Act::AllowNeeded => self.routine_allow_needed(client, &id),
                    Act::Edit => {
                        self.routine_edit(client, &id);
                        None
                    }
                    Act::Pause => self.routine_pause(&id, Some(true)),
                    Act::Resume => self.routine_pause(&id, Some(false)),
                    Act::Delete => {
                        self.routine_delete(client, &id);
                        None
                    }
                };
                if let Some(note) = note {
                    self.mission_note(client, note);
                }
            }
        }
    }

    /// A CI job's kept log in a pane beside the one you're in, in place of
    /// one read there before, paged from `line` (its failing step's first)
    /// or else from its end (`switchboard view --log`), as the CI view's
    /// "Read the failing log" showed it in a popup with `less +G`.
    fn read_ci_log(&mut self, client: ClientId, job: &str, name: &str, line: Option<usize>) {
        const MARK: &str = "@ci_log";
        let file = crate::sources::ciboard::log_file(job);
        if !file.is_file() {
            self.mission_note(client, format!("{name}'s log is no longer kept."));
            return;
        }
        self.page_beside(client, MARK, &file, line);
    }

    /// A file paged in a pane beside the one you're in (`switchboard view
    /// --log`), marked `mark`, in place of one with that mark read there
    /// before, from `line` or else from its end.
    fn page_beside(&mut self, client: ClientId, mark: &str, file: &Path, line: Option<usize>) {
        self.mission_close(client);
        let Some(pane) = self.current_pane(client) else {
            return;
        };
        let Some(tab) = self.panes.get(&pane).map(|s| s.tab) else {
            return;
        };
        let mut beside = pane;
        if let Some(old) = self.marked_in(tab, mark) {
            self.kill_pane(old);
            if old == pane {
                match self.tabs.get(&tab) {
                    Some(t) => beside = t.active,
                    None => return,
                }
            }
        }
        let mut args = vec!["view".to_string(), "--log".into()];
        args.extend(line.map(|n| format!("+{n}")));
        args.push(file.to_string_lossy().into_owned());
        self.open_beside(beside, mark, args, 50);
    }

    /// Runs `work` in a task of its own under the busy box, which closes
    /// when it is done; what it gives is said in the footer, and `after`
    /// done on the server.
    fn mission_busy<F>(&mut self, client: ClientId, label: &str, after: Option<After>, work: F)
    where
        F: std::future::Future<Output = Option<String>> + Send + 'static,
    {
        self.mission_busy_with(client, label, async move { (work.await, after) });
    }

    /// As `mission_busy`, the work itself saying what the server does after.
    fn mission_busy_with<F>(&mut self, client: ClientId, label: &str, work: F)
    where
        F: std::future::Future<Output = (Option<String>, Option<After>)> + Send + 'static,
    {
        if tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        self.open(client, Box::new(Busy::new(format!("{label}…"))), None);
        self.mission_spin_later();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let (note, after) = work.await;
            let _ = tx.send(Event::Mission(MissionEvent::Done {
                client,
                note,
                after,
            }));
        });
    }

    /// A notification marked read in Linear: off the Inbox at once, read
    /// in the history, and the Inbox asked again.
    fn inbox_dropped(&mut self, id: &str) {
        self.notice_history_mark("inbox", id, Sidebar::Read, true);
        self.led_to("inbox", id, None, "marked read in Linear");
        self.sources.drop_inbox_item(id);
        self.inbox_notices();
        self.refresh_sources(Some(crate::sources::INBOX));
    }

    /// The Slack view's reply box (board 108), `text` typed to start with.
    /// Send leaves the text, as typed, as Settings' `slack_send_as` says,
    /// for the conversation and thread the item named: as a draft in Slack
    /// (board 109), with nothing asked since nothing is sent, or at once,
    /// which asks first, naming who can read it, a no coming back to the
    /// box with the text kept, and only a yes sending.
    pub(crate) fn slack_reply_box(
        &mut self,
        client: ClientId,
        reply: crate::day::slack_reply::Reply,
        text: &str,
    ) {
        self.slack_reply_box_with(client, reply, text, None);
    }

    /// `slack_reply_box`, saying in the box why the last Send didn't send.
    fn slack_reply_box_with(
        &mut self,
        client: ClientId,
        reply: crate::day::slack_reply::Reply,
        text: &str,
        failed: Option<String>,
    ) {
        use crate::day::slack_reply::Way;
        let boxed = super::reply_box::ReplyBox::new(reply.clone(), text)
            .failed(failed)
            .as_draft(Way::now() == Way::Draft);
        self.open(
            client,
            Box::new(boxed),
            Some(Box::new(move |server: &mut Server, client, outcome| {
                let Outcome::Picked(p) = outcome else {
                    return;
                };
                let text = p.query;
                // As the setting says now, were it changed under the box.
                if Way::now() == Way::Draft {
                    server.slack_reply_deliver(client, reply, text, Way::Draft);
                    return;
                }
                let first = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
                let quoted = format!("\"{}\"", crate::ui::draw::cut(first, 60));
                let question = Confirm::new(reply.question()).list(vec![reply.readers(), quoted]);
                server.ask(client, question, move |server, client, yes| {
                    if !yes {
                        server.slack_reply_box(client, reply, &text);
                        return;
                    }
                    server.slack_reply_deliver(client, reply, text, Way::Now);
                });
            })),
        );
    }

    /// Leaves `text` for where `reply` says, as `way` says, through
    /// `slack_reply::deliver`, the one way a message leaves as you. A draft
    /// made: its conversation opened in Slack, the thread for a thread's
    /// reply, so the draft is in front of you, and the box left behind
    /// saying where it is. Sent: said, and what came before it read. Not
    /// delivered: said why, the reply's box back with the text kept.
    fn slack_reply_deliver(
        &mut self,
        client: ClientId,
        reply: crate::day::slack_reply::Reply,
        text: String,
        way: crate::day::slack_reply::Way,
    ) {
        use crate::day::slack_reply::{Delivered, Way, deliver};
        let tx = self.tx.clone();
        let label = match way {
            Way::Draft => "Making the draft",
            Way::Now => "Sending it",
        };
        self.mission_busy_with(client, label, async move {
            match deliver(&crate::day::ClaudeNow, &reply, &text, way).await {
                Ok(Delivered::Drafted(link)) => {
                    let opening = match link.clone() {
                        Some(link) => open_under_busy(tx, client, link).await,
                        None => None,
                    };
                    let drafted = After::SlackDrafted {
                        reply: Box::new(reply),
                        link,
                        opening,
                    };
                    (None, Some(drafted))
                }
                Ok(Delivered::Sent(ts)) => (
                    Some(format!("Sent to {}.", reply.place)),
                    Some(After::SlackReplied {
                        channel: reply.channel.clone(),
                        thread: reply.thread.clone(),
                        ts,
                        shown: reply.shown.clone(),
                    }),
                ),
                Err(none) if way == Way::Draft => (
                    None,
                    Some(After::SlackNoDraft {
                        reply: Box::new(reply),
                        text,
                        why: none.why,
                        draft_there: none.draft_there,
                    }),
                ),
                Err(none) => (
                    None,
                    Some(After::SlackReplyAgain {
                        reply: Box::new(reply),
                        text,
                        why: none.why,
                    }),
                ),
            }
        });
    }

    /// The box a draft leaves behind: where it is, "OK" chosen, and "Take
    /// me to the draft", which opens it again.
    fn slack_drafted(
        &mut self,
        client: ClientId,
        reply: crate::day::slack_reply::Reply,
        link: Option<String>,
        opening: Option<String>,
    ) {
        let mut body = reply.drafted();
        if let Some(said) = opening {
            body.push_str(&format!("\n{said}"));
        }
        let mut options = vec![MenuOption::new("OK")];
        if link.is_some() {
            options.push(MenuOption::new("Take me to the draft"));
        }
        let menu = Menu::new("A draft in Slack", options).body(body);
        self.choose(client, menu, move |server, client, chose| {
            if let (Some(1), Some(link)) = (chose, link) {
                server.mission_do(client, Do::Open(link), "Take me to the draft");
            }
        });
    }

    /// The box a draft Slack didn't make leaves: why, and, where a draft
    /// is already there, "Take me to it"; then the reply's box again with
    /// the text kept.
    fn slack_no_draft(
        &mut self,
        client: ClientId,
        reply: crate::day::slack_reply::Reply,
        text: String,
        why: String,
        draft_there: bool,
    ) {
        let there = reply.link.clone().filter(|_| draft_there);
        let mut options = vec![MenuOption::new("OK")];
        if there.is_some() {
            options.push(MenuOption::new("Take me to it"));
        }
        let menu = Menu::new("No draft made", options).body(why);
        self.choose(client, menu, move |server, client, chose| {
            server.slack_reply_box(client, reply, &text);
            if let (Some(1), Some(link)) = (chose, there) {
                server.mission_do(client, Do::Open(link), "Take me to it");
            }
        });
    }

    /// What a busy option leaves the server to do once it has answered.
    fn mission_after(&mut self, client: ClientId, after: After) {
        match after {
            After::DropInbox(id) => self.inbox_dropped(&id),
            After::Worktrees => {
                // A worktree just removed leaves the rows wt-revive gave at
                // once, not at its next look.
                self.mission.revived.retain(|r| r.tree.is_dir());
                self.mission_worktrees(true);
                self.mission_revived(true);
            }
            After::Calendar => self.calendar_reread(),
            After::SlackReminderDone(id) => self.slack_reminder_done(&id),
            After::SlackReplied {
                channel,
                thread,
                ts,
                shown,
            } => self.slack_replied(&channel, thread.as_deref(), ts.as_deref(), shown.as_deref()),
            After::SlackReplyAgain { reply, text, why } => {
                self.slack_reply_box_with(client, *reply, &text, Some(why))
            }
            After::SlackDrafted {
                reply,
                link,
                opening,
            } => self.slack_drafted(client, *reply, link, opening),
            After::SlackNoDraft {
                reply,
                text,
                why,
                draft_there,
            } => self.slack_no_draft(client, *reply, text, why, draft_there),
            After::Tickets => {
                self.refresh_sources(Some(crate::sources::TICKETS));
                self.refresh_sources(Some(crate::sources::INBOX));
            }
            After::CloseTicket {
                ticket,
                title,
                issue,
                state,
                name,
            } => {
                self.ask(
                    client,
                    Confirm::new(format!("Move {ticket} to {name}?")).list(vec![title]),
                    move |server, client, yes| {
                        if !yes {
                            return;
                        }
                        server.mission_busy_with(client, "Working on it", async move {
                            match crate::sources::tickets::close_ticket(&issue, &state).await {
                                Ok(true) => (
                                    Some(format!("Moved {ticket} to {name}.")),
                                    Some(After::Tickets),
                                ),
                                Ok(false) => (Some("Linear didn't close it.".into()), None),
                                Err(e) => {
                                    (Some(format!("Couldn't close it: {}", first_line(&e))), None)
                                }
                            }
                        });
                    },
                );
            }
        }
    }

    /// Goes to a tab: mission control closes, the tab comes to the front and
    /// its pane takes the focus, in its own session, listed in its sidebar.
    pub fn mission_go(&mut self, client: ClientId, tab: TabId) {
        self.mission_close(client);
        let Some(t) = self.tabs.get(&tab) else {
            return;
        };
        let pane = t.active;
        let session = self.session_of_tab(tab).map(|s| s.name.clone());
        if let Some(s) = &session {
            self.sidebar_unfold_to(s, tab);
        }
        self.focus_pane(pane);
        if let Some(s) = session
            && self.clients.get(&client).is_some_and(|c| c.session != s)
        {
            self.switch_client(client, &s);
        }
    }

    /// The tab, and its pane in copy mode on the last line showing `text`,
    /// the match selected; with the pane gone or the text no longer there,
    /// the tab alone, saying so.
    pub(crate) fn mission_go_to_match(
        &mut self,
        client: ClientId,
        tab: TabId,
        pane: PaneId,
        text: &str,
    ) {
        self.mission_go(client, tab);
        let Some(slot) = self.panes.get_mut(&pane).filter(|s| s.tab == tab) else {
            return;
        };
        let pattern = regex::escape(text);
        if !slot.pane.copy_find(&pattern) {
            self.say(Some(client), "That match is no longer in the pane.");
            return;
        }
        slot.copy = Some(crate::mux::server::CopyMode {
            exit_at_bottom: false,
            search: Some((pattern, false)),
        });
        self.focus_pane(pane);
        self.dirty_tab(tab);
    }

    /// A look at a tab without going there; Enter in it goes there.
    fn mission_peek(&mut self, client: ClientId, tab: TabId) {
        self.open_peek(client, tab, None, true);
    }

    /// A tab's name typed in; empty names it after its directory again.
    pub(crate) fn rename_tab(&mut self, client: ClientId, tab: TabId) {
        if !self.tabs.contains_key(&tab) {
            return;
        }
        let had = self.tab_own_name(tab).unwrap_or("").to_string();
        let ask = Ask::new(
            "Rename the tab",
            "A name of its own; empty to name it after its directory again.",
        )
        .text(had)
        .empty_ok();
        self.about_tab(client, tab, move |s| {
            s.open(
                client,
                Box::new(ask),
                Some(Box::new(move |server: &mut Server, client, outcome| {
                    let Outcome::Picked(p) = outcome else {
                        return;
                    };
                    // The tab's `@tab_name`, as tmux's rename set it, so `/wt`,
                    // `/wt` and `/shape`, which set and clear that option, still
                    // name the tab after a rename here.
                    if let Some(t) = server.tabs.get_mut(&tab) {
                        t.name.clear();
                        // The name typed is the tab's whole title now, so a
                        // full title kept for a note's item goes.
                        t.opts.remove("@tab_title");
                        if p.value.is_empty() {
                            t.opts.remove("@tab_name");
                        } else {
                            t.opts.insert("@tab_name".into(), p.value.clone());
                        }
                    }
                    server.name_tab(tab);
                    server.mission_redraw();
                    server.dirty_tab(tab);
                    let said = if p.value.is_empty() {
                        "Named after its directory again.".to_string()
                    } else {
                        format!("Renamed it {}.", p.value)
                    };
                    server.mission_note(client, said);
                })),
            )
        });
    }

    /// A tab opened in `dir` in the client's session, its shell typing
    /// `command` once it has drawn its prompt.
    fn start_in(&mut self, client: ClientId, dir: &Path, name: Option<String>, command: &str) {
        if let Some(pane) = self.mission_new_tab(client, dir, None, name) {
            self.send_later(
                TYPE_AFTER,
                Event::Mission(MissionEvent::Press {
                    pane,
                    presses: vec![
                        Press::Text(command.to_string()),
                        Press::Key(Key::new(KeyCode::Enter, Mods::NONE)),
                    ],
                }),
            );
        }
    }

    fn mission_new_tab(
        &mut self,
        client: ClientId,
        dir: &Path,
        command: Option<Vec<String>>,
        name: Option<String>,
    ) -> Option<PaneId> {
        let session = self.clients.get(&client).map(|c| c.session.clone())?;
        let dir = if dir.is_dir() {
            dir.to_path_buf()
        } else {
            paths::home()
        };
        match self.new_tab(
            &session,
            Some(dir),
            command,
            name,
            crate::mux::server::Opened::Front,
        ) {
            Ok((_, pane)) => Some(pane),
            Err(e) => {
                self.say(Some(client), &format!("Couldn't open a tab: {e}"));
                None
            }
        }
    }

    /// Types into the prompt of the Claude in `pane`: `i` first while its
    /// screen shows the prompt in vim's Normal mode (`answer::into_prompt`),
    /// so what is typed arrives whole. Everything that types a command or a
    /// message into a tab's Claude goes through here (the PR view's
    /// /resolve-conflicts, the Slack tab's /slack-updates, a CI loop
    /// started from mission control, a message answering a tab, a reply
    /// sent to another tab); the relay does the same through its own
    /// `tabs::type_into`.
    pub(crate) fn type_into_claude(&mut self, pane: PaneId, presses: Vec<Press>) {
        let screen = self
            .panes
            .get(&pane)
            .map(|slot| slot.pane.text(0))
            .unwrap_or_default();
        self.press(pane, answer::into_prompt(&screen, presses));
    }

    /// Presses keys and text in a pane, a paste given a moment to land
    /// before what follows it.
    pub(crate) fn press(&mut self, pane: PaneId, presses: Vec<Press>) {
        let mut rest = presses.into_iter();
        while let Some(p) = rest.next() {
            let Some(slot) = self.panes.get_mut(&pane) else {
                return;
            };
            let modes = slot.pane.modes();
            let bytes = match p {
                Press::Key(k) => input::encode_key(&k, &modes),
                Press::Text(t) => t.into_bytes(),
                Press::Paste(t) => input::encode_paste(&t, &modes),
                Press::Settle => {
                    let left: Vec<Press> = rest.collect();
                    if !left.is_empty() {
                        self.send_later(
                            Duration::from_millis(answer::PASTE_SETTLE_MS),
                            Event::Mission(MissionEvent::Press {
                                pane,
                                presses: left,
                            }),
                        );
                    }
                    return;
                }
            };
            let _ = slot.pane.write(&bytes);
        }
    }

    // ── Answering a tab ─────────────────────────────────────────────────────

    /// The prompt a tab's Claude shows, read off its screen.
    pub(crate) fn prompt_of(&self, tab: TabId) -> Option<answer::Prompt> {
        let pane = self.claude_pane_of(tab)?;
        let slot = self.panes.get(&pane)?;
        answer::options_in(&slot.pane.text(60))
    }

    fn claude_pane_of(&self, tab: TabId) -> Option<PaneId> {
        self.sidebar_all_tabs()
            .into_iter()
            .find(|t| t.id == tab)
            .and_then(|t| t.claude_pane)
    }

    /// The box answering a tab: its prompt and options, or a line to send it
    /// a message.
    pub fn open_answer(&mut self, client: ClientId, tab: TabId, note: Option<String>) {
        let Some(model) = self.sidebar_all_tabs().into_iter().find(|t| t.id == tab) else {
            return;
        };
        if model.claude_pane.is_none() {
            self.mission_note(client, "That tab has no Claude to answer.");
            return;
        }
        let found = self.prompt_of(tab);
        let title = format!("Answer {} {}", model.index, model.name);
        self.about_tab(client, tab, move |s| {
            s.open(
                client,
                Box::new(AnswerBox::new(title, found, note)),
                Some(Box::new(move |server: &mut Server, client, outcome| {
                    if let Outcome::Picked(p) = outcome {
                        server.answer_tab(client, tab, &p.value);
                    }
                })),
            )
        });
    }

    /// Presses a reply in a tab: an option's number, `submit`, your own
    /// answer, or, with no prompt showing, a message.
    fn answer_tab(&mut self, client: ClientId, tab: TabId, reply: &str) {
        let Some(pane) = self.claude_pane_of(tab) else {
            self.mission_note(client, "That tab has no Claude to answer.");
            return;
        };
        match self.prompt_of(tab) {
            Some(found) => match answer::answer_for(&found, reply) {
                None => self.open_answer(
                    client,
                    tab,
                    Some(format!("\"{reply}\" isn't one of its options.")),
                ),
                Some(a) => {
                    self.press(pane, answer::presses(&found, &a));
                    self.send_later(
                        REANSWER_AFTER,
                        Event::Mission(MissionEvent::Reanswer {
                            client,
                            tab,
                            note: answer::answer_said(&a),
                        }),
                    );
                }
            },
            None if reply.trim().is_empty() => {
                self.open_answer(client, tab, Some("Nothing to send.".into()))
            }
            // Claude's fullscreen view scrolled back hides what it waits
            // on, which the message's keys and Enter would answer unseen.
            None if (self.agents.state(pane) == Some(crate::agent::State::Waiting)
                || self.agents.get(pane).is_some_and(|a| a.asked.is_some()))
                && self
                    .panes
                    .get(&pane)
                    .is_some_and(|slot| answer::scrolled_back(&slot.pane.text(0))) =>
            {
                self.mission_note(
                    client,
                    "Its Claude's view is scrolled up past what it waits on: scroll it down first.",
                );
            }
            None => {
                self.type_into_claude(pane, answer::message_presses(reply));
                self.mission_note(client, "Sent.");
            }
        }
    }

    // ── Routines ────────────────────────────────────────────────────────────

    /// The Routines view's keys: n a new routine; on the chosen one, r
    /// runs it now, e changes it, p pauses or resumes it, o opens its
    /// last session in a tab, x deletes it after asking; i the chosen one in the list's
    /// place on a narrow screen, Esc back. True when it took the key.
    fn routines_key(&mut self, client: ClientId, m: &mut Mission, k: Key) -> bool {
        if !on_routines(m) || !plain(&k) {
            return false;
        }
        let Some(view) = m.current().cloned() else {
            return false;
        };
        if k.code == KeyCode::Char('n') {
            self.routine_new(client);
            return true;
        }
        if k.code == KeyCode::Char('i') {
            m.routines_pane = !m.routines_pane;
            return true;
        }
        if k.code == KeyCode::Esc && m.routines_pane {
            m.routines_pane = false;
            return true;
        }
        let chosen = crate::routines::view::chosen(m, &view);
        let Some(id) = chosen
            .and_then(|i| crate::routines::view::id_of(&view.data.items[i]))
            .map(str::to_string)
        else {
            return false;
        };
        if let Some(i) = chosen {
            m.cursor = i as isize;
        }
        let note = match k.code {
            KeyCode::Char('r') => self.routine_run_now(client, &id),
            KeyCode::Char('e') => {
                self.routine_edit(client, &id);
                None
            }
            KeyCode::Char('p') => self.routine_pause(&id, None),
            KeyCode::Char('o') => {
                self.routine_open_session(client, &id);
                None
            }
            KeyCode::Char('x') => {
                self.routine_delete(client, &id);
                None
            }
            _ => return false,
        };
        if note.is_some() {
            m.note = note;
        }
        true
    }

    // ── Reminders ───────────────────────────────────────────────────────────

    /// The Reminders view's keys with its month drawn: h and l a day, j
    /// and k a week, H and L a month, t today; Enter the chosen day's
    /// reminder's menu (its tab's), or on a day with none, says `m`
    /// sets one; i the pane's lists under a narrow month. True when it
    /// took the key.
    fn reminders_key(&mut self, client: ClientId, m: &mut Mission, k: Key) -> bool {
        if !on_reminders(m) || !plain(&k) {
            return false;
        }
        let Some(view) = m.current().cloned() else {
            return false;
        };
        use super::reminders::{chosen_day, step};
        let day = chosen_day(m);
        let to = match k.code {
            KeyCode::Char('h') | KeyCode::Left => Some(step(day, -1, 0)),
            KeyCode::Char('l') | KeyCode::Right => Some(step(day, 1, 0)),
            KeyCode::Char('j') | KeyCode::Down => Some(step(day, 7, 0)),
            KeyCode::Char('k') | KeyCode::Up => Some(step(day, -7, 0)),
            KeyCode::Char('H') => Some(step(day, 0, -1)),
            KeyCode::Char('L') => Some(step(day, 0, 1)),
            _ => None,
        };
        if let Some(to) = to {
            choose_day(m, to);
            if m.focus.is_some() {
                m.focus = Some(Focus::Item);
            }
            return true;
        }
        let chosen = super::reminders::chosen(m, &view);
        let tab = chosen.and_then(|i| match view.data.items[i].target {
            Target::Tab(t) => Some(t),
            _ => None,
        });
        // What acts on the reminder the pane shows takes the cursor to it, so
        // the view keeps it chosen once it changes.
        if let Some(i) = chosen {
            m.cursor = i as isize;
        }
        match (k.code, tab) {
            (KeyCode::Char('i'), _) => m.reminders_lists = !m.reminders_lists,
            (KeyCode::Enter, Some(_)) => {
                m.cursor = chosen.unwrap_or(0) as isize;
                self.mission_enter(client, m);
            }
            (KeyCode::Enter, None) => {
                let words = format!(
                    "No reminder on {} {} {}. m sets one.",
                    remind::short_day(day.weekday()),
                    remind::short_month(day.month()),
                    day.day()
                );
                self.say(Some(client), &words);
            }
            _ => return false,
        }
        true
    }

    /// `e` on a day of the Reminders view with none: a tab to remind
    /// about, then its reminder's box with that day at 9:00 typed in
    /// (today with nothing typed, and a day gone by refused).
    fn remind_on_day(&mut self, client: ClientId, day: jiff::civil::Date) {
        let tabs = self.sidebar_all_tabs();
        if tabs.is_empty() {
            return;
        }
        let today = util::local_now().date();
        if day < today {
            self.say(Some(client), "That day has gone by.");
            return;
        }
        let rows = tabs
            .iter()
            .map(|t| PickItem {
                text: format!("{} {}", t.index, t.name),
                value: t.id.0.to_string(),
                ..PickItem::default()
            })
            .collect();
        // Today keeps the times offered; another day starts at 9:00 there.
        let typed = if day == today {
            String::new()
        } else {
            // With its year, so a day a year or more on stays that day.
            format!("{}-{:02}-{:02} 9am", day.year(), day.month(), day.day())
        };
        let picker = Picker::new(
            format!(
                "Remind me about a tab on {} {} {}",
                remind::short_day(day.weekday()),
                remind::short_month(day.month()),
                day.day()
            ),
            rows,
        );
        self.pick(client, picker, move |server, client, picked| {
            let Some(picked) = picked else {
                return;
            };
            if let Some(tab) = picked.value.parse().ok().map(TabId) {
                server.remind_tab_with(client, tab, &typed);
            }
        });
    }

    /// A tab's reminder cleared, and said through the footer.
    pub(crate) fn clear_reminder(&mut self, client: ClientId, tab: TabId) {
        let name = self.remind_name(tab).unwrap_or_default();
        self.write_reminder(tab, None);
        self.mission_note(client, format!("Reminder cleared for {name}."));
    }

    /// A tab's reminder set, and said through the footer.
    pub(crate) fn set_reminder(&mut self, client: ClientId, tab: TabId, at: i64, note: &str) {
        let name = self.remind_name(tab).unwrap_or_default();
        self.write_reminder(tab, Some((at, note)));
        self.mission_note(
            client,
            format!("Reminder set for {name}, {}.", remind::said(at)),
        );
    }

    /// A tab's reminder set (its time, in seconds, and what for, empty for
    /// nothing) or, with `None`, cleared: its `@remind_at` and
    /// `@remind_note`, and `@remind_said` taken off so a new one is said
    /// when it comes due. The workspace's box and the phone (src/phone)
    /// both set one through it.
    pub(crate) fn write_reminder(&mut self, tab: TabId, reminder: Option<(i64, &str)>) {
        if let Some(t) = self.tabs.get_mut(&tab) {
            t.opts.remove("@remind_said");
            match reminder {
                Some((at, note)) => {
                    t.opts.insert("@remind_at".into(), at.to_string());
                    if note.is_empty() {
                        t.opts.remove("@remind_note");
                    } else {
                        t.opts.insert("@remind_note".into(), note.to_string());
                    }
                }
                None => {
                    t.opts.remove("@remind_at");
                    t.opts.remove("@remind_note");
                }
            }
        }
        self.dirty_tab(tab);
        self.mission_redraw();
    }

    /// A tab's number and name, as its reminder's words name it.
    fn remind_name(&self, tab: TabId) -> Option<String> {
        self.session_of_tab(tab)?;
        Some(format!("{} {}", self.tab_number(tab), self.tab_label(tab)))
    }

    /// "Remind me about this tab": the reminder's box (board
    /// 34, `remind_box`); else when (remind.sh's times, or one typed),
    /// then what for, with a reminder set, clearing it coming first. The
    /// times are a list to pick from or type in, as remind.sh's fzf was: a
    /// choice's number and Enter takes it, and what is typed wins over the
    /// row it resembles when it names a time (`2m` over "In 20 minutes").
    pub fn remind_tab(&mut self, client: ClientId, tab: TabId) {
        self.remind_tab_with(client, tab, "");
    }

    /// `remind_tab`, with `typed` in the box's When to start with.
    pub(crate) fn remind_tab_with(&mut self, client: ClientId, tab: TabId, typed: &str) {
        if !settings::is_on("reminders") {
            self.say(Some(client), "Reminders are off in Settings.");
            return;
        }
        let Some(name) = self.remind_name(tab) else {
            return;
        };
        let opt = |key: &str| {
            self.tabs
                .get(&tab)
                .and_then(|t| t.opts.get(key))
                .filter(|v| !v.is_empty())
                .cloned()
        };
        let set_at = opt("@remind_at").and_then(|v| v.parse::<i64>().ok());
        let set = set_at.map(|at| (at, opt("@remind_note").unwrap_or_default()));
        let boxed = super::remind_box::RemindBox::new(&name, set).typed(typed);
        self.open(
            client,
            Box::new(boxed),
            Some(Box::new(move |server: &mut Server, client, outcome| {
                let Outcome::Picked(p) = outcome else {
                    return;
                };
                match (p.index, p.value.parse::<i64>()) {
                    (1, _) => server.clear_reminder(client, tab),
                    (_, Ok(at)) => server.set_reminder(client, tab, at, &p.query),
                    _ => {}
                }
            })),
        );
    }

    // ── Settings ────────────────────────────────────────────────────────────

    /// A Settings row changed, as its menu's first choice: an on/off
    /// flipped or a choice to the next, a setting kept in a file of yours,
    /// or a repository's landing.
    fn change_item(&mut self, client: ClientId, m: &mut Mission, item: &Item) {
        if let Some(key) = item.data.get("setting").and_then(Value::as_str) {
            let key = key.to_string();
            if let Some(note) = self.change_setting(&key) {
                m.note = Some(note);
            } else if key == crate::setup::way::SETTING {
                let on = settings::is_on(&key);
                self.claude_plugin_setup(
                    client,
                    format!(
                        "Load Switchboard as a plugin: {}",
                        if on { "on" } else { "off" }
                    ),
                );
            }
        } else if item.data.get("own").is_some() {
            self.change_own(client, m, item);
        } else if item.data.get("repo_setting").is_some() {
            self.change_repo(client, m, item);
        }
    }

    /// A Settings row's menu: changing it, then its file in your editor.
    fn setting_choices(&self, item: &Item) -> Vec<Choice> {
        let mut out = Vec::new();
        let change = |label: String| Choice {
            key: 'c',
            label,
            act: Do::Change(Box::new(item.clone())),
        };
        if let Some(key) = item.data.get("setting").and_then(Value::as_str) {
            let option = settings::read_settings().into_iter().find(|o| o.key == key);
            match option {
                Some(o) if o.choices.is_none() && o.is_on() => {
                    out.push(change("Turn it off".into()))
                }
                Some(o) if o.choices.is_none() => out.push(change("Turn it on".into())),
                Some(o) => out.extend(choices::every_choice(&o)),
                None => out.push(change("Change it".into())),
            }
        } else if item.data.get("own").is_some() || item.data.get("repo_setting").is_some() {
            out.push(change("Change it".into()));
        }
        // The relay's choices of what reaches you lead to its own view.
        let relay_kind = item
            .data
            .get("setting")
            .and_then(Value::as_str)
            .is_some_and(|k| k.starts_with("relay_send_") || k.starts_with("relay_quiet"));
        if relay_kind && settings::is_on("view_relay") {
            out.push(Choice {
                key: 'v',
                label: "Open the Slack relay view".into(),
                act: Do::OpenView(ViewId::Relay.slug()),
            });
        }
        let (dir, file) = settings_file_of(Some(item));
        out.push(Choice {
            key: 'e',
            label: format!("Open {file} in your editor"),
            act: Do::EditFile { dir, file },
        });
        out
    }

    /// A setting to its next value, written to the settings file and taken
    /// up at once; what went wrong, if anything.
    fn change_setting(&mut self, key: &str) -> Option<String> {
        let option = settings::read_settings()
            .into_iter()
            .find(|o| o.key == key)?;
        self.set_setting(key, option.next_value())
    }

    /// A setting set to one of its choices from its menu, or a number typed
    /// in, and Settings drawn again; what went wrong said.
    fn set_setting_shown(&mut self, client: ClientId, key: &str, value: Value) {
        if let Some(note) = self.set_setting(key, value) {
            self.mission_note(client, note);
        }
        self.mission_prepare(client);
        self.mission_redraw();
    }

    /// A setting's number or folder typed in, starting from the one it
    /// has, for an option that takes one besides its choices
    /// (`settings::Typed`).
    fn type_setting(&mut self, client: ClientId, key: String) {
        use settings::TypedKind;
        let Some(option) = settings::read_settings().into_iter().find(|o| o.key == key) else {
            return;
        };
        let Some(typed) = option.typed.clone() else {
            return;
        };
        let (title, hint, now) = match typed.kind {
            TypedKind::Number => (
                format!("{} ({})", option.label, typed.unit),
                format!("A number from {}.", typed.least),
                typed.number(&option.value).map(|n| n.to_string()),
            ),
            TypedKind::Folder => (
                option.label.clone(),
                "A folder, as ~/... or an absolute path.".to_string(),
                typed.words(&option.value),
            ),
        };
        let ask = Ask::new(title, hint).text(now.unwrap_or_default());
        self.open(
            client,
            Box::new(ask),
            Some(Box::new(move |server: &mut Server, client, outcome| {
                let Outcome::Picked(p) = outcome else {
                    return;
                };
                let given = p.value.trim();
                let value = Value::from(given);
                let refused = match typed.kind {
                    TypedKind::Number => typed
                        .number(&value)
                        .is_none()
                        .then(|| format!("Not saved: {given} isn't a number from {}", typed.least)),
                    TypedKind::Folder => (!typed.folder(&value).is_some_and(|d| d.is_dir()))
                        .then(|| format!("Not saved: {given} isn't a folder")),
                };
                if let Some(refused) = refused {
                    server.mission_note(client, refused);
                    return;
                }
                server.set_setting_shown(client, &key, value);
            })),
        );
    }

    /// A setting set to `value`, written to the settings file and taken up
    /// at once; what went wrong, if anything.
    fn set_setting(&mut self, key: &str, value: Value) -> Option<String> {
        if let Err(e) = settings::write_setting(key, value) {
            return Some(format!("Couldn't save it: {e}"));
        }
        settings::reload_settings();
        self.fit_sidebars();
        for c in self.clients.values_mut() {
            c.dirty = true;
        }
        None
    }

    /// A file of yours (or the settings file) opened in your editor in a
    /// tab of its own, mission control closed.
    fn edit_own_file(&mut self, client: ClientId, dir: &Path, file: &str) {
        let _ = std::fs::create_dir_all(dir);
        let path = dir.join(file);
        let editor = std::env::var("VISUAL")
            .ok()
            .filter(|e| !e.trim().is_empty())
            .or_else(|| {
                std::env::var("EDITOR")
                    .ok()
                    .filter(|e| !e.trim().is_empty())
            })
            .unwrap_or_else(|| "vi".into());
        let mut command: Vec<String> = editor.split_whitespace().map(str::to_string).collect();
        command.push(path.to_string_lossy().into_owned());
        self.mission_close(client);
        self.mission_new_tab(client, dir, Some(command), Some(file.to_string()));
    }

    /// Enter on a setting kept in a file of yours: typed in, starting from
    /// what it is; a Chrome profile picked from Chrome's own; a whole file
    /// opened in your editor in a tab of its own.
    fn change_own(&mut self, client: ClientId, m: &mut Mission, item: &Item) {
        use crate::usage::own;
        let o = &item.data["own"];
        let text = |k: &str| o.get(k).and_then(Value::as_str).unwrap_or("").to_string();
        let (file, key, label, typed) = (text("file"), text("key"), text("label"), text("typed"));
        let min = o.get("min").and_then(Value::as_i64).unwrap_or(0);
        let kind = match text("kind").as_str() {
            "number" => own::Kind::Number,
            "list" => own::Kind::List,
            "map" => own::Kind::Map,
            "profile" => own::Kind::Profile,
            "file" => own::Kind::File,
            "rules" => own::Kind::Rules,
            "phones" => own::Kind::Phones,
            _ => own::Kind::Text,
        };
        let dir = own::dir_of(&file);
        match kind {
            own::Kind::Rules => self.change_rules(client, dir),
            own::Kind::Phones => {
                let devices: Vec<String> = crate::phone::pairing::devices()
                    .into_iter()
                    .map(|d| d.device)
                    .collect();
                if devices.is_empty() {
                    m.note = Some("No phone is paired.".into());
                    return;
                }
                let options = devices
                    .iter()
                    .map(|d| MenuOption::new(format!("Unpair {d}")))
                    .collect();
                self.choose(
                    client,
                    Menu::new(label, options),
                    move |server, client, chose| {
                        let Some(device) = chose.and_then(|i| devices.get(i)) else {
                            return;
                        };
                        let said = match crate::phone::pairing::revoke(device) {
                            Ok(true) => format!("{device} is no longer paired."),
                            Ok(false) => format!("{device} wasn't paired."),
                            Err(e) => format!("Couldn't unpair {device}: {e}"),
                        };
                        server.mission.own = None;
                        server.mission_note(client, said);
                    },
                );
            }
            own::Kind::Profile => {
                let Some(profiles) = own::chrome_profiles() else {
                    m.note = Some(
                        "Chrome has no profiles here: it isn't installed, or hasn't been opened yet."
                            .into(),
                    );
                    return;
                };
                let mut labels: Vec<String> = profiles
                    .iter()
                    .map(|p| {
                        if p.name == p.dir {
                            p.name.clone()
                        } else {
                            format!("{} ({})", p.name, p.dir)
                        }
                    })
                    .collect();
                labels.push("None (the default browser)".into());
                let mut dirs: Vec<String> = profiles.iter().map(|p| p.dir.clone()).collect();
                dirs.push(String::new());
                let at = profiles
                    .iter()
                    .position(|p| p.dir == typed)
                    .unwrap_or(profiles.len());
                let menu =
                    Menu::new(label.clone(), labels.iter().map(MenuOption::new).collect()).at(at);
                self.choose(client, menu, move |server, client, chose| {
                    let Some(value) = chose.and_then(|i| dirs.get(i)) else {
                        return;
                    };
                    let said = match own::write_own(&dir, &file, &key, kind, value, min) {
                        Ok(()) => format!("Saved {label} in {}", crate::files::shown(&file)),
                        Err(e) => format!("Not saved: {e}"),
                    };
                    server.mission.own = None;
                    server.mission_note(client, said);
                });
            }
            own::Kind::File => self.edit_own_file(client, &dir, &file),
            _ => {
                let hint = match kind {
                    own::Kind::Number => "A number; empty for the default.",
                    own::Kind::List => "Comma-separated; empty to unset it.",
                    own::Kind::Map => "site=profile, comma-separated; empty to unset it.",
                    _ => "Empty to unset it.",
                };
                let ask = Ask::new(format!("{label} ({file})"), hint)
                    .text(typed)
                    .empty_ok();
                self.open(
                    client,
                    Box::new(ask),
                    Some(Box::new(move |server: &mut Server, client, outcome| {
                        let Outcome::Picked(p) = outcome else {
                            return;
                        };
                        let saved = own::write_own(&dir, &file, &key, kind, &p.value, min);
                        let said = match &saved {
                            Ok(()) => format!("Saved {label} in {}", crate::files::shown(&file)),
                            Err(e) => format!("Not saved: {e}"),
                        };
                        if file == "sidebar.json" {
                            server.sidebar_load();
                        }
                        server.mission.own = None;
                        // Which file of your rules' repository is read:
                        // ~/.claude/CLAUDE.md is written again to import it.
                        if file == "claude.json" && saved.is_ok() {
                            server.claude_md_setup(client, said);
                        } else {
                            server.mission_note(client, said);
                        }
                    })),
                );
            }
        }
    }

    /// Enter on a repository's landing row in Settings: how its work lands
    /// from a menu (automatic leaves it to the default), where its work is
    /// kept, its review and where its tests run from a menu each, squash on or off, and a command typed in, written to
    /// ~/.config/switchboard/repos.json (`repos::set_repo`), as the daemon's
    /// `changeRepo` did.
    fn change_repo(&mut self, client: ClientId, m: &mut Mission, item: &Item) {
        let r = &item.data["repo_setting"];
        let repo = r["repo"].as_str().unwrap_or("").to_string();
        let field = r["field"].as_str().unwrap_or("").to_string();
        let now = r["value"].clone();
        let save = move |server: &mut Server, client: ClientId, value: Value| {
            let ask_first = server.autopilot_would_close_unasked(&repo, &field, &value);
            let write = move |server: &mut Server, client: ClientId| {
                let said = match crate::repos::set_repo(&repo, &field, value) {
                    Ok(()) => format!(
                        "Saved {repo}'s {field} in {}",
                        crate::files::shown("repos.json")
                    ),
                    Err(e) => format!("Not saved: {e}"),
                };
                server.mission.own = None;
                server.mission_note(client, said);
            };
            // Autopilot landing there without asking would start taking
            // its issues, closing each it lands unasked: asked first, as
            // starting it on them is.
            if ask_first {
                let (question, list) = crate::autopilot::line::issues_question();
                server.ask(
                    client,
                    crate::ui::Confirm::new(question).list(list),
                    move |server, client, yes| {
                        if yes {
                            write(server, client);
                        } else {
                            server.mission.own = None;
                            server.mission_note(client, "Not saved".to_string());
                        }
                    },
                );
            } else {
                write(server, client);
            }
        };
        let name = r["repo"].as_str().unwrap_or("").to_string();
        match r["field"].as_str().unwrap_or("") {
            field @ ("landing" | "tickets" | "review" | "testIn") => {
                let landing = field == "landing";
                let options: Vec<(&str, &str)> = match field {
                    "landing" => vec![
                        ("automatic", "Automatic: GitHub's rules, else its CLAUDE.md"),
                        ("pr", "Through a PR"),
                        ("direct", "Straight on the default branch"),
                    ],
                    "tickets" => vec![
                        (
                            "unset",
                            "Not set: /shape, wt <number>, M-i and autopilot refuse",
                        ),
                        ("github", "GitHub issues"),
                        ("linear", "Linear"),
                        ("notes", "A note's checklist, with a plan note each"),
                    ],
                    "testIn" => vec![
                        ("here", "Here, in the worktree"),
                        ("ci", "In CI, on a ci/ branch of the landing's own"),
                    ],
                    _ => crate::repos::REVIEWS.iter().map(|r| (*r, *r)).collect(),
                };
                let at = options
                    .iter()
                    .position(|(v, _)| now.as_str() == Some(v))
                    .unwrap_or(0);
                let values: Vec<String> = options.iter().map(|(v, _)| v.to_string()).collect();
                let title = if landing {
                    format!("{name}: how its work lands")
                } else if field == "tickets" {
                    format!("{name}: where its work is kept")
                } else if field == "testIn" {
                    format!("{name}: where the tests run")
                } else {
                    format!("{name}: the review before the push")
                };
                let menu = Menu::new(
                    title,
                    options.iter().map(|(_, l)| MenuOption::new(*l)).collect(),
                )
                .at(at);
                self.choose(client, menu, move |server, client, chose| {
                    if let Some(v) = chose.and_then(|i| values.get(i)) {
                        save(server, client, Value::String(v.clone()));
                    }
                });
            }
            "squash" => {
                let on = now.as_bool().unwrap_or(true);
                let said = match crate::repos::set_repo(&name, "squash", Value::Bool(!on)) {
                    Ok(()) => format!(
                        "Saved {name}'s squash in {}",
                        crate::files::shown("repos.json")
                    ),
                    Err(e) => format!("Not saved: {e}"),
                };
                self.mission.own = None;
                m.note = Some(said);
            }
            field => {
                let title = match field {
                    "issueLabel" => format!("{name}: the label autopilot takes issues by"),
                    "test" => format!("{name}: the tests"),
                    "afterRebase" => format!("{name}: the check after a clean rebase"),
                    _ => format!("{name}: the command before the push"),
                };
                let about = if field == "issueLabel" {
                    "One GitHub label, with no quotes; empty for none, so autopilot takes no issues."
                } else {
                    "A shell command run in the worktree; empty for none."
                };
                let ask = Ask::new(title, about)
                    .text(now.as_str().unwrap_or("").to_string())
                    .empty_ok();
                self.open(
                    client,
                    Box::new(ask),
                    Some(Box::new(move |server: &mut Server, client, outcome| {
                        if let Outcome::Picked(p) = outcome {
                            save(server, client, Value::String(p.value.trim().to_string()));
                        }
                    })),
                );
            }
        }
    }

    /// Each GitHub repository the tabs are in, once, with a directory of it.
    fn tab_repos(&mut self) -> Vec<(String, PathBuf)> {
        let mut out: Vec<(String, PathBuf)> = Vec::new();
        for tab in self.source_tabs() {
            if let (Some(repo), Some(path)) = (tab.repo, tab.path)
                && !out
                    .iter()
                    .any(|(r, _)| r.eq_ignore_ascii_case(&repo.to_string()))
            {
                out.push((repo.to_string(), path));
            }
        }
        out
    }

    /// Going to Settings works out, in the background, how each repository
    /// a tab is in lands, for its Landing rows, where neither repos.json
    /// nor the last day says; only with the github setting on, since it asks
    /// GitHub first, and each repository at most every `LANDING_RETRY`.
    fn work_out_landing(&mut self) {
        if !settings::is_on("github") || tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        let data = crate::repos::read_safely();
        let cache = crate::repos::cache_file();
        let now = util::now_ms();
        for (repo, dir) in self.tab_repos() {
            let set = crate::repos::landing_entry(&data, &repo).contains_key("landing");
            let fresh = crate::repos::cached_in(&cache, &repo)
                .is_some_and(|c| now - c.at < crate::repos::DAY_MS);
            let asked = self
                .mission
                .landing_asked
                .get(&repo)
                .is_some_and(|at| at.elapsed() < LANDING_RETRY);
            if set || fresh || asked {
                continue;
            }
            self.mission
                .landing_asked
                .insert(repo.clone(), Instant::now());
            tokio::task::spawn_blocking(move || {
                if let Err(e) = crate::repos::landing_for(&dir, false) {
                    eprintln!("landing: {repo}: {e}");
                }
            });
        }
    }

    /// Settings' "Your CLAUDE.md": where your rules come from, the local
    /// file, another file or a git repository, the last two typed in;
    /// written to claude.json, then setup's claude-md part run (the daemon's
    /// `claudeMdChoices` and `setClaudeMd`).
    fn change_rules(&mut self, client: ClientId, dir: PathBuf) {
        let menu = Menu::new(
            "Where your CLAUDE.md's rules come from",
            vec![
                MenuOption::new(format!("Local file, {}", crate::files::shown("CLAUDE.md"))),
                MenuOption::new("Another file…"),
                MenuOption::new("A git repository…"),
            ],
        );
        self.choose(client, menu, move |server, client, chose| {
            let (title, hint, repo) = match chose {
                Some(0) => {
                    server.save_rules(client, &dir, None, None);
                    return;
                }
                Some(1) => (
                    "Your CLAUDE.md: a file",
                    "The path of the file that holds your rules (~ for your home).",
                    false,
                ),
                Some(2) => (
                    "Your CLAUDE.md: a repository",
                    "Its URL, as git clone takes it. Your rules are its CLAUDE.md, at its root or in a folder; it is cloned into ~/.local/share/switchboard/claude-md and pulled each time setup runs.",
                    true,
                ),
                _ => return,
            };
            let ask = Ask::new(title, hint);
            server.open(
                client,
                Box::new(ask),
                Some(Box::new(move |server: &mut Server, client, outcome| {
                    let Outcome::Picked(p) = outcome else {
                        return;
                    };
                    let typed = p.value.trim().to_string();
                    if typed.is_empty() {
                        return;
                    }
                    if repo {
                        server.save_rules(client, &dir, Some(&typed), None);
                    } else {
                        server.save_rules(client, &dir, None, Some(&typed));
                    }
                })),
            );
        });
    }

    fn save_rules(&mut self, client: ClientId, dir: &Path, repo: Option<&str>, path: Option<&str>) {
        self.mission.own = None;
        match crate::usage::own::write_rules(dir, repo, path) {
            Ok(()) => {
                let (_, words) = crate::usage::own::rules_source(
                    dir,
                    &crate::usage::own::read_own(dir, "claude.json").unwrap_or_default(),
                );
                self.claude_md_setup(client, format!("Your CLAUDE.md: {words}"));
            }
            Err(e) => self.mission_note(client, format!("Not saved: {e}")),
        }
    }

    /// Setup's claude-md part, run after claude.json changed, which clones a
    /// repository named and writes ~/.claude/CLAUDE.md to import the rules;
    /// `said` is said with what it changed, or why it couldn't finish.
    fn claude_md_setup(&mut self, client: ClientId, said: String) {
        self.mission_busy(client, "Writing your CLAUDE.md", None, async move {
            let result = tokio::task::spawn_blocking(crate::setup::run_claude_md)
                .await
                .unwrap_or_else(|e| Err(e.to_string()));
            Some(match result {
                Ok(changed) if changed.is_empty() => said,
                Ok(changed) => format!("{said} ({changed})"),
                Err(why) => format!("{said}; setup couldn't finish: {why}"),
            })
        });
    }

    /// Setup's `--move`, run after "Load Switchboard as a plugin" changed,
    /// which moves Switchboard's hooks, skills and instructions into its
    /// plugin or back into ~/.claude (src/setup/way.rs); `said` is said with
    /// where they are now, or why it couldn't finish. Panes started from now
    /// on follow the setting; a Claude already running keeps what it loaded,
    /// and a shell already open the environment it started with, which
    /// setup's last line says how to take (`way::OPEN_TABS`).
    fn claude_plugin_setup(&mut self, client: ClientId, said: String) {
        self.mission_busy(client, "Moving Switchboard over", None, async move {
            let result = tokio::task::spawn_blocking(crate::setup::run_move)
                .await
                .unwrap_or_else(|e| Err(e.to_string()));
            Some(match result {
                Ok(changed) if changed.is_empty() => said,
                Ok(changed) => format!("{said}. {changed}"),
                Err(why) => format!("{said}; setup couldn't finish: {why}"),
            })
        });
    }

    // ── Worktrees ───────────────────────────────────────────────────────────

    /// A merged worktree removed: `git worktree remove` without force, so
    /// one with changes not committed, or files git doesn't track, is kept
    /// and git's reason said.
    fn remove_worktrees(&mut self, client: ClientId, dirs: Vec<PathBuf>) {
        self.mission_busy(
            client,
            "Working on it",
            Some(After::Worktrees),
            async move {
                tokio::task::spawn_blocking(move || {
                    let said: Vec<String> = dirs.iter().map(|d| remove_one_worktree(d)).collect();
                    Some(said.join(" "))
                })
                .await
                .unwrap_or(None)
            },
        );
    }

    /// Every merged tab closed and its worktree removed, asked first: one
    /// with changes not committed is kept and said.
    fn close_merged(&mut self, client: ClientId, list: Vec<TabId>) {
        let all = self.sidebar_all_tabs();
        let shown: Vec<String> = all
            .iter()
            .filter(|t| list.contains(&t.id))
            .map(|t| format!("{} {}", t.index, t.name))
            .collect();
        let n = list.len();
        let question = format!(
            "Close {n} merged tab{} and remove their worktrees?",
            if n == 1 { "" } else { "s" }
        );
        self.ask(
            client,
            Confirm::new(question).list(shown),
            move |server, client, yes| {
                if !yes {
                    return;
                }
                // A tab that left Merged while the question was open (back at
                // work, pinned, waiting on you) stays open.
                let rules = crate::sidebar::model::Rules::now();
                let still_merged: Vec<TabId> = server
                    .sidebar_all_tabs()
                    .iter()
                    .filter(|t| {
                        list.contains(&t.id)
                            && crate::sidebar::model::own_stage(t, &rules)
                                == crate::sidebar::model::Stage::Merged
                    })
                    .map(|t| t.id)
                    .collect();
                let list = still_merged;
                if list.is_empty() {
                    server.say(
                        Some(client),
                        "No merged tab is left to close: each left Merged.",
                    );
                    return;
                }
                let n = list.len();
                let paths: Vec<PathBuf> = list
                    .iter()
                    .filter_map(|t| server.tab_path(*t).map(Path::to_path_buf))
                    .collect();
                for t in &list {
                    server.kill_tab(*t);
                }
                // What the tabs left open use (a second tab M-t opened in the
                // same worktree, still at work), which stays.
                let open = server.sweep_open();
                server.mission_busy(
                    client,
                    "Working on it",
                    Some(After::Worktrees),
                    async move {
                        let kept = tokio::task::spawn_blocking(move || {
                            let mut kept = Vec::new();
                            for path in paths {
                                let Some((main, path)) = merged_worktree(&path) else {
                                    continue;
                                };
                                if crate::worktree_sweep::is_open(&path, &open) {
                                    continue;
                                }
                                let mut git = std::process::Command::new("git");
                                git.arg("-C").arg(main).arg("worktree");
                                if path.exists() {
                                    git.arg("remove").arg(&path);
                                } else {
                                    git.arg("prune");
                                }
                                if util::output_of(&mut git).is_err() {
                                    kept.push(
                                        path.file_name()
                                            .map(|f| f.to_string_lossy().into_owned())
                                            .unwrap_or_default(),
                                    );
                                }
                            }
                            kept
                        })
                        .await
                        .unwrap_or_default();
                        let mut said =
                            format!("Closed {n} merged tab{}", if n == 1 { "" } else { "s" });
                        if !kept.is_empty() {
                            said.push_str(&format!(
                                "; kept the worktree{} with uncommitted changes: {}",
                                if kept.len() == 1 { "" } else { "s" },
                                kept.join(", ")
                            ));
                        }
                        said.push('.');
                        Some(said)
                    },
                );
            },
        );
    }
}

/// A merged tab's worktree and the directory its removal is run from,
/// asked of git while it is there, wherever it was made (a repository's
/// `.claude/worktrees`, a folder `wt` was given, the demo's
/// `~/code/worktrees`) and wherever its git directory is (a clone made with
/// `--separate-git-dir` too): the repository's shared git directory, which
/// `git worktree remove` runs from, and the top of the worktree the tab is
/// in; else, gone already, from a `.claude/worktrees` path, which is pruned.
/// `None` for a tab in the main clone, whose git directory is the shared
/// one, so it is never removed. Runs git, so only off the server's task.
fn merged_worktree(path: &Path) -> Option<(PathBuf, PathBuf)> {
    if path.exists() {
        let out = util::output_of(std::process::Command::new("git").arg("-C").arg(path).args([
            // No --path-format=absolute, which git before 2.31 lacks: a
            // relative answer is resolved against the folder asked.
            "rev-parse",
            "--show-toplevel",
            "--git-dir",
            "--git-common-dir",
        ]))
        .ok()?;
        let text = String::from_utf8_lossy(&out.stdout);
        let real = |p: &str| {
            let p = path.join(p.trim());
            p.canonicalize().unwrap_or(p)
        };
        let mut lines = text.lines();
        let (root, own, common) = (
            real(lines.next()?),
            real(lines.next()?),
            real(lines.next()?),
        );
        return (own != common).then_some((common, root));
    }
    let text = path.to_string_lossy();
    text.split_once("/.claude/worktrees/")
        .map(|(main, _)| (PathBuf::from(main), path.to_path_buf()))
}

/// An error's first line, for a note.
fn first_line(e: &anyhow::Error) -> String {
    e.to_string().lines().next().unwrap_or("").to_string()
}

/// A view's title before a picker's scope was added to it, as the habits
/// count it.
fn base_title(v: &View) -> String {
    v.title.split(" · ").next().unwrap_or(&v.title).to_string()
}

// ── The mouse ───────────────────────────────────────────────────────────────

/// What the server does after the mouse has changed mission control.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Then {
    Nothing,
    Redraw,
    Close,
    /// The wheel, moving as a key does.
    Key(Key),
    /// A second click on the chosen one: Enter.
    Enter,
    /// A footer's key clicked: pressed as it is, a filter being typed
    /// taking it as it would the key.
    Press(Key),
    /// A view or the cursor changed: shown again.
    Prepare,
    /// A picture in a Slack thread clicked: shown.
    SlackPlay,
    /// Part `n` of the chosen row's switch clicked: set.
    Part(usize),
}

/// What a count with no upstream watches: the repository's config file.
const CONFIG: &str = "config";

/// Where what a count watches for a push is now: when the repository's
/// config file last changed, which a push setting an upstream or a changed
/// upstream writes, and the commit its upstream's ref is at, read from
/// git's files (none for `CONFIG`, no upstream).
fn watched_at(tree: &Path, watched: &str) -> String {
    let config = crate::tabs::git::locate(tree)
        .and_then(|p| std::fs::metadata(p.common_dir.join("config")).ok())
        .and_then(|m| m.modified().ok())
        .map(|t| format!("{t:?}"))
        .unwrap_or_default();
    let at = match watched {
        "" | CONFIG => String::new(),
        r => crate::tabs::git::ref_at(tree, r).unwrap_or_default(),
    };
    format!("{config} {at}")
}

/// A worktree's branch counted at `head`: commits ahead of the default
/// branch (on origin when it is there) and not on its upstream, with git;
/// what git can't count is `None`, kept as the answer for that commit.
fn count_branch(tree: &Path, head: String) -> views::Counts {
    let counted = count_with_git(tree);
    let upstream = counted.as_ref().map(|c| c.3.clone()).unwrap_or_default();
    views::Counts {
        head,
        default: counted.as_ref().map(|c| c.0.clone()).unwrap_or_default(),
        ahead: counted.as_ref().map(|c| c.1),
        to_push: counted.as_ref().and_then(|c| c.2),
        upstream_at: watched_at(tree, &upstream),
        upstream,
    }
}

/// The default branch, the commits ahead of it, those not on the upstream
/// (`None` with none), and what to watch for a push: the upstream's ref as
/// git names it, however the config says it, else the repository's config
/// (`CONFIG`), which a push setting one writes.
fn count_with_git(tree: &Path) -> Option<(String, u32, Option<u32>, String)> {
    let git = |args: &[&str]| -> Option<String> {
        let out = util::output_of(
            std::process::Command::new("git")
                .arg("-C")
                .arg(tree)
                .args(args),
        )
        .ok()?;
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
    };
    let place = crate::tabs::git::locate(tree)?;
    let default = crate::tabs::git::default_branch(&place.common_dir)?;
    let against = [format!("origin/{default}"), default.clone()]
        .into_iter()
        .find(|r| git(&["rev-parse", "--verify", "--quiet", r]).is_some())?;
    let right = |range: &str| -> Option<u32> {
        let out = git(&["rev-list", "--left-right", "--count", range])?;
        out.split_whitespace().nth(1)?.parse().ok()
    };
    let ahead = right(&format!("{against}...HEAD"))?;
    // With no upstream, the repository's config alone is watched, which the
    // push that sets one writes, whatever remote it goes to.
    let upstream =
        git(&["rev-parse", "--symbolic-full-name", "@{upstream}"]).unwrap_or_else(|| CONFIG.into());
    Some((default, ahead, right("@{upstream}...HEAD"), upstream))
}

/// Whether `key` is a pane resize key taken by the pane beside a board,
/// whose left edge it moves as it moves a divider: left widening it, right
/// narrowing it, nothing at its least or most. With no pane there, the key
/// goes on as any other, as it did before there was one.
fn resize_key(bindings: &crate::mux::keys::Bindings, m: &mut Mission, key: Key) -> bool {
    let Some(crate::mux::keys::Action::Resize(dir, cells)) = bindings.root.get(&key) else {
        return false;
    };
    if m.side_shown().is_none() || !settings::is_on("resize_side_panes") {
        return false;
    }
    let cells = *cells as isize;
    match dir {
        crate::mux::Direction::Left => m.resize_side(-cells),
        crate::mux::Direction::Right => m.resize_side(cells),
        _ => false,
    };
    true
}

/// The columns a pane resize key left or right moves the sidebar's edge by
/// while mission control's list of views is drawn in the sidebar's place
/// and has the keys; none when `key` is no such key, the list is a column
/// of its own or the keys are in the view, or Settings' `sidebar_resize`
/// is off.
fn nav_resize_key(bindings: &crate::mux::keys::Bindings, m: &Mission, key: Key) -> Option<i32> {
    use crate::mux::Direction;
    let Some(crate::mux::keys::Action::Resize(dir @ (Direction::Left | Direction::Right), cells)) =
        bindings.root.get(&key)
    else {
        return None;
    };
    // A row of it chosen, or its filter being typed, as either has the keys.
    // Only a chord with Alt, as the default resize keys are, or a function
    // key: the list's own keys (j, k, C-u, the arrows, a character typed in
    // its filter, a C-w's direction) have neither, and stay the list's
    // whatever keys.json binds to them.
    // Up and Down are the list's whatever their mods (M-Up moves through
    // it); Left and Right with Alt it has no use for.
    let alt_letter =
        key.mods.alt && matches!(key.code, KeyCode::Char(_) | KeyCode::Left | KeyCode::Right);
    if !alt_letter && !matches!(key.code, KeyCode::F(_)) {
        return None;
    }
    let typing = m.nav_filter.as_ref().is_some_and(|f| f.typing);
    let list_has_the_keys = typing || matches!(m.focus, Some(Focus::View(_) | Focus::Group(_)));
    if !m.nav_in_sidebar() || !list_has_the_keys || !settings::is_on("sidebar_resize") {
        return None;
    }
    Some(i32::from(*cells) * if *dir == Direction::Right { 1 } else { -1 })
}

/// The pane beside a board's width kept in its file once it was resized and
/// isn't being dragged.
fn save_side(m: &mut Mission) {
    if m.side_drag.is_some() {
        return;
    }
    if let Some(key) = m.side_unsaved.take()
        && let Some(&width) = m.side_widths.get(&key)
    {
        super::save_side_width(&key, width);
    }
}

/// A mouse event on mission control, against where its parts were last
/// drawn: the mouse moving lights what a click would press (not the chosen
/// view or row, nor anything on the screen's edge, the way out of it); the
/// wheel moves card by card on the tabs, row by row elsewhere; a click on
/// a view's name goes there, on a card or row chooses it, and on the
/// chosen one does what Enter does.
fn mouse_in(m: &mut Mission, mouse: Mouse, hover: bool) -> Then {
    // A drag of the pane's bar ends at any press, so one let go over the
    // sidebar, which never reaches here, leaves no drag behind.
    if mouse.kind == MouseKind::Press {
        m.pane_bar_drag = None;
        m.side_drag = None;
    }
    let (cols, rows) = m.size;
    let hit = m
        .hits
        .iter()
        .find(|h| h.y == mouse.row && mouse.col >= h.x0 && mouse.col <= h.x1)
        .map(|h| h.spot);
    match (mouse.kind, mouse.button) {
        (MouseKind::Move, _) => {
            if !hover {
                return Then::Nothing;
            }
            // The footer's keys are on the bottom edge, and light there.
            let edge = mouse.col == 0 || mouse.col + 1 >= cols || mouse.row + 1 >= rows;
            let footer_key = matches!(hit, Some(Spot::Key(_)));
            let lit = hit.filter(|_| footer_key || !edge).filter(|s| match s {
                Spot::Side(_) => false,
                Spot::Group(g) => first_of(m, *g) != Some(m.view),
                Spot::View(v) => *v != m.view,
                Spot::Index(i) => *i != m.cursor || m.chosen_check().is_some(),
                Spot::Check(i, n) => *i != m.cursor || m.chosen_check() != Some(*n),
                Spot::Cell(_) => false,
                _ => true,
            });
            if lit == m.hover {
                Then::Nothing
            } else {
                m.hover = lit;
                Then::Redraw
            }
        }
        (MouseKind::Press, MouseButton::WheelUp | MouseButton::WheelDown) => {
            if m.filter.as_ref().is_some_and(|f| f.typing) && !m.picker {
                return Then::Nothing;
            }
            let up = mouse.button == MouseButton::WheelUp;
            // The pane beside scrolls under the wheel, a keycap on it too.
            let side = m
                .hits
                .iter()
                .filter(|h| h.y == mouse.row && mouse.col >= h.x0 && mouse.col <= h.x1)
                .find_map(|h| match h.spot {
                    Spot::Side(last) => Some(last),
                    _ => None,
                });
            if let Some(last) = side {
                let lines = if up { -SIDE_WHEEL } else { SIDE_WHEEL };
                return if m.scroll_side(lines, last) {
                    Then::Redraw
                } else {
                    Then::Nothing
                };
            }
            // The wheel over the Slack board's sections walks them, which
            // take the focus, as a click on one gives it.
            if slack_sections_at(m, mouse.col)
                && let Some(v) = m.current()
            {
                let by = if up { -1 } else { 1 };
                m.cursor = super::slack_board::section_step(&v.data.items, m.cursor, by);
                m.pane = Some((m.view, Pane::Panel));
                return Then::Prepare;
            }
            // The wheel over the list moves the list, which takes the
            // focus from another pane, as a click on it would.
            m.pane = None;
            // The wheel stops at a list's ends, where j and k wrap (the
            // Notifications view's), as it does in every menu.
            let to = row_step(m, if up { -1 } else { 1 });
            let would_wrap = m.view != 0 && if up { to > m.cursor } else { to < m.cursor };
            if would_wrap {
                return Then::Nothing;
            }
            let key = match (m.view == 0, up) {
                (true, true) => 'h',
                (true, false) => 'l',
                (false, true) => 'k',
                (false, false) => 'j',
            };
            Then::Key(Key::new(KeyCode::Char(key), Mods::NONE))
        }
        (MouseKind::Press, MouseButton::Right) => right_press(m, mouse, hit),
        // Dragging the pane's scroll bar, from a press on it: the pane
        // moves with the mouse up and down, whatever its column.
        (MouseKind::Drag, MouseButton::Left) if m.pane_bar_drag.is_some() => {
            // The bar's row the mouse is level with, past its ends too.
            let bar = m.hits.iter().find_map(|h| match h.spot {
                Spot::PaneBar { row, body, total } => {
                    let top = h.y.saturating_sub(row as u16);
                    Some((usize::from(mouse.row.saturating_sub(top)), body, total))
                }
                _ => None,
            });
            match bar {
                Some(bar) if m.side_bar_to(bar, m.pane_bar_drag) => Then::Redraw,
                _ => Then::Nothing,
            }
        }
        // Dragging the pane's edge, from a press on it: the pane's left
        // edge follows the mouse's column.
        (MouseKind::Drag, MouseButton::Left) if m.side_drag.is_some() => {
            let right = m.side_drag.unwrap_or_default();
            let width = usize::from(right.saturating_sub(mouse.col));
            if m.set_side_width(width) {
                Then::Redraw
            } else {
                Then::Nothing
            }
        }
        (MouseKind::Release, _) if m.side_drag.is_some() => {
            m.side_drag = None;
            Then::Redraw
        }
        (MouseKind::Release, _) if m.pane_bar_drag.is_some() => {
            m.pane_bar_drag = None;
            Then::Nothing
        }
        (MouseKind::Press, MouseButton::Left) => {
            let Some(spot) = hit else {
                return Then::Nothing;
            };
            // A key on the footer is pressed as typed, the focus kept for it.
            if let Spot::Key(key) = spot {
                m.hover = None;
                // The click is the key, never the one Tab's focus is on.
                m.focus = None;
                // A key acts on the chosen row or the view: a filter being
                // typed is kept, done typing, rather than given the key; the
                // filter's own footer (the arrows move, Enter keeps it, esc
                // clears it) takes its key as typed, the filter typing on.
                let filters_own = m.filter.as_ref().is_some_and(|f| f.typing)
                    && matches!(
                        key.code,
                        KeyCode::Enter | KeyCode::Esc | KeyCode::Up | KeyCode::Down
                    );
                if !filters_own && let Some(f) = m.filter.as_mut() {
                    f.typing = false;
                }
                return Then::Press(key);
            }
            // A click puts away the focus ring Tab draws, and the light.
            m.focus = None;
            m.hover = None;
            // A click in one of the view's panes gives it the focus, as
            // `C-w` and a direction do.
            let pane = match spot {
                // A picture may be the list's thumbnail or the thread's, so
                // it leaves the focus where it is.
                Spot::Side(_) | Spot::PaneBar { .. } => Some(Pane::Beside),
                Spot::Category(_) => Some(Pane::Panel),
                Spot::Choose(_) if slack_sections_at(m, mouse.col) => Some(Pane::Panel),
                Spot::Index(_)
                | Spot::Menu(_)
                | Spot::Part(..)
                | Spot::Check(..)
                | Spot::Choose(_) => Some(Pane::List),
                _ => None,
            };
            if let Some(pane) = pane {
                m.pane = Some((m.view, pane));
                // Another pane takes the keys from a filter being typed,
                // its words kept.
                if pane != Pane::List
                    && let Some(f) = m.filter.as_mut()
                {
                    f.typing = false;
                }
            }
            match spot {
                Spot::Close => Then::Close,
                Spot::Filter => {
                    let text = m.filter.take().map(|f| f.text).unwrap_or_default();
                    m.filter = Some(Filter { text, typing: true });
                    // What the filter finds is the list's, which takes the focus.
                    m.pane = None;
                    Then::Prepare
                }
                Spot::Group(g) if first_of(m, g) != Some(m.view) => {
                    open_group(m, g);
                    Then::Prepare
                }
                Spot::Group(_) => Then::Redraw,
                Spot::Cell(cell) => {
                    m.focus_cell(cell);
                    Then::Prepare
                }
                Spot::View(v) if v != m.view => {
                    show_view(m, v);
                    Then::Prepare
                }
                Spot::View(_) => Then::Redraw,
                Spot::Index(i) if i == m.cursor && m.chosen_check().is_none() => {
                    // The Reminders view's chosen day goes to the reminder
                    // clicked, which Enter then acts on.
                    follow_reminder(m);
                    Then::Enter
                }
                Spot::Check(i, n) if i == m.cursor && m.chosen_check() == Some(n) => Then::Enter,
                Spot::Check(i, n) => {
                    m.cursor = i;
                    choose_check(m, Some(n));
                    Then::Prepare
                }
                Spot::Key(k) => Then::Press(k),
                // A row's menu button or a setting's switch: chosen and
                // opened or flipped in one click, a filter being typed
                // ending first so Enter reaches the row.
                Spot::Menu(i) => {
                    if let Some(f) = m.filter.as_mut() {
                        f.typing = false;
                    }
                    m.cursor = i;
                    Then::Enter
                }
                Spot::Part(i, n) => {
                    if let Some(f) = m.filter.as_mut() {
                        f.typing = false;
                    }
                    m.cursor = i;
                    Then::Part(n)
                }
                Spot::Index(i) => {
                    m.cursor = i;
                    choose_check(m, None);
                    follow_reminder(m);
                    Then::Prepare
                }
                Spot::Day(day) => {
                    choose_day(m, day);
                    Then::Prepare
                }
                Spot::Side(_) | Spot::InPlace => Then::Redraw,
                Spot::SideEdge(width) => {
                    m.side_drag = Some(mouse.col + width as u16);
                    Then::Redraw
                }
                Spot::PaneBar { row, body, total } => {
                    m.pane_bar_drag = Some(m.side_bar_grab((row, body, total)));
                    if m.side_bar_to((row, body, total), None) {
                        Then::Redraw
                    } else {
                        Then::Nothing
                    }
                }
                Spot::Kind(key) => {
                    choose_kind(m, key);
                    Then::Prepare
                }
                Spot::Category(i) => {
                    m.cursor = i;
                    Then::Prepare
                }
                Spot::Choose(i) => {
                    m.cursor = i;
                    if m.focus.is_some() {
                        m.focus = Some(Focus::Item);
                    }
                    Then::Prepare
                }
                // A picture clicked in the thread shows, a filter being
                // typed ending first.
                Spot::Picture(n) => {
                    if let Some(f) = m.filter.as_mut() {
                        f.typing = false;
                    }
                    m.slack_clicked = Some(n);
                    Then::SlackPlay
                }
            }
        }
        _ => Then::Nothing,
    }
}

/// A right press on mission control: on an item (a card, a row, a failed
/// check, a key of the Keys board) it is chosen and its menu opened, as
/// Enter on it opens it, also where a left click would act at once; where
/// nothing clickable is in a view's area (the pane beside included), the
/// view's menu, as `m` opens it; on the bar, the footer and anything else
/// that takes a click, nothing.
fn right_press(m: &mut Mission, mouse: Mouse, hit: Option<Spot>) -> Then {
    let (_, rows) = m.size;
    let bar_ends = m
        .hits
        .iter()
        .filter(|h| matches!(h.spot, Spot::Group(_) | Spot::View(_) | Spot::Close))
        .map(|h| h.y)
        .max();
    let on_bar = bar_ends.is_some_and(|y| mouse.row <= y);
    let on_footer = mouse.row + 1 >= rows;
    let chosen = match hit {
        Some(Spot::Index(i) | Spot::Menu(i) | Spot::Part(i, _) | Spot::Choose(i)) => {
            // The list takes the focus, so the Enter that opens its menu
            // reaches it rather than a focused pane.
            m.pane = Some((m.view, Pane::List));
            m.cursor = i;
            choose_check(m, None);
            follow_reminder(m);
            true
        }
        Some(Spot::Check(i, n)) => {
            m.pane = Some((m.view, Pane::List));
            m.cursor = i;
            choose_check(m, Some(n));
            true
        }
        Some(Spot::Side(_)) | None if !on_bar && !on_footer => false,
        // A view shown at once with others but without the focus: given
        // it, as a left click gives it, and its menu opened, since its
        // rows take no click until it has the focus.
        Some(Spot::Cell(cell)) => {
            m.focus_cell(cell);
            false
        }
        _ => return Then::Nothing,
    };
    // As a left click does: the focus ring and the light put away, and a
    // filter being typed done typing and a view's name being typed after
    // `v` put away, so Enter or `m` reaches the view.
    m.focus = None;
    m.hover = None;
    m.jump = None;
    if let Some(f) = m.filter.as_mut() {
        f.typing = false;
    }
    if chosen {
        Then::Enter
    } else {
        Then::Press(Key::new(KeyCode::Char('m'), Mods::NONE))
    }
}

/// The Slack board's messages on screen, by their places in `view`: the
/// chosen section's, or with the thread in the list's place on a narrow
/// screen, the chosen message alone.
fn slack_shown(m: &Mission, view: &View) -> Vec<usize> {
    let thread_alone = m.slack_thread_open
        && settings::is_on("slack_threads")
        && !super::slack_board::beside(m.laid().0 as usize);
    if thread_alone {
        usize::try_from(m.cursor)
            .ok()
            .filter(|c| *c < view.data.items.len())
            .into_iter()
            .collect()
    } else {
        super::slack_board::shown(&view.data.items, m.cursor)
    }
}

/// Whether column `col` is the Slack board's sections column, its own
/// pane, in the view in front, where its sections were last drawn (a cell
/// of views shown at once starts where it is).
fn slack_sections_at(m: &Mission, col: u16) -> bool {
    let shown = m.current().is_some_and(super::render::slack_board_shown);
    shown
        && m.hits.iter().any(|h| {
            matches!(h.spot, Spot::Choose(_))
                && usize::from(h.x1 - h.x0) + 1 == super::slack_board::SECTIONS_W
                && (h.x0..=h.x1).contains(&col)
        })
}

/// The Keys board's moves: h and l between groups, j and k
/// between the group's keys (the arrows too). True when it took the key.
fn keys_board_key(m: &mut Mission, k: Key) -> bool {
    let Some(view) = m.current() else {
        return false;
    };
    if !super::render::keys_board_shown(m, view, m.laid().0 as usize) || !plain(&k) {
        return false;
    }
    let items = &view.data.items;
    let to = match k.code {
        KeyCode::Char('h') | KeyCode::Left => super::keys_board::group_step(items, m.cursor, -1),
        KeyCode::Char('l') | KeyCode::Right => super::keys_board::group_step(items, m.cursor, 1),
        KeyCode::Char('j') | KeyCode::Down => super::keys_board::row_step(items, m.cursor, 1),
        KeyCode::Char('k') | KeyCode::Up => super::keys_board::row_step(items, m.cursor, -1),
        _ => return false,
    };
    if let Some(to) = to {
        m.cursor = to as isize;
    }
    // As `move_to`: a move takes Tab's focus to the chosen row.
    if m.focus.is_some() {
        m.focus = Some(Focus::Item);
    }
    true
}

// ── Moving about ────────────────────────────────────────────────────────────

/// Chooses the chosen CI item's failed check `n`, or its own row with
/// none; the pane beside starts again where it shows that check's line
/// that says why.
fn choose_check(m: &mut Mission, n: Option<usize>) {
    let chosen = m.chosen_item().and_then(|item| {
        let key = views::ci_check_key(views::ci_checks(item).get(n?)?);
        Some((item.id.clone(), key))
    });
    let was = m.chosen_check();
    m.ci_check = chosen;
    if m.chosen_check() != was {
        m.side = None;
    }
}

/// The day `by` days from the one the Calendar shows (`None` is today):
/// `None` for today, else its date while the feed was read for it; `Err`
/// says why it can't be shown.
fn calendar_step(
    feed: &crate::day::calendar::Feed,
    shown: Option<&str>,
    by: i64,
    now: &jiff::Zoned,
) -> Result<Option<String>, String> {
    let today = now.date();
    let from = shown
        .and_then(|d| d.parse::<jiff::civil::Date>().ok())
        .unwrap_or(today);
    let to = from
        .checked_add(jiff::Span::new().days(by))
        .map_err(|e| e.to_string())?;
    if to == today {
        return Ok(None);
    }
    let date = to.to_string();
    crate::day::calendar::other_day_view(feed, &date, now)?;
    Ok(Some(date))
}

/// Whether the Reminders view is in front, drawn as its month.
/// Whether the Routines view is in front.
fn on_routines(m: &Mission) -> bool {
    m.current().is_some_and(|v| v.id == ViewId::Routines)
}

/// What the context-menu key (S-F10) presses in mission control, as a right
/// click opens: Enter, the chosen item's menu, or with none chosen m, the
/// view's. A filter or a view's name being typed ends first, as for the
/// palette, so the key isn't typed into it.
fn context_menu_key(m: &mut Mission) -> Key {
    m.jump = None;
    if let Some(f) = m.filter.as_mut() {
        f.typing = false;
    }
    super::nav::end_typing(m);
    // On a row of the list of views, that view's menu, as a right click on
    // the row opens it: the view brought to the front first.
    if let Some(Focus::View(v)) = m.focus {
        if v != m.view {
            show_view(m, v);
        }
        return Key::new(KeyCode::Char('m'), Mods::NONE);
    }
    let an_item_is_chosen = matches!(m.focus, None | Some(Focus::Item))
        && match m.current() {
            // The Reminders month chooses within its chosen day alone.
            Some(v) if on_reminders(m) => super::reminders::chosen(m, v).is_some(),
            Some(_) => m.chosen_item().is_some(),
            None => !m.order().is_empty(),
        };
    if an_item_is_chosen {
        // As a right click on the item, which gives the list the focus, so
        // the Enter reaches the item rather than Settings' categories.
        m.pane = None;
        Key::new(KeyCode::Enter, Mods::NONE)
    } else {
        Key::new(KeyCode::Char('m'), Mods::NONE)
    }
}

fn on_reminders(m: &Mission) -> bool {
    m.current().is_some_and(|v| v.id == ViewId::Reminders)
}

/// The Reminders view's chosen day moved to `day` (today's kept as
/// none, so the view follows the clock past midnight), and its first
/// reminder chosen.
fn choose_day(m: &mut Mission, day: jiff::civil::Date) {
    m.reminders_day = (day != super::reminders::today(m)).then_some(day);
    let Some(view) = m.current() else {
        return;
    };
    let tz = &m.snap.tz;
    if let Some(i) = view
        .data
        .items
        .iter()
        .position(|i| super::reminders::day_of(i, tz) == day)
    {
        m.cursor = i as isize;
    }
}

/// A reminder chosen by a click, Tab or a move takes the chosen day to its
/// own, so what acts on the chosen day's reminder acts on it.
fn follow_reminder(m: &mut Mission) {
    if !on_reminders(m) {
        return;
    }
    if let Some(day) = m
        .chosen_item()
        .map(|i| super::reminders::day_of(i, &m.snap.tz))
    {
        m.reminders_day = (day != super::reminders::today(m)).then_some(day);
    }
}

/// A CI item as its failed check `n` would be its only one: the check's
/// job, name and link in its place, so its menu reads that check's log
/// and opens that check.
fn with_check(mut item: Item, n: usize) -> Item {
    let Some(check) = views::ci_checks(&item).get(n).map(|c| (*c).clone()) else {
        return item;
    };
    if let Some(ci) = item.data.get_mut("ci").and_then(Value::as_object_mut) {
        let logged = check.get("step").is_some_and(Value::is_object);
        ci.insert(
            "job".into(),
            check.get("job").cloned().unwrap_or(Value::Null),
        );
        ci.insert(
            "jobName".into(),
            check.get("name").cloned().unwrap_or(Value::Null),
        );
        ci.insert("logged".into(), Value::Bool(logged));
        ci.insert("check".into(), check);
    }
    item
}

/// UX 2.0's Landing view's own key: `i` shows the chosen worktree's last
/// output over the list on a screen too narrow to show it under the rows,
/// until `i` or Esc. What acts on the worktree is in its menu, which Enter
/// opens. True when it took the key.
fn landing_key(m: &mut Mission, k: Key) -> bool {
    if !m.current().is_some_and(|v| v.id == ViewId::Landing) {
        return false;
    }
    if m.landing_output && (k.code == KeyCode::Esc || (k.code == KeyCode::Char('i') && plain(&k))) {
        m.landing_output = false;
        return true;
    }
    let narrow = (m.laid().0 as usize) < super::render::LANDING_OUTPUT_AT;
    let has_output = m
        .chosen_item()
        .is_some_and(|i| i.data["landing"]["record"]["output"].is_object());
    if k.code == KeyCode::Char('i') && plain(&k) && narrow && has_output {
        m.landing_output = true;
        return true;
    }
    false
}

/// A worktree removed with `git worktree remove` from its main clone,
/// which keeps one with changes not committed or files git doesn't track;
/// what became of it, in words.
fn remove_one_worktree(dir: &Path) -> String {
    let name = dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let Some((_, common)) = crate::sources::landing::git_of(dir) else {
        return format!("Kept {name}: not a git worktree.");
    };
    let main = common.parent().map(Path::to_path_buf).unwrap_or(common);
    match util::output_of(
        std::process::Command::new("git")
            .arg("-C")
            .arg(&main)
            .args(["worktree", "remove"])
            .arg(dir),
    ) {
        Ok(_) => format!("Removed {name}."),
        Err(e) => {
            let why = e.to_string();
            if why.contains("modified or untracked files") {
                return format!("Kept {name}: it has changes not committed.");
            }
            let why = why.split_once(": ").map_or(why.as_str(), |(_, w)| w);
            format!("Kept {name}: {}", why.trim_start_matches("fatal: "))
        }
    }
}

/// UX 2.0's Reopen view's own key: `i` shows the chosen row's pane (what
/// reopening brings back, its last screen) over the list on a screen too
/// narrow to show it under the rows, until `i` or Esc. True when it took
/// the key.
fn reopen_key(m: &mut Mission, k: Key) -> bool {
    if !m.current().is_some_and(|v| v.id == ViewId::Reopen) {
        return false;
    }
    if m.reopen_pane && (k.code == KeyCode::Esc || (k.code == KeyCode::Char('i') && plain(&k))) {
        m.reopen_pane = false;
        return true;
    }
    let narrow = (m.laid().0 as usize) < super::reopen_board::PANE_AT;
    let on_a_row = m
        .chosen_item()
        .is_some_and(|i| crate::usage::reopen::of(i).is_some());
    if k.code == KeyCode::Char('i') && plain(&k) && narrow && on_a_row {
        m.reopen_pane = true;
        return true;
    }
    false
}

/// The Calendar's own keys, with its day drawn: `h` and `l` move a day
/// (`j` and `k` still walk every event), the days back or forward they
/// move. What acts on the chosen event is in its menu, which Enter opens.
fn calendar_key(m: &Mission, k: Key) -> Option<i64> {
    // A day shown as a list (an empty one the feed failed since, today or
    // another) still moves a day, or there would be no way past it.
    m.current()
        .filter(|v| v.id == ViewId::Calendar && v.data.setup.is_none())?;
    if !plain(&k) {
        return None;
    }
    match k.code {
        KeyCode::Char('h') | KeyCode::Left => Some(-1),
        KeyCode::Char('l') | KeyCode::Right => Some(1),
        _ => None,
    }
}

/// What a key does in the CI view, before mission control's
/// own keys: `j` and `k` move through the PRs and their failed checks'
/// lines. What acts on the chosen check is in its menu, which Enter opens.
/// True when it took the key.
fn ci_key(m: &mut Mission, k: Key) -> bool {
    let Some(view) = m.current().filter(|v| v.id == ViewId::Ci) else {
        return false;
    };
    let count = view.data.items.len() as isize;
    let Some(item) = m.chosen_item().cloned() else {
        return false;
    };
    let checks: Vec<Value> = views::ci_checks(&item).into_iter().cloned().collect();
    let check = m.chosen_check();
    let none = plain(&k);
    let down = matches!(k.code, KeyCode::Char('j') | KeyCode::Down) && none
        || (k.code == KeyCode::Char('n') && k.mods == Mods::CTRL);
    let up = matches!(k.code, KeyCode::Char('k') | KeyCode::Up) && none
        || (k.code == KeyCode::Char('p') && k.mods == Mods::CTRL);
    if (down || up) && m.focus.is_some() {
        // Moving takes the focus Tab gave a name or the filter back to the
        // items, as moving between rows does.
        m.focus = Some(Focus::Item);
    }
    if down {
        match check {
            None if !checks.is_empty() => choose_check(m, Some(0)),
            Some(n) if n + 1 < checks.len() => choose_check(m, Some(n + 1)),
            _ => {
                // The last line of the last PR stays where it is.
                let was = m.cursor;
                let to = row_step(m, 1);
                move_to(m, to, count);
                if m.cursor != was {
                    choose_check(m, None);
                }
            }
        }
        return true;
    }
    if up {
        match check {
            Some(0) => choose_check(m, None),
            Some(n) => choose_check(m, Some(n - 1)),
            None => {
                let was = m.cursor;
                let to = row_step(m, -1);
                move_to(m, to, count);
                let last = m
                    .chosen_item()
                    .map(|i| views::ci_checks(i).len())
                    .filter(|_| m.cursor != was)
                    .and_then(|n| n.checked_sub(1));
                choose_check(m, last);
            }
        }
        return true;
    }
    false
}

/// The first place the cursor can be: -1, the line of the note worked
/// continuously, while it leads the tabs (src/autopilot); else 0.
/// In Settings the cursor rests on a setting, never on a category's
/// heading, which Enter does nothing on: opened, filtered or sent to the
/// top, it goes down to the first setting under the heading.
/// The spinners' frame by the clock, held behind a box until
/// BOXED_SPIN_MS has passed since they last turned.
fn spin_frame(m: &mut Mission, boxed: bool) -> usize {
    let now = util::now_ms();
    if boxed && now - m.spun_at < BOXED_SPIN_MS {
        m.snap.frame
    } else {
        m.spun_at = now;
        (now as u64 / SPINNER_MS) as usize
    }
}

/// The cursor kept within the view in front, off a group's heading.
fn fit_cursor(m: &mut Mission) {
    let count = match m.current() {
        Some(v) => v.data.items.len(),
        None => m.order().len(),
    } as isize;
    m.cursor = m.cursor.min(count - 1).max(floor(m));
    settle(m);
}

/// Whether a key only moves the cursor or Tab's focus (`mission_key_in`'s
/// h j k l, the arrows, C-n and C-p, g and G, Home and End, a tab's number,
/// Tab and Shift-Tab with tab_focus), which leaves the snapshot standing:
/// not while a filter or a view's name is typed, and not Enter or Space,
/// which press what the focus is on.
fn only_moves(m: &Mission, key: Key) -> bool {
    let k = key.normalized();
    if m.filter.as_ref().is_some_and(|f| f.typing) || m.jump.is_some() {
        return false;
    }
    // The Calendar's chosen event brings its photos and notes, and h and
    // l move a day, the Slack board's chosen message its thread, and the
    // Reminders view's keys move its day and act on its reminders, so each
    // takes its snapshot again.
    if on_reminders(m)
        || m.current()
            .is_some_and(|v| v.id == ViewId::Calendar || super::render::slack_board_shown(v))
    {
        return false;
    }
    // So does the Inbox's chosen notification, with its ticket, and the
    // Reopen board's chosen row, with its last screen.
    if m.current()
        .is_some_and(|v| inbox_board::shown(v) || super::reopen_board::shown(v))
    {
        return false;
    }
    let none = plain(&k);
    let ctrl = k.mods == Mods::CTRL;
    match k.code {
        KeyCode::Char('h' | 'j' | 'k' | 'l' | 'g') => none,
        KeyCode::Left | KeyCode::Right | KeyCode::Up | KeyCode::Down | KeyCode::Home => none,
        KeyCode::Char('n' | 'p') => ctrl,
        KeyCode::Char('G') | KeyCode::End => true,
        KeyCode::Tab | KeyCode::BackTab => m.snap.tab_focus,
        KeyCode::Char('1'..='9') => none && m.view == 0,
        _ => false,
    }
}

/// The tab of the chosen card on the Tabs view.
fn chosen_tab(m: &Mission) -> Option<TabId> {
    if m.view != 0 {
        return None;
    }
    let at = usize::try_from(m.cursor).ok()?;
    m.order().get(at).copied()
}

/// Whether the chosen place is a label of the shelf folded on a narrow
/// screen, which Enter opens.
fn shelf_label_chosen(m: &Mission) -> bool {
    if m.view != 0 || m.shelf_open {
        return false;
    }
    if super::render::across(m.laid().0 as usize) > 1 {
        return false;
    }
    chosen_tab(m).is_some_and(|tab| {
        m.snap
            .tabs
            .iter()
            .any(|t| t.id == tab && super::render::on_shelf_tab(t, &m.snap.rules))
    })
}

/// Whether the chosen place is the label of Other repos folded while the
/// sidebar's repository filter is on, which Enter opens.
fn repos_label_chosen(m: &Mission) -> bool {
    if m.view != 0 || !super::render::repos_folded(m) {
        return false;
    }
    chosen_tab(m).is_some_and(|tab| {
        m.snap.tabs.iter().any(|t| {
            t.id == tab
                && crate::sidebar::model::stage_of(t, &m.snap.rules)
                    == crate::sidebar::model::Stage::OtherRepos
        })
    })
}

fn settle(m: &mut Mission) {
    let Some(view) = m
        .current()
        .filter(|v| views::grouped(&v.id) || m.grid.is_some())
    else {
        return;
    };
    let items = &view.data.items;
    let at = m.cursor.max(0) as usize;
    let over = |i: &Item| m.passes_over(i);
    // The next row to rest on, else the one before (End on a last row
    // passed over).
    if items.get(at).is_some_and(over)
        && let Some(next) = (at..items.len())
            .find(|i| !over(&items[*i]))
            .or_else(|| (0..at).rev().find(|i| !over(&items[*i])))
    {
        m.cursor = next as isize;
    }
}

fn floor(m: &Mission) -> isize {
    if m.view == 0 && m.snap.pilot.is_some() {
        -1
    } else {
        0
    }
}

/// The cursor to `to`, kept within the view; moving takes the focus Tab
/// gave a view's name or the filter back to the items.
fn move_to(m: &mut Mission, to: isize, count: isize) {
    m.cursor = to.min(count - 1).max(floor(m));
    if m.focus.is_some() {
        m.focus = Some(Focus::Item);
    }
    follow_reminder(m);
}

/// j and k: in the tabs a row of cards, keeping the column as far as the
/// row reaches (a card in a shorter box spans several rows, so moving down
/// starts from the last row it is in and moving up from the first); in a
/// list a row. The Notifications view's list wraps, past its last row to
/// its first and back, as every menu does (`ui::menu::wrap_step`).
fn row_step(m: &Mission, by: isize) -> isize {
    if m.view != 0 {
        let mut to = m.cursor + by;
        // In Settings and Keys a heading is passed over, staying put
        // when only headings are left that way.
        if let Some(view) = m
            .current()
            .filter(|v| views::grouped(&v.id) || m.grid.is_some())
        {
            let items = &view.data.items;
            while to >= 0 && items.get(to as usize).is_some_and(|i| m.passes_over(i)) {
                to += by;
            }
            if to < 0 || to as usize >= items.len() {
                if view.id != ViewId::Notifications {
                    return m.cursor;
                }
                let rests = |i: &usize| !m.passes_over(&items[*i]);
                let wrapped = if by > 0 {
                    (0..items.len()).find(rests)
                } else {
                    (0..items.len()).rev().find(rests)
                };
                return wrapped.map_or(m.cursor, |i| i as isize);
            }
        }
        return to;
    }
    let g = super::render::tabs_grid(m, m.laid().0 as usize);
    // The line of the note worked continuously, above the first row: j goes
    // down to the first card, and k from the first row up to it.
    if m.cursor < 0 {
        return match g.rows.first().and_then(|r| r.first()) {
            Some(&first) if by > 0 => first as isize,
            _ => m.cursor,
        };
    }
    let cursor = m.cursor.max(0) as usize;
    let holding: Vec<usize> = g
        .rows
        .iter()
        .enumerate()
        .filter(|(_, r)| r.contains(&cursor))
        .map(|(i, _)| i)
        .collect();
    let Some(&at) = (if by > 0 {
        holding.last()
    } else {
        holding.first()
    }) else {
        return m.cursor;
    };
    let Some(next) = at.checked_add_signed(by).and_then(|i| g.rows.get(i)) else {
        return if by < 0 && floor(m) < 0 { -1 } else { m.cursor };
    };
    let column = g.rows[at].iter().position(|c| *c == cursor).unwrap_or(0);
    next[column.min(next.len() - 1)] as isize
}

/// The key after `C-w`: `h` (or `C-h`, ←) the pane at the left, `l` (or
/// `C-l`, →) the one at the right, `w` (or `C-w`) the next round and `W`
/// the one before; a direction with no pane that way does nothing. False
/// for any other key, which is then taken as itself.
fn window_key(m: &mut Mission, k: Key) -> bool {
    let either = plain(&k) || k.mods == Mods::CTRL;
    let (by, round) = match k.code {
        KeyCode::Char('h') | KeyCode::Left if either => (-1, false),
        KeyCode::Char('l') | KeyCode::Right if either => (1, false),
        KeyCode::Char('w') if either => (1, true),
        KeyCode::Char('W') if plain(&k) => (-1, true),
        KeyCode::Char('j') | KeyCode::Down if either => return section_step(m, 1),
        KeyCode::Char('k') | KeyCode::Up if either => return section_step(m, -1),
        _ => return false,
    };
    // The list of views is the leftmost stop, before the view's own
    // panes: its rows take the keys as Tab's focus gives them.
    let nav = 1;
    let on_nav = matches!(m.focus, Some(Focus::View(_) | Focus::Group(_)));
    let panes = m.panes();
    let stops = (nav + panes.len()) as isize;
    let at = if on_nav {
        0
    } else {
        (nav + panes.iter().position(|p| *p == m.pane_now()).unwrap_or(0)) as isize
    };
    let to = if round {
        Some((at + by).rem_euclid(stops))
    } else {
        Some(at + by).filter(|to| (0..stops).contains(to))
    };
    match to {
        // The view in front, or the first its filter leaves when it hides it.
        Some(0) if nav == 1 => {
            m.focus = Some(Focus::View(m.view));
            nav_focus_listed(m);
        }
        Some(to) => {
            m.pane = Some((m.view, panes[to as usize - nav]));
            // The pane takes the keys at once, from the list of views or a
            // control Tab's focus was on.
            if m.focus.is_some() {
                m.focus = Some(Focus::Item);
            }
        }
        None => {}
    }
    true
}

/// `C-w j` and `C-w k`: the cursor to the first item of the next or
/// previous section a view stacks in its list, staying put with none that
/// way; false, the key taken as itself, in a view of one section. Nothing
/// in a pane other than the list.
fn section_step(m: &mut Mission, by: isize) -> bool {
    let starts = m.sections();
    if starts.is_empty() {
        return false;
    }
    if m.pane_now() != Pane::List {
        return true;
    }
    let cursor = m.cursor.max(0) as usize;
    let at = starts.iter().rposition(|&s| s <= cursor).unwrap_or(0) as isize;
    let to = if cursor < starts[0] && by > 0 {
        0
    } else {
        at + by
    };
    if let Some(&start) = usize::try_from(to).ok().and_then(|to| starts.get(to)) {
        m.cursor = start as isize;
        if m.focus.is_some() {
            m.focus = Some(Focus::Item);
        }
    }
    true
}

/// The focused pane's own keys, where it isn't the list: in the Settings
/// board's categories `j` and `k` walk the categories, the options
/// following, and Enter goes back into the options (in the Slack board's
/// sections, the sections, their messages following, and Enter into them); in a pane beside, `j`
/// and `k` scroll it a line, `g` and `G` to its top and end. False for
/// any other key, which the view takes as it would.
fn pane_key(m: &mut Mission, k: Key) -> bool {
    let down = matches!(k.code, KeyCode::Char('j') | KeyCode::Down) && plain(&k)
        || k.code == KeyCode::Char('n') && k.mods == Mods::CTRL;
    let up = matches!(k.code, KeyCode::Char('k') | KeyCode::Up) && plain(&k)
        || k.code == KeyCode::Char('p') && k.mods == Mods::CTRL;
    match m.pane_now() {
        Pane::List => false,
        Pane::Panel => {
            let by = match k.code {
                _ if down => 1,
                _ if up => -1,
                KeyCode::Enter => {
                    m.pane = None;
                    return true;
                }
                KeyCode::Char(' ') if plain(&k) => {
                    m.pane = None;
                    return true;
                }
                _ => return false,
            };
            if let Some(view) = m.current() {
                m.cursor = if view.id == ViewId::Slack {
                    super::slack_board::section_step(&view.data.items, m.cursor, by)
                } else {
                    super::settings_board::side_step(&view.data.items, m.cursor, by)
                };
            }
            true
        }
        Pane::Beside => {
            let Some(last) = m.side_last() else {
                return false;
            };
            let lines = match k.code {
                _ if down => 1,
                _ if up => -1,
                KeyCode::Char('g') | KeyCode::Home if plain(&k) => -(last as isize),
                KeyCode::Char('G') | KeyCode::End => last as isize,
                _ => return false,
            };
            m.scroll_side(lines, last);
            true
        }
    }
}

/// h and l: in the tabs the card before or after; on a board the next
/// column with items in it, at the same row as far as it reaches.
fn side_step(m: &Mission, by: isize) -> isize {
    let Some(view) = m.current() else {
        return m.cursor + by;
    };
    if view.id == ViewId::Settings && super::render::settings_board_shown(m) {
        return super::settings_board::side_step(&view.data.items, m.cursor, by);
    }
    if super::render::pilot_board_shown(view, m.laid().0 as usize) {
        return pilot_side_step(m, &view.data.items, by);
    }
    let Some(columns) = view.columns() else {
        return m.cursor;
    };
    let counts: Vec<usize> = columns
        .iter()
        .map(|c| {
            view.data
                .items
                .iter()
                .filter(|i| i.group.as_deref() == Some(c))
                .count()
        })
        .collect();
    let starts: Vec<usize> = counts
        .iter()
        .scan(0, |sum, n| {
            let at = *sum;
            *sum += n;
            Some(at)
        })
        .collect();
    let cursor = m.cursor.max(0) as usize;
    let Some(at) = (0..columns.len())
        .rev()
        .find(|&i| cursor >= starts[i] && counts[i] > 0)
    else {
        return m.cursor;
    };
    let row = cursor - starts[at];
    // The PR and ticket boards draw their last group as a row
    // under the columns, where h and l go from card to card, h from its
    // first card to the columns.
    let in_week_row = matches!(view.id, ViewId::Prs | ViewId::Tickets) && at == columns.len() - 1;
    if in_week_row {
        let next = row as isize + by;
        if next >= 0 && (next as usize) < counts[at] {
            return m.cursor + by;
        }
        if by > 0 {
            return m.cursor;
        }
    }
    // A column its cell leaves out (a board's week row in a short cell) is
    // passed over as an empty one.
    let shown = |c: usize| {
        (starts[c]..starts[c] + counts[c]).any(|i| {
            view.data
                .items
                .get(i)
                .is_some_and(|item| !m.passes_over(item))
        })
    };
    let mut i = at as isize + by;
    while i >= 0 && (i as usize) < columns.len() {
        let c = i as usize;
        if counts[c] > 0 && shown(c) {
            return (starts[c] + row.min(counts[c] - 1)) as isize;
        }
        i += by;
    }
    m.cursor
}

/// h and l on the Autopilot board: the next column with cards in it, at
/// the same row as far as it reaches. Its items aren't in their columns'
/// order alone (headings lead them and the skipped), so each column
/// is found by its items' group.
fn pilot_side_step(m: &Mission, items: &[crate::items::Item], by: isize) -> isize {
    let columns: Vec<Vec<usize>> = crate::autopilot::view::COLUMNS
        .iter()
        .map(|c| {
            (0..items.len())
                .filter(|&i| items[i].group.as_deref() == Some(*c))
                .collect()
        })
        .collect();
    let cursor = m.cursor.max(0) as usize;
    let Some((at, row)) = columns
        .iter()
        .enumerate()
        .find_map(|(c, held)| Some((c, held.iter().position(|&i| i == cursor)?)))
    else {
        return m.cursor;
    };
    let mut c = at as isize + by;
    while c >= 0 && (c as usize) < columns.len() {
        let held = &columns[c as usize];
        if let Some(&last) = held.last() {
            return held.get(row).copied().unwrap_or(last) as isize;
        }
        c += by;
    }
    m.cursor
}

/// `]` and `[` (`in_group`), Tab and Shift-Tab: the next or last view,
/// round, in the bar's order, and `]` and `[` round the views of the
/// group in front.
fn switch_view(m: &mut Mission, by: isize, in_group: bool) {
    let groups = m.groups();
    let order: Vec<usize> = if in_group {
        groups
            .into_iter()
            .find(|(_, held)| held.contains(&m.view))
            .map(|(_, held)| held)
            .unwrap_or_default()
    } else {
        groups.into_iter().flat_map(|(_, held)| held).collect()
    };
    let Some(at) = order.iter().position(|v| *v == m.view) else {
        return;
    };
    let to = order[(at as isize + by).rem_euclid(order.len() as isize) as usize];
    show_view(m, to);
}

/// } and {: the first view of the next or last group,
/// round; Tab's focus on a group's name follows.
fn switch_group(m: &mut Mission, by: isize) {
    let groups = m.groups();
    if groups.is_empty() {
        return;
    }
    let at = groups
        .iter()
        .position(|(_, held)| held.contains(&m.view))
        .unwrap_or(0);
    let (g, held) = &groups[(at as isize + by).rem_euclid(groups.len() as isize) as usize];
    show_view(m, held[0]);
    if let Some(Focus::Group(_)) = m.focus {
        m.focus = Some(Focus::Group(*g));
    }
}

/// A group's first view.
fn first_of(m: &Mission, g: views::Group) -> Option<usize> {
    m.groups()
        .into_iter()
        .find(|(x, _)| *x == g)
        .map(|(_, held)| held[0])
}

/// A group's name pressed or clicked: its first view, the group in front's
/// too.
fn open_group(m: &mut Mission, g: views::Group) {
    if let Some(first) = first_of(m, g)
        && first != m.view
    {
        show_view(m, first);
    }
}

/// Keys for the list of groups and views (src/mission/nav.rs): while its
/// filter is typed, the filter's; while Tab's focus is on a view's row,
/// `j` `k` and the arrows move it through the listed views, which Enter
/// opens (with Settings' `mission_nav_follow` on, each comes to the front
/// as the focus lands on it), and `/` types a filter that narrows them by
/// name. Whether the key was the list's.
fn nav_key(m: &mut Mission, k: Key) -> bool {
    let taken = nav_key_in(m, k);
    if taken {
        m.nav_scrolled = false;
    }
    taken
}

/// The focus on the first view the list's filter leaves, for Enter,
/// unless it is on one it leaves already; on none when it leaves none.
fn nav_focus_listed(m: &mut Mission) {
    let order = super::nav::order(m);
    if matches!(m.focus, Some(Focus::View(v)) if order.contains(&v)) {
        return;
    }
    // With none left, nothing for Enter to open.
    m.focus = order.first().map(|&first| Focus::View(first));
}

fn nav_key_in(m: &mut Mission, k: Key) -> bool {
    if let Some(f) = m.nav_filter.as_mut().filter(|f| f.typing) {
        match k.code {
            KeyCode::Enter => f.typing = false,
            KeyCode::Esc => m.nav_filter = None,
            KeyCode::Char('c') if k.mods == Mods::CTRL => m.nav_filter = None,
            KeyCode::Backspace => {
                f.text.pop();
            }
            KeyCode::Char('u') if k.mods == Mods::CTRL => f.text.clear(),
            KeyCode::Char(c) if !k.mods.ctrl && !k.mods.alt => f.text.push(c),
            _ => return true,
        }
        // A filter left with no view isn't kept once typed: the whole list
        // comes back, the focus on the view in front, so the keys still
        // reach the list.
        if !m.nav_filter.as_ref().is_some_and(|f| f.typing) && super::nav::order(m).is_empty() {
            m.nav_filter = None;
            m.focus = Some(Focus::View(m.view));
            return true;
        }
        nav_focus_listed(m);
        return true;
    }
    let Some(Focus::View(at)) = m.focus else {
        return false;
    };
    if !plain(&k) && !matches!(k.code, KeyCode::Up | KeyCode::Down) {
        return false;
    }
    let by: isize = match k.code {
        KeyCode::Char('j') | KeyCode::Down => 1,
        KeyCode::Char('k') | KeyCode::Up => -1,
        KeyCode::Char('/') => {
            let text = m.nav_filter.take().map(|f| f.text).unwrap_or_default();
            m.nav_filter = Some(Filter { text, typing: true });
            return true;
        }
        KeyCode::Esc if m.nav_filter.is_some() => {
            m.nav_filter = None;
            return true;
        }
        _ => return false,
    };
    let order = super::nav::order(m);
    let Some(last) = order.len().checked_sub(1) else {
        return true;
    };
    let to = match order.iter().position(|&v| v == at) {
        Some(i) => (i as isize + by).clamp(0, last as isize) as usize,
        None => 0,
    };
    m.focus = Some(Focus::View(order[to]));
    if follows(m) && order[to] != m.view {
        // Back and forward still know the place before the walk, until the
        // cursor rests (`Mission::nav_held`).
        let held = m.trail_view();
        show_view(m, order[to]);
        m.nav_held = Some((held, util::now_ms()));
    }
    true
}

/// Whether moving the list of views' cursor brings each view to the front
/// (Settings' `mission_nav_follow`), the list one Tab stop: not while
/// several views show at once, where a view is a cell to focus.
fn follows(m: &Mission) -> bool {
    m.snap.nav_follow && m.grid.is_none()
}

/// Enter on the list's row of the view in front: the keys go to the view,
/// its first pane, as `C-w l` gives them, and the view joins the trail now:
/// a walk to it rests at once, settled at the next sync as a rest is.
fn into_view(m: &mut Mission) {
    if let Some(held) = m.nav_held.as_mut() {
        held.1 = 0;
    }
    if let Some(&first) = m.panes().first() {
        m.pane = Some((m.view, first));
    }
    m.focus = Some(Focus::Item);
}

/// A view brought to the front: focus on a view's name follows, and on
/// the filter goes to the items of a board, which has none.
fn show_view(m: &mut Mission, to: usize) {
    // Opened, not walked past: a visit at once.
    m.nav_held = None;
    // The list of groups and views follows the view brought to the front.
    m.nav_scrolled = false;
    // A view brought to the front starts on its list.
    m.pane = None;
    if m.grid.is_some() {
        match m.grid_shown().iter().find(|(_, at)| *at == to) {
            Some(&(cell, _)) => {
                m.focus_cell(cell);
                return;
            }
            None => m.grid = None,
        }
    }
    m.view = to;
    if let Some(Focus::View(_)) = m.focus {
        m.focus = Some(Focus::View(m.view));
    }
    if m.focus == Some(Focus::Filter) && m.current().and_then(View::columns).is_some() {
        m.focus = Some(Focus::Item);
    }
    m.cursor = 0;
    // The Slack board brought to the front opens at its newest row, even
    // picked again while in front.
    m.slack_fresh = m.current().is_some_and(|v| v.id == ViewId::Slack);
    m.scope = None;
}

/// Tab and Shift-Tab with tab_focus: focus a section at a time, in reading
/// order, round: each view's row in the list of groups and
/// views, else each view's name; the filter (not on a board), a side panel,
/// each section of the list (`Mission::sections`, the whole list where it
/// has one) at its first row, a pane beside, the footer's keys and easy
/// mode's ×. A section's own rows are reached with `j` and `k`.
fn move_focus(m: &mut Mission, by: isize) {
    #[derive(Clone, Copy, PartialEq)]
    enum Stop {
        View(usize),
        Kind(usize),
        Filter,
        Section(isize),
        Pane(Pane),
        Key(Key),
        Close,
    }
    let pane_was = m.pane_now();
    let panes = m.panes();
    let board = m.current().and_then(View::columns).is_some();
    let count = match m.current() {
        Some(v) => v.data.items.len(),
        None => m.order().len(),
    };
    // Each listed view's row (src/mission/nav.rs), or, while the list's
    // cursor brings each view to the front, the list as one stop, `j` and
    // `k` moving through it.
    let order = super::nav::order(m);
    let follow = follows(m);
    let mut stops: Vec<Stop> = if follow {
        // The view in front's row, or the first the list's filter leaves
        // when it hides that one.
        let row = Some(m.view)
            .filter(|v| order.contains(v))
            .or(order.first().copied());
        row.map(Stop::View).into_iter().collect()
    } else {
        order.into_iter().map(Stop::View).collect()
    };
    if notice_board(m) {
        stops.extend((1..=7).map(Stop::Kind));
    }
    if !board {
        stops.push(Stop::Filter);
    }
    if panes.contains(&Pane::Panel) {
        stops.push(Stop::Pane(Pane::Panel));
    }
    // A grouped view's headings are no stop, the cursor never resting on
    // one, nor are the Usage view's rows that are neither a tab nor a
    // service; a list of one section is one stop at its first row.
    let heading = |i: isize| {
        m.current().is_some_and(|v| {
            let item = v.data.items.get(i as usize);
            (views::grouped(&v.id)
                && item.is_some_and(|it| views::is_header(it) && !views::is_summary(it)))
                || ((v.id == ViewId::Usage || m.grid.is_some())
                    && item.is_some_and(|i| m.passes_over(i)))
        })
    };
    let mut starts: Vec<isize> = m.sections().into_iter().map(|s| s as isize).collect();
    // Beside a side panel (Settings' categories, the Slack board's
    // sections) the list shows only the part the panel chose, which the
    // cursor names, so the list's stop is where the cursor is.
    // While a filter searches Settings, every category with a match shows,
    // as sections; the Slack board still shows the section chosen.
    let searching = m.filter.as_ref().is_some_and(|f| !f.text.trim().is_empty());
    let panel_chooses = m.current().is_some_and(|v| {
        v.id == ViewId::Settings && super::render::settings_board_shown(m) && !searching
            || super::render::slack_board_shown(v)
    }) && (floor(m)..count as isize).contains(&m.cursor)
        && !heading(m.cursor);
    if starts.is_empty() && panel_chooses {
        starts.push(m.cursor);
    }
    if starts.is_empty() {
        starts = m.board_groups().into_iter().map(|s| s as isize).collect();
    }
    if starts.is_empty() {
        starts.extend((floor(m)..count as isize).find(|i| !heading(*i)));
    }
    let section_of = |cursor: isize| {
        starts
            .iter()
            .rposition(|s| *s <= cursor)
            .map_or(starts.first().copied(), |at| Some(starts[at]))
    };
    stops.extend(starts.iter().map(|s| Stop::Section(*s)));
    // The keys drawn: the Reminders pane's after the chosen row's section,
    // the footer's, on the last row, at the end.
    let mut seen = Vec::new();
    let keys: Vec<(u16, Key)> = m
        .hits
        .iter()
        .filter_map(|h| match h.spot {
            Spot::Key(k) => Some((h.y, k)),
            _ => None,
        })
        .filter(|(_, k)| {
            let new = !seen.contains(k);
            seen.push(*k);
            new
        })
        .collect();
    let foot = keys.iter().map(|(y, _)| *y).max();
    let (footer, pane): (Vec<_>, Vec<_>) = keys.into_iter().partition(|(y, _)| Some(*y) == foot);
    let chosen = section_of(m.cursor).map(Stop::Section);
    if let Some(at) = stops.iter().position(|s| Some(*s) == chosen) {
        for (n, (_, k)) in pane.into_iter().enumerate() {
            stops.insert(at + 1 + n, Stop::Key(k));
        }
    }
    if panes.contains(&Pane::Beside) {
        stops.push(Stop::Pane(Pane::Beside));
    }
    stops.extend(footer.into_iter().map(|(_, k)| Stop::Key(k)));
    if m.snap.close {
        stops.push(Stop::Close);
    }
    let at = match m.focus {
        None => None,
        Some(Focus::Group(_)) => None,
        Some(Focus::View(_)) if follow => Some(0),
        Some(Focus::View(v)) => stops.iter().position(|s| *s == Stop::View(v)),
        Some(Focus::Kind(k)) => stops.iter().position(|s| *s == Stop::Kind(k)),
        Some(Focus::Filter) => stops.iter().position(|s| *s == Stop::Filter),
        Some(Focus::Item) if pane_was != Pane::List => {
            stops.iter().position(|s| *s == Stop::Pane(pane_was))
        }
        Some(Focus::Item) => stops.iter().position(|s| Some(*s) == chosen),
        Some(Focus::Close) => stops.iter().position(|s| *s == Stop::Close),
        Some(Focus::Key(k)) => stops.iter().position(|s| *s == Stop::Key(k)),
    };
    let n = stops.len() as isize;
    if n == 0 {
        return;
    }
    let to = match at {
        None if by > 0 => 0,
        None => n - 1,
        Some(i) => (i as isize + by).rem_euclid(n),
    };
    // Tab hands the keys back to the list from a pane it leaves, and
    // landing on a pane gives them to it, as `C-w` does; on the footer's
    // keys and the × the pane keeps them, its footer the one drawn.
    let keeps_pane = matches!(stops[to as usize], Stop::Key(_) | Stop::Close);
    if !keeps_pane {
        m.pane = Some((m.view, Pane::List));
    }
    match stops[to as usize] {
        Stop::View(v) => m.focus = Some(Focus::View(v)),
        Stop::Kind(k) => m.focus = Some(Focus::Kind(k)),
        Stop::Filter => m.focus = Some(Focus::Filter),
        Stop::Section(i) => {
            m.focus = Some(Focus::Item);
            m.cursor = i;
            follow_reminder(m);
        }
        Stop::Pane(pane) => {
            m.focus = Some(Focus::Item);
            m.pane = Some((m.view, pane));
        }
        Stop::Close => m.focus = Some(Focus::Close),
        Stop::Key(k) => m.focus = Some(Focus::Key(k)),
    }
}

/// Whether the Notifications view in front shows the history with its
/// keycaps (UX 2.0).
fn notice_board(m: &Mission) -> bool {
    notice_history_here(m)
}

/// Whether the Notifications view in front shows the history, by day.
fn notice_history_here(m: &Mission) -> bool {
    m.snap.notice_counts.is_some() && m.current().is_some_and(|v| v.id == ViewId::Notifications)
}

/// The Notifications view narrowed to a kind's notifications by its key
/// (1 all), from the top.
fn choose_kind(m: &mut Mission, key: usize) {
    m.notice_kind = key;
    m.cursor = 0;
}

/// Keys while typing a view's name after `v`: the first view whose name
/// starts with it, else has it anywhere, the tabs included, lit as it is
/// typed; Enter goes there, Esc doesn't.
fn jump_key(m: &mut Mission, k: Key) {
    let Some(j) = m.jump.as_mut() else {
        return;
    };
    match k.code {
        KeyCode::Esc => m.jump = None,
        KeyCode::Char('c') if k.mods == Mods::CTRL => m.jump = None,
        KeyCode::Enter => {
            if let Some(v) = j.matched {
                show_view(m, v);
            }
            m.jump = None;
            return;
        }
        KeyCode::Backspace => {
            j.text.pop();
        }
        KeyCode::Char(c) if !k.mods.ctrl && !k.mods.alt => j.text.push(c),
        _ => return,
    }
    let Some(j) = m.jump.as_mut() else {
        return;
    };
    let names: Vec<String> = std::iter::once("tabs".to_string())
        .chain(m.snap.views.iter().map(|v| v.title.to_lowercase()))
        .collect();
    let text = j.text.trim().to_lowercase();
    j.matched = if text.is_empty() {
        None
    } else {
        names
            .iter()
            .position(|n| n.starts_with(&text))
            .or_else(|| names.iter().position(|n| n.contains(&text)))
    };
}

/// Opens `url` under a busy box, which says where it is opening as soon as
/// that is read and lasts until the browser comes to the front; gives what
/// to say when it didn't open or hasn't shown yet.
async fn open_under_busy(
    tx: tokio::sync::mpsc::UnboundedSender<Event>,
    client: ClientId,
    url: String,
) -> Option<String> {
    let opened = tokio::task::spawn_blocking(move || {
        if crate::links::says_it() {
            let text = crate::links::opening_words(&url);
            let _ = tx.send(Event::Mission(MissionEvent::Relabel { client, text }));
        }
        crate::links::open_url_watched(&url)
    })
    .await;
    match opened {
        Ok(Ok(true)) => None,
        Ok(Ok(false)) => Some("The browser hasn't shown it yet.".into()),
        Ok(Err(e)) => Some(format!("Couldn't open it: {e}")),
        Err(e) => Some(format!("Couldn't open it: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::items::ViewData;
    use crate::mission::{HitBox, WINDOW_WAIT_MS};
    use crate::mux::TabId;
    use crate::sidebar::model::TabModel;
    use crate::sidebar::model::tests::rules;

    #[test]
    fn a_merged_tabs_worktree_is_found_from_git_wherever_it_was_made() {
        let dir = std::env::temp_dir().join(format!("sb-merged-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let main = dir.join("repo");
        let elsewhere = dir.join("worktrees/abc-1-thing");
        std::fs::create_dir_all(&main).unwrap();
        let git = |at: &Path, args: &[&str]| {
            let ok = std::process::Command::new("git")
                .arg("-C")
                .arg(at)
                .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
                .args(args)
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .output()
                .unwrap()
                .status
                .success();
            assert!(ok, "git {args:?}");
        };
        git(&main, &["init", "-q", "-b", "main"]);
        git(&main, &["commit", "-q", "--allow-empty", "-m", "start"]);
        git(
            &main,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "abc-1",
                &elsewhere.to_string_lossy(),
            ],
        );
        std::fs::create_dir_all(elsewhere.join("src")).unwrap();
        let real = |p: &Path| p.canonicalize().unwrap();

        // A worktree outside .claude/worktrees, from its top or a folder in it.
        let expected = Some((real(&main.join(".git")), real(&elsewhere)));
        assert_eq!(merged_worktree(&elsewhere), expected);
        assert_eq!(merged_worktree(&elsewhere.join("src")), expected);
        // The main clone itself is never removed.
        assert_eq!(merged_worktree(&main), None);
        // A clone whose git directory is kept apart from it.
        let apart = dir.join("apart");
        let meta = dir.join("apart-meta");
        let apart_wt = apart.join(".claude/worktrees/abc-3");
        std::fs::create_dir_all(&apart).unwrap();
        git(
            &dir,
            &[
                "init",
                "-q",
                "-b",
                "main",
                "--separate-git-dir",
                &meta.to_string_lossy(),
                &apart.to_string_lossy(),
            ],
        );
        git(&apart, &["commit", "-q", "--allow-empty", "-m", "start"]);
        git(
            &apart,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "abc-3",
                &apart_wt.to_string_lossy(),
            ],
        );
        assert_eq!(
            merged_worktree(&apart_wt),
            Some((real(&meta), real(&apart_wt)))
        );
        assert_eq!(merged_worktree(&apart), None);
        // git removes each from the directory given.
        for wt in [&elsewhere, &apart_wt] {
            let (from, top) = merged_worktree(wt).unwrap();
            git(&from, &["worktree", "remove", &top.to_string_lossy()]);
            assert!(!wt.exists(), "{}", wt.display());
        }
        // A worktree already gone is pruned from its .claude/worktrees path.
        let gone = main.join(".claude/worktrees/abc-2-gone");
        assert_eq!(merged_worktree(&gone), Some((main.clone(), gone.clone())));
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn calendar_mission() -> Mission {
        let event = |id: &str, title: &str, h: i64, extra: serde_json::Value| {
            let mut data = serde_json::json!({
                "title": title, "id": id, "you": "not answered",
                "start": format!("2026-11-02T{h:02}:00:00Z"),
                "end": format!("2026-11-02T{h:02}:30:00Z"),
            });
            if let (Some(d), Some(x)) = (data.as_object_mut(), extra.as_object()) {
                d.extend(x.clone());
            }
            Item::new(id, title).data(data)
        };
        let view = View {
            id: ViewId::Calendar,
            title: "Calendar".into(),
            data: ViewData {
                items: vec![
                    event(
                        "g1",
                        "Standup",
                        9,
                        serde_json::json!({ "join": "https://meet.google.com/x" }),
                    ),
                    event("g2", "Review", 10, serde_json::json!({ "you": "yours" })),
                ],
                at: Some(1),
                ..ViewData::default()
            },
            loaded: true,
        };
        let mut m = mission(0, vec![view]);
        m.view = 1;
        m
    }

    #[test]
    fn the_calendars_keys_move_a_day_and_leave_its_actions_to_its_menu() {
        let key = |c: char| Key::new(KeyCode::Char(c), Mods::NONE);
        let m = calendar_mission();
        assert_eq!(calendar_key(&m, key('l')), Some(1));
        assert_eq!(calendar_key(&m, key('h')), Some(-1));
        assert!(
            calendar_key(&m, key('j')).is_none(),
            "j walks the events, as everywhere"
        );
        assert_eq!(row_step(&m, 1), 1, "j moves to the next event, not joining");
        for c in ['o', 'y', 'n', 'm'] {
            assert_eq!(calendar_key(&m, key(c)), None, "{c} is in the event's menu");
        }
    }

    #[test]
    fn h_and_l_move_a_day_through_the_feeds_days_and_say_why_not_past_them() {
        use crate::day::calendar::{Feed, SavedDay};
        let now = jiff::civil::date(2026, 11, 2)
            .at(12, 0, 0, 0)
            .to_zoned(jiff::tz::TimeZone::UTC)
            .unwrap();
        let alone = Feed::default();
        let said = calendar_step(&alone, None, 1, &now).unwrap_err();
        assert!(said.starts_with("Only today is saved"), "{said}");
        let mut feed = Feed::default();
        for date in ["2026-11-01", "2026-11-02", "2026-11-03"] {
            feed.days.insert(
                date.into(),
                SavedDay {
                    date: date.into(),
                    ..SavedDay::default()
                },
            );
        }
        assert_eq!(
            calendar_step(&feed, None, 1, &now),
            Ok(Some("2026-11-03".into()))
        );
        assert_eq!(
            calendar_step(&feed, Some("2026-11-03"), -1, &now),
            Ok(None),
            "back to today"
        );
        assert_eq!(
            calendar_step(&feed, None, -1, &now),
            Ok(Some("2026-11-01".into()))
        );
        let mut m = calendar_mission();
        m.snap.views[0].data.items.clear();
        m.snap.views[0].data.error = Some("Google answered 500".into());
        assert_eq!(
            calendar_key(&m, Key::new(KeyCode::Char('l'), Mods::NONE)),
            Some(1),
            "an empty today the feed failed since moves on too"
        );
        m.calendar_day = Some("2026-11-03".into());
        m.snap.views[0].data.items.clear();
        m.snap.views[0].data.error = Some("Google answered 500".into());
        assert_eq!(
            calendar_key(&m, Key::new(KeyCode::Char('h'), Mods::NONE)),
            Some(-1),
            "an empty day the feed failed since still moves back"
        );
        let mut gone = feed.clone();
        gone.take(crate::day::calendar::Fetched::NoAddress, 2);
        assert!(
            calendar_step(&gone, None, 1, &now)
                .unwrap_err()
                .starts_with("Only today"),
            "an address taken away takes its days with it"
        );
        let past = calendar_step(&feed, Some("2026-11-03"), 1, &now).unwrap_err();
        assert!(past.contains("two weeks ahead"), "{past}");
        // A feed failing since says so on the other days too.
        feed.failed("Google answered 500".into(), 1);
        let other = crate::day::calendar::other_day_view(&feed, "2026-11-03", &now).unwrap();
        assert_eq!(other.error.as_deref(), Some("Google answered 500"));
    }

    fn mission(tabs: usize, views: Vec<View>) -> Mission {
        let mut m = Mission::new();
        m.snap.tabs = (1..=tabs)
            .map(|n| TabModel::new(TabId(n as u32), n, &format!("tab {n}")))
            .collect();
        m.snap.views = views;
        m.snap.rules = rules();
        m.size = (3 * 56, 40);
        m
    }

    fn list(title: &str, n: usize) -> View {
        View {
            id: ViewId::Reopen,
            title: title.into(),
            data: ViewData {
                items: (0..n)
                    .map(|i| Item::new(i.to_string(), format!("item {i}")))
                    .collect(),
                ..ViewData::default()
            },
            loaded: true,
        }
    }

    /// The CI view: #12 failing two checks, the first with
    /// its step read, then #13 passing.
    fn ci_mission() -> Mission {
        let failing = Item::new("f", "#12 Fix").data(serde_json::json!({ "ci": {
            "number": 12, "repo": "acme/app", "state": "failing",
            "url": "https://github.com/acme/app/pull/12",
            "checks": [
                { "name": "test", "job": "77", "url": "https://github.com/acme/app/actions/runs/1/job/77",
                  "step": { "first": 40, "lines": ["Run test"], "whyAt": 0 } },
                { "name": "deploy", "job": null, "url": "https://ci.example/2" },
            ],
        } }));
        let passing = Item::new("p", "#13 Docs").data(serde_json::json!({ "ci": {
            "number": 13, "repo": "acme/app", "state": "passing" } }));
        let view = View {
            id: ViewId::Ci,
            title: "CI".into(),
            data: ViewData {
                items: views::ci_sections(&[failing, passing]),
                ..ViewData::default()
            },
            loaded: true,
        };
        let mut m = mission(1, vec![view]);
        m.view = 1;
        m.cursor = 1;
        m
    }

    #[test]
    fn in_the_ci_view_j_and_k_go_through_a_pr_s_failed_checks() {
        let key = |k: &str| Key::parse(k).unwrap();
        let mut m = ci_mission();
        let at = |m: &Mission| (m.cursor, m.chosen_check());
        m.focus = Some(Focus::Filter);
        assert!(ci_key(&mut m, key("j")));
        assert_eq!(at(&m), (1, Some(0)));
        assert_eq!(m.focus, Some(Focus::Item), "the focus back on the items");
        ci_key(&mut m, key("j"));
        assert_eq!(at(&m), (1, Some(1)));
        ci_key(&mut m, key("j"));
        assert_eq!(at(&m), (3, None), "over the Passing heading to #13");
        ci_key(&mut m, key("j"));
        assert_eq!(at(&m), (3, None), "the last row stays");
        ci_key(&mut m, key("k"));
        assert_eq!(at(&m), (1, Some(1)), "back up to the last check's line");
        ci_key(&mut m, key("k"));
        ci_key(&mut m, key("k"));
        assert_eq!(at(&m), (1, None), "the PR's own row");
        // A look that finds the chosen check passing lets it go, and one
        // that drops another keeps it chosen, wherever it now is.
        m.ci_check = Some(("f".into(), "deploy\thttps://ci.example/2".into()));
        let checks = &mut m.snap.views[0].data.items[1].data["ci"]["checks"];
        checks.as_array_mut().unwrap().remove(0);
        assert_eq!(m.chosen_check(), Some(0), "the same check");
        let checks = &mut m.snap.views[0].data.items[1].data["ci"]["checks"];
        checks.as_array_mut().unwrap().pop();
        assert_eq!(m.chosen_check(), None, "its first shows");
        m.forget_side_elsewhere();
        assert_eq!(m.ci_check, None);
        // A failing PR last keeps its last check chosen.
        let mut m = ci_mission();
        m.snap.views[0].data.items.truncate(2);
        for _ in 0..3 {
            ci_key(&mut m, key("j"));
        }
        assert_eq!(at(&m), (1, Some(1)), "its last check stays chosen");
    }

    #[test]
    fn two_failed_checks_of_one_name_are_chosen_apart() {
        let key = |k: &str| Key::parse(k).unwrap();
        let mut m = ci_mission();
        let checks = &mut m.snap.views[0].data.items[1].data["ci"]["checks"];
        checks[1]["name"] = serde_json::json!("test");
        ci_key(&mut m, key("j"));
        ci_key(&mut m, key("j"));
        assert_eq!(m.chosen_check(), Some(1), "the second, by its own page");
        let item = m.chosen_item().cloned().unwrap();
        assert_eq!(
            with_check(item, 1).data["ci"]["check"]["url"],
            "https://ci.example/2"
        );
        // And two of one page, by their names.
        let checks = &mut m.snap.views[0].data.items[1].data["ci"]["checks"];
        checks[1]["name"] = serde_json::json!("deploy");
        checks[1]["url"] = checks[0]["url"].clone();
        ci_key(&mut m, key("k"));
        ci_key(&mut m, key("k"));
        ci_key(&mut m, key("j"));
        ci_key(&mut m, key("j"));
        assert_eq!(m.chosen_check(), Some(1), "the second, by its own name");
    }

    #[test]
    fn in_the_ci_view_the_menu_is_the_chosen_checks_and_its_keys_are_gone() {
        let key = |k: &str| Key::parse(k).unwrap();
        let mut m = ci_mission();
        for k in ["l", "o", "M-r", "v"] {
            assert!(!ci_key(&mut m, key(k)), "{k} is no key of the view's");
        }
        let repos_of = |_: &Path| Vec::new();
        let habit = |_: &str| Vec::new();
        let ctx = Ctx {
            tabs: &[],
            repos_of: &repos_of,
            home: PathBuf::from("/home"),
            habit: &habit,
            to_tab: false,
            to_tab_submits: true,
            slack_reply: false,
            shows: &|_| true,
        };
        let item = m.snap.views[0].data.items[1].clone();
        let acts = |item: Item| -> Vec<Do> {
            choices::item_choices(&ViewId::Ci, &item, &[], &ctx)
                .into_iter()
                .map(|c| c.act)
                .collect()
        };
        let first = acts(with_check(item.clone(), 0));
        assert!(first.contains(&Do::ReadLog {
            job: "77".into(),
            name: "test".into(),
            line: Some(40),
        }));
        assert!(first.contains(&Do::Open(
            "https://github.com/acme/app/actions/runs/1/job/77".into()
        )));
        // A check with no Actions job has no log, and opens its own page.
        let second = acts(with_check(item, 1));
        assert!(!second.iter().any(|a| matches!(a, Do::ReadLog { .. })));
        assert!(second.contains(&Do::Open("https://ci.example/2".into())));
    }

    #[test]
    fn only_a_key_that_moves_leaves_the_snapshot_standing() {
        let key = |k: &str| Key::parse(k).unwrap();
        let mut m = mission(3, vec![list("Closed", 3)]);
        for k in ["j", "k", "h", "l", "Down", "g", "G", "C-n", "3", "Tab"] {
            assert!(only_moves(&m, key(k)), "{k}");
        }
        for k in ["Enter", "Space", "]", "/", "v", "q", "Escape", "r"] {
            assert!(!only_moves(&m, key(k)), "{k}");
        }
        // A tab's number moves only among the cards.
        m.view = 1;
        assert!(!only_moves(&m, key("3")));
        // Typed into a filter, j is text.
        m.filter = Some(Filter {
            text: String::new(),
            typing: true,
        });
        assert!(!only_moves(&m, key("j")));
        m.filter = None;
        m.jump = Some(Jump::default());
        assert!(!only_moves(&m, key("j")));
    }

    #[test]
    fn brackets_go_round_the_views_and_tab_moves_focus_through_names_filter_and_the_list() {
        let mut m = mission(2, vec![list("Closed", 3), list("Other", 1)]);
        switch_view(&mut m, 1, true);
        assert_eq!(m.view, 1);
        switch_view(&mut m, -1, true);
        switch_view(&mut m, -1, true);
        assert_eq!(m.view, 2, "round to the last");
        m.view = 0;
        move_focus(&mut m, 1);
        assert_eq!(m.focus, Some(Focus::View(0)));
        for _ in 0..3 {
            move_focus(&mut m, 1);
        }
        assert_eq!(m.focus, Some(Focus::Filter));
        move_focus(&mut m, 1);
        assert_eq!((m.focus, m.cursor), (Some(Focus::Item), 0));
        // A list of one section is one stop, its rows reached with j and k.
        m.cursor = 1;
        move_focus(&mut m, 1);
        assert_eq!(m.focus, Some(Focus::View(0)), "round to the first");
    }

    /// The Usage view as the server lists it: the total and the limits,
    /// still, then the tabs, the services and the sessions.
    fn usage_sections() -> Mission {
        let still = |id: &str| Item::new(id, id).data(serde_json::json!({ "still": true }));
        let usage = View {
            id: ViewId::Usage,
            title: "Usage".into(),
            data: ViewData {
                items: vec![
                    still("spent"),
                    still("limit:five_hour"),
                    Item::new("tab:tab:1", "1 api"),
                    Item::new("tab:tab:2", "2 web"),
                    Item::new("requests:GitHub", "GitHub"),
                    Item::new("requests:Linear", "Linear"),
                    still("requests:uncounted"),
                    Item::new("session:a", "a"),
                ],
                ..ViewData::default()
            },
            loaded: true,
        };
        let mut m = mission(1, vec![usage]);
        m.view = 1;
        m
    }

    #[test]
    fn tab_goes_through_the_usage_view_a_section_at_a_time_and_round() {
        let mut m = usage_sections();
        m.cursor = 3;
        m.focus = Some(Focus::Item);
        move_focus(&mut m, 1);
        assert_eq!((m.focus, m.cursor), (Some(Focus::Item), 4), "the services");
        move_focus(&mut m, 1);
        assert_eq!((m.focus, m.cursor), (Some(Focus::Item), 7), "the sessions");
        move_focus(&mut m, 1);
        assert_eq!(m.focus, Some(Focus::View(0)), "round to the first stop");
        move_focus(&mut m, -1);
        assert_eq!((m.focus, m.cursor), (Some(Focus::Item), 7));
        m.cursor = 5;
        move_focus(&mut m, -1);
        assert_eq!(
            (m.focus, m.cursor),
            (Some(Focus::Item), 2),
            "the tabs' first"
        );
        move_focus(&mut m, -1);
        assert_eq!(m.focus, Some(Focus::Filter));
    }

    #[test]
    fn tab_from_settings_categories_keeps_the_category_chosen_there() {
        let settings = View {
            id: ViewId::Settings,
            title: "Settings".into(),
            data: ViewData {
                items: vec![
                    views::header("a", "First", 1),
                    Item::new("one", "one"),
                    views::header("b", "Second", 1),
                    Item::new("two", "two"),
                ],
                ..ViewData::default()
            },
            loaded: true,
        };
        let mut m = mission(1, vec![settings]);
        m.size = (200, 50);
        m.view = 1;
        m.cursor = 3;
        m.focus = Some(Focus::Item);
        m.pane = Some((1, Pane::Panel));
        assert_eq!(m.pane_now(), Pane::Panel);
        move_focus(&mut m, 1);
        assert_eq!(
            (m.focus, m.pane_now(), m.cursor),
            (Some(Focus::Item), Pane::List, 3),
            "the second category's rows"
        );
        // Back from the footer the same, whatever panes C-w knows of.
        m.pane = None;
        m.focus = Some(Focus::Close);
        m.snap.close = true;
        move_focus(&mut m, -1);
        assert_eq!((m.focus, m.cursor), (Some(Focus::Item), 3));
        // A filter shows every category with a match: Tab goes through them.
        m.filter = Some(Filter {
            text: "o".into(),
            typing: false,
        });
        m.cursor = 1;
        move_focus(&mut m, 1);
        assert_eq!(
            (m.focus, m.cursor),
            (Some(Focus::Item), 3),
            "the next category"
        );
    }

    #[test]
    fn tab_goes_column_to_column_on_a_board() {
        let item = |n: u32, column: &str| Item::new(n.to_string(), format!("#{n}")).group(column);
        let board = View {
            id: ViewId::Prs,
            title: "PRs".into(),
            data: ViewData {
                items: vec![
                    item(1, "Draft"),
                    item(2, "In review"),
                    item(3, "In review"),
                    item(4, "Merged"),
                ],
                ..ViewData::default()
            },
            loaded: true,
        };
        let mut m = mission(0, vec![board]);
        m.view = 1;
        m.cursor = 2;
        m.focus = Some(Focus::Item);
        move_focus(&mut m, 1);
        assert_eq!(
            (m.focus, m.cursor),
            (Some(Focus::Item), 3),
            "the last column"
        );
        move_focus(&mut m, -1);
        move_focus(&mut m, -1);
        assert_eq!(
            (m.focus, m.cursor),
            (Some(Focus::Item), 0),
            "the first column"
        );
    }

    #[test]
    fn tab_goes_through_the_routines_board_s_sections() {
        let routine = |n: u32, section: u8| {
            Item::new(n.to_string(), n.to_string()).data(serde_json::json!({ "section": section }))
        };
        let routines = View {
            id: ViewId::Routines,
            title: "Routines".into(),
            data: ViewData {
                items: vec![routine(1, 0), routine(2, 0), routine(3, 1), routine(4, 2)],
                ..ViewData::default()
            },
            loaded: true,
        };
        let mut m = mission(0, vec![routines]);
        m.view = 1;
        m.cursor = 1;
        m.focus = Some(Focus::Item);
        move_focus(&mut m, 1);
        assert_eq!(m.cursor, 2, "Once");
        move_focus(&mut m, 1);
        assert_eq!(m.cursor, 3, "Ran once");
    }

    #[test]
    fn tab_goes_reminder_to_reminder_in_the_month() {
        let mut m = mission(0, vec![view(ViewId::Reminders, "Reminders")]);
        m.snap.views[0].data.items = (0..3)
            .map(|i| Item::new(i.to_string(), format!("reminder {i}")))
            .collect();
        m.view = 1;
        m.cursor = 0;
        m.focus = Some(Focus::Item);
        move_focus(&mut m, 1);
        assert_eq!(m.cursor, 1);
        move_focus(&mut m, 1);
        assert_eq!(m.cursor, 2);
    }

    #[test]
    fn tab_from_a_list_of_one_section_goes_to_the_pane_beside_and_back() {
        let mut m = mission(1, vec![list("Closed", 3)]);
        m.view = 1;
        m.cursor = 2;
        m.focus = Some(Focus::Item);
        m.hits = vec![HitBox {
            y: 2,
            x0: 60,
            x1: 90,
            spot: Spot::Side(10),
        }];
        move_focus(&mut m, 1);
        assert_eq!((m.focus, m.pane_now()), (Some(Focus::Item), Pane::Beside));
        // The pane's footer keys keep the pane, whose footer they are.
        let scroll = Key::new(KeyCode::Char('G'), Mods::NONE);
        m.hits.push(HitBox {
            y: 30,
            x0: 2,
            x1: 8,
            spot: Spot::Key(scroll),
        });
        move_focus(&mut m, 1);
        assert_eq!(
            (m.focus, m.pane_now()),
            (Some(Focus::Key(scroll)), Pane::Beside)
        );
        move_focus(&mut m, -1);
        assert_eq!((m.focus, m.pane_now()), (Some(Focus::Item), Pane::Beside));
        move_focus(&mut m, -1);
        assert_eq!(
            (m.focus, m.pane_now(), m.cursor),
            (Some(Focus::Item), Pane::List, 0),
            "the list's first row"
        );
        move_focus(&mut m, -1);
        assert_eq!(m.focus, Some(Focus::Filter));
    }

    #[test]
    fn tab_stops_on_a_narrow_board_s_more_markers() {
        let mut m = mission(1, vec![view(ViewId::Prs, "PRs")]);
        m.view = 1;
        m.cursor = 0;
        m.focus = Some(Focus::Item);
        let right = Key::new(KeyCode::Right, Mods::NONE);
        m.hits = vec![crate::mission::HitBox {
            y: 3,
            x0: 40,
            x1: 47,
            spot: Spot::Key(right),
        }];
        move_focus(&mut m, 1);
        assert_eq!(m.focus, Some(Focus::Key(right)), "after the chosen card");
    }

    #[test]
    fn shift_tab_passes_over_the_usage_rows_the_cursor_does() {
        let still = |id: &str| Item::new(id, id).data(serde_json::json!({ "still": true }));
        let usage = View {
            id: ViewId::Usage,
            title: "Usage".into(),
            data: ViewData {
                items: vec![
                    still("spent"),
                    Item::new("tab:tab:1", "1 api").data(serde_json::json!({ "tab_key": "tab:1" })),
                    still("limit:five_hour"),
                    Item::new("requests:GitHub", "GitHub"),
                    still("requests:uncounted"),
                ],
                ..ViewData::default()
            },
            loaded: true,
        };
        let mut m = mission(1, vec![usage]);
        m.view = 1;
        m.cursor = 3;
        m.focus = Some(Focus::Item);
        move_focus(&mut m, -1);
        assert_eq!((m.focus, m.cursor), (Some(Focus::Item), 1));
        move_focus(&mut m, -1);
        assert_eq!(m.focus, Some(Focus::Filter), "past the total to the filter");
        // End on the last row, passed over, rests on the one before.
        m.cursor = 4;
        fit_cursor(&mut m);
        assert_eq!(m.cursor, 3);
    }

    fn view(id: ViewId, title: &str) -> View {
        View {
            id,
            ..list(title, 1)
        }
    }

    #[test]
    fn braces_go_round_the_groups_and_brackets_round_the_group_in_front() {
        // In the order the server builds them, not the bar's.
        let mut m = mission(
            1,
            vec![
                view(ViewId::Notifications, "Notifications"),
                view(ViewId::WorkMap, "Work map"),
                view(ViewId::Calendar, "Calendar"),
                view(ViewId::Prs, "PRs"),
                view(ViewId::Ci, "CI"),
                view(ViewId::Settings, "Settings"),
            ],
        );
        switch_view(&mut m, 1, true);
        assert_eq!(m.view, 2, "the Map, after the tabs in Work");
        switch_view(&mut m, 1, true);
        assert_eq!(m.view, 0, "round Work to the tabs");
        switch_group(&mut m, 1);
        assert_eq!(m.view, 4, "Code's first, PRs");
        switch_view(&mut m, -1, true);
        assert_eq!(m.view, 5, "round Code to CI");
        switch_group(&mut m, 1);
        assert_eq!(m.view, 3, "Day's first, the Calendar");
        switch_group(&mut m, -2);
        assert_eq!(m.view, 0);
        switch_group(&mut m, -1);
        assert_eq!(m.view, 6, "round to System's Settings");
        switch_view(&mut m, 1, false);
        assert_eq!(m.view, 0, "Tab goes on past the group, round");
        // Tab goes through every listed view's row, in the list's order.
        m.focus = None;
        move_focus(&mut m, 1);
        assert_eq!(m.focus, Some(Focus::View(0)));
        move_focus(&mut m, 1);
        assert_eq!(m.focus, Some(Focus::View(2)), "the Map, after the tabs");
        move_focus(&mut m, 1);
        assert_eq!(m.focus, Some(Focus::View(4)), "Code's PRs");
        open_group(&mut m, views::Group::Day);
        assert_eq!(m.view, 3, "a group's name pressed: its first view");
        m.view = 1;
        open_group(&mut m, views::Group::Day);
        assert_eq!(m.view, 3, "the group in front's too");
    }

    #[test]
    fn j_and_k_move_through_the_listed_views_and_slash_narrows_them() {
        let mut m = mission(
            1,
            vec![
                view(ViewId::WorkMap, "Work map"),
                view(ViewId::Prs, "PRs"),
                view(ViewId::Settings, "Settings"),
            ],
        );
        let key = |c| Key::new(KeyCode::Char(c), Mods::NONE);
        assert!(
            !nav_key(&mut m, key('j')),
            "the view's while the list has no focus"
        );
        m.focus = Some(Focus::View(0));
        assert!(nav_key(&mut m, key('j')));
        assert_eq!(m.focus, Some(Focus::View(1)), "the Map");
        assert!(nav_key(&mut m, Key::new(KeyCode::Down, Mods::NONE)));
        assert_eq!(m.focus, Some(Focus::View(2)), "PRs");
        nav_key(&mut m, key('k'));
        nav_key(&mut m, key('k'));
        nav_key(&mut m, key('k'));
        assert_eq!(m.focus, Some(Focus::View(0)), "held at the top");
        nav_key(&mut m, key('/'));
        nav_key(&mut m, key('s'));
        nav_key(&mut m, key('e'));
        assert_eq!(super::super::nav::order(&m), vec![3], "Settings alone");
        assert_eq!(m.focus, Some(Focus::View(3)), "on the first it leaves");
        nav_key(&mut m, key('z'));
        assert!(super::super::nav::order(&m).is_empty());
        assert_eq!(m.focus, None, "nothing left for Enter to open");
        nav_key(&mut m, Key::new(KeyCode::Enter, Mods::NONE));
        assert!(m.nav_filter.is_none(), "a filter that left none isn't kept");
        assert_eq!(m.focus, Some(Focus::View(m.view)));
        nav_key(&mut m, key('/'));
        nav_key(&mut m, key('s'));
        nav_key(&mut m, Key::new(KeyCode::Esc, Mods::NONE));
        assert!(m.nav_filter.is_none());
        assert_eq!(super::super::nav::order(&m), vec![0, 1, 2, 3]);
    }

    #[test]
    fn with_the_list_following_j_shows_each_view_and_back_counts_one_step_until_it_rests() {
        let mut m = mission(
            1,
            vec![
                view(ViewId::WorkMap, "Work map"),
                view(ViewId::Prs, "PRs"),
                view(ViewId::Settings, "Settings"),
            ],
        );
        m.snap.nav_follow = true;
        let key = |c| Key::new(KeyCode::Char(c), Mods::NONE);
        let before = m.trail_view();
        m.focus = Some(Focus::View(0));
        assert!(nav_key(&mut m, key('j')));
        assert_eq!(m.view, 1, "the Map in front");
        assert_eq!(m.focus, Some(Focus::View(1)), "the list keeps the keys");
        nav_key(&mut m, key('j'));
        assert_eq!(m.view, 2, "PRs in front");
        assert_eq!(m.trail_view(), before, "walked past: no step yet");
        // Rested a second: PRs is the place back and forward know.
        m.nav_held.as_mut().unwrap().1 -= super::super::NAV_SETTLE_MS;
        assert_eq!(m.trail_view(), Some(m.current().unwrap().slug()));
        // Tab goes from the list into the view (its first stop, a board's
        // first card), never along the list, and Shift-Tab back to the view
        // in front's row.
        move_focus(&mut m, 1);
        assert_eq!((m.view, m.focus), (2, Some(Focus::Item)));
        move_focus(&mut m, -1);
        assert_eq!(m.focus, Some(Focus::View(2)));
        // Enter there moves into the view, which joins the trail at once.
        nav_key(&mut m, key('k'));
        assert!(m.nav_held.is_some());
        into_view(&mut m);
        assert_eq!((m.view, m.focus), (1, Some(Focus::Item)));
        assert_eq!(m.trail_view(), Some(m.current().unwrap().slug()));
        assert!(m.take_settled_walk(), "the walk settles at the next sync");
        assert!(m.nav_held.is_none());
        // With the list's filter hiding the view in front, Shift-Tab comes
        // back to the first row it leaves, which Enter opens.
        m.nav_filter = Some(Filter {
            text: "settings".into(),
            typing: false,
        });
        m.focus = Some(Focus::Filter);
        move_focus(&mut m, -1);
        assert_eq!(m.focus, Some(Focus::View(3)));
    }

    #[test]
    fn with_the_list_not_following_j_leaves_the_view_in_front() {
        let mut m = mission(1, vec![view(ViewId::Prs, "PRs")]);
        m.focus = Some(Focus::View(0));
        nav_key(&mut m, Key::new(KeyCode::Char('j'), Mods::NONE));
        assert_eq!((m.view, m.focus), (0, Some(Focus::View(1))));
    }

    #[test]
    fn v_goes_to_a_view_by_the_start_of_its_name_else_by_any_part() {
        let mut m = mission(1, vec![list("Reopen", 1), list("Merge queues", 1)]);
        m.jump = Some(Jump::default());
        for c in "que".chars() {
            jump_key(&mut m, Key::new(KeyCode::Char(c), Mods::NONE));
        }
        assert_eq!(m.jump.as_ref().unwrap().matched, Some(2));
        jump_key(&mut m, Key::new(KeyCode::Enter, Mods::NONE));
        assert_eq!((m.view, m.jump.is_none()), (2, true));
    }

    #[test]
    fn v_in_a_grid_focuses_a_view_shown_and_leaves_it_for_another() {
        let views = vec![
            view(ViewId::Reopen, "Reopen"),
            view(ViewId::Landing, "Merge queues"),
            view(ViewId::Inbox, "Inbox"),
        ];
        let mut m = mission(1, views);
        let choice = super::super::grid::Choice {
            views: vec!["tabs".into(), "landing".into()],
            layout: super::super::grid::Layout::Side,
        };
        m.grid = Some(super::super::grid::Grid::new(choice));
        m.settle_grid();
        assert_eq!(m.view, 0);
        let jump = |m: &mut Mission, text: &str| {
            m.jump = Some(Jump::default());
            for c in text.chars() {
                jump_key(m, Key::new(KeyCode::Char(c), Mods::NONE));
            }
            jump_key(m, Key::new(KeyCode::Enter, Mods::NONE));
        };
        jump(&mut m, "merge");
        m.settle_grid();
        assert_eq!((m.view, m.grid.as_ref().map(|g| g.focus)), (2, Some(1)));
        jump(&mut m, "inbox");
        m.settle_grid();
        assert_eq!((m.view, m.grid.is_none()), (3, true));
    }

    #[test]
    fn j_and_k_move_a_row_of_cards_keeping_the_column() {
        // Five working tabs, four across: rows of 4 and 1.
        let mut m = mission(5, Vec::new());
        for t in &mut m.snap.tabs {
            t.claude = Some(crate::sidebar::claude::SessionRef::default());
            t.state = crate::sidebar::model::TabState::Working;
        }
        m.size = (3 * 56, 40);
        m.cursor = 2;
        assert_eq!(
            row_step(&m, 1),
            4,
            "under the third, the last of the shorter row"
        );
        m.cursor = 4;
        assert_eq!(row_step(&m, -1), 0);
        assert_eq!(row_step(&m, 1), 4, "nothing below the last row");
        m.cursor = 1;
        assert_eq!(row_step(&m, -1), 1, "nothing above the first row");
    }

    // test/tmux-sidebar.test.js's "k from the top row of tabs reaches the
    // line of the note worked continuously", and Tab stops on it too.
    #[test]
    fn k_from_the_first_row_reaches_the_line_of_the_note_worked_continuously() {
        let mut m = mission(5, Vec::new());
        m.snap.pilot = Some(crate::autopilot::Summary {
            note: "Work".into(),
            busy: 0,
            open: 0,
            slots: 3,
            cap: 5,
            next: String::new(),
            paused: false,
            away: false,
            ready_only: false,
            skipped: 0,
            spend_limit: 0.0,
            spent: 0.0,
            weekly_limit: 0,
            weekly: None,
            lands_alone: false,
            hands_free: false,
            target: String::new(),
            drained: false,
            failing: String::new(),
        });
        m.cursor = 0;
        let to = row_step(&m, -1);
        move_to(&mut m, to, 5);
        assert_eq!(m.cursor, -1, "on the mode's line");
        let to = row_step(&m, 1);
        move_to(&mut m, to, 5);
        assert_eq!(m.cursor, 0, "j goes back down to the first tab");
        m.focus = Some(Focus::Filter);
        move_focus(&mut m, 1);
        assert_eq!((m.focus, m.cursor), (Some(Focus::Item), -1), "a Tab stop");
    }

    #[test]
    fn h_and_l_on_a_board_go_to_the_next_column_with_items_at_the_same_row() {
        let item = |n: u32, column: &str| Item::new(n.to_string(), format!("#{n}")).group(column);
        let board = View {
            id: ViewId::Prs,
            title: "PRs".into(),
            data: ViewData {
                items: vec![
                    item(1, "Draft"),
                    item(2, "In review"),
                    item(3, "In review"),
                    item(4, "Merged"),
                ],
                ..ViewData::default()
            },
            loaded: true,
        };
        let mut m = mission(0, vec![board]);
        m.view = 1;
        m.cursor = 0;
        assert_eq!(side_step(&m, 1), 1);
        m.cursor = 2;
        assert_eq!(side_step(&m, 1), 3, "past the empty columns");
        assert_eq!(side_step(&m, -1), 0);
    }

    #[test]
    fn h_and_l_go_along_the_row_of_the_week_s_cards() {
        let item = |n: u32, column: &str| Item::new(n.to_string(), format!("#{n}")).group(column);
        let columns = crate::sources::prboard::COLUMNS;
        let week = columns[columns.len() - 1];
        let board = View {
            id: ViewId::Prs,
            title: "PRs".into(),
            data: ViewData {
                items: vec![item(1, "In review"), item(2, week), item(3, week)],
                ..ViewData::default()
            },
            loaded: true,
        };
        let mut m = mission(0, vec![board]);
        m.view = 1;
        m.cursor = 1;
        assert_eq!(side_step(&m, 1), 2, "along the row");
        m.cursor = 2;
        assert_eq!(side_step(&m, 1), 2, "its last card stays");
        assert_eq!(side_step(&m, -1), 1);
        m.cursor = 1;
        assert_eq!(side_step(&m, -1), 0, "from its first card to the columns");
    }

    #[test]
    fn in_a_short_cell_l_passes_over_the_week_row_it_leaves_out() {
        let item = |n: u32, column: &str| Item::new(n.to_string(), format!("#{n}")).group(column);
        let columns = crate::sources::prboard::COLUMNS;
        let week = columns[columns.len() - 1];
        let board = View {
            id: ViewId::Prs,
            title: "PRs".into(),
            data: ViewData {
                items: vec![item(1, columns[0]), item(2, week)],
                ..ViewData::default()
            },
            loaded: true,
        };
        let mut m = mission(0, vec![board, view(ViewId::Inbox, "Inbox")]);
        let choice = super::super::grid::Choice {
            views: vec!["prs".into(), "inbox".into()],
            layout: super::super::grid::Layout::Side,
        };
        m.grid = Some(super::super::grid::Grid::new(choice));
        m.settle_grid();
        m.cells = vec![(0, (60, 12))];
        m.cursor = 0;
        assert_eq!(side_step(&m, 1), 0, "nothing shown to its right");
        m.cells = vec![(0, (60, 30))];
        assert_eq!(side_step(&m, 1), 1, "the week row, once it fits");
    }

    #[test]
    fn h_and_l_on_the_autopilot_board_go_by_column_past_its_headings() {
        let item = |id: &str, column: Option<&str>| {
            let i = Item::new(id, id);
            match column {
                Some(c) => i.group(c),
                None => i,
            }
        };
        let board = View {
            id: ViewId::Autopilot,
            title: "Autopilot".into(),
            data: ViewData {
                items: vec![
                    item("heading", None),
                    item("next 1", Some("Next up")),
                    item("next 2", Some("Next up")),
                    item("tab 4", Some("Waiting on you")),
                    item("tab 3", Some("Landed")),
                    item("skipped heading", None),
                    item("skipped", Some("Skipped")),
                ],
                ..ViewData::default()
            },
            loaded: true,
        };
        let mut m = mission(0, vec![board]);
        m.size = (120, 40);
        m.view = 1;
        m.cursor = 2;
        assert_eq!(side_step(&m, 1), 3, "past the empty Working column");
        m.cursor = 3;
        assert_eq!(side_step(&m, 1), 4);
        assert_eq!(side_step(&m, -1), 1, "the same row, as far as it reaches");
        m.cursor = 4;
        assert_eq!(side_step(&m, 1), 6, "Skipped, past its heading");
        m.cursor = 6;
        assert_eq!(side_step(&m, 1), 6, "the last column stays");
        assert_eq!(side_step(&m, -1), 4);
    }

    #[test]
    fn the_autopilot_drawing_layout_footer_and_keys_agree_on_board_or_list() {
        let pilot = |items: Vec<Item>, loaded: bool| {
            let mut m = mission(
                0,
                vec![View {
                    id: ViewId::Autopilot,
                    title: "Autopilot".into(),
                    data: ViewData {
                        items,
                        ..ViewData::default()
                    },
                    loaded,
                }],
            );
            m.size = (120, 30);
            m.view = 1;
            m.cursor = 0;
            m
        };
        let cards = || {
            vec![
                Item::new("next", "next").group("Next up"),
                Item::new("tab 4", "tab 4").group("Waiting on you"),
            ]
        };
        // What each of the four copies makes of it: the drawing (the board
        // draws its empty Working column), the layout's sections (none on a
        // board), the footer's keys and `l`.
        let seen = |m: &Mission| {
            let (lines, _) = super::super::render::screen(m, 120, 30);
            let text: Vec<String> = lines
                .iter()
                .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
                .collect();
            let drawn = text.iter().any(|l| l.to_lowercase().contains("working"));
            let footer = text.last().is_some_and(|l| l.contains("column"));
            let laid = m.sections().is_empty();
            let keys = side_step(m, 1) != m.cursor;
            [drawn, laid, footer, keys]
        };
        assert_eq!(seen(&pilot(cards(), true)), [true; 4], "the board");
        let unloaded = seen(&pilot(cards(), false));
        assert_eq!(unloaded, [false; 4], "the list until it loads");
        let mut empty = pilot(Vec::new(), true);
        empty.cursor = -1;
        let (lines, _) = super::super::render::screen(&empty, 120, 30);
        let footer: String = lines
            .last()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .unwrap_or_default();
        assert!(
            !footer.contains("column"),
            "no items, the list's keys: {footer}"
        );
    }

    #[test]
    fn in_the_ci_view_a_footer_key_and_a_check_line_take_a_click() {
        let mut m = ci_mission();
        let (_, hits) = super::super::render::screen(&m, 160, 20);
        m.size = (160, 20);
        m.hits = hits;
        let at = |m: &Mission, spot: Spot| {
            let h = m.hits.iter().find(|h| h.spot == spot).expect("drawn");
            (h.x0, h.y)
        };
        let click = |(col, row): (u16, u16)| Mouse {
            kind: MouseKind::Press,
            button: MouseButton::Left,
            col,
            row,
            mods: Mods::NONE,
        };
        let enter = Key::new(KeyCode::Enter, Mods::NONE);
        let footer = at(&m, Spot::Key(enter));
        assert_eq!(footer.1, 19, "on the footer");
        assert_eq!(
            mouse_in(&mut m, click(footer), true),
            Then::Press(enter),
            "pressed as the key is, so a filter being typed keeps it"
        );
        let second = at(&m, Spot::Check(1, 1));
        assert_eq!(mouse_in(&mut m, click(second), true), Then::Prepare);
        assert_eq!(m.chosen_check(), Some(1));
        assert_eq!(mouse_in(&mut m, click(second), true), Then::Enter);
    }

    #[test]
    fn a_click_on_a_switch_while_a_filter_is_typed_ends_it_and_flips_the_row() {
        let mut m = mission(0, vec![list("Closed", 3)]);
        m.view = 1;
        m.filter = Some(Filter {
            text: "x".into(),
            typing: true,
        });
        m.hits = vec![crate::mission::HitBox {
            y: 3,
            x0: 10,
            x1: 13,
            spot: Spot::Menu(2),
        }];
        let click = Mouse {
            kind: MouseKind::Press,
            button: MouseButton::Left,
            col: 11,
            row: 3,
            mods: Mods::NONE,
        };
        assert_eq!(mouse_in(&mut m, click, true), Then::Enter);
        assert_eq!(m.cursor, 2);
        assert!(m.filter.as_ref().is_some_and(|f| !f.typing));
    }

    /// The Tickets board on a screen 160 wide, its one ticket
    /// chosen, its description beside it.
    fn tickets_board() -> Mission {
        let ticket = Item::new("1", "Fix it")
            .group("Todo")
            .data(serde_json::json!({ "description": "Do it." }));
        let mut tickets = list("Tickets", 0);
        tickets.id = ViewId::Tickets;
        tickets.data.items = vec![ticket];
        let mut m = mission(1, vec![tickets]);
        m.view = 1;
        m.size = (160, 30);
        m
    }

    #[test]
    fn the_resize_keys_move_the_left_edge_of_the_pane_beside_the_board_in_front() {
        assert_eq!(
            super::super::side_keys(&crate::mux::keys::Bindings::default()),
            "C-M-S-h C-M-S-l",
            "as the footer names them"
        );
        let mut m = tickets_board();
        assert!(m.resize_side(-5), "left widens it");
        assert_eq!(m.side_widths.get("tickets"), Some(&55));
        assert_eq!(m.side_unsaved.as_deref(), Some("tickets"));
        assert!(m.resize_side(10), "right narrows it");
        assert_eq!(m.side_widths.get("tickets"), Some(&45));
        assert_eq!(m.side_widths.get("prs"), None, "the board in front alone");
        m.side_widths
            .insert("tickets".into(), super::super::render::SIDE_MIN);
        assert!(!m.resize_side(5), "no narrower than the least");
        // With no pane shown (a narrow screen has it under the board),
        // nothing.
        m.size = (120, 30);
        assert!(!m.resize_side(-5));
        assert_eq!(
            m.side_widths.get("tickets"),
            Some(&super::super::render::SIDE_MIN)
        );
        // A ticket with no description shows its pane all the same, and
        // resizes.
        m.size = (160, 30);
        m.snap.views[0].data.items[0].data = serde_json::json!({});
        assert!(m.resize_side(-5));
    }

    #[test]
    fn a_drag_of_the_panes_edge_resizes_it_to_the_mouse() {
        let mut m = tickets_board();
        let (_, hits) = super::super::render::screen(&m, 160, 30);
        m.hits = hits;
        let edge = *m
            .hits
            .iter()
            .find(|h| matches!(h.spot, Spot::SideEdge(_)))
            .expect("the pane's edge");
        assert_eq!((edge.x0, edge.spot), (110, Spot::SideEdge(50)));
        let mouse = |kind, col| Mouse {
            kind,
            button: MouseButton::Left,
            col,
            row: edge.y,
            mods: Mods::NONE,
        };
        assert_eq!(
            mouse_in(&mut m, mouse(MouseKind::Press, 110), true),
            Then::Redraw
        );
        assert_eq!(m.side_drag, Some(160));
        assert_eq!(
            mouse_in(&mut m, mouse(MouseKind::Drag, 95), true),
            Then::Redraw
        );
        assert_eq!(m.side_widths.get("tickets"), Some(&65));
        save_side(&mut m);
        assert_eq!(
            m.side_unsaved.as_deref(),
            Some("tickets"),
            "kept while dragged"
        );
        assert_eq!(
            mouse_in(&mut m, mouse(MouseKind::Release, 95), true),
            Then::Redraw
        );
        assert_eq!(m.side_drag, None);
    }

    #[test]
    fn a_click_on_a_slack_threads_picture_plays_it_with_no_key() {
        let mut m = mission(1, vec![list("Reopen", 1)]);
        m.view = 1;
        m.size = (120, 20);
        m.filter = Some(Filter {
            text: "x".into(),
            typing: true,
        });
        m.hits = vec![crate::mission::HitBox {
            y: 5,
            x0: 10,
            x1: 20,
            spot: Spot::Picture(2),
        }];
        let click = Mouse {
            kind: MouseKind::Press,
            button: MouseButton::Left,
            col: 12,
            row: 5,
            mods: Mods::NONE,
        };
        assert_eq!(mouse_in(&mut m, click, true), Then::SlackPlay);
        assert_eq!(m.slack_clicked, Some(2));
        assert!(!m.filter.as_ref().unwrap().typing, "the filter done typing");
    }

    #[test]
    fn a_click_chooses_a_view_or_a_row_and_a_second_click_takes_it() {
        // A plain list: the Reopen view's board needs its rows' kinds.
        let plain = View {
            id: ViewId::WorkMap,
            ..list("Closed", 3)
        };
        let mut m = mission(2, vec![plain]);
        m.view = 1;
        let draw = |m: &mut Mission| {
            let (_, hits) = super::super::render::screen(m, 120, 20);
            m.size = (120, 20);
            m.hits = hits;
        };
        draw(&mut m);
        let at = |m: &Mission, spot: Spot| {
            let h = m.hits.iter().find(|h| h.spot == spot).expect("drawn");
            (h.x0 + 1, h.y)
        };
        let mouse = |kind, button, (col, row): (u16, u16)| Mouse {
            kind,
            button,
            col,
            row,
            mods: Mods::NONE,
        };
        // The mouse over the second row lights it; over the chosen one, not.
        let row = at(&m, Spot::Index(1));
        assert_eq!(
            mouse_in(&mut m, mouse(MouseKind::Move, MouseButton::None, row), true),
            Then::Redraw
        );
        assert_eq!(m.hover, Some(Spot::Index(1)));
        // The pointer is a hand over a row, the arrow off the rows.
        use crate::ui::{Overlay, pointer::Pointer};
        let look = crate::ui::Look::default();
        assert_eq!(m.pointer(row.0, row.1, &look), Some(Pointer::Hand));
        let bare = (0..20u16)
            .flat_map(|y| (0..120u16).map(move |x| (x, y)))
            .find(|&(x, y)| !m.hits.iter().any(|h| h.y == y && x >= h.x0 && x <= h.x1))
            .expect("a cell nothing is on");
        assert_eq!(m.pointer(bare.0, bare.1, &look), Some(Pointer::Arrow));
        let chosen = at(&m, Spot::Index(0));
        mouse_in(
            &mut m,
            mouse(MouseKind::Move, MouseButton::None, chosen),
            true,
        );
        assert_eq!(m.hover, None);
        // A click chooses it, a second takes it.
        assert_eq!(
            mouse_in(
                &mut m,
                mouse(MouseKind::Press, MouseButton::Left, row),
                true
            ),
            Then::Prepare
        );
        assert_eq!(m.cursor, 1);
        assert_eq!(
            mouse_in(
                &mut m,
                mouse(MouseKind::Press, MouseButton::Left, row),
                true
            ),
            Then::Enter
        );
    }

    fn settings_mission() -> Mission {
        let head = |id: &str| Item::new(id, id).data(serde_json::json!({ "header": true }));
        let option = |id: &str| Item::new(id, id);
        let view = View {
            id: ViewId::Settings,
            title: "Settings".into(),
            data: ViewData {
                items: vec![
                    head("header:Look"),
                    option("a"),
                    option("b"),
                    head("header:Tabs"),
                    option("c"),
                ],
                ..ViewData::default()
            },
            loaded: true,
        };
        let mut m = mission(1, vec![view]);
        m.size = (160, 40);
        m.view = 1;
        m.cursor = 2;
        m
    }

    #[test]
    fn c_w_h_to_a_filtered_list_of_views_chooses_a_view_it_shows() {
        let key = |k: &str| Key::parse(k).unwrap();
        let mut m = settings_mission();
        m.nav_filter = Some(Filter {
            text: "tabs".into(),
            typing: false,
        });
        window_key(&mut m, key("h"));
        window_key(&mut m, key("h"));
        assert_eq!(m.focus, Some(Focus::View(0)), "the Settings row is hidden");
    }

    #[test]
    fn c_w_h_focuses_the_settings_categories_whose_j_walks_them_and_c_w_l_goes_back() {
        let key = |k: &str| Key::parse(k).unwrap();
        let mut m = settings_mission();
        assert_eq!(m.panes(), vec![Pane::Panel, Pane::List]);
        assert!(window_key(&mut m, key("h")));
        assert_eq!(m.pane_now(), Pane::Panel);
        assert!(window_key(&mut m, key("h")), "on to the list of views");
        assert_eq!(m.focus, Some(Focus::View(1)));
        assert!(window_key(&mut m, key("h")), "nothing further left");
        assert_eq!(m.focus, Some(Focus::View(1)));
        window_key(&mut m, key("l"));
        assert_eq!((m.pane_now(), m.focus), (Pane::Panel, Some(Focus::Item)));
        assert!(pane_key(&mut m, key("j")));
        assert_eq!(m.cursor, 4, "the next category's first option");
        assert!(pane_key(&mut m, key("k")));
        assert_eq!(m.cursor, 1);
        assert!(
            !pane_key(&mut m, key("m")),
            "the view's menu stays the view's"
        );
        assert!(window_key(&mut m, key("C-l")));
        assert_eq!(m.pane_now(), Pane::List);
        assert!(!pane_key(&mut m, key("j")), "the list moves as it would");
        window_key(&mut m, key("h"));
        assert!(pane_key(&mut m, key("Enter")));
        assert_eq!(m.pane_now(), Pane::List, "Enter goes back into the options");
        assert!(
            !window_key(&mut m, key("x")),
            "any other key is taken as itself"
        );
    }

    #[test]
    fn a_pane_under_the_list_stays_lit_and_one_in_the_lists_place_is_greyed() {
        use ratatui::layout::Rect;
        let area = Rect::new(0, 0, 100, 10);
        let side_rows = |m: &mut Mission, x0: u16| {
            for y in 5..9 {
                m.hits.push(HitBox {
                    y,
                    x0,
                    x1: 98,
                    spot: Spot::Side(10),
                });
            }
        };
        // A pane stacked under the list's rows, from the left edge: a
        // preview.
        let mut m = ci_mission();
        m.hits.push(HitBox {
            y: 2,
            x0: 2,
            x1: 60,
            spot: Spot::Index(1),
        });
        side_rows(&mut m, 0);
        assert_eq!(m.pane_now(), Pane::List);
        assert_eq!(m.without_keys(area), vec![]);
        // A pane its board drew in the list's place (a narrow Slack's
        // thread): no preview.
        let mut m = ci_mission();
        m.hits.push(HitBox::in_place(5));
        side_rows(&mut m, 1);
        assert_eq!(m.pane_now(), Pane::List);
        let greyed = m.without_keys(area);
        assert_eq!(greyed.len(), 4, "{greyed:?}");
        assert!(greyed.iter().all(|r| (r.x, r.right()) == (1, 99)));
    }

    #[test]
    fn the_list_stays_lit_from_the_panel_over_it_and_the_panel_is_greyed_from_the_list() {
        use ratatui::layout::Rect;
        let key = |k: &str| Key::parse(k).unwrap();
        let mut m = settings_mission();
        let area = Rect::new(0, 0, 160, 10);
        for y in 2..5 {
            m.hits.push(HitBox {
                y,
                x0: 0,
                x1: 19,
                spot: Spot::Category(y as isize),
            });
        }
        // The list has the keys: the panel's columns are greyed.
        assert_eq!(m.pane_now(), Pane::List);
        let greyed = m.without_keys(area);
        assert!(!greyed.is_empty());
        assert!(
            greyed.iter().all(|r| (r.x, r.right()) == (0, 20)),
            "{greyed:?}"
        );
        // The panel has them: the options under it stay lit.
        window_key(&mut m, key("h"));
        assert_eq!(m.pane_now(), Pane::Panel);
        assert_eq!(m.without_keys(area), vec![]);
    }

    #[test]
    fn every_pane_without_the_keys_is_greyed_and_the_view_while_the_list_of_views_has_them() {
        use ratatui::layout::Rect;
        let key = |k: &str| Key::parse(k).unwrap();
        let mut m = ci_mission();
        let (nav, area) = (Rect::new(0, 0, 20, 10), Rect::new(21, 0, 139, 10));
        m.nav_area = Some(nav);
        assert_eq!(
            m.without_keys(area),
            vec![nav],
            "a view of one pane: only the list of views"
        );
        m.hits.push(HitBox {
            y: 2,
            x0: 2,
            x1: 70,
            spot: Spot::Index(1),
        });
        for y in 2..9 {
            m.hits.push(HitBox {
                y,
                x0: 80,
                x1: 138,
                spot: Spot::Side(10),
            });
        }
        // The list has the keys: the pane beside shows its chosen row, so
        // only the list of views is greyed.
        assert_eq!(m.without_keys(area), vec![nav]);
        // The pane beside has them: the list is, up to the rule between.
        window_key(&mut m, key("l"));
        assert_eq!(m.pane_now(), Pane::Beside);
        let greyed = m.without_keys(area);
        assert_eq!((greyed[0], greyed.len()), (nav, 8));
        assert!(
            greyed[1..]
                .iter()
                .all(|r| (r.x, r.right()) == (21, 21 + 80 - super::super::RULE_ROOM)),
            "{greyed:?}"
        );
        // The list of views has them: the whole view is.
        m.focus = Some(Focus::View(1));
        assert_eq!(m.without_keys(area), vec![area]);
    }

    #[test]
    fn c_w_l_focuses_the_pane_beside_whose_j_scrolls_it_and_another_view_has_its_list() {
        let key = |k: &str| Key::parse(k).unwrap();
        let mut m = ci_mission();
        assert_eq!(
            m.panes(),
            vec![Pane::List],
            "no pane beside until one is drawn"
        );
        assert!(window_key(&mut m, key("l")));
        assert_eq!(m.pane_now(), Pane::List);
        m.hits.push(HitBox {
            y: 3,
            x0: 80,
            x1: 120,
            spot: Spot::Side(10),
        });
        window_key(&mut m, key("l"));
        assert_eq!(m.pane_now(), Pane::Beside);
        let cursor = m.cursor;
        assert!(pane_key(&mut m, key("j")));
        pane_key(&mut m, key("j"));
        assert_eq!(
            (m.cursor, m.side_top()),
            (cursor, 2),
            "scrolled, the same row chosen"
        );
        pane_key(&mut m, key("G"));
        assert_eq!(m.side_top(), 10);
        pane_key(&mut m, key("g"));
        assert_eq!(m.side_top(), 0);
        window_key(&mut m, key("w"));
        assert_eq!(
            m.focus,
            Some(Focus::View(1)),
            "w goes round to the list of views"
        );
        window_key(&mut m, key("w"));
        assert_eq!((m.pane_now(), m.focus), (Pane::List, Some(Focus::Item)));
        window_key(&mut m, key("W"));
        window_key(&mut m, key("W"));
        assert_eq!(m.pane_now(), Pane::Beside);
        m.focus = None;
        // The wheel over the list moves the list, giving it the focus; a
        // control Tab's focus is on takes Enter before the pane does.
        let wheel = Mouse {
            kind: MouseKind::Press,
            button: MouseButton::WheelDown,
            col: 5,
            row: 5,
            mods: Mods::NONE,
        };
        assert_eq!(
            mouse_in(&mut m, wheel, true),
            Then::Key(Key::new(KeyCode::Char('j'), Mods::NONE))
        );
        assert_eq!(m.pane_now(), Pane::List);
        m.view = 0;
        assert_eq!(m.pane_now(), Pane::List, "another view has its list");
    }

    fn usage_mission() -> Mission {
        let still = |id: &str| Item::new(id, id).data(serde_json::json!({ "still": true }));
        let row = |id: &str| Item::new(id, id);
        let view = View {
            id: ViewId::Usage,
            title: "Usage".into(),
            data: ViewData {
                items: vec![
                    still("spent"),
                    still("limit:week"),
                    row("tab:a"),
                    row("tab:b"),
                    row("requests:github"),
                    row("requests:linear"),
                    row("session:1"),
                    row("session:2"),
                ],
                ..ViewData::default()
            },
            loaded: true,
        };
        let mut m = mission(1, vec![view]);
        m.view = 1;
        m.cursor = 3;
        m
    }

    #[test]
    fn usage_sections_are_its_tabs_services_and_sessions() {
        assert_eq!(usage_mission().sections(), vec![2, 4, 6]);
    }

    #[test]
    fn c_w_j_and_k_move_between_a_views_stacked_sections() {
        let key = |k: &str| Key::parse(k).unwrap();
        let mut m = usage_mission();
        assert!(window_key(&mut m, key("j")));
        assert_eq!(m.cursor, 4, "the first service");
        assert!(window_key(&mut m, key("C-j")));
        assert_eq!(m.cursor, 6, "the first session");
        assert!(window_key(&mut m, key("j")));
        assert_eq!(m.cursor, 6, "no section further down");
        m.cursor = 7;
        assert!(window_key(&mut m, key("k")));
        assert_eq!(m.cursor, 4, "the section before the one it is in");
        assert!(window_key(&mut m, key("k")));
        assert!(window_key(&mut m, key("k")));
        assert_eq!(m.cursor, 2, "no section further up");
        let mut one = mission(1, vec![list("Reopen", 3)]);
        one.view = 1;
        assert!(one.sections().is_empty());
        assert!(
            !window_key(&mut one, key("j")),
            "a view of one section takes it as itself"
        );
    }

    #[test]
    fn sections_follow_headings_and_groups() {
        let head = |id: &str| Item::new(id, id).data(serde_json::json!({ "header": true }));
        let in_group = |id: &str, g: &str| {
            let mut i = Item::new(id, id);
            i.group = Some(g.into());
            i
        };
        let view = View {
            id: ViewId::Reopen,
            title: "Reopen".into(),
            data: ViewData {
                items: vec![
                    in_group("a", "Tabs"),
                    in_group("b", "Tabs"),
                    in_group("c", "Sessions"),
                ],
                ..ViewData::default()
            },
            loaded: true,
        };
        let mut m = mission(1, vec![view]);
        m.view = 1;
        assert_eq!(m.sections(), vec![0, 2]);
        let view = View {
            id: ViewId::Notifications,
            title: "Notifications".into(),
            data: ViewData {
                items: vec![
                    head("header:x"),
                    Item::new("a", "a"),
                    head("header:y"),
                    Item::new("b", "b"),
                ],
                ..ViewData::default()
            },
            loaded: true,
        };
        m.snap.views = vec![view];
        assert_eq!(m.sections(), vec![1, 3]);
        // The Work map's repositories are headings the cursor rests on: a
        // folded one, with nothing under it, is a section of its own, and
        // the line merged work folds under stays in its repository's.
        let view = View {
            id: ViewId::WorkMap,
            title: "Work map".into(),
            data: ViewData {
                items: vec![
                    head("repo:a"),
                    Item::new("a1", "a1"),
                    head("merged:a"),
                    head("repo:b"),
                    head("repo:c"),
                    Item::new("c1", "c1"),
                ],
                ..ViewData::default()
            },
            loaded: true,
        };
        m.snap.views = vec![view];
        assert_eq!(m.sections(), vec![0, 3, 4]);
    }

    #[test]
    fn the_inbox_board_s_sections_are_its_days() {
        let day = 86_400_000;
        let at = |id: &str, t: i64| {
            let mut i = Item::new(id, id);
            i.time = Some(t);
            i
        };
        let view = View {
            id: ViewId::Inbox,
            title: "Inbox".into(),
            data: ViewData {
                items: vec![
                    at("a", 10 * day + 1_000),
                    at("b", 10 * day + 2_000),
                    at("c", 9 * day + 1_000),
                ],
                ..ViewData::default()
            },
            loaded: true,
        };
        let mut m = mission(1, vec![view]);
        m.snap.tz = jiff::tz::TimeZone::UTC;
        m.view = 1;
        assert_eq!(m.sections(), vec![0, 2]);
    }

    #[test]
    fn c_w_waits_a_second_for_its_second_key() {
        let mut m = Mission::new();
        m.window_key = Some(1_000);
        assert!(m.window_waits(1_000 + WINDOW_WAIT_MS));
        assert!(!m.window_waits(1_001 + WINDOW_WAIT_MS));
    }

    #[test]
    fn leaving_the_slack_board_marks_seen_what_its_sections_showed_of_the_whole_look() {
        let mut m = Mission::new();
        m.slack_all = ["a", "b"].into_iter().map(String::from).collect();
        assert_eq!(m.slack_seen(), None, "the plain list: the whole look");
        m.slack_viewed = Some(Default::default());
        assert_eq!(
            m.slack_seen(),
            Some(Vec::new()),
            "the board, a filter leaving it nothing: nothing seen"
        );
        m.slack_viewed
            .get_or_insert_default()
            .insert("a".into(), crate::items::Item::new("a", "Ada"));
        let seen = m.slack_seen().expect("one section shown");
        assert_eq!(
            seen.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(),
            ["a"]
        );
        // A filter leaving only "a" in the view doesn't make "b" shown.
        m.slack_viewed
            .get_or_insert_default()
            .insert("b".into(), crate::items::Item::new("b", "Bo"));
        assert_eq!(m.slack_seen(), None, "every one shown");
    }
}
