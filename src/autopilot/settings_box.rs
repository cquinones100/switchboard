//! Autopilot's settings in one box (UX 2.0 board 35), in place of the
//! chain of boxes that set it up and the menu options that each changed one
//! count: "Work a note" while the mode is off, which starts it, and
//! "Autopilot · <note>" while it is on, which saves what changed at once.
//! No tmux-version file is behind it; it takes the place of
//! `autopilot_setup`'s chain (src/autopilot/serve.rs) with UX 2.0 on.
//!
//! Under a status line (working, paused or off, its tabs working and open,
//! today's spend against the limit), each setting is a row: the note (Enter
//! picks another), the repository and the branch it lands on (each typed,
//! a suggestion dim after what is typed that → takes, how it lands under
//! the branch), the tabs at once and the most open (steppers, − and + or ←
//! and →), the spend limit (dollars, with a meter of today's spend against
//! it), Planned only, Land without asking and Hands-free (switches, Space
//! flips one), and the note's skipped items (u takes one back) with the
//! items left out for needing a choice. Under a rule, its keys as keycaps:
//! M-↵ Save (Start while off), p Pause or Resume and s Stop while on, m
//! the menu's other options, esc Cancel, asking first when anything
//! changed.
//!
//! Rows are laid out with `kit::columns`, so the box fills any width as
//! board 45 says: under 160 columns the steppers' words go, then the line
//! under Lands on saying the dim rest is a suggestion, then each switch's
//! second line, and the skipped items fold to a line with their count.
//!
//! `Fields` is plain data, read from the mode and applied to it, so the box
//! is tested without a server. It closes with `Outcome::Picked`, its index
//! what was pressed (`SAVE`, `PAUSE`, `STOP`, `NOTE`, `MENU`) and its value
//! the fields as JSON, which the server reopens the box with when it has
//! more to ask (another note, a confirm said no, a problem to show).

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;
use serde::{Deserialize, Serialize};

use super::mode::{Mode, note_title};
use crate::mux::compose::Frame;
use crate::mux::input::{Key, KeyCode, Mods, Mouse};
use crate::ui::draw::{self, BoxLine, BoxSpec, Drawn, Gesture, Hit};
use crate::ui::images::{self, Tones};
use crate::ui::kit::{self, Col, Mark, Surface, Tone};
use crate::ui::pointer::Pointer;
use crate::ui::{Look, Outcome, Overlay, Picked, Step};

/// Its inside, while the screen leaves room around it.
const WIDTH: usize = 92;
/// The screen narrower than which its rows drop their extras.
pub const WIDE: u16 = 160;

/// What it closed with, as `Picked::index`.
pub const SAVE: usize = 0;
pub const PAUSE: usize = 1;
pub const STOP: usize = 2;
pub const NOTE: usize = 3;
pub const MENU: usize = 4;

/// The settings as typed in the box.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Fields {
    /// The note's path within the notes directory.
    pub note: String,
    /// The repository as typed (`~/` for the home directory).
    pub repo: String,
    /// The branch it lands on; empty for the default one.
    pub target: String,
    pub slots: u32,
    pub cap: u32,
    /// Dollars as typed; empty for no limit.
    pub limit: String,
    pub ready_only: bool,
    pub land_without_asking: bool,
    pub hands_free: bool,
    /// The note's skipped items still skipped, by their first line.
    pub skipped: Vec<String>,
    /// The ID of the note those skipped items are of, which another note
    /// picked in the box changes along with them.
    pub skipped_of: String,
    /// The skipped items taken back in the box, the only ones saving takes
    /// back, so an item skipped meanwhile (wtn refusing it) stays skipped.
    pub taken_back: Vec<String>,
}

impl Fields {
    pub fn from_mode(mode: &Mode) -> Fields {
        Fields {
            note: mode.note.clone(),
            repo: if mode.set_up() {
                crate::paths::tilde(&mode.repo)
            } else {
                String::new()
            },
            target: mode.target.clone(),
            slots: mode.slots,
            cap: mode.cap,
            limit: limit_text(mode.spend_limit),
            ready_only: mode.ready_only,
            land_without_asking: mode.land_without_asking,
            hands_free: mode.hands_free,
            skipped: mode.skipped_here().map(str::to_string).collect(),
            skipped_of: mode.note_id.clone(),
            taken_back: Vec::new(),
        }
    }

    /// The spend limit typed, 0 for none; `None` when it isn't dollars.
    pub fn limit(&self) -> Option<f64> {
        let text = self.limit.trim();
        if text.is_empty() {
            return Some(0.0);
        }
        let n: f64 = text.strip_prefix('$').unwrap_or(text).trim().parse().ok()?;
        (n.is_finite() && n >= 0.0).then_some(n)
    }

    /// The repository typed, `~/` read as the home directory.
    pub fn repo_path(&self) -> std::path::PathBuf {
        let text = self.repo.trim();
        match text.strip_prefix("~/") {
            Some(rest) => crate::paths::home().join(rest),
            None if text == "~" => crate::paths::home(),
            None => std::path::PathBuf::from(text),
        }
    }

    /// The counts, the switches, the target, the spend limit and the
    /// skipped items written into `mode`, the cap never below the slots and
    /// Hands-free landing without asking, as the menu's options keep them;
    /// the note and the repository are the server's to set, since each
    /// needs a look at the files. A limit that isn't dollars is left as it
    /// was. What it said of a passed limit goes with a new limit.
    pub fn apply(&self, mode: &mut Mode) {
        mode.slots = self.slots.max(1);
        mode.cap = self.cap.max(mode.slots);
        mode.target = self.target.trim().to_string();
        if let Some(limit) = self.limit() {
            if mode.spend_limit != limit && mode.said.starts_with(super::pilot::SPENT) {
                mode.said.clear();
            }
            mode.spend_limit = limit;
        }
        mode.ready_only = self.ready_only;
        mode.hands_free = self.hands_free;
        mode.land_without_asking = self.land_without_asking || self.hands_free;
        mode.skipped
            .retain(|x| x.note != self.skipped_of || !self.taken_back.contains(&x.text));
    }

    /// What changed from `was`, in words, for what the box says on saving.
    pub fn changes(&self, was: &Fields) -> Vec<String> {
        let mut out = Vec::new();
        let on_off = |on: bool| if on { "on" } else { "off" };
        if self.note != was.note {
            out.push(format!("works {}", note_title(&self.note)));
        }
        if self.repo.trim() != was.repo.trim() {
            out.push(format!("in {}", self.repo.trim()));
        }
        if self.target.trim() != was.target.trim() {
            out.push(match self.target.trim() {
                "" => "lands on the default branch".to_string(),
                t => format!("lands on {t}"),
            });
        }
        if self.slots != was.slots {
            out.push(format!("{} tabs at once", self.slots));
        }
        if self.cap.max(self.slots) != was.cap.max(was.slots) {
            out.push(format!("{} open at most", self.cap.max(self.slots)));
        }
        if self.limit() != was.limit() {
            out.push(match self.limit() {
                Some(l) if l > 0.0 => format!("spend limit ${} a day", dollars_said(l)),
                _ => "no spend limit".to_string(),
            });
        }
        if self.ready_only != was.ready_only {
            out.push(format!("planned only {}", on_off(self.ready_only)));
        }
        let lands = |f: &Fields| f.land_without_asking || f.hands_free;
        if lands(self) != lands(was) {
            out.push(format!("land without asking {}", on_off(lands(self))));
        }
        if self.hands_free != was.hands_free {
            out.push(format!("hands-free {}", on_off(self.hands_free)));
        }
        for gone in &self.taken_back {
            out.push(format!("back in line: {gone}"));
        }
        out
    }
}

/// A limit in dollars as the field shows it: none empty, else exactly as
/// stored, so saving another setting never rounds it.
pub fn limit_text(limit: f64) -> String {
    if limit <= 0.0 {
        String::new()
    } else {
        format!("{limit}")
    }
}

/// A limit in dollars as said: whole dollars without cents, else to the cent.
fn dollars_said(limit: f64) -> String {
    if limit.fract() == 0.0 {
        format!("{limit}")
    } else {
        format!("{limit:.2}")
    }
}

/// What the box shows besides the fields, read as it opens.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Context {
    /// The mode is on (Save, Pause and Stop), else off (Start).
    pub on: bool,
    pub paused: bool,
    /// Its tabs working, and open.
    pub busy: usize,
    pub open: usize,
    /// Today's Claude spend, in dollars.
    pub spent: f64,
    /// The repository the landing words are for, as typed in the field.
    pub landing_repo: String,
    /// How it lands there, in words ("straight on main through
    /// queue-and-clean, as repos.json says").
    pub landing: String,
    /// The repositories it suggests, as `~/` paths.
    pub repos: Vec<String>,
    /// The branches it suggests, the default one first.
    pub branches: Vec<String>,
    /// The note's items left out for needing a choice.
    pub needs_choice: Vec<String>,
    /// Whether Hands-free is offered (the autopilot_handsfree setting).
    pub offer_hands_free: bool,
    /// Why it couldn't save, shown under the Repository field.
    pub problem: String,
}

/// What Tab's focus or the mouse is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    Note,
    Repo,
    Target,
    Slots,
    Cap,
    Limit,
    Planned,
    Land,
    HandsFree,
    Skipped(usize),
    /// The skipped items folded to their count, on a narrow screen.
    Folded,
    Save,
    Pause,
    Stop,
    Menu,
    Cancel,
    Close,
    /// The question before leaving changes: leave them, or stay.
    Leave,
    Stay,
    /// A skipped item's "Take it back" keycap.
    TakeBack(usize),
    /// A stepper's − or +, by the field it changes.
    Less(Field),
    More(Field),
}

/// A stepper's field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    Slots,
    Cap,
}

/// A part on a line: the line's index, its first and last column, the part.
type Placed = (usize, u16, u16, Part);

/// An image to place once the box is drawn: its line, its column, its
/// width, and what it is.
#[derive(Clone, Debug, PartialEq)]
enum Picture {
    Switch(bool),
    Meter(f32, ratatui::style::Color),
}

/// An image on a line: the line's index, its column, its width, what it is.
type Pictured = (usize, u16, u16, Picture);

pub struct SettingsBox {
    fields: Fields,
    /// The fields as it opened, to tell whether anything changed.
    was: Fields,
    ctx: Context,
    focus: Part,
    hover: Option<Part>,
    /// Esc pressed with changes: asking whether to leave them.
    leaving: Option<Step>,
    /// Where the focus goes back to once the question says stay.
    back_to: Part,
    /// The skipped items shown though the screen is narrow.
    unfolded: bool,
    /// The screen was 160 columns or more when it was last drawn.
    screen_wide: bool,
    drawn: Option<Drawn>,
    parts: Vec<Placed>,
    pictures: Vec<Pictured>,
}

impl SettingsBox {
    /// The box for `fields`, `was` what they were before this box or the
    /// one it reopens, the focus on `focus`.
    pub fn new(fields: Fields, was: Fields, ctx: Context, focus: Part) -> SettingsBox {
        SettingsBox {
            fields,
            was,
            ctx,
            focus,
            hover: None,
            leaving: None,
            back_to: focus,
            unfolded: false,
            screen_wide: true,
            drawn: None,
            parts: Vec::new(),
            pictures: Vec::new(),
        }
    }

    pub fn fields(&self) -> &Fields {
        &self.fields
    }

    pub fn title(&self) -> String {
        if self.ctx.on {
            format!("Autopilot · {}", note_title(&self.fields.note))
        } else {
            "Work a note".to_string()
        }
    }

    fn changed(&self) -> bool {
        self.fields != self.was
    }

    fn stops(&self, look: &Look, wide: bool) -> Vec<Part> {
        let mut out = vec![
            Part::Note,
            Part::Repo,
            Part::Target,
            Part::Slots,
            Part::Cap,
            Part::Limit,
            Part::Planned,
            Part::Land,
        ];
        if self.ctx.offer_hands_free || self.fields.hands_free {
            out.push(Part::HandsFree);
        }
        if self.folded(wide) {
            out.push(Part::Folded);
        } else {
            // The items needing a choice are stops too, after the skipped
            // ones, so a long list's window reaches them.
            let n = self.fields.skipped.len() + self.ctx.needs_choice.len();
            out.extend((0..n).map(Part::Skipped));
        }
        out.push(Part::Save);
        if self.ctx.on {
            out.extend([Part::Pause, Part::Stop, Part::Menu]);
        }
        out.push(Part::Cancel);
        if look.close {
            out.push(Part::Close);
        }
        out
    }

    fn folded(&self, wide: bool) -> bool {
        !wide && !self.unfolded && !self.fields.skipped.is_empty()
    }

    fn step_focus(&mut self, by: isize, look: &Look) {
        let stops = self.stops(look, self.screen_wide);
        let at = stops.iter().position(|p| *p == self.focus).unwrap_or(0) as isize;
        self.focus = stops[(at + by).rem_euclid(stops.len() as isize) as usize];
    }

    fn close(&self, index: usize) -> Step {
        Step::Close(Outcome::Picked(Picked {
            index,
            value: serde_json::to_string(&self.fields).unwrap_or_default(),
            query: String::new(),
        }))
    }

    fn step(&mut self, field: Field, by: i64) -> Step {
        let n = match field {
            Field::Slots => &mut self.fields.slots,
            Field::Cap => &mut self.fields.cap,
        };
        let next = (*n as i64 + by).clamp(1, 99) as u32;
        if next == *n {
            return Step::Same;
        }
        *n = next;
        // The cap follows the slots up, and the slots the cap down.
        match field {
            Field::Slots if self.fields.cap < next => self.fields.cap = next,
            Field::Cap if self.fields.slots > next => self.fields.slots = next,
            _ => {}
        }
        Step::Redraw
    }

    fn flip(&mut self, part: Part) -> Step {
        let f = &mut self.fields;
        match part {
            Part::Planned => f.ready_only = !f.ready_only,
            // Hands-free lands without asking, so landing goes off with it.
            Part::Land => {
                f.land_without_asking = !(f.land_without_asking || f.hands_free);
                if !f.land_without_asking {
                    f.hands_free = false;
                }
            }
            Part::HandsFree => {
                f.hands_free = !f.hands_free;
                if f.hands_free {
                    f.land_without_asking = true;
                }
            }
            _ => return Step::Same,
        }
        Step::Redraw
    }

    fn press(&mut self, part: Part) -> Step {
        match part {
            Part::Note => self.close(NOTE),
            Part::Repo | Part::Target | Part::Limit | Part::Slots | Part::Cap => {
                self.focus = part;
                Step::Redraw
            }
            Part::Planned | Part::Land | Part::HandsFree => {
                self.focus = part;
                self.flip(part)
            }
            Part::Skipped(_) => {
                self.focus = part;
                Step::Redraw
            }
            Part::Folded => {
                self.unfolded = true;
                self.focus = Part::Skipped(0);
                Step::Redraw
            }
            Part::Less(f) => self.step(f, -1),
            Part::More(f) => self.step(f, 1),
            Part::Save => self.close(SAVE),
            Part::Pause if self.ctx.on => self.leave(self.close(PAUSE)),
            Part::Stop if self.ctx.on => self.leave(self.close(STOP)),
            Part::Menu if self.ctx.on => self.leave(self.close(MENU)),
            Part::Cancel | Part::Close => self.cancel(),
            Part::Leave => self
                .leaving
                .take()
                .unwrap_or(Step::Close(Outcome::Dismissed)),
            Part::TakeBack(i) => self.unskip(i),
            Part::Stay => {
                self.leaving = None;
                self.focus = self.back_to;
                Step::Redraw
            }
            _ => Step::Same,
        }
    }

    fn cancel(&mut self) -> Step {
        self.leave(Step::Close(Outcome::Dismissed))
    }

    /// `then`, which leaves the box without saving (cancelling, or Pause,
    /// Stop and More…), asked about first when anything was changed.
    fn leave(&mut self, then: Step) -> Step {
        if self.changed() {
            self.leaving = Some(then);
            self.back_to = self.focus;
            self.focus = Part::Leave;
            Step::Redraw
        } else {
            then
        }
    }

    fn unskip(&mut self, i: usize) -> Step {
        if i >= self.fields.skipped.len() {
            return Step::Same;
        }
        let text = self.fields.skipped.remove(i);
        self.fields.taken_back.push(text);
        let n = self.fields.skipped.len() + self.ctx.needs_choice.len();
        self.focus = if n == 0 {
            Part::Save
        } else {
            Part::Skipped(i.min(n - 1))
        };
        Step::Redraw
    }

    fn text_field(&mut self) -> Option<&mut String> {
        match self.focus {
            Part::Repo => Some(&mut self.fields.repo),
            Part::Target => Some(&mut self.fields.target),
            Part::Limit => Some(&mut self.fields.limit),
            _ => None,
        }
    }

    /// The rest of the suggestion for what is typed in a field: the first
    /// it suggests that starts with what is typed and is longer.
    fn suggestion(&self, part: Part) -> Option<String> {
        let (typed, list) = match part {
            Part::Repo => (&self.fields.repo, &self.ctx.repos),
            // The branches read are the repository's it opened on, so
            // none are suggested once another is typed.
            Part::Target if self.fields.repo.trim() == self.ctx.landing_repo.trim() => {
                (&self.fields.target, &self.ctx.branches)
            }
            _ => return None,
        };
        list.iter()
            .find(|s| s.starts_with(typed.as_str()) && s.len() > typed.len())
            .map(|s| s[typed.len()..].to_string())
    }

    fn take_suggestion(&mut self) -> Step {
        let Some(rest) = self.suggestion(self.focus) else {
            return Step::Same;
        };
        if let Some(text) = self.text_field() {
            text.push_str(&rest);
        }
        Step::Redraw
    }

    /// The box's lines `inner` wide, where each part on them is, and where
    /// its images go; `wide` while the screen is 160 columns or more, and
    /// `room` the most skipped and needing-a-choice items shown at once
    /// (a window kept around the one with the focus), `None` for all.
    fn lines(
        &self,
        inner: usize,
        wide: bool,
        room: Option<usize>,
        look: &Look,
    ) -> (Vec<BoxLine>, Vec<Placed>, Vec<Pictured>) {
        let theme = &look.theme;
        let cols = inner.saturating_sub(1);
        let mut lines: Vec<BoxLine> = Vec::new();
        let mut parts: Vec<Placed> = Vec::new();
        let mut pictures = Vec::new();
        let ctx = &self.ctx;
        let f = &self.fields;

        // The status line.
        let (mark, state) = match (ctx.on, ctx.paused) {
            (true, false) => (Mark::Working, "working"),
            (true, true) => (Mark::Later, "paused"),
            _ => (Mark::Idle, "off"),
        };
        let mut status = vec![
            Span::raw(" "),
            kit::mark(mark, 0, theme),
            Span::raw(" "),
            Span::styled(state, Tone::Text.bold(theme)),
        ];
        let mut facts = Vec::new();
        if ctx.on {
            facts.push(format!("{} tabs working, {} open", ctx.busy, ctx.open));
        }
        facts.push(match f.limit() {
            Some(l) if l > 0.0 => format!("${:.2} of ${:.2} today", ctx.spent, l),
            _ => format!("${:.2} today", ctx.spent),
        });
        status.push(Span::styled(
            format!(" · {}", facts.join(" · ")),
            Tone::Dim.style(theme),
        ));
        lines.push(BoxLine::spans(kit::fit(status, inner, Style::new())));
        lines.push(BoxLine::blank());

        let names = [
            "Note",
            "Repository",
            "Lands on",
            "Tabs at once",
            "Most open",
            "Spend limit",
            "Planned only",
            "Land without asking",
            "Hands-free",
        ];
        let name_w = kit::natural(names).min(20);
        let name_col = Col::Fixed {
            min: 4,
            natural: name_w,
            cap: 20,
        };
        let name = |text: &str, part: Part| -> Vec<Span<'static>> {
            let on = self.focus == part;
            let style = if on {
                Tone::Accent.bold(theme)
            } else {
                Tone::Dim.style(theme)
            };
            vec![Span::styled(text.to_string(), style)]
        };
        // A row of cells at the columns `columns` gives, a space before.
        let row = |cells: Vec<Vec<Span<'static>>>, at: &[Option<(usize, usize)>]| {
            let mut line = vec![Span::raw(" ")];
            for (cell, place) in cells.into_iter().zip(at) {
                if let Some((x, w)) = place {
                    kit::place(&mut line, cell, (x + 1, *w));
                }
            }
            line
        };
        let field_cols = kit::columns(
            &[
                name_col,
                Col::Stretch {
                    weight: 1,
                    min: 10,
                    max: None,
                },
            ],
            cols,
            2,
            &[],
        );
        let input_at = field_cols[1].unwrap_or((0, cols));
        let placed = |at: (usize, usize)| ((at.0 + 1) as u16, (at.0 + at.1) as u16);
        // Words under a field, wrapped in its column, lined up with what is
        // typed in it.
        let under = |text: &str, style: Style| -> Vec<BoxLine> {
            draw::wrap(text, input_at.1.saturating_sub(3))
                .into_iter()
                .map(|l| {
                    BoxLine::spans(row(
                        vec![Vec::new(), vec![Span::styled(format!("   {l}"), style)]],
                        &field_cols,
                    ))
                })
                .collect()
        };

        // Note: Enter picks another.
        let note_text = if f.note.is_empty() {
            "pick a note".to_string()
        } else {
            note_title(&f.note)
        };
        let note = self.input(
            &note_text,
            None,
            input_at.1,
            Part::Note,
            Some("↵ another note"),
            look,
        );
        parts.push((
            lines.len(),
            placed(input_at).0,
            placed(input_at).1,
            Part::Note,
        ));
        lines.push(
            BoxLine::spans(row(vec![name("Note", Part::Note), note], &field_cols)).clickable(),
        );

        // Repository, its suggestion → takes.
        let repo = self.input(
            &f.repo,
            self.suggestion(Part::Repo),
            input_at.1,
            Part::Repo,
            None,
            look,
        );
        parts.push((
            lines.len(),
            placed(input_at).0,
            placed(input_at).1,
            Part::Repo,
        ));
        lines.push(
            BoxLine::spans(row(vec![name("Repository", Part::Repo), repo], &field_cols))
                .clickable(),
        );
        if !ctx.problem.is_empty() {
            lines.extend(under(&ctx.problem, Tone::Needs.style(theme)));
        }

        // Lands on, and how it lands.
        let target = self.input(
            &f.target,
            self.suggestion(Part::Target),
            input_at.1,
            Part::Target,
            None,
            look,
        );
        parts.push((
            lines.len(),
            placed(input_at).0,
            placed(input_at).1,
            Part::Target,
        ));
        lines.push(
            BoxLine::spans(row(
                vec![name("Lands on", Part::Target), target],
                &field_cols,
            ))
            .clickable(),
        );
        let lands = if f.repo.trim() == ctx.landing_repo.trim() && !ctx.landing.is_empty() {
            ctx.landing.clone()
        } else {
            "how it lands is checked as it starts".to_string()
        };
        let branch = match f.target.trim() {
            "" => "the default branch".to_string(),
            t => t.to_string(),
        };
        let words = format!("{branch}: {lands}");
        lines.extend(under(&words, Tone::Dim.style(theme)));
        if wide {
            lines.extend(under(
                "empty for the default branch; the dim rest is a suggestion, → takes it",
                Tone::Faint.style(theme),
            ));
        }

        // The steppers.
        let stepper_cols = kit::columns(
            &[
                name_col,
                Col::Fixed {
                    min: 9,
                    natural: 9,
                    cap: 9,
                },
                Col::Stretch {
                    weight: 1,
                    min: 8,
                    max: None,
                },
            ],
            cols,
            2,
            &[2],
        );
        for (label, field, part, n, what) in [
            (
                "Tabs at once",
                Field::Slots,
                Part::Slots,
                f.slots,
                "how many of its tabs work at once; one waiting on you doesn't count",
            ),
            (
                "Most open",
                Field::Cap,
                Part::Cap,
                f.cap.max(f.slots),
                "the most of its tabs open at once, those waiting on you included",
            ),
        ] {
            let at = stepper_cols[1].unwrap_or((0, 9));
            let x = (at.0 + 1) as u16;
            let lit = |p: Part| self.hover == Some(p);
            let cap_style = |p: Part| {
                let s = Tone::Accent.bold(theme);
                let bg = if lit(p) {
                    Surface::Chosen.color(theme)
                } else {
                    Surface::Panel.color(theme)
                };
                s.bg(bg)
            };
            let number = if self.focus == part {
                Style::new().fg(theme.bright).add_modifier(Modifier::BOLD)
            } else {
                Tone::Text.bold(theme)
            };
            let stepper = vec![
                Span::styled(" − ", cap_style(Part::Less(field))),
                Span::styled(format!("{n:^3}"), number),
                Span::styled(" + ", cap_style(Part::More(field))),
            ];
            parts.push((lines.len(), x, x + 2, Part::Less(field)));
            parts.push((lines.len(), x + 3, x + 5, part));
            parts.push((lines.len(), x + 6, x + 8, Part::More(field)));
            let words = if wide {
                vec![Span::styled(what.to_string(), Tone::Dim.style(theme))]
            } else {
                Vec::new()
            };
            lines.push(
                BoxLine::spans(row(vec![name(label, part), stepper, words], &stepper_cols))
                    .clickable(),
            );
        }

        // The spend limit, with today's spend against it.
        let spent = format!("${:.2} today", ctx.spent);
        let spent_w = draw::width(&spent);
        let limit_cols = kit::columns(
            &[
                name_col,
                Col::Fixed {
                    min: 12,
                    natural: 30.min(cols.saturating_sub(name_w + spent_w + 4)),
                    cap: 30,
                },
                Col::Stretch {
                    weight: 1,
                    min: 6,
                    max: None,
                },
                Col::Right {
                    natural: spent_w,
                    cap: spent_w,
                },
            ],
            cols,
            2,
            &[2],
        );
        let limit_at = limit_cols[1].unwrap_or((0, 12));
        let limit = self.input(&f.limit, None, limit_at.1, Part::Limit, None, look);
        parts.push((
            lines.len(),
            placed(limit_at).0,
            placed(limit_at).1,
            Part::Limit,
        ));
        let mut cells = vec![name("Spend limit", Part::Limit), limit];
        if let Some(meter_at) = limit_cols[2] {
            let (frac, tone) = match f.limit() {
                Some(l) if l > 0.0 => {
                    let frac = (ctx.spent / l) as f32;
                    let tone = if frac >= 1.0 {
                        Tone::Failed
                    } else if frac >= 0.8 {
                        Tone::Needs
                    } else {
                        Tone::Done
                    };
                    (frac.min(1.0), tone)
                }
                _ => (0.0, Tone::Dim),
            };
            let filled = (meter_at.1 as f32 * frac).round() as usize;
            cells.push(vec![
                Span::styled("━".repeat(filled), tone.style(theme)),
                Span::styled(
                    "━".repeat(meter_at.1.saturating_sub(filled)),
                    Style::new().fg(kit::border(theme)),
                ),
            ]);
            pictures.push((
                lines.len(),
                (meter_at.0 + 1) as u16,
                meter_at.1 as u16,
                Picture::Meter(frac, tone.fg(theme)),
            ));
        } else {
            cells.push(Vec::new());
        }
        cells.push(vec![Span::styled(spent, Tone::Dim.style(theme))]);
        lines.push(BoxLine::spans(row(cells, &limit_cols)).clickable());
        lines.extend(under(
            "dollars a day, empty for none: it opens no new tab past it; the ones working finish",
            Tone::Faint.style(theme),
        ));

        // The switches.
        let switch_cols = kit::columns(
            &[
                name_col,
                Col::Fixed {
                    min: 3,
                    natural: images::SWITCH_COLS as usize,
                    cap: images::SWITCH_COLS as usize,
                },
                Col::Stretch {
                    weight: 1,
                    min: 8,
                    max: Some(kit::WRAP_MAX),
                },
            ],
            cols,
            2,
            &[],
        );
        let tones = Tones::now();
        let mut switches = vec![
            (
                "Planned only",
                Part::Planned,
                f.ready_only,
                "take only items whose plan is settled; off takes those without a plan too, after them",
            ),
            (
                "Land without asking",
                Part::Land,
                f.land_without_asking || f.hands_free,
                "land on the branch once the tests and two clean reviews pass, telling you after; off asks before each push",
            ),
        ];
        if ctx.offer_hands_free || f.hands_free {
            switches.push((
                "Hands-free",
                Part::HandsFree,
                f.hands_free,
                "decide for you, taking the recommended option and writing it in the item's plan; off asks you",
            ));
        }
        for (label, part, on, what) in switches {
            let at = switch_cols[1].unwrap_or((0, 4));
            let words_at = switch_cols[2].unwrap_or((0, 8));
            let (text, color) = images::switch_text(on, &tones);
            let mut style = Style::new().fg(color);
            if self.focus == part || self.hover == Some(part) {
                style = style.bg(Surface::Chosen.color(theme));
            }
            let wrapped = draw::wrap(what, words_at.1);
            let shown = if wide { 2 } else { 1 };
            let first = wrapped.first().cloned().unwrap_or_default();
            parts.push((
                lines.len(),
                (at.0 + 1) as u16,
                (words_at.0 + words_at.1) as u16,
                part,
            ));
            pictures.push((
                lines.len(),
                (at.0 + 1) as u16,
                at.1 as u16,
                Picture::Switch(on),
            ));
            lines.push(
                BoxLine::spans(row(
                    vec![
                        name(label, part),
                        vec![Span::styled(text, style)],
                        vec![Span::styled(first, Tone::Dim.style(theme))],
                    ],
                    &switch_cols,
                ))
                .clickable(),
            );
            for more in wrapped.iter().skip(1).take(shown - 1) {
                lines.push(BoxLine::spans(row(
                    vec![
                        Vec::new(),
                        Vec::new(),
                        vec![Span::styled(more.clone(), Tone::Dim.style(theme))],
                    ],
                    &switch_cols,
                )));
            }
        }

        // The skipped items, and those needing a choice.
        let n = f.skipped.len() + ctx.needs_choice.len();
        if n > 0 {
            lines.push(BoxLine::blank());
            let tone = if self.focus == Part::Folded || matches!(self.focus, Part::Skipped(_)) {
                Tone::Accent
            } else {
                Tone::Dim
            };
            if self.folded(wide) {
                let mut label = kit::label("Skipped", tone, None, inner, theme);
                label.truncate(2);
                label.push(Span::styled(
                    format!("  {n} · ↵ shows them"),
                    Tone::Dim.style(theme),
                ));
                parts.push((lines.len(), 0, inner as u16, Part::Folded));
                let fill = (self.focus == Part::Folded).then(|| Surface::Chosen.color(theme));
                lines.push(BoxLine::spans(label).fill(fill).clickable());
            } else {
                lines.push(BoxLine::spans(kit::label(
                    "Skipped",
                    tone,
                    Some(n),
                    inner,
                    theme,
                )));
                // The item stretches, what it is sits at the right edge.
                let item_cols = |right: usize| {
                    kit::columns(
                        &[
                            Col::Stretch {
                                weight: 1,
                                min: 10,
                                max: None,
                            },
                            Col::Right {
                                natural: right,
                                cap: right,
                            },
                        ],
                        cols,
                        2,
                        &[1],
                    )
                };
                let room = room.unwrap_or(n).clamp(1, n);
                let start = match self.focus {
                    Part::Skipped(i) if i >= room => i + 1 - room,
                    _ => 0,
                };
                let shown = start..start + room;
                for (i, text) in f.skipped.iter().enumerate() {
                    if !shown.contains(&i) {
                        continue;
                    }
                    let chosen = self.focus == Part::Skipped(i);
                    let lit = self.hover == Some(Part::Skipped(i));
                    // What it is, then a keycap a click or u takes it back with.
                    let said = "skipped by you ";
                    let mut words = vec![Span::styled(said, Tone::Dim.style(theme))];
                    let back = Part::TakeBack(i);
                    let cap =
                        kit::keycap("u", "Take it back", self.hover == Some(back), false, theme);
                    let cap_w = kit::spans_width(&cap);
                    words.extend(cap);
                    let at = item_cols(kit::spans_width(&words));
                    if let Some((x, _)) = at[1] {
                        let x0 = x + 1 + draw::width(said);
                        parts.push((
                            lines.len(),
                            x0 as u16,
                            (x0 + cap_w).saturating_sub(2) as u16,
                            back,
                        ));
                    }
                    let line = row(
                        vec![
                            vec![Span::styled(text.clone(), Tone::Text.style(theme))],
                            words,
                        ],
                        &at,
                    );
                    let fill = if chosen {
                        Some(Surface::Chosen.color(theme))
                    } else if lit {
                        Some(Surface::Panel.color(theme))
                    } else {
                        None
                    };
                    parts.push((lines.len(), 0, inner as u16, Part::Skipped(i)));
                    lines.push(BoxLine::spans(line).fill(fill).clickable());
                }
                for (i, text) in ctx.needs_choice.iter().enumerate() {
                    let at = f.skipped.len() + i;
                    if !shown.contains(&at) {
                        continue;
                    }
                    let chosen = self.focus == Part::Skipped(at);
                    let mut chip = kit::chip("needs a choice", Tone::Needs, false, theme);
                    chip.pop();
                    let line = row(
                        vec![
                            vec![Span::styled(text.clone(), Tone::Text.style(theme))],
                            chip.clone(),
                        ],
                        &item_cols(kit::spans_width(&chip)),
                    );
                    let fill = if chosen {
                        Some(Surface::Chosen.color(theme))
                    } else if self.hover == Some(Part::Skipped(at)) {
                        Some(Surface::Panel.color(theme))
                    } else {
                        None
                    };
                    parts.push((lines.len(), 0, inner as u16, Part::Skipped(at)));
                    lines.push(BoxLine::spans(line).fill(fill).clickable());
                }
                let hidden = n - shown.len();
                if hidden > 0 {
                    lines.push(BoxLine::text(
                        format!(" … {hidden} more, Tab or ↓ through them"),
                        Tone::Dim.style(theme),
                    ));
                }
            }
        }

        // The keys, as keycaps under a rule.
        lines.push(BoxLine::spans(vec![kit::rule(inner, theme)]));
        if self.leaving.is_some() {
            let mut line = vec![
                Span::raw(" "),
                Span::styled(
                    "Leave without saving your changes? [Y/n]",
                    Style::new().fg(draw::YELLOW).add_modifier(Modifier::BOLD),
                ),
                Span::raw("  "),
            ];
            for (key, what, part) in [("↵", "Yes", Part::Leave), ("n", "No", Part::Stay)] {
                let x = kit::spans_width(&line);
                let lit = self.focus == part || self.hover == Some(part);
                let spans = kit::keycap(key, what, lit, false, theme);
                let w = kit::spans_width(&spans);
                line.extend(spans);
                parts.push((
                    lines.len(),
                    x as u16,
                    (x + w).saturating_sub(2) as u16,
                    part,
                ));
            }
            lines.push(BoxLine::spans(line).clickable());
            return (lines, parts, pictures);
        }
        let mut caps = vec![Span::raw(" ")];
        let mut cap = |key: &str, what: &str, part: Part, caps: &mut Vec<Span<'static>>| {
            let x = kit::spans_width(caps);
            let lit = self.focus == part || self.hover == Some(part);
            let spans = kit::keycap(key, what, lit, false, theme);
            let w = kit::spans_width(&spans);
            caps.extend(spans);
            parts.push((
                lines.len(),
                x as u16,
                (x + w).saturating_sub(2) as u16,
                part,
            ));
        };
        cap(
            "M-↵",
            if ctx.on { "Save" } else { "Start" },
            Part::Save,
            &mut caps,
        );
        if ctx.on {
            cap(
                "p",
                if ctx.paused { "Resume" } else { "Pause" },
                Part::Pause,
                &mut caps,
            );
            cap("s", "Stop", Part::Stop, &mut caps);
            cap("m", "More…", Part::Menu, &mut caps);
        }
        cap("esc", "Cancel", Part::Cancel, &mut caps);
        lines.push(BoxLine::spans(caps).clickable());
        (lines, parts, pictures)
    }

    /// A field `cols` wide on the panel: a `›`, what is typed, the cursor
    /// while it has the focus, the rest of a suggestion faint after it, and
    /// `hint` (or `→ takes` for a suggestion, with the focus) at the right.
    fn input(
        &self,
        text: &str,
        suggestion: Option<String>,
        cols: usize,
        part: Part,
        hint: Option<&str>,
        look: &Look,
    ) -> Vec<Span<'static>> {
        let theme = &look.theme;
        let focused = self.focus == part;
        let lit = self.hover == Some(part);
        let mut typed = vec![
            Span::styled(
                "› ",
                if focused {
                    Tone::Accent.bold(theme)
                } else {
                    Tone::Dim.bold(theme)
                },
            ),
            Span::styled(text.to_string(), Tone::Text.style(theme)),
        ];
        let placeholder = match part {
            Part::Target => "the default branch",
            Part::Limit => "none",
            _ => "",
        };
        if focused && part != Part::Note {
            typed.push(Span::styled("▏", Tone::Accent.style(theme)));
        }
        let mut right = Vec::new();
        let suggestion = suggestion.filter(|_| focused);
        if text.is_empty() && suggestion.is_none() {
            typed.push(Span::styled(placeholder, Tone::Faint.style(theme)));
        }
        if let Some(rest) = suggestion {
            typed.push(Span::styled(rest, Tone::Faint.style(theme)));
            right = kit::hints(&[("→", "takes")], theme);
        } else if let Some(h) = hint {
            right = vec![Span::styled(h.to_string(), Tone::Dim.style(theme))];
        }
        let inner = cols.saturating_sub(2);
        let room = inner.saturating_sub(kit::spans_width(&right) + 1);
        let mut line = kit::fit(typed, room, Style::new());
        if inner > kit::spans_width(&line) + kit::spans_width(&right) {
            line.extend(right);
        }
        let bg = if focused || lit {
            Surface::Chosen.color(theme)
        } else {
            Surface::Panel.color(theme)
        };
        let mut out = vec![Span::styled(" ", Style::new().bg(bg))];
        out.extend(
            kit::fit(line, inner, Style::new())
                .into_iter()
                .map(|s| Span::styled(s.content, s.style.bg(bg))),
        );
        out.push(Span::styled(" ", Style::new().bg(bg)));
        out
    }
}

impl SettingsBox {
    /// The part a click on a cell of the box presses: while the question
    /// before leaving is up, only Leave and Stay.
    fn part_at(&self, hit: Hit) -> Option<Part> {
        let part = match hit {
            Hit::Close => Some(Part::Close),
            Hit::Line(line, x) => self
                .parts
                .iter()
                .find(|(l, x0, x1, _)| *l == line && x >= *x0 && x <= *x1)
                .map(|p| p.3),
            _ => None,
        };
        part.filter(|p| self.leaving.is_none() || matches!(p, Part::Leave | Part::Stay))
    }
}

impl Overlay for SettingsBox {
    /// A hand over a part a click presses (only Leave and Stay while the
    /// question before leaving is up), the arrow elsewhere.
    fn pointer(&self, col: u16, row: u16, look: &Look) -> Option<Pointer> {
        let hit = self.drawn.as_ref().map(|d| d.hit(col, row));
        let pressed = hit.and_then(|h| self.part_at(h)).is_some();
        Some(if pressed && look.mouse {
            Pointer::Hand
        } else {
            Pointer::Arrow
        })
    }

    fn kind(&self) -> &'static str {
        "autopilot"
    }

    fn stands_out(&self) -> Option<Rect> {
        self.drawn.as_ref().map(|d| d.outer)
    }

    fn draw(&mut self, buf: &mut Buffer, look: &Look) {
        self.screen_wide = buf.area.width >= WIDE;
        let inner = (buf.area.width as usize).saturating_sub(6).min(WIDTH);
        let (mut lines, mut parts, mut pictures) = self.lines(inner, self.screen_wide, None, look);
        // Too tall for the screen, so the keycaps and the question before
        // leaving stay in sight (draw_box keeps a row clear above and
        // below, and two for the keys): first drawn as on a narrow screen,
        // without the extras; then the items in a window; then scrolled.
        let fits = (buf.area.height as usize).saturating_sub(6);
        if lines.len() > fits && self.screen_wide {
            self.screen_wide = false;
            (lines, parts, pictures) = self.lines(inner, false, None, look);
        }
        let items = self.fields.skipped.len() + self.ctx.needs_choice.len();
        if lines.len() > fits && items > 0 && !self.folded(self.screen_wide) {
            let room = items.saturating_sub(lines.len() - fits + 1).max(1);
            (lines, parts, pictures) = self.lines(inner, self.screen_wide, Some(room), look);
        }
        if lines.len() > fits && fits > 2 {
            // The rows above the keys scrolled to keep the focus in sight,
            // the rule and the keys (or the question) always shown.
            let body = lines.len() - 2;
            let room = fits - 2;
            let focus_at = parts
                .iter()
                .find(|p| p.3 == self.focus)
                .map_or(0, |p| p.0)
                .min(body.saturating_sub(1));
            let top = (focus_at + 1).saturating_sub(room).min(body - room);
            let moved = |l: usize| {
                if l >= body {
                    Some(l - body + room)
                } else if (top..top + room).contains(&l) {
                    Some(l - top)
                } else {
                    None
                }
            };
            lines.drain(top + room..body);
            lines.drain(..top);
            parts = parts
                .into_iter()
                .filter_map(|(l, a, b, p)| moved(l).map(|l| (l, a, b, p)))
                .collect();
            pictures = pictures
                .into_iter()
                .filter_map(|(l, a, b, p)| moved(l).map(|l| (l, a, b, p)))
                .collect();
        }
        self.parts = parts;
        self.pictures = pictures;
        let title = self.title();
        let mut spec = BoxSpec::new(&title, &lines);
        spec.keys =
            Some("Tab ⇧Tab field  − + or ← → a count  Space a switch  M-↵ save  esc cancel");
        spec.width = inner as u16;
        spec.close_lit = self.focus == Part::Close || self.hover == Some(Part::Close);
        self.drawn = Some(draw::draw_box(buf, &spec, look));
    }

    fn draw_frame(&mut self, frame: &mut Frame, look: &Look) {
        self.draw(&mut frame.buf, look);
        let Some(drawn) = &self.drawn else {
            return;
        };
        for (i, (line, x, w, picture)) in self.pictures.iter().enumerate() {
            let Some((y, _)) = drawn.rows.iter().find(|(_, l)| l == line) else {
                continue;
            };
            let area = Rect::new(drawn.inner.x + x, *y, *w, 1).intersection(drawn.inner);
            let name = format!("autopilot/box/{i}");
            match picture {
                Picture::Switch(on) => images::switch(frame, &name, area, *on),
                Picture::Meter(frac, color) => {
                    images::meter(frame, &name, area, *frac, *color, None)
                }
            }
        }
    }

    fn key(&mut self, key: Key, look: &Look) -> Step {
        let k = key.normalized();
        if self.leaving.is_some() {
            // Tab moves between Yes and No; Enter takes the one chosen.
            return match k.code {
                KeyCode::Tab | KeyCode::BackTab | KeyCode::Left | KeyCode::Right => {
                    self.focus = if self.focus == Part::Leave {
                        Part::Stay
                    } else {
                        Part::Leave
                    };
                    Step::Redraw
                }
                KeyCode::Enter if self.focus == Part::Stay => self.press(Part::Stay),
                KeyCode::Enter | KeyCode::Char('y') | KeyCode::Char('Y') => self.press(Part::Leave),
                _ => self.press(Part::Stay),
            };
        }
        let in_text = self.text_field().is_some();
        match k.code {
            KeyCode::Esc => return self.cancel(),
            KeyCode::Char('c') if k.mods == Mods::CTRL => return Step::Close(Outcome::Dismissed),
            KeyCode::Enter if k.mods.alt => return self.close(SAVE),
            KeyCode::Char('s') if k.mods == Mods::ALT => return self.close(SAVE),
            KeyCode::Tab => {
                self.step_focus(1, look);
                return Step::Redraw;
            }
            KeyCode::BackTab => {
                self.step_focus(-1, look);
                return Step::Redraw;
            }
            KeyCode::Down => {
                self.step_focus(1, look);
                return Step::Redraw;
            }
            KeyCode::Up => {
                self.step_focus(-1, look);
                return Step::Redraw;
            }
            _ => {}
        }
        match (self.focus, k.code) {
            (Part::Slots | Part::Cap, KeyCode::Left | KeyCode::Char('-') | KeyCode::Char('−')) => {
                let f = if self.focus == Part::Slots {
                    Field::Slots
                } else {
                    Field::Cap
                };
                return self.step(f, -1);
            }
            (Part::Slots | Part::Cap, KeyCode::Right | KeyCode::Char('+') | KeyCode::Char('=')) => {
                let f = if self.focus == Part::Slots {
                    Field::Slots
                } else {
                    Field::Cap
                };
                return self.step(f, 1);
            }
            (Part::Repo | Part::Target, KeyCode::Right) => return self.take_suggestion(),
            (Part::Planned | Part::Land | Part::HandsFree, KeyCode::Char(' ') | KeyCode::Enter) => {
                return self.flip(self.focus);
            }
            (Part::Skipped(i), KeyCode::Char('u')) => return self.unskip(i),
            (Part::Note, KeyCode::Enter | KeyCode::Char(' ')) => return self.close(NOTE),
            (Part::Folded, KeyCode::Enter | KeyCode::Char(' ')) => return self.press(Part::Folded),
            (
                Part::Save | Part::Pause | Part::Stop | Part::Menu | Part::Cancel | Part::Close,
                KeyCode::Enter | KeyCode::Char(' '),
            ) => return self.press(self.focus),
            // Enter in a field goes on to the next.
            (_, KeyCode::Enter) => {
                self.step_focus(1, look);
                return Step::Redraw;
            }
            (_, KeyCode::Char('p')) if !in_text && self.ctx.on => return self.press(Part::Pause),
            (_, KeyCode::Char('s')) if !in_text && self.ctx.on => return self.press(Part::Stop),
            (_, KeyCode::Char('m')) if !in_text && self.ctx.on => return self.press(Part::Menu),
            _ => {}
        }
        let Some(text) = self.text_field() else {
            return Step::Same;
        };
        match k.code {
            KeyCode::Backspace => {
                text.pop();
                Step::Redraw
            }
            KeyCode::Char('u') if k.mods == Mods::CTRL => {
                text.clear();
                Step::Redraw
            }
            KeyCode::Char(c) if !k.mods.ctrl && !k.mods.alt => {
                text.push(c);
                Step::Redraw
            }
            _ => Step::Same,
        }
    }

    fn mouse(&mut self, mouse: Mouse, look: &Look) -> Step {
        let Some(drawn) = &self.drawn else {
            return Step::Same;
        };
        let hit = drawn.hit(mouse.col, mouse.row);
        let part = self.part_at(hit);
        match draw::gesture(&mouse) {
            Gesture::Click if look.mouse && self.leaving.is_some() => match part {
                Some(p) => self.press(p),
                None => Step::Same,
            },
            Gesture::Click if look.mouse => match part {
                Some(p) => self.press(p),
                None if hit == Hit::Outside => self.cancel(),
                None => Step::Same,
            },
            Gesture::Move if look.hover => {
                if part == self.hover {
                    Step::Same
                } else {
                    self.hover = part;
                    Step::Redraw
                }
            }
            _ => Step::Same,
        }
    }

    fn paste(&mut self, text: &str, _look: &Look) -> Step {
        match self.text_field() {
            Some(field) => {
                field.push_str(&text.replace(['\n', '\r'], " "));
                Step::Redraw
            }
            None => Step::Same,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::mode::Skipped;
    use super::*;

    fn key(code: KeyCode) -> Key {
        Key::new(code, Mods::NONE)
    }

    fn mode() -> Mode {
        Mode {
            on: true,
            note: "Work.md".into(),
            note_id: "id1".into(),
            repo: "/src/app".into(),
            slots: 2,
            cap: 4,
            spend_limit: 10.0,
            land_without_asking: true,
            skipped: vec![
                Skipped {
                    note: "id1".into(),
                    text: "Tidy the logs".into(),
                },
                Skipped {
                    note: "id1".into(),
                    text: "Rename the flag".into(),
                },
                Skipped {
                    note: "other".into(),
                    text: "Elsewhere".into(),
                },
            ],
            ..Mode::default()
        }
    }

    fn ctx() -> Context {
        Context {
            on: true,
            busy: 2,
            open: 3,
            spent: 6.2,
            landing_repo: "/src/app".into(),
            landing: "straight on it through queue-and-clean, as repos.json says".into(),
            repos: vec!["/src/app".into(), "/src/api".into()],
            branches: vec!["main".into(), "team/next".into()],
            needs_choice: vec!["Pick a color".into()],
            offer_hands_free: true,
            ..Context::default()
        }
    }

    fn opened(focus: Part) -> SettingsBox {
        let f = Fields::from_mode(&mode());
        SettingsBox::new(f.clone(), f, ctx(), focus)
    }

    fn text_of(b: &SettingsBox, inner: usize, wide: bool) -> String {
        let (lines, _, _) = b.lines(inner, wide, None, &Look::default());
        lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn fields_round_trip_a_mode_keeping_the_cap_at_least_the_slots_and_the_rest() {
        let was = mode();
        let mut fields = Fields::from_mode(&was);
        assert_eq!(fields.skipped, ["Tidy the logs", "Rename the flag"]);
        assert_eq!(fields.limit, "10");
        let mut m = was.clone();
        fields.apply(&mut m);
        assert_eq!(m, was, "applying the fields as read changes nothing");

        fields.slots = 6;
        fields.limit = "$12.5".into();
        fields.hands_free = true;
        fields.land_without_asking = false;
        fields.skipped.retain(|s| s != "Tidy the logs");
        fields.taken_back.push("Tidy the logs".into());
        fields.apply(&mut m);
        assert_eq!((m.slots, m.cap), (6, 6));
        assert_eq!(m.spend_limit, 12.5);
        assert!(m.hands_free && m.land_without_asking);
        assert_eq!(
            m.skipped
                .iter()
                .map(|s| s.text.as_str())
                .collect::<Vec<_>>(),
            ["Rename the flag", "Elsewhere"],
            "another note's skipped items stay"
        );
        assert_eq!(
            (m.note, m.repo, m.on),
            (was.note.clone(), was.repo.clone(), was.on)
        );

        let said = fields.changes(&Fields::from_mode(&was)).join(", ");
        assert_eq!(
            said,
            "6 tabs at once, 6 open at most, spend limit $12.50 a day, hands-free on, back in line: Tidy the logs"
        );
        fields.limit = "ten".into();
        assert_eq!(fields.limit(), None);
    }

    #[test]
    fn taking_back_an_item_of_another_note_picked_leaves_the_first_note_s() {
        let mut m = mode();
        let mut fields = Fields::from_mode(&m);
        // Another note picked, its skipped items in place of the first's.
        fields.note = "Other.md".into();
        fields.skipped_of = "other".into();
        fields.skipped = vec!["Elsewhere".into()];
        fields.taken_back = vec!["Elsewhere".into()];
        fields.skipped.clear();
        fields.apply(&mut m);
        assert_eq!(
            m.skipped
                .iter()
                .map(|s| s.text.as_str())
                .collect::<Vec<_>>(),
            ["Tidy the logs", "Rename the flag"]
        );
        assert!(
            fields
                .changes(&Fields::from_mode(&mode()))
                .contains(&"back in line: Elsewhere".to_string())
        );
    }

    #[test]
    fn an_item_skipped_while_the_box_is_open_stays_skipped() {
        let mut m = mode();
        let mut b = opened(Part::Skipped(0));
        b.key(key(KeyCode::Char('u')), &Look::default());
        // wtn refused another item meanwhile.
        m.skipped.push(Skipped {
            note: "id1".into(),
            text: "Refused one".into(),
        });
        b.fields().apply(&mut m);
        assert_eq!(
            m.skipped
                .iter()
                .map(|s| s.text.as_str())
                .collect::<Vec<_>>(),
            ["Rename the flag", "Elsewhere", "Refused one"]
        );
    }

    #[test]
    fn a_limit_with_more_than_cents_is_kept_as_it_was() {
        let mut m = mode();
        m.spend_limit = 0.004;
        let mut fields = Fields::from_mode(&m);
        fields.slots = 3;
        fields.apply(&mut m);
        assert_eq!(m.spend_limit, 0.004);
    }

    #[test]
    fn a_new_limit_takes_back_what_it_said_of_the_old_one() {
        let mut m = mode();
        m.said = format!("{} $10", super::super::pilot::SPENT);
        let mut fields = Fields::from_mode(&m);
        fields.apply(&mut m);
        assert!(!m.said.is_empty());
        fields.limit = "20".into();
        fields.apply(&mut m);
        assert!(m.said.is_empty());
    }

    #[test]
    fn the_box_shows_every_setting_with_the_spend_and_the_skipped_items() {
        let b = opened(Part::Note);
        assert_eq!(b.title(), "Autopilot · Work");
        let text = text_of(&b, WIDTH, true);
        for want in [
            "working · 2 tabs working, 3 open · $6.20 of $10.00 today",
            "Note",
            "› Work",
            "↵ another note",
            "› /src/app",
            "Lands on",
            "the default branch: straight on it through queue-and-clean",
            " −  2  + ",
            " −  4  + ",
            "how many of its tabs work at once",
            "› 10",
            "$6.20 today",
            "Planned only",
            "Land without asking",
            "Hands-free",
            "SKIPPED",
            "Tidy the logs",
            "Pick a color",
            "needs a choice",
            "M-↵  Save",
            "p  Pause",
            "s  Stop",
            "esc  Cancel",
        ] {
            assert!(text.contains(want), "{want}:\n{text}");
        }
        let tones = Tones::now();
        assert!(text.contains(images::switch_text(true, &tones).0), "{text}");
        assert!(
            text.contains(images::switch_text(false, &tones).0),
            "{text}"
        );

        let mut off = opened(Part::Note);
        off.ctx.on = false;
        off.ctx.offer_hands_free = false;
        off.fields.hands_free = false;
        assert_eq!(off.title(), "Work a note");
        let text = text_of(&off, WIDTH, true);
        assert!(text.contains("M-↵  Start"), "{text}");
        assert!(text.contains("○ off"), "{text}");
        assert!(!text.contains("p  Pause"), "{text}");
        assert!(!text.contains("Hands-free"), "{text}");
    }

    #[test]
    fn a_narrow_screen_drops_the_extras_and_folds_the_skipped_items() {
        let b = opened(Part::Note);
        let wide = text_of(&b, WIDTH, true);
        let narrow = text_of(&b, WIDTH, false);
        assert!(!narrow.contains("how many of its tabs"), "{narrow}");
        assert!(!narrow.contains("the dim rest is a suggestion"), "{narrow}");
        assert!(wide.contains("the dim rest is a suggestion"), "{wide}");
        assert!(narrow.contains("3 · ↵ shows them"), "{narrow}");
        assert!(!narrow.contains("Tidy the logs"), "{narrow}");
        // The dollars spent stay at the box's right edge, whatever its width.
        for inner in [60, WIDTH] {
            let (lines, _, _) = b.lines(inner, true, None, &Look::default());
            let row = lines
                .iter()
                .map(|l| {
                    l.spans
                        .iter()
                        .map(|s| s.content.as_ref())
                        .collect::<String>()
                })
                .find(|l| l.contains("$6.20 today"))
                .unwrap();
            assert_eq!(draw::width(&row), inner, "{row}");
            assert!(row.ends_with("$6.20 today"), "{row}");
        }
    }

    #[test]
    fn a_long_skipped_list_shows_a_window_around_the_focus_above_the_keys() {
        let mut b = opened(Part::Skipped(0));
        b.fields.skipped = (0..40).map(|i| format!("Item {i}")).collect();
        b.was = b.fields.clone();
        let text = |b: &SettingsBox| {
            let (lines, _, _) = b.lines(WIDTH, true, Some(5), &Look::default());
            lines
                .iter()
                .map(|l| {
                    l.spans
                        .iter()
                        .map(|s| s.content.as_ref())
                        .collect::<String>()
                })
                .collect::<Vec<_>>()
                .join("\n")
        };
        b.ctx.needs_choice = (0..3).map(|i| format!("Choice {i}")).collect();
        let first = text(&b);
        assert!(
            first.contains("Item 4") && !first.contains("Item 5"),
            "{first}"
        );
        assert!(first.contains("… 38 more"), "{first}");
        assert!(first.contains("M-↵  Save"), "{first}");
        b.focus = Part::Skipped(20);
        let later = text(&b);
        assert!(
            later.contains("Item 20") && !later.contains("Item 15 "),
            "{later}"
        );
        // Tab reaches the items needing a choice at the end of the list.
        b.focus = Part::Skipped(39);
        b.key(key(KeyCode::Tab), &Look::default());
        b.key(key(KeyCode::Tab), &Look::default());
        assert_eq!(b.focus, Part::Skipped(41));
        assert!(text(&b).contains("Choice 1"));
        assert_eq!(b.key(key(KeyCode::Char('u')), &Look::default()), Step::Same);
        // Drawn on a screen 40 rows tall, the keycaps stay in it.
        let mut buf = Buffer::empty(Rect::new(0, 0, 170, 40));
        b.draw(&mut buf, &Look::default());
        let screen: String = (0..40)
            .map(|y| {
                (0..170)
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(screen.contains("Cancel"), "{screen}");
    }

    #[test]
    fn on_a_short_screen_the_keys_and_the_question_before_leaving_stay_in_sight() {
        let look = Look::default();
        let mut b = opened(Part::Limit);
        b.ctx.problem = "Autopilot lands only straight onto a branch for now, and this repository lands through a PR (repos.json)".into();
        let screen = |b: &mut SettingsBox| {
            let mut buf = Buffer::empty(Rect::new(0, 0, 80, 24));
            b.draw(&mut buf, &look);
            (0..24)
                .map(|y| {
                    (0..80)
                        .map(|x| buf[(x, y)].symbol().to_string())
                        .collect::<String>()
                })
                .collect::<Vec<_>>()
                .join("\n")
        };
        let shown = screen(&mut b);
        assert!(shown.contains("Cancel"), "{shown}");
        // The focus low in the box scrolls it into sight.
        b.focus = Part::HandsFree;
        let shown = screen(&mut b);
        assert!(
            shown.contains("Hands-free") && shown.contains("Cancel"),
            "{shown}"
        );
        assert!(b.parts.iter().any(|p| p.3 == Part::HandsFree));
        b.focus = Part::Limit;
        b.key(key(KeyCode::Char('5')), &look);
        b.key(key(KeyCode::Esc), &look);
        let shown = screen(&mut b);
        assert!(shown.contains("Leave without saving"), "{shown}");
        assert!(
            b.parts.iter().any(|p| p.3 == Part::Leave),
            "its Yes takes a click"
        );
    }

    #[test]
    fn another_repository_typed_suggests_none_of_the_first_s_branches() {
        let mut b = opened(Part::Target);
        assert_eq!(b.suggestion(Part::Target).as_deref(), Some("main"));
        b.fields.repo = "/src/api".into();
        assert_eq!(b.suggestion(Part::Target), None);
    }

    #[test]
    fn keys_step_flip_take_back_and_save_or_ask_before_leaving() {
        let look = Look::default();
        let mut b = opened(Part::Slots);
        b.key(key(KeyCode::Char('+')), &look);
        b.key(key(KeyCode::Right), &look);
        b.key(key(KeyCode::Right), &look);
        assert_eq!((b.fields.slots, b.fields.cap), (5, 5), "the cap follows");
        b.key(key(KeyCode::Tab), &look);
        b.key(key(KeyCode::Left), &look);
        assert_eq!((b.fields.slots, b.fields.cap), (4, 4), "the slots follow");
        b.focus = Part::Planned;
        b.key(key(KeyCode::Char(' ')), &look);
        assert!(b.fields.ready_only);
        b.focus = Part::Skipped(0);
        b.key(key(KeyCode::Char('u')), &look);
        assert_eq!(b.fields.skipped, ["Rename the flag"]);
        // Its keycap takes a click too.
        let (_, parts, _) = b.lines(WIDTH, true, None, &look);
        assert!(parts.iter().any(|p| p.3 == Part::TakeBack(0)));
        b.press(Part::TakeBack(0));
        assert!(b.fields.skipped.is_empty());
        assert_eq!(b.fields.taken_back, ["Tidy the logs", "Rename the flag"]);
        // Typing in the repository, its suggestion taken with →.
        b.focus = Part::Repo;
        b.key(key(KeyCode::Char('u')), &look);
        assert_eq!(b.fields.repo, "/src/appu", "u types in a field");
        for _ in 0..5 {
            b.key(key(KeyCode::Backspace), &look);
        }
        b.key(key(KeyCode::Right), &look);
        assert_eq!(b.fields.repo, "/src/app");
        // Esc with changes asks; n stays, Enter leaves.
        assert_eq!(b.key(key(KeyCode::Esc), &look), Step::Redraw);
        assert!(text_of(&b, WIDTH, true).contains("Leave without saving your changes? [Y/n]"));
        assert_eq!(b.key(key(KeyCode::Char('n')), &look), Step::Redraw);
        assert_eq!(b.focus, Part::Repo, "the focus back where it was");
        b.key(key(KeyCode::Esc), &look);
        b.key(key(KeyCode::Tab), &look);
        assert_eq!(b.focus, Part::Stay);
        assert_eq!(b.key(key(KeyCode::Enter), &look), Step::Redraw);
        b.key(key(KeyCode::Esc), &look);
        assert_eq!(
            b.key(key(KeyCode::Enter), &look),
            Step::Close(Outcome::Dismissed)
        );
        // M-↵ saves from any field, with the fields as typed.
        let mut b = opened(Part::Limit);
        b.key(key(KeyCode::Char('5')), &look);
        match b.key(Key::new(KeyCode::Enter, Mods::ALT), &look) {
            Step::Close(Outcome::Picked(p)) => {
                assert_eq!(p.index, SAVE);
                let f: Fields = serde_json::from_str(&p.value).unwrap();
                assert_eq!(f.limit, "105");
            }
            other => panic!("{other:?}"),
        }
        // More… with a change asks first, then opens the menu.
        let mut b = opened(Part::Slots);
        b.key(key(KeyCode::Char('+')), &look);
        assert_eq!(b.key(key(KeyCode::Char('m')), &look), Step::Redraw);
        assert!(matches!(
            b.key(key(KeyCode::Enter), &look),
            Step::Close(Outcome::Picked(p)) if p.index == MENU
        ));
        // p and s outside a field pause and stop.
        let mut b = opened(Part::Slots);
        assert!(matches!(
            b.key(key(KeyCode::Char('p')), &look),
            Step::Close(Outcome::Picked(p)) if p.index == PAUSE
        ));
        assert!(matches!(
            b.key(key(KeyCode::Char('s')), &look),
            Step::Close(Outcome::Picked(p)) if p.index == STOP
        ));
        // Esc with nothing changed closes.
        assert_eq!(
            b.key(key(KeyCode::Esc), &look),
            Step::Close(Outcome::Dismissed)
        );
    }
}
