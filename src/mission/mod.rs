//! Mission control (M-Space): every tab as a card with its latest Claude
//! turn, in stages by what needs you, and every view the sidebar daemon
//! drew beside them (lib/sidebar/daemon.js's mission client, render.js's
//! `missionLines`, views.js), now an overlay the server draws over a
//! client's whole screen, from the same model the sidebar draws from, so the
//! two never disagree about a tab.
//!
//! Its own keys only move about: h j k l, g G, numbers, `[` `]` between
//! views, Tab and Shift-Tab moving focus (with tab_focus), `v` to go to a
//! view by name, `/` to filter, `?` for its keys, Esc and q to close, M-w
//! closing it whatever is open over it, and `r` for a plugin's view.
//! Everything that acts on what is chosen is in the menu Enter opens on it,
//! a box over the view; an option that takes a moment shows a box with a
//! spinner until it is done. A click takes what it lands on, the wheel
//! moves, and with hover_highlight the mouse lights what a click would
//! press.
//!
//! The overlay (`Mission`) holds only what one client is doing with it (the
//! view, the cursor, the filter, the focus) and a snapshot of what it shows,
//! which the server takes before each draw and each key (`serve`), so
//! drawing is a plain function of the snapshot (`render`), and the keys are
//! handled with the whole server at hand.
//!
//! - `views`: the views after the tabs, the Work map and Settings among them.
//! - `render`: the screen, as lines, and where a click lands.
//! - `calendar`: the Calendar view drawn as a day, hours down the side.
//! - `choices`: what the menu Enter opens offers, as data.
//! - `answer`: answering a tab's prompt, as the Slack relay does.
//! - `ask`: a line to type in a box.
//! - `busy`: the box an option shows while it works.
//! - `remind`: a tab's reminder (remind.sh).
//! - `serve`: the server's side: opening it, its keys and mouse, the menus,
//!   the reminders said once, the worktrees read in the background.

pub mod answer;
pub mod ask;
pub mod busy;
pub mod calendar;
pub mod choices;
pub mod remind;
pub mod render;
mod serve;
pub mod views;

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;

use ratatui::buffer::Buffer;

use crate::items::Item;
use crate::mux::input::{Key, Mouse};
use crate::mux::server::ClientId;
use crate::mux::{PaneId, TabId};
use crate::sidebar::model::{Rules, TabModel};
use crate::sidebar::render::Filter;
use crate::ui::{Look, Overlay, Step, Theme};

pub use serve::{Opening, open_link};
pub use views::{Revived, Scope, View, ViewId, Worktree};

/// Where Tab has put the focus: a view's name in the bar, the filter, the
/// chosen item, or easy mode's × (render.js's `FOCUS`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Focus {
    View(usize),
    Filter,
    Item,
    Close,
}

/// What a click on a part of the screen means, and what the mouse is over:
/// a view's name, the filter, a card or a row (by its place in the view;
/// -1 is a line above the cards), or easy mode's ×.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Spot {
    View(usize),
    Filter,
    Index(isize),
    Close,
}

/// Where a click on the screen lands: row `y`, columns `x0` to `x1`
/// (inclusive), counted from 0.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HitBox {
    pub y: u16,
    pub x0: u16,
    pub x1: u16,
    pub spot: Spot,
}

/// A view's name being typed after `v`, and the view it names.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Jump {
    pub text: String,
    pub matched: Option<usize>,
}

/// What mission control shows, as the server last put it together: the
/// tabs (narrowed by the filter) and the views after them (each as this
/// client sees it), with what drawing them depends on.
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub tabs: Vec<TabModel>,
    pub views: Vec<View>,
    pub rules: Rules,
    pub theme: Theme,
    /// sidebar.json's cardWidth and cardHeight.
    pub card: (u16, u16),
    pub tab_focus: bool,
    /// Easy mode's × at the bar's right end.
    pub close: bool,
    pub now_secs: i64,
    /// The time zone the Calendar's day is drawn in.
    pub tz: jiff::tz::TimeZone,
    /// The spinner's frame.
    pub frame: usize,
    /// The line of the note worked continuously, leading the tabs while
    /// the mode is on (src/autopilot); the cursor's -1 is on it.
    pub pilot: Option<crate::autopilot::Summary>,
}

impl Default for Snapshot {
    fn default() -> Snapshot {
        Snapshot {
            tabs: Vec::new(),
            views: Vec::new(),
            rules: Rules {
                now_ms: 0,
                reminders: true,
                build_failing: true,
            },
            theme: Theme::DARK,
            card: (56, 13),
            tab_focus: true,
            close: false,
            now_secs: 0,
            tz: jiff::tz::TimeZone::UTC,
            frame: 0,
            pilot: None,
        }
    }
}

/// One client's mission control.
pub struct Mission {
    /// The view in front: 0 the tabs, else `snap.views[view - 1]`.
    pub view: usize,
    /// The chosen card or row, by its place in the view.
    pub cursor: isize,
    pub filter: Option<Filter>,
    pub jump: Option<Jump>,
    pub focus: Option<Focus>,
    pub hover: Option<Spot>,
    /// M-j's and M-o's picker: the Work map scoped, a filter typing, Enter
    /// going to the chosen row and Esc closing.
    pub scope: Option<Scope>,
    pub picker: bool,
    /// The Work map's merged work shown under its header.
    pub show_merged: bool,
    /// What the last action did, in the footer until the next key.
    pub note: Option<String>,
    pub snap: Snapshot,
    /// Where each part was last drawn, for clicks and the mouse.
    pub hits: Vec<HitBox>,
    /// The screen it was last drawn on.
    pub size: (u16, u16),
    /// The view last brought to the front, by its title ("Tabs" for the
    /// cards), for what going to one does.
    pub(crate) came: Option<String>,
}

impl Mission {
    pub fn new() -> Mission {
        Mission {
            view: 0,
            cursor: 0,
            filter: None,
            jump: None,
            focus: None,
            hover: None,
            scope: None,
            picker: false,
            show_merged: false,
            note: None,
            snap: Snapshot::default(),
            hits: Vec::new(),
            size: (0, 0),
            came: None,
        }
    }

    /// The view in front, after the tabs; `None` on the tabs.
    pub fn current(&self) -> Option<&View> {
        self.view
            .checked_sub(1)
            .and_then(|i| self.snap.views.get(i))
    }

    /// The tabs in the order the cards show them.
    pub fn order(&self) -> Vec<TabId> {
        let cw = render::card_width(self.size.0 as usize, self.snap.card);
        let g = render::grid(&self.snap.tabs, &self.snap.rules, self.size.0 as usize, cw);
        g.order.iter().map(|&i| self.snap.tabs[i].id).collect()
    }

    /// The chosen item of a view after the tabs.
    pub fn chosen_item(&self) -> Option<&Item> {
        let at = usize::try_from(self.cursor).ok()?;
        self.current()?.data.items.get(at)
    }
}

impl Default for Mission {
    fn default() -> Mission {
        Mission::new()
    }
}

impl Overlay for Mission {
    fn kind(&self) -> &'static str {
        "mission"
    }

    fn draw(&mut self, buf: &mut Buffer, _look: &Look) {
        let area = buf.area;
        self.size = (area.width, area.height);
        let (lines, hits) = render::screen(self, area.width as usize, area.height as usize);
        self.hits = hits;
        crate::sidebar::render::draw(buf, area, &lines);
    }

    // The server takes mission control's keys and mouse itself
    // (`Server::mission_input`), with the whole server at hand.
    fn key(&mut self, _key: Key, _look: &Look) -> Step {
        Step::Same
    }

    fn mouse(&mut self, _mouse: Mouse, _look: &Look) -> Step {
        Step::Same
    }
}

/// What an option that worked under the busy box leaves the server to do
/// once it has answered.
#[derive(Debug)]
pub enum After {
    /// An Inbox notification marked read: off the view at once.
    DropInbox(String),
    /// Worktrees removed: the Work map reads them again.
    Worktrees,
    /// A ticket closed: the ticket board and the Inbox ask again.
    Tickets,
    /// Linear has said which state closing a ticket moves it to: asked
    /// about first, since it changes Linear for everyone.
    CloseTicket {
        ticket: String,
        title: String,
        issue: String,
        state: String,
        name: String,
    },
}

/// What the server is told for mission control.
#[derive(Debug)]
pub enum MissionEvent {
    /// Time for the spinners to turn.
    Spin,
    /// An option at work under the busy box has done: what to say of it.
    Done {
        client: ClientId,
        note: Option<String>,
        after: Option<After>,
    },
    /// The rest of what an answer or a typed command presses in a pane,
    /// once what came before has landed.
    Press {
        pane: PaneId,
        presses: Vec<answer::Press>,
    },
    /// After an answer: the tab's prompt read again, the next question of
    /// several shown, or what was done said.
    Reanswer {
        client: ClientId,
        tab: TabId,
        note: String,
    },
    /// The worktrees of the tabs' repositories, as git lists them, by
    /// repository.
    Worktrees(HashMap<PathBuf, Vec<Worktree>>),
    /// The worktrees no tab is in, from wherever they are known; `None`
    /// when the look failed or took too long, so the rows had stay.
    Revived(Option<Vec<Revived>>),
}

/// What the server keeps for mission control across clients.
#[derive(Default)]
pub struct MissionState {
    pub(crate) spin_pending: bool,
    pub(crate) worktrees: HashMap<PathBuf, Vec<Worktree>>,
    pub(crate) worktrees_at: Option<Instant>,
    pub(crate) worktrees_busy: bool,
    /// The worktrees no tab is in, known elsewhere (`wt-revive --json`).
    pub revived: Vec<Revived>,
    pub(crate) revived_at: Option<Instant>,
    pub(crate) revived_busy: bool,
    /// Your own files' Settings rows, read at most every few seconds.
    pub(crate) own: Option<(Instant, Vec<Item>)>,
}
