//! What the diff draws around its change: its header and its own settings
//! view when it runs alone (`switchboard-diff`), and its menu. New with the
//! diff as an app of its own, as boards 119 and 120 draw them; in the
//! workspace the tab's header is the workspace's and the diff's options are
//! rows of Settings, so the view and the header are left out and the menu
//! points there.
//!
//! The settings view takes the diff's place under the header (M-m, the
//! menu's first option, or the Settings button of the row at the bottom):
//! each of the diff's own options a row with its name, a "changed" chip,
//! its key and whether it is on, the chosen one opened to its whole
//! description and its default, as Switchboard's Settings rows draw them,
//! from the one declaration both read (the diff's manifest). Enter, Space
//! or a click on the chosen row turns it on or off, written to the settings
//! file at once (`host::write_setting`) and followed at once
//! (`settings_changed`). Under them, the look for a newer release, and the
//! release to update to when one is out.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line as Spans, Span};
use serde_json::Value;
use switchboard_plugin_api::{Scope, Setting};
use switchboard_ui::draw::{cut, put_line, width, wrap};
use switchboard_ui::input::{self, Key, Mods, Mouse, MouseKind};
use switchboard_ui::kit::{self, Surface, Tone};
use switchboard_ui::{Menu, MenuOption, Outcome, Overlay, Step};

use super::{Act, App, BUTTONS, Button, Focus, Msg, Options, Telling};
use crate::tell::Teller;
use crate::update::{self, Newer};
use crate::{host, settings, util};

/// The widest a description wraps, however wide the screen.
const TEXT_MAX: usize = 100;

/// The settings view while it is open: the chosen row and the first line
/// on the screen.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct View {
    at: usize,
    top: usize,
}

/// A row of the settings view.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Row {
    /// One of the diff's options.
    Option(Setting),
    /// The release to update to.
    Update(Newer),
}

/// What the menu's options do, in its order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Choice {
    /// The diff's own settings view, alone.
    Settings,
    /// Mission control's Settings, in the workspace.
    WorkspaceSettings,
    /// One of the row's buttons, by its place in `BUTTONS`.
    Keys(usize),
    /// The Story view or the files, with a story told in chapters.
    Mode(super::Mode),
    /// The file list's width you gave it forgotten (width.rs).
    DefaultWidth,
    /// What runs laid over the column, or taken away.
    Runs,
    /// The story forgotten and told again, by the model the setting
    /// names now.
    TellAgain,
    /// The change a menu was opened on in the column, shown in the files.
    InFiles,
    /// Its place copied as path:line.
    CopyPlace,
}

/// The diff's menu, and what each of its options does.
pub(super) struct Open {
    menu: Menu,
    choices: Vec<Choice>,
    /// The look it opened with, read once: drawing reads no settings.
    look: switchboard_ui::Look,
}

/// The option an option builds on, by its name, while that one is off:
/// the story needs the files in reading order, and the story's pages,
/// progress and notes need the story (`Options::read`).
fn needs_off(key: &str) -> Option<&'static str> {
    let off = |k: &str| !settings::is_on(k);
    match key {
        "diff_story" if off("diff_order") => Some("Files in reading order"),
        "diff_story_pages" | "diff_story_progress" | "diff_story_notes"
            if off("diff_order") || off("diff_story") =>
        {
            Some("Diff told as a story")
        }
        _ => None,
    }
}

/// A key as the boxes of `switchboard-ui` take one.
fn ui_key(key: &KeyEvent) -> Option<Key> {
    let code = match key.code {
        KeyCode::Char(c) => input::KeyCode::Char(c),
        KeyCode::Enter => input::KeyCode::Enter,
        KeyCode::Tab => input::KeyCode::Tab,
        KeyCode::BackTab => input::KeyCode::BackTab,
        KeyCode::Backspace => input::KeyCode::Backspace,
        KeyCode::Esc => input::KeyCode::Esc,
        KeyCode::Left => input::KeyCode::Left,
        KeyCode::Right => input::KeyCode::Right,
        KeyCode::Up => input::KeyCode::Up,
        KeyCode::Down => input::KeyCode::Down,
        KeyCode::Home => input::KeyCode::Home,
        KeyCode::End => input::KeyCode::End,
        KeyCode::PageUp => input::KeyCode::PageUp,
        KeyCode::PageDown => input::KeyCode::PageDown,
        KeyCode::Delete => input::KeyCode::Delete,
        _ => return None,
    };
    Some(Key {
        code,
        mods: Mods {
            // A capital letter comes as itself, with Shift already in it.
            shift: key.modifiers.contains(KeyModifiers::SHIFT)
                && !matches!(key.code, KeyCode::Char(_)),
            alt: key.modifiers.contains(KeyModifiers::ALT),
            ctrl: key.modifiers.contains(KeyModifiers::CONTROL),
        },
    })
}

/// A mouse event as the boxes of `switchboard-ui` take one.
fn ui_mouse(m: &MouseEvent) -> Option<Mouse> {
    let button = |b: MouseButton| match b {
        MouseButton::Left => input::MouseButton::Left,
        MouseButton::Right => input::MouseButton::Right,
        MouseButton::Middle => input::MouseButton::Middle,
    };
    let (kind, button) = match m.kind {
        MouseEventKind::Down(b) => (MouseKind::Press, button(b)),
        MouseEventKind::Up(b) => (MouseKind::Release, button(b)),
        MouseEventKind::Drag(b) => (MouseKind::Drag, button(b)),
        MouseEventKind::Moved => (MouseKind::Move, input::MouseButton::None),
        MouseEventKind::ScrollUp => (MouseKind::Press, input::MouseButton::WheelUp),
        MouseEventKind::ScrollDown => (MouseKind::Press, input::MouseButton::WheelDown),
        _ => return None,
    };
    Some(Mouse {
        kind,
        button,
        col: m.column,
        row: m.row,
        mods: Mods::NONE,
    })
}

impl App {
    // ── The header and the row of buttons ───────────────────────────────────

    /// Whether the screen's first row is the diff's own header: alone,
    /// where no workspace heads its tab. A commit of the history is drawn
    /// under the header of the diff it was opened from.
    pub(super) fn has_header(&self) -> bool {
        self.opts.alone && !self.in_history
    }

    /// Whether the screen's last row is the row of buttons: in easy mode,
    /// and always alone, where its Settings button is how a click reaches
    /// the settings view.
    pub(super) fn has_buttons(&self) -> bool {
        self.opts.easy || self.opts.alone
    }

    /// The row's buttons in its order: each of `BUTTONS` (Ask only with
    /// diff_ask), and alone a Settings button before Close.
    pub(super) fn bar_buttons(&self) -> Vec<(Button, &'static str)> {
        let mut bar: Vec<(Button, &'static str)> = BUTTONS
            .iter()
            .filter(|(_, k)| self.opts.ask || *k != "ga")
            .enumerate()
            .map(|(i, (label, _))| (Button::Bar(i), *label))
            .collect();
        // With no change on the screen (nothing to show, one that couldn't
        // be read, one being read, or the story's cover over it), only
        // what works there: Close, and Settings.
        let covered = self.cover.is_some() && self.telling == Telling::Waiting;
        let no_change = self.nothing.is_some() || self.failed.is_some() || self.loading.is_some();
        // A question being typed takes the keys: only Close (which closes
        // it) and Settings.
        let typing = self.ask_box.is_some() && self.settings.is_none();
        if no_change || covered || typing {
            bar.drain(..bar.len().saturating_sub(1));
        }
        // A commit of the history doesn't change and asks nothing: no
        // Refresh, no Ask.
        if self.history.is_some() || self.in_history {
            bar.retain(|(b, _)| match b {
                Button::Bar(i) => {
                    let keys = BUTTONS
                        .iter()
                        .filter(|(_, k)| self.opts.ask || *k != "ga")
                        .nth(*i)
                        .map(|(_, k)| *k);
                    !matches!(keys, Some("R" | "ga"))
                }
                _ => true,
            });
        }
        if self.opts.alone {
            let before_close = bar.len().saturating_sub(1);
            bar.insert(before_close, (Button::Settings, "Settings"));
        }
        bar
    }

    /// The header, alone: the app's name, the repository and what the
    /// branch is compared with, and at the right end its version, or, when
    /// a newer release is out, the dim line that says so.
    pub(super) fn draw_header(&self, buf: &mut Buffer) {
        let theme = self.theme();
        let cols = buf.area.width as usize;
        let fill = Style::new().bg(Surface::Panel.color(&theme));
        for x in 0..buf.area.width {
            buf[(x, 0)].reset();
            buf[(x, 0)].set_style(fill);
        }
        let right = match &self.newer {
            Some(newer) => newer.line(),
            None => env!("CARGO_PKG_VERSION").to_string(),
        };
        let mut left = vec![Span::styled(
            format!(" {}", update::APP),
            Tone::Text.bold(&theme),
        )];
        if let Some(change) = &self.change {
            // The working tree's changes have no base to name.
            let against = if change.working() {
                "the working tree's changes".to_string()
            } else {
                change.against()
            };
            left.push(Span::styled(
                format!("   {} · {against}", self.top_shown),
                Tone::Dim.style(&theme),
            ));
        }
        let room = cols.saturating_sub(width(&right) + 3);
        let mut line = kit::fit(left, room, Style::new());
        line.push(Span::raw(
            " ".repeat(cols.saturating_sub(room + width(&right) + 1)),
        ));
        line.push(Span::styled(right, Tone::Dim.style(&theme)));
        let line: Vec<Span<'static>> = line
            .into_iter()
            .map(|s| {
                let style = s.style.bg(Surface::Panel.color(&theme));
                Span::styled(s.content, style)
            })
            .collect();
        put_line(buf, 0, 0, buf.area.width, &Spans::from(line));
    }

    // ── The settings view ───────────────────────────────────────────────────

    /// Whether the settings view is open.
    pub fn in_settings(&self) -> bool {
        self.settings.is_some()
    }

    /// M-m, the menu's Settings, or the Settings button: the view opened in
    /// the diff's place, or closed back to the diff. Only alone.
    pub(super) fn toggle_settings(&mut self) {
        if !self.opts.alone {
            return;
        }
        // From a commit of the history, the view is the branch's diff's.
        if self.in_history {
            self.wants = Some(super::Wants::Settings);
            return;
        }
        // A selection of the diff's text ends: its keys (Esc, y) are the
        // view's while it is open.
        self.selection = None;
        self.visual = None;
        self.list_visual = None;
        self.settings = match self.settings {
            Some(_) => None,
            None => {
                // The settings as the file has them now, changed since the
                // diff opened included (in Switchboard, or another diff),
                // which the diff then follows too.
                host::reread_settings();
                let before = self.opts;
                self.opts = self.opts.read_again();
                self.settings_changed(before);
                self.follow_update_check();
                // The file named as the view opens, not as it draws.
                self.settings_path = host::settings_file();
                self.settings_shown = util::tilde(&self.settings_path);
                Some(View::default())
            }
        };
        self.menu = None;
        self.filtering = false;
        self.pending = None;
    }

    /// The view's rows: the diff's options that are its own alone as in the
    /// workspace, then the look for a newer release, then the release to
    /// update to when one is out.
    pub(super) fn settings_rows(&self) -> Vec<Row> {
        let manifest = crate::manifest();
        let of = |scope: Scope| {
            manifest
                .settings
                .iter()
                .filter(move |s| s.scope == scope)
                .cloned()
                .map(Row::Option)
        };
        let mut rows: Vec<Row> = of(Scope::Both).chain(of(Scope::Alone)).collect();
        if let Some(newer) = &self.newer {
            rows.push(Row::Update(newer.clone()));
        }
        rows
    }

    /// How many of the view's options differ from their defaults.
    fn changed_settings(&self) -> usize {
        self.settings_rows()
            .iter()
            .filter(|row| match row {
                Row::Option(s) => settings::value(&s.key).is_some_and(|v| v != s.default),
                Row::Update(_) => false,
            })
            .count()
    }

    pub(super) fn settings_key(&mut self, key: KeyEvent) -> Act {
        let Some(view) = self.settings else {
            return Act::Same;
        };
        let last = self.settings_rows().len().saturating_sub(1);
        let at = match key.code {
            KeyCode::Esc | KeyCode::Char('q') => {
                self.settings = None;
                return Act::Redraw;
            }
            KeyCode::Char('j') | KeyCode::Down => (view.at + 1).min(last),
            // Tab and Shift-Tab reach each row in turn, coming round.
            KeyCode::Tab => (view.at + 1) % (last + 1),
            KeyCode::BackTab => (view.at + last) % (last + 1),
            KeyCode::Char('k') | KeyCode::Up => view.at.saturating_sub(1),
            KeyCode::Char('g') | KeyCode::Home => 0,
            KeyCode::Char('G') | KeyCode::End => last,
            KeyCode::Enter | KeyCode::Char(' ') => {
                self.take_setting(view.at);
                return Act::Redraw;
            }
            _ => return Act::Same,
        };
        self.settings = Some(View { at, ..view });
        Act::Redraw
    }

    /// The mouse in the settings view: a click on a row chooses it, one on
    /// the chosen row takes it, the wheel moves through the rows.
    pub(super) fn settings_mouse(&mut self, m: MouseEvent, hit: Option<Button>) -> Act {
        let Some(view) = self.settings else {
            return Act::Same;
        };
        let last = self.settings_rows().len().saturating_sub(1);
        match (m.kind, hit) {
            (MouseEventKind::ScrollDown, _) => {
                self.settings = Some(View {
                    at: (view.at + 1).min(last),
                    ..view
                });
            }
            (MouseEventKind::ScrollUp, _) => {
                self.settings = Some(View {
                    at: view.at.saturating_sub(1),
                    ..view
                });
            }
            (MouseEventKind::Down(MouseButton::Left), Some(Button::Option(i))) if i == view.at => {
                self.take_setting(i);
            }
            (MouseEventKind::Down(MouseButton::Left), Some(Button::Option(i))) => {
                self.settings = Some(View { at: i, ..view });
            }
            (MouseEventKind::Down(MouseButton::Left), Some(button)) => return self.press(button),
            _ => return Act::Same,
        }
        Act::Redraw
    }

    /// Enter on a row: an option turned on or off, kept in the settings
    /// file and followed at once; the release, installed off the screen's
    /// chapter.
    fn take_setting(&mut self, at: usize) {
        match self.settings_rows().get(at) {
            Some(Row::Option(setting)) => {
                let on = settings::value(&setting.key).unwrap_or(setting.default.clone())
                    == Value::Bool(true);
                // Into the file the view names; one moved since (your files
                // kept in a repository, turned on meanwhile) is named anew
                // and the change asked for again.
                if host::settings_file() != self.settings_path {
                    host::reread_settings();
                    self.settings_path = host::settings_file();
                    self.settings_shown = util::tilde(&self.settings_path);
                    // The settings read from there are followed at once.
                    let before = self.opts;
                    self.opts = self.opts.read_again();
                    self.settings_changed(before);
                    self.follow_update_check();
                    self.say(
                        "Your settings are kept elsewhere now: the view shows them from there.",
                    );
                    return;
                }
                match host::write_setting_at(&self.settings_path, &setting.key, Value::Bool(!on)) {
                    Ok(()) => {
                        let before = self.opts;
                        self.opts = self.opts.read_again();
                        self.settings_changed(before);
                        // The whole file was read again: the look follows its
                        // setting as it is now, whichever row changed.
                        if setting.key == crate::UPDATE_CHECK {
                            self.newer = None;
                        }
                        self.follow_update_check();
                    }
                    Err(why) => self.say(why),
                }
            }
            Some(Row::Update(newer)) if !self.updating => {
                self.updating = true;
                // The release the row names, not one out since.
                let version = newer.version.clone();
                self.say(format!("Updating to {} {version}…", update::APP));
                let tx = self.tx.clone();
                std::thread::spawn(move || {
                    let done = update::update(false, false, Some(&version), &|_| {});
                    let _ = tx.send(Msg::Updated(done));
                });
            }
            _ => {}
        }
    }

    /// The look for a newer release as its setting is now: off, nothing is
    /// said of one found before; on, with none known, the day's look, from
    /// what it kept or asked of GitHub.
    fn follow_update_check(&mut self) {
        if !update::turned_on() {
            self.newer = None;
        } else if !self.installed && update::allowed() {
            let tx = self.tx.clone();
            std::thread::spawn(move || {
                let _ = tx.send(Msg::Newer(update::look(util::now_secs())));
            });
        }
    }

    /// What an update asked for from the view came to, said.
    pub(super) fn updated(&mut self, outcome: Result<update::Outcome, String>) {
        self.updating = false;
        match outcome {
            Ok(update::Outcome::Installed(version)) => {
                self.newer = None;
                // This run is the binary before: it offers nothing more.
                self.installed = true;
                self.say(format!(
                    "Updated to {} {version}.\nIt runs the next time you start it.",
                    update::APP
                ));
            }
            Ok(update::Outcome::UpToDate | update::Outcome::Replaced) => {
                self.newer = None;
                // Updated from elsewhere since this run started: this run
                // is the binary before, which offers nothing more.
                let replaced = matches!(outcome, Ok(update::Outcome::Replaced));
                if replaced || update::installed_version().is_some() {
                    self.installed = true;
                    self.say(format!(
                        "{} was updated since this one started.\nIt runs the next time you start it.",
                        update::APP
                    ));
                } else {
                    self.say(format!("{} is up to date.", update::APP));
                }
            }
            Ok(update::Outcome::Out(text)) => self.say(text),
            Ok(update::Outcome::Withdrawn(version)) => {
                self.newer = None;
                self.say(format!(
                    "{} {version} is no longer offered.\nThe next look finds what is.",
                    update::APP
                ));
            }
            Err(why) => self.say(format!("Couldn't update: {why}")),
        }
    }

    /// The options changed, from `before`: the diff follows them at once.
    /// A change to what is read (diff_uncommitted) reads the change anew;
    /// one to the order or the story drops the story told and one being
    /// told, and asks again for what is now on; the rest only draw anew.
    pub(super) fn settings_changed(&mut self, before: Options) {
        let now = self.opts;
        if now == before {
            return;
        }
        // A commit of the history was opened with the options before: back
        // to the whole branch, which follows the new ones.
        self.leave_history();
        self.lines.clear();
        // The Story view's column too, which draws the notes.
        self.relay();
        if now.list_wide != before.list_wide || now.resize != before.resize {
            let to = if now.list_wide && self.kept_width().is_none() {
                self.fit_width()
            } else {
                self.narrow_width()
            };
            self.list_widened = now.list_wide && self.kept_width().is_none();
            self.relay_out(to);
        }
        // Ask turned off, whatever else changed with it: a question being
        // typed and an answer go first, so nothing of them asks again.
        if !now.ask {
            self.ask_box = None;
            if self.answer.is_some() {
                self.close_answer();
            }
        }
        if now.uncommitted != before.uncommitted && !self.in_history {
            // The working tree's changes (--working) are read the same
            // whatever the setting says: nothing to read anew.
            if let Some((dir, base, working)) = self.opened.clone().filter(|o| !o.2) {
                self.stop_story();
                // The change on the screen is of the setting before: it
                // gives way to the reading at once, so nothing is asked of
                // it, or about it, meanwhile.
                self.loading = Some("Reading the change…".into());
                // What the reading before said (nothing to show, or why it
                // failed) gives way to this one until it answers.
                self.nothing = None;
                self.failed = None;
                // A question being typed keeps its draft; an answer of the
                // change before goes.
                if self.answer.is_some() {
                    self.close_answer();
                }
                // Under its number, so a reading a later change of the
                // setting outran is dropped when it comes.
                self.reloads += 1;
                let id = self.reloads;
                // An R still reading the change as it was is dropped too.
                self.refreshes += 1;
                let tx = self.tx.clone();
                std::thread::spawn(move || {
                    let (read, said) = std::sync::mpsc::channel();
                    super::load(dir, base, working, now.uncommitted, read);
                    for msg in said {
                        let _ = tx.send(Msg::Reloaded(id, Box::new(msg)));
                    }
                });
                return;
            }
        }
        // With no change on the screen (nothing to show, or one that
        // couldn't be read), nothing is asked of the one read before.
        // Nor while the change is read anew, which asks for its own story
        // and order once it is read.
        if self.nothing.is_some() || self.failed.is_some() || self.loading.is_some() {
            return;
        }
        // Ask turned off: a question being typed and an answer go, so
        // nothing of them asks again.
        if !now.ask {
            self.ask_box = None;
            if self.answer.is_some() {
                self.close_answer();
            }
        }
        // A page open as a page of its own, with pages turned off, would
        // have no key to leave it: back to the whole diff.
        if !now.pages_alone && self.alone.take().is_some() {
            self.whole_at = None;
            self.sort();
            self.pick_first();
        }
        if now.story != before.story || now.order != before.order {
            self.stop_story();
            self.show_story(None);
            self.ask_story(false);
            self.sort();
            if !self.moved {
                self.pick_first();
            }
        }
    }

    /// The story told, and one being told, dropped.
    fn stop_story(&mut self) {
        if self.telling == Telling::Waiting {
            self.teller.stop();
            self.teller = Teller::default();
        }
        self.telling = Telling::Off;
        self.cover = None;
        self.progress = None;
        self.story_key = None;
        self.smart.clear();
        self.asked_order = false;
        self.alone = None;
        self.whole_at = None;
        self.told = None;
        self.story = None;
    }

    /// The view's lines, `cols` wide, each with the row it belongs to (for
    /// its clicks), the chosen row's on the Chosen surface.
    fn settings_lines(&self, cols: usize, at: usize) -> Vec<(Option<usize>, Vec<Span<'static>>)> {
        let theme = self.theme();
        let dim = Tone::Dim.style(&theme);
        let rows = self.settings_rows();
        let options = rows
            .iter()
            .filter(|r| matches!(r, Row::Option(s) if s.scope == Scope::Both))
            .count();
        let mut lines: Vec<(Option<usize>, Vec<Span<'static>>)> = Vec::new();
        let text = |t: String, style: Style| vec![Span::raw("  "), Span::styled(t, style)];
        lines.push((None, Vec::new()));
        lines.push((None, text("Settings".into(), Tone::Text.bold(&theme))));
        let changed = match self.changed_settings() {
            0 => "none changed".to_string(),
            n => format!("{n} changed from its default"),
        };
        lines.push((
            None,
            text(
                format!(
                    "{} {} · {} · {changed}",
                    update::APP,
                    env!("CARGO_PKG_VERSION"),
                    self.settings_shown
                ),
                dim,
            ),
        ));
        if let Some(newer) = &self.newer {
            lines.push((None, text(newer.line(), dim)));
        }
        lines.push((None, vec![kit::rule(cols, &theme)]));
        let wide = cols.saturating_sub(6).min(TEXT_MAX);
        for (i, row) in rows.iter().enumerate() {
            if i == 0 || i == options {
                lines.push((None, Vec::new()));
                let (name, count) = if i == 0 {
                    ("Options", Some(options))
                } else {
                    ("Updates", None)
                };
                let mut label = vec![Span::raw(" ")];
                label.extend(kit::label(
                    name,
                    Tone::Dim,
                    count,
                    cols.saturating_sub(2),
                    &theme,
                ));
                lines.push((None, label));
            }
            let chosen = i == at;
            let lit = self.hover == Some(Button::Option(i));
            let (mut left, right, about, under) = match row {
                Row::Option(s) => {
                    let value = settings::value(&s.key).unwrap_or(s.default.clone());
                    let on = value == Value::Bool(true);
                    let mut left = vec![Span::styled(s.label.clone(), Tone::Text.bold(&theme))];
                    if value != s.default {
                        left.push(Span::raw(" "));
                        left.extend(kit::chip("changed", Tone::Needs, false, &theme));
                    }
                    // On, but held off by the option it builds on.
                    if on && let Some(needs) = needs_off(&s.key) {
                        left.push(Span::raw(" "));
                        left.extend(kit::chip(
                            &format!("needs {needs}"),
                            Tone::Faint,
                            false,
                            &theme,
                        ));
                    }
                    let mut right = vec![Span::styled(format!("{}  ", s.key), dim)];
                    right.extend(kit::switch_chip(
                        if on { "on " } else { "off" },
                        on,
                        lit,
                        false,
                        &theme,
                    ));
                    let default = if s.default == Value::Bool(true) {
                        "on"
                    } else {
                        "off"
                    };
                    let under = vec![
                        Span::styled("default ", dim),
                        Span::styled(default, Tone::Text.style(&theme)),
                        Span::styled("  ·  Enter: on or off, or back to the default", dim),
                    ];
                    (left, right, s.description.clone(), under)
                }
                Row::Update(newer) => {
                    let mut left = vec![
                        Span::styled(
                            format!("Update to {}", newer.version),
                            Tone::Text.bold(&theme),
                        ),
                        Span::raw("  "),
                    ];
                    left.extend(kit::chip(
                        &format!("{} is out", newer.version),
                        Tone::Done,
                        false,
                        &theme,
                    ));
                    let ago = switchboard_ui::text::ago(newer.published, util::now_secs());
                    let right = if ago.is_empty() {
                        Vec::new()
                    } else {
                        vec![Span::styled(format!("released {ago} ago "), dim)]
                    };
                    let about = format!(
                        "Runs {} update: downloads {}, checks its signature, and puts it in place of this one",
                        update::APP,
                        newer.version
                    );
                    let under = vec![Span::styled("Enter: update now", dim)];
                    (left, right, about, under)
                }
            };
            let room = cols.saturating_sub(kit::spans_width(&right) + 3);
            left.insert(0, Span::raw(" "));
            let mut name = kit::fit(left, room, Style::new());
            name.extend(right);
            lines.push((Some(i), kit::row(name, cols, chosen, &theme)));
            let about_lines = if chosen {
                let mut all = wrap(&about, wide);
                all.push(String::new());
                all
            } else {
                vec![cut(&about, wide)]
            };
            for (n, about_line) in about_lines.iter().enumerate() {
                let last = chosen && n + 1 == about_lines.len();
                let mut spans = vec![Span::raw("   ")];
                if last {
                    spans.extend(under.clone());
                } else {
                    spans.push(Span::styled(about_line.clone(), dim));
                }
                let line = if chosen {
                    kit::row(spans, cols, false, &theme)
                        .into_iter()
                        .map(|s| {
                            let style = s.style.bg(Surface::Chosen.color(&theme));
                            Span::styled(s.content, style)
                        })
                        .collect()
                } else {
                    kit::row(spans, cols, false, &theme)
                };
                lines.push((Some(i), line));
            }
            if chosen {
                lines.push((None, Vec::new()));
            }
        }
        lines
    }

    /// The settings view, in the diff's place over the row of buttons.
    pub(super) fn draw_settings(&mut self, buf: &mut Buffer) {
        let Some(mut view) = self.settings else {
            return;
        };
        let cols = self.cols as usize;
        let height = self.rows.saturating_sub(u16::from(self.has_buttons())) as usize;
        let rows = self.settings_rows().len();
        view.at = view.at.min(rows.saturating_sub(1));
        let lines = self.settings_lines(cols, view.at);
        // The chosen row stays in sight, with its description.
        let first = lines.iter().position(|(row, _)| *row == Some(view.at));
        let last = lines.iter().rposition(|(row, _)| *row == Some(view.at));
        if let (Some(first), Some(last)) = (first, last) {
            if last >= view.top + height {
                view.top = last + 1 - height;
            }
            // Its section's label with it, when it is the section's first.
            let lead = if view.at == 0 { 0 } else { first };
            view.top = view.top.min(lead);
        }
        view.top = view.top.min(lines.len().saturating_sub(height));
        self.settings = Some(view);
        for (y, (row, line)) in lines.iter().skip(view.top).take(height).enumerate() {
            put_line(buf, 0, y as u16, self.cols, &Spans::from(line.clone()));
            if let Some(i) = row {
                self.buttons
                    .push((Button::Option(*i), Rect::new(0, y as u16, self.cols, 1)));
            }
        }
        if self.has_buttons() {
            self.draw_buttons(buf);
        }
    }

    // ── The menu ────────────────────────────────────────────────────────────

    /// `m` in the file list, or a right click: the diff's menu, Settings
    /// first, then what the row's buttons do. Alone, Settings opens the
    /// diff's own view; in the workspace it says where they are and opens
    /// mission control's Settings.
    pub(super) fn open_menu(&mut self, near: Option<(u16, u16)>) {
        // The first key of a pair typed before it isn't waited on, and a
        // selection of the diff's text ends: the keys are the menu's.
        self.pending = None;
        self.selection = None;
        self.visual = None;
        self.list_visual = None;
        self.count = None;
        let mut choices = Vec::new();
        let mut options = Vec::new();
        // With a story told in chapters, its two views first, the one
        // shown chosen as the menu opens, then What runs over the column.
        if self.mapped() {
            for (label, mode) in super::reading::MENU {
                choices.push(Choice::Mode(mode));
                options.push(MenuOption::new(label));
            }
            if self.on_story() && self.story.as_ref().is_some_and(|s| !s.runs.is_empty()) {
                choices.push(Choice::Runs);
                options.push(MenuOption::new(if self.reader.runs {
                    "Hide what runs"
                } else {
                    "What runs"
                }));
            }
        }
        // Only a story told, never the page of what isn't committed yet
        // drawn without asking.
        if self.told.is_some() && !self.in_history && self.telling != super::Telling::Waiting {
            choices.push(Choice::TellAgain);
            options.push(MenuOption::new("Tell it again"));
        }
        choices.push(if self.opts.alone {
            Choice::Settings
        } else {
            Choice::WorkspaceSettings
        });
        options.push(MenuOption::new(if self.opts.alone {
            "Settings"
        } else {
            "Settings are in Switchboard's Settings (Diff)"
        }));
        for (i, (label, keys)) in BUTTONS.iter().enumerate() {
            // Ask only with diff_ask; in a commit of the history, which
            // doesn't change, no Refresh.
            if (*keys == "ga" && !self.opts.ask) || (*keys == "R" && self.in_history) {
                continue;
            }
            choices.push(Choice::Keys(i));
            options.push(MenuOption::new(*label));
        }
        if self.offers_default_width() {
            choices.push(Choice::DefaultWidth);
            options.push(MenuOption::new("Use the file list's default width"));
        }
        let shown = choices
            .iter()
            .position(|c| *c == Choice::Mode(self.mode))
            .unwrap_or(0);
        let mut menu = Menu::new("Diff", options).at(shown);
        // Beside the click, unless boxes go in the middle (boxes_near_click).
        if let Some(at) = near.filter(|_| switchboard_ui::settings::is_on("boxes_near_click")) {
            menu.open_near(at);
        }
        let mut look = switchboard_ui::Look::read();
        look.theme = self.theme();
        self.menu = Some(Open {
            menu,
            choices,
            look,
        });
    }

    /// The menu of a change in the Story view's column, beside a click
    /// or in the middle: shown in the files, or its place copied.
    pub(super) fn open_line_menu(&mut self, near: Option<(u16, u16)>) {
        self.pending = None;
        self.selection = None;
        let title = match &self.reader.target {
            Some(
                super::reading::Does::File { path, .. } | super::reading::Does::Change { path, .. },
            ) => path.clone(),
            _ => return,
        };
        let choices = vec![Choice::InFiles, Choice::CopyPlace];
        let options = vec![
            MenuOption::new("Show it in the files"),
            MenuOption::new("Copy its place (path:line)"),
        ];
        let mut menu = Menu::new(&title, options);
        if let Some(at) = near.filter(|_| switchboard_ui::settings::is_on("boxes_near_click")) {
            menu.open_near(at);
        }
        let mut look = switchboard_ui::Look::read();
        look.theme = self.theme();
        self.menu = Some(Open {
            menu,
            choices,
            look,
        });
    }

    pub(super) fn draw_menu(&mut self, buf: &mut Buffer) {
        if let Some(Open { menu, look, .. }) = &mut self.menu {
            menu.draw(buf, look);
        }
    }

    pub(super) fn menu_key(&mut self, key: KeyEvent) -> Act {
        let (Some(Open { menu, look, .. }), Some(key)) = (&mut self.menu, ui_key(&key)) else {
            return Act::Same;
        };
        let step = menu.key(key, look);
        self.menu_step(step)
    }

    /// Text pasted while the menu is up: into its search.
    pub(super) fn menu_paste(&mut self, text: &str) -> Act {
        let Some(Open { menu, look, .. }) = &mut self.menu else {
            return Act::Same;
        };
        let step = menu.paste(text, look);
        self.menu_step(step)
    }

    /// The pointer's shape over the menu while it is up: a hand on its
    /// options, the arrow elsewhere, where a click closes it.
    pub(super) fn menu_pointer(&self, x: u16, y: u16) -> Option<switchboard_ui::pointer::Pointer> {
        use switchboard_ui::overlay::Overlay;
        let Open { menu, look, .. } = self.menu.as_ref()?;
        menu.pointer(x, y, look)
    }

    pub(super) fn menu_mouse(&mut self, m: MouseEvent) -> Act {
        let (Some(Open { menu, look, .. }), Some(mouse)) = (&mut self.menu, ui_mouse(&m)) else {
            return Act::Same;
        };
        let step = menu.mouse(mouse, look);
        self.menu_step(step)
    }

    fn menu_step(&mut self, step: Step) -> Act {
        match step {
            Step::Same | Step::Through => Act::Same,
            Step::Redraw => Act::Redraw,
            Step::Close(Outcome::Chose(i)) => {
                let choice = self
                    .menu
                    .take()
                    .and_then(|open| open.choices.get(i).copied());
                self.take_choice(choice)
            }
            Step::Close(_) | Step::Pass => {
                self.menu = None;
                Act::Redraw
            }
        }
    }

    fn take_choice(&mut self, choice: Option<Choice>) -> Act {
        match choice {
            Some(Choice::Settings) => self.toggle_settings(),
            Some(Choice::Mode(mode)) => self.choose_mode(mode),
            Some(Choice::DefaultWidth) => self.default_width(),
            Some(Choice::Runs) => self.toggle_runs(),
            Some(Choice::TellAgain) => self.tell_again(),
            Some(Choice::InFiles) => self.show_target(),
            Some(Choice::CopyPlace) => self.copy_target(),
            Some(Choice::WorkspaceSettings) => {
                // Mission control's Settings, where the Diff category holds
                // them; with no server to ask, where they are is said.
                let opened = self.opts.server
                    && host::host()
                        .command(&["press", "settings"])
                        .is_some_and(|(code, ..)| code == 0);
                if !opened {
                    self.say("The diff's settings are in Switchboard's Settings, under Diff.");
                }
            }
            Some(Choice::Keys(i)) => {
                let keys = BUTTONS.get(i).map_or("", |(_, keys)| *keys);
                self.focus = Focus::List;
                for c in keys.chars() {
                    if self.key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)) == Act::Quit {
                        return Act::Quit;
                    }
                }
            }
            None => {}
        }
        Act::Redraw
    }
}

impl Options {
    /// The options as the settings say them now, with what isn't a
    /// setting's kept as it is (whether Haiku, the server and the clipboard
    /// are there, and whether the terminal is light).
    pub(super) fn read_again(self) -> Options {
        Options {
            haiku: self.haiku,
            server: self.server,
            clipboard: self.clipboard,
            alone: self.alone,
            light: self.light,
            ..Options::read()
        }
    }
}

/// A button's label drawn as the kit draws Switchboard's own: bold in the
/// accent on the panel surface between its ends, on the chosen surface
/// under the mouse, dark on the accent while what it opens is in front.
pub(super) fn button(
    label: &str,
    hover: bool,
    lit: bool,
    round: bool,
    theme: &switchboard_ui::Theme,
) -> Vec<Span<'static>> {
    let press = if lit {
        kit::Press::Lit
    } else if hover {
        kit::Press::Hover
    } else {
        kit::Press::Rest
    };
    kit::button(label, None, press, round, theme)
        .into_iter()
        .map(|s| {
            let style = s.style.remove_modifier(Modifier::DIM);
            Span::styled(s.content, style)
        })
        .collect()
}
