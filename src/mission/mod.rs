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
//! closing it whatever is open over it, `r` for a plugin's view, and a
//! few views' own (the Calendar's `o` join and `y` `n` `m` reply, its `h`
//! `l` a day back or forward, the Inbox's `o` go to the tab, `x` dismiss,
//! M-l in Linear and `i` its ticket).
//! Everything that acts on what is chosen is in the menu Enter opens on it,
//! a box over the view; an option that takes a moment shows a box with a
//! spinner until it is done. A click takes what it lands on, the wheel
//! moves, and with hover_highlight the mouse lights what a click would
//! press.
//!
//! The overlay (`Mission`) holds only what one client is doing with it (the
//! view, the cursor, the filter, the focus) and a snapshot of what it shows,
//! which the server takes again before a draw or a key once anything has
//! changed since (`serve`), so drawing is a plain function of the snapshot
//! (`render`), and the keys are handled with the whole server at hand.
//!
//! - `views`: the views after the tabs, the Work map and Settings among them.
//! - `keymap`: the Keys view, every key Switchboard binds (M-/).
//! - `keys_board`: with UX 2.0, the Keys view as a board: the groups, the
//!   keyboard as a heatmap of the week's presses, the chosen key, the
//!   group's table.
//! - `find_key`: the Find a key box `?` opens on the Keys view: a key
//!   pressed comes up with what it does.
//! - `render`: the screen, as lines, and where a click lands.
//! - `slack_board`: the Slack view as UX 2.0's board, the chosen message's
//!   thread beside the list.
//! - `calendar`: the Calendar view drawn as a day, hours down the side,
//!   the chosen event beside it with who is going as photos.
//! - `history`: the Notifications view as a history by day.
//! - `inbox_board`: with UX 2.0, the Inbox as a board: the notifications
//!   by day with who acted's photos, the chosen one in its ticket beside.
//! - `choices`: what the menu Enter opens offers, as data.
//! - `answer`: answering a tab's prompt, as the Slack relay does.
//! - `ask`: a line to type in a box.
//! - `busy`: the box an option shows while it works.
//! - `remind`: a tab's reminder (remind.sh).
//! - `reminders`: the Reminders view, every tab's reminder on a month.
//! - `remind_box`: with UX 2.0, the reminder's box (board 34): when, what
//!   for, and the reminder set now.
//! - `resources_board`: with UX 2.0, the Resources view as a board: the
//!   whole computer, each tab's figures and sparklines, the chosen one's
//!   process tree.
//! - `settings_board`: with UX 2.0, the Settings view as a board: the
//!   categories and your files at the left, the category in front's
//!   options with their switches beside them.
//! - `diagrams`: the Mermaid diagrams of the description beside the PR and
//!   ticket boards, drawn as pictures off the server's task and kept.
//! - `serve`: the server's side: opening it, its keys and mouse, the menus,
//!   the reminders said once, the worktrees read in the background.

pub mod answer;
pub mod ask;
pub mod busy;
pub mod calendar;
pub mod choices;
pub mod diagrams;
pub mod find_key;
pub mod history;
pub mod inbox_board;
pub mod keymap;
pub mod keys_board;
pub mod remind;
pub mod remind_box;
pub mod reminders;
pub mod render;
pub mod resources_board;
mod serve;
pub mod settings_board;
pub mod slack_board;
pub mod summary;
pub mod usage_board;
pub mod views;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use ratatui::buffer::Buffer;

use crate::items::Item;
use crate::mux::input::{Key, Mouse};
use crate::mux::server::ClientId;
use crate::mux::{PaneId, TabId};
use crate::sidebar::model::{Rules, TabModel};
use crate::sidebar::render::Filter;
use crate::ui::{Look, Overlay, Step, Theme};

pub use serve::Opening;
pub use views::{Group, Revived, Scope, View, ViewId, Worktree};

/// Where Tab has put the focus: a group's name (with UX 2.0) or a view's
/// in the bar, the filter, the chosen item, or easy mode's × (render.js's
/// `FOCUS`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Focus {
    Group(Group),
    View(usize),
    Filter,
    Item,
    Close,
    /// A keycap of the Notifications view, by its key.
    Kind(usize),
    /// A keycap that presses its key (the Calendar's chosen event's).
    Key(Key),
}

/// What a click on a part of the screen means, and what the mouse is over:
/// a group's name, a view's name, the filter, a card or a row (by its place in the view;
/// -1 is a line above the cards), a button on a row that opens its menu
/// (the Autopilot heading's), or easy mode's ×.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Spot {
    Group(Group),
    View(usize),
    Filter,
    Index(isize),
    Menu(isize),
    Close,
    /// A row of the pane beside a board, which the wheel scrolls; it holds
    /// the most the pane's first line shown can be, for `J` and the wheel
    /// to stop at its end.
    Side(usize),
    /// A failed check's line under its PR in the CI view: the PR's place
    /// in the view and the check's among its failed ones.
    Check(isize, usize),
    /// A key named in the footer, which a click presses.
    Key(Key),
    /// A keycap of the Notifications view, by its key.
    Kind(usize),
    /// A category of the Settings board, by the place of its first row,
    /// which a click chooses without changing it.
    Category(isize),
    /// A row chosen by a click elsewhere (a group in the Keys board's
    /// panel, a key of its keyboard), which a second click never presses.
    Choose(isize),
    /// A picture or video in the Slack board's thread, by its place among
    /// the thread's files, which a click shows as `p` does.
    Picture(usize),
    /// A day of the Reminders view's month, or a month ahead by its first
    /// day, which a click chooses.
    Day(jiff::civil::Date),
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
    /// With UX 2.0, each tab's screen as its cards show it: the last lines
    /// of its Claude's pane (else its active one) and its branch.
    pub screens: HashMap<TabId, Screen>,
    /// The line of the note worked continuously, leading the tabs while
    /// the mode is on (src/autopilot); the cursor's -1 is on it.
    pub pilot: Option<crate::autopilot::Summary>,
    /// Every tab, whatever the filter leaves of `tabs`, for finding the tab
    /// a PR is open in (`Mission::pr_tab`).
    pub all_tabs: Vec<TabModel>,
    /// The client's terminal draws the kit's half circles (src/ui/kit.rs).
    pub caps: bool,
    /// The terminal's cell in pixels where the description beside a board
    /// draws its diagrams as pictures: the client draws the server's images,
    /// Settings' `view_images` is on and `image_protocol` isn't sixel.
    /// `None` keeps them text.
    pub cell: Option<(u16, u16)>,
    /// The diagrams ready to draw there, by their key in the layout
    /// (`diagrams`).
    pub diagrams: HashMap<String, diagrams::Ready>,
    /// The Notifications view's counts, while it shows the history; `None`
    /// when it shows the sidebar's notifications as they are.
    pub notice_counts: Option<history::Counts>,
    /// The photos of the people the view in front draws, those loaded
    /// (`render::people`).
    pub photos: HashMap<crate::photos::Who, Arc<image::RgbaImage>>,
    /// The Calendar's next working day after the day it shows.
    pub calendar_next: Option<crate::day::calendar::NextDay>,
    /// The note items that name the Calendar's chosen event, or what it
    /// names, once read (`calendar::notes_naming`).
    pub calendar_notes: Vec<calendar::NoteLine>,
    /// The Inbox's chosen notification's ticket as last read, or
    /// `{"error": why}`; `None` until it is (`inbox_board`).
    pub inbox_ticket: Option<serde_json::Value>,
    /// The newest commit naming each of the Inbox's tickets.
    pub inbox_links: HashMap<String, crate::sources::inbox::Commit>,
    /// The Slack view's chosen message's thread, once read.
    pub slack_thread: Option<crate::day::slack_thread::Thread>,
    /// The Slack files' pictures read and decoded, by file ID.
    pub slack_pictures: HashMap<String, diagrams::Ready>,
    /// Today's Calendar and the Tickets, unfiltered, for the Slack board's
    /// From your work.
    pub slack_work: Vec<views::View>,
    /// Why the chosen Slack thread couldn't be read, when it couldn't.
    pub slack_thread_failed: Option<String>,
    /// Its files that couldn't be read, by ID.
    pub slack_files_failed: HashMap<String, String>,
    /// Its files kept on this computer, by ID.
    pub slack_files_kept: std::collections::HashSet<String>,
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
                ux2: false,
            },
            theme: Theme::DARK,
            card: (56, 13),
            tab_focus: true,
            close: false,
            now_secs: 0,
            tz: jiff::tz::TimeZone::UTC,
            frame: 0,
            screens: HashMap::new(),
            pilot: None,
            all_tabs: Vec::new(),
            caps: false,
            cell: None,
            diagrams: HashMap::new(),
            notice_counts: None,
            photos: HashMap::new(),
            calendar_next: None,
            calendar_notes: Vec::new(),
            inbox_ticket: None,
            inbox_links: HashMap::new(),
            slack_thread: None,
            slack_pictures: HashMap::new(),
            slack_work: Vec::new(),
            slack_thread_failed: None,
            slack_files_failed: Default::default(),
            slack_files_kept: Default::default(),
        }
    }
}

/// A pane's screen as styled lines, shared between the server's cache and
/// the snapshots that show it.
pub type Lines = Arc<[Vec<ratatui::text::Span<'static>>]>;

/// What a UX 2.0 card shows of a tab's pane: its screen's lines in the
/// pane's own colors, and the branch its directory is on.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Screen {
    pub lines: Lines,
    pub branch: Option<String>,
    /// The options a waiting Claude shows, numbered from 1 (a permission
    /// prompt's are Claude Code's own).
    pub options: Vec<String>,
    /// The question over those options, as the screen shows it.
    pub question: String,
    /// The options the screen shows more of than their label, by number.
    pub more: Vec<u32>,
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
    /// Which of the Work map's repositories are folded, and which show
    /// their merged work.
    pub folds: views::Folds,
    /// The Usage board shows this week rather than today (`w`).
    pub usage_week: bool,
    /// What the last action did, in the footer until the next key.
    pub note: Option<String>,
    pub snap: Snapshot,
    /// The server's count of changes (`MissionState::changes`) when the
    /// snapshot was put together: while it is the same, the snapshot
    /// stands, and a draw or a key needs only the panes' screens and the
    /// spinners' frame again.
    pub(crate) taken_at: Option<u64>,
    /// Where each part was last drawn, for clicks and the mouse.
    pub hits: Vec<HitBox>,
    /// The meters, sparklines and avatars it last drew, placed as images over its text (`images`),
    /// where they sit on the whole screen.
    pub meters: Vec<render::Image>,
    /// The diagrams it last laid out as pictures beside a board, placed the
    /// same way.
    pub pictures: Vec<render::Picture>,
    /// The description whose diagrams were last asked for, by its hash and
    /// theme, so a draw asks only once the chosen item's changes.
    pub(crate) diagrams_asked: Option<u64>,
    /// The screen it was last drawn on: with UX 2.0, the area right of
    /// the sidebar.
    pub size: (u16, u16),
    /// With UX 2.0, the sidebar's columns, which it leaves showing on its
    /// left; 0 to cover the whole screen.
    pub left: u16,
    /// The view last brought to the front, by its title ("Tabs" for the
    /// cards), for what going to one does.
    pub(crate) came: Option<String>,
    /// When the cards' spinners last turned (ms), which a box over mission
    /// control holds them to for `BOXED_SPIN_MS`.
    pub(crate) spun_at: i64,
    /// The Landing view kept while this client has it in front, or opens
    /// on it, with nothing landing.
    pub keep_landing: bool,
    /// Autopilot's view kept the same way, with the mode off.
    pub keep_autopilot: bool,
    /// How far the pane beside a board is scrolled (`J`, `K`, the wheel
    /// over it): the view and the item it was scrolled on, and its first
    /// line shown. Another view or item shows its pane from the top.
    pub side: Option<(usize, String, usize)>,
    /// The CI view's chosen failed check: the PR's item id and the check's
    /// key (`views::ci_check_key`, its name and page), so a look that changes which checks fail keeps the same one
    /// chosen; none, another item's, or a check no longer failing is the
    /// PR's own row, whose pane shows its first.
    pub ci_check: Option<(String, String)>,
    /// The Notifications view's keycap lit: 1 all, else a kind's
    /// (`sidebar::history::Kind::key`).
    pub notice_kind: usize,
    /// The Notifications view's days unfolded, by their date.
    pub unfolded: HashSet<String>,
    /// What the Resources view sorts its tabs by (`s`).
    pub resources_sort: resources_board::Sort,
    /// The day the Calendar view shows (YYYY-MM-DD), `h` and `l` moving
    /// it; `None` is today.
    pub calendar_day: Option<String>,
    /// The event a Calendar keycap's focus was given on, by its item's id:
    /// once another is chosen (a filter, a day, a view), the focus goes
    /// back to the item rather than press its key on the new one.
    pub keycap_for: Option<String>,
    /// On a screen too narrow for the Inbox's ticket beside its list, `i`
    /// shows it in the list's place until `i` or Esc.
    pub inbox_pane: bool,
    /// `i` has the Slack view's thread in the list's place, on a screen
    /// too narrow for both.
    pub slack_thread_open: bool,
    /// The Slack thread's picture a click chose, for the `p` it presses.
    pub slack_clicked: Option<usize>,
    /// The Slack file `p` last showed, by the message's item ID and the
    /// file's ID, for the next `p` to step past.
    pub slack_shown: Option<(String, String)>,
    /// The Reminders view's chosen day, the keys moving it and the month
    /// following; `None` is today.
    pub reminders_day: Option<jiff::civil::Date>,
    /// With the pane under the month, its This month and Later lists shown
    /// (`i`).
    pub reminders_lists: bool,
    /// With UX 2.0, the tab whose card shows its screen (`s`), and the
    /// cursor it was asked at: moving the cursor puts the card back.
    pub card_screen: Option<(TabId, isize)>,
    /// With UX 2.0 on a screen with room for one column of cards, whether
    /// the shelf's Landing and Idle show their tabs or only their labels.
    pub shelf_open: bool,
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
            folds: views::Folds::default(),
            usage_week: false,
            note: None,
            snap: Snapshot::default(),
            taken_at: None,
            hits: Vec::new(),
            meters: Vec::new(),
            pictures: Vec::new(),
            diagrams_asked: None,
            size: (0, 0),
            left: 0,
            came: None,
            spun_at: 0,
            keep_landing: false,
            keep_autopilot: false,
            side: None,
            ci_check: None,
            notice_kind: 1,
            unfolded: HashSet::new(),
            resources_sort: resources_board::Sort::default(),
            calendar_day: None,
            keycap_for: None,
            inbox_pane: false,
            slack_thread_open: false,
            slack_clicked: None,
            slack_shown: None,
            reminders_day: None,
            reminders_lists: false,
            card_screen: None,
            shelf_open: false,
        }
    }

    /// The first line the pane beside a board shows: where it was scrolled
    /// while the view and the chosen item are the ones it was scrolled on,
    /// else the top.
    pub fn side_top(&self) -> usize {
        match (&self.side, self.chosen_item()) {
            (Some((view, id, top)), Some(item)) if *view == self.view && *id == item.id => *top,
            _ => self.side_start(),
        }
    }

    /// Where the pane beside a list starts before it is scrolled: the top,
    /// but in the CI view a few lines above the chosen check's line that
    /// says why, so it shows without scrolling.
    fn side_start(&self) -> usize {
        match (self.current(), self.chosen_item()) {
            (Some(view), Some(item)) if view.id == ViewId::Ci && self.snap.rules.ux2 => {
                render::ci_why_row(item, self.chosen_check().unwrap_or(0))
                    .map_or(0, |row| row.saturating_sub(render::CI_ABOVE_WHY))
            }
            _ => 0,
        }
    }

    /// The CI view's chosen failed check, by its place among the chosen
    /// PR's now; `None` on the PR's own row, and once a look finds the
    /// chosen check no longer failing.
    pub fn chosen_check(&self) -> Option<usize> {
        let (id, key) = self.ci_check.as_ref()?;
        let view = self.current()?;
        let item = self.chosen_item()?;
        if view.id != ViewId::Ci || item.id != *id {
            return None;
        }
        views::ci_checks(item)
            .iter()
            .position(|c| views::ci_check_key(c) == *key)
    }

    /// Scrolls the pane beside a board by `lines` (up when negative),
    /// kept between its top and `last`, the most its first line shown can
    /// be; false when it didn't move.
    pub fn scroll_side(&mut self, lines: isize, last: usize) -> bool {
        let Some(id) = self.chosen_item().map(|i| i.id.clone()) else {
            return false;
        };
        let was = self.side_top().min(last);
        let to = was.saturating_add_signed(lines).min(last);
        self.side = Some((self.view, id, to));
        to != was
    }

    /// Lets the pane's scroll go once another view or item is chosen, so
    /// coming back to the one it was made on shows that pane from the top.
    pub fn forget_side_elsewhere(&mut self) {
        // A chosen CI check no longer failing lets go, with its scroll.
        if self.ci_check.is_some() && self.chosen_check().is_none() {
            self.ci_check = None;
            self.side = None;
        }
        let chosen = self.chosen_item().map(|i| i.id.clone());
        if self
            .side
            .as_ref()
            .is_some_and(|(view, id, _)| *view != self.view || chosen.as_ref() != Some(id))
        {
            self.side = None;
        }
    }

    /// The most the first line of the pane beside a board can be, as it
    /// was last drawn; `None` with no pane shown.
    pub fn side_last(&self) -> Option<usize> {
        self.hits.iter().find_map(|h| match h.spot {
            Spot::Side(last) => Some(last),
            _ => None,
        })
    }

    /// The view in front, after the tabs; `None` on the tabs.
    /// Whether the Usage view's services with no requests today are left
    /// out, as today's board leaves them (the week's board and the plain
    /// list show them).
    pub fn week_only_hidden(&self) -> bool {
        !self.usage_week && self.snap.rules.ux2 && self.size.0 as usize >= usage_board::BOARD_AT
    }

    pub fn current(&self) -> Option<&View> {
        self.view
            .checked_sub(1)
            .and_then(|i| self.snap.views.get(i))
    }

    /// With UX 2.0, the bar's groups and the views in each
    /// (`views::groups`); `None` without it, whose bar is one row.
    pub fn groups(&self) -> Option<Vec<(Group, Vec<usize>)>> {
        self.snap.rules.ux2.then(|| views::groups(&self.snap.views))
    }

    /// The tabs in the order the cards show them.
    pub fn order(&self) -> Vec<TabId> {
        let g = render::tabs_grid(self, self.size.0 as usize);
        g.order.iter().map(|&i| self.snap.tabs[i].id).collect()
    }

    /// The tab PR `number` of `repo` is open in, among every tab, a filter
    /// or not (the shown tabs when the snapshot holds no others).
    pub fn pr_tab(&self, number: u64, repo: &str) -> Option<&TabModel> {
        let tabs = if self.snap.all_tabs.is_empty() {
            &self.snap.tabs
        } else {
            &self.snap.all_tabs
        };
        render::tab_of_pr(tabs, number, repo)
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
        // A card's screen (`s`) ends once the cursor has moved off it.
        if self.card_screen.is_some_and(|(_, at)| at != self.cursor) {
            self.card_screen = None;
        }
        let whole = buf.area;
        let left = self.left.min(whole.width.saturating_sub(1));
        let area =
            ratatui::layout::Rect::new(whole.x + left, whole.y, whole.width - left, whole.height);
        self.size = (area.width, area.height);
        self.forget_side_elsewhere();
        let (lines, hits, images, pictures) =
            render::drawn(self, area.width as usize, area.height as usize);
        self.hits = hits;
        self.pictures = pictures
            .into_iter()
            .map(|picture| render::Picture {
                x: picture.x + area.x,
                y: picture.y + area.y,
                ..picture
            })
            .collect();
        self.meters = images
            .into_iter()
            .map(|meter| render::Image {
                x: meter.x + area.x,
                y: meter.y + area.y,
                ..meter
            })
            .collect();
        crate::sidebar::render::draw(buf, area, &lines);
    }

    fn draw_frame(&mut self, frame: &mut crate::mux::compose::Frame, look: &Look) {
        self.draw(&mut frame.buf, look);
        for image in &self.meters {
            let area = ratatui::layout::Rect::new(image.x, image.y, image.width, image.height);
            match &image.shape {
                render::Shape::Meter { frac, color, tick } => {
                    crate::ui::images::meter(frame, &image.name, area, *frac, *color, *tick)
                }
                render::Shape::Series {
                    bars,
                    max,
                    over,
                    marks,
                } => crate::ui::images::series(frame, &image.name, area, bars, *max, *over, marks),
                render::Shape::Switch { on } => {
                    crate::ui::images::switch(frame, &image.name, area, *on)
                }
                render::Shape::Keyboard { heat, outlined } => crate::ui::images::keyboard(
                    frame,
                    &image.name,
                    area,
                    heat,
                    outlined,
                    crate::mux::compose::colors::ACCENT,
                ),
                render::Shape::Avatar { who, label, color } => {
                    let photo = who.as_ref().and_then(|w| self.snap.photos.get(w));
                    crate::ui::images::avatar(
                        frame,
                        &image.name,
                        area,
                        label,
                        *color,
                        photo.map(|p| p.as_ref()),
                    )
                }
                render::Shape::Picture { key } => {
                    if let Some(p) = self.snap.slack_pictures.get(key) {
                        crate::ui::images::picture_shared(
                            frame,
                            &image.name,
                            area,
                            key,
                            &p.image,
                            p.content,
                        );
                    }
                }
            }
        }
        for picture in &self.pictures {
            if let Some(diagram) = self.snap.diagrams.get(&picture.key) {
                crate::ui::images::picture_cut(
                    frame,
                    picture.id,
                    ratatui::layout::Rect::new(picture.x, picture.y, picture.cols, picture.shown),
                    picture.from,
                    picture.rows,
                    &diagram.image,
                    diagram.content,
                );
            }
        }
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
    /// An invitation answered: the calendar feed read again.
    Calendar,
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
    /// What the busy box says, once the work knows more: where a link
    /// opens.
    Relabel { client: ClientId, text: String },
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
    /// repository, and how each repository lands, in words.
    Worktrees(HashMap<PathBuf, Vec<Worktree>>, HashMap<PathBuf, String>),
    /// A worktree's branch counted for the Work map's chosen row.
    Counted(PathBuf, views::Counts),
    /// An Inbox ticket read, from its cache file or Linear, or why it
    /// couldn't be (`{"error": why}`); `last` once the read is over.
    InboxTicket {
        ticket: String,
        value: Option<serde_json::Value>,
        last: bool,
    },
    /// An Inbox notification marked read in Linear as its menu closed, or
    /// why it couldn't be.
    InboxRead { id: String, error: Option<String> },
    /// The note items that name a Calendar event, by its item's id.
    CalendarNotes(String, Vec<calendar::NoteLine>),
    /// The worktrees no tab is in, from wherever they are known; `None`
    /// when the look failed or took too long, so the rows had stay.
    Revived(Option<Vec<Revived>>),
    /// A diagram of the description beside a board, drawn for a theme, or
    /// `None` for one that couldn't be, which stays its text.
    Diagram {
        source: String,
        light: bool,
        image: Option<image::RgbaImage>,
    },
    /// Haiku's sentence on a turn, by its session and when its prompt was
    /// given; `None` when the run failed.
    Summary {
        session: String,
        turn_at: i64,
        text: Option<String>,
    },
}

/// What the server keeps for mission control across clients.
#[derive(Default)]
pub struct MissionState {
    /// Counts the events that may change what mission control shows (all
    /// but a pane's output, a spinner's turn and a client's keys, which
    /// mission control's own handling accounts for), so a snapshot taken
    /// since stands.
    pub(crate) changes: u64,
    /// Each pane's screen as a card shows it, with the pane's `version`
    /// when it was drawn, so only a pane that has changed is drawn again.
    pub(crate) screens: HashMap<PaneId, (u64, Lines)>,
    pub(crate) spin_pending: bool,
    pub(crate) worktrees: HashMap<PathBuf, Vec<Worktree>>,
    pub(crate) worktrees_at: Option<Instant>,
    pub(crate) worktrees_busy: bool,
    /// How each repository lands, in words, by its main checkout.
    pub(crate) landings: HashMap<PathBuf, String>,
    /// The Work map's chosen rows' branches counted, by worktree; one
    /// count at a time (`counting`).
    pub(crate) counts: HashMap<PathBuf, views::Counts>,
    pub(crate) counting: Option<PathBuf>,
    /// The worktrees no tab is in, known elsewhere (`wt-revive --json`).
    pub revived: Vec<Revived>,
    pub(crate) revived_at: Option<Instant>,
    pub(crate) revived_busy: bool,
    /// Your own files' Settings rows, read at most every few seconds.
    pub(crate) own: Option<(Instant, Vec<Item>)>,
    /// When each repository's landing was last worked out from Settings.
    pub(crate) landing_asked: HashMap<String, Instant>,
    /// The diagrams of the descriptions beside the boards, asked for and
    /// drawn.
    pub(crate) diagrams: diagrams::Diagrams,
    /// The note items found for each Calendar event, and when; one read at
    /// a time.
    pub(crate) calendar_notes: HashMap<String, (Instant, Vec<calendar::NoteLine>)>,
    pub(crate) calendar_notes_busy: bool,
    /// The Inbox's tickets as last read, and when each was last asked
    /// for; one read at a time.
    pub(crate) inbox_tickets: HashMap<String, serde_json::Value>,
    pub(crate) inbox_asked: HashMap<String, Instant>,
    pub(crate) inbox_ticket_busy: bool,
    /// What a key does once the mission control it was pressed in is back
    /// in its overlay, for what goes to another view or closes it (Enter
    /// on a notification), which acts on the overlay itself.
    pub(crate) after_key: Option<AfterKey>,
    /// Each tab's last turn in a sentence, as Haiku wrote it
    /// (`summary`).
    pub(crate) summaries: summary::Summaries,
}

/// See `MissionState::after_key`.
pub(crate) type AfterKey = Box<dyn FnOnce(&mut crate::mux::server::Server) + Send>;
