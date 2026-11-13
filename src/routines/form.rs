//! The routine's box (UX 2.0 board 30), from `n` and `e` in the Routines
//! view and the palette's "New routine": its fields down a column of their
//! names. Name; Directory, with a directory of your tabs that starts with
//! what is typed shown dim after it, → taking it; When, read back as it is
//! typed ("✓ Monday to Friday at 8:45 AM", or what it couldn't read) with
//! examples and the next three runs as chips; Runs in, If it's busy and If
//! it's missed as options; Prompt, several lines; Model.
//!
//! Tab and Shift-Tab move between the fields and the keycaps, Enter moves
//! to the next field (and adds a line in Prompt), ← → or Space change an
//! option, M-Enter saves from any field, M-r saves and runs it now, and Esc
//! cancels, asking first (a second Esc) when anything was typed. Each field
//! and option takes a click, the box gets easy mode's ×.
//!
//! It closes with `Outcome::Picked`, its value the `Draft` as JSON and its
//! index 1 when it is to run now.

use std::path::Path;

use jiff::Zoned;
use ratatui::buffer::Buffer;
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;
use serde::{Deserialize, Serialize};

use super::routine::{Routine, TabPolicy, WhenBusy, WhenMissed};
use super::schedule::{EXAMPLES, Schedule};
use super::view::when_words;
use crate::mux::input::{Key, KeyCode, Mods, Mouse};
use crate::sidebar::text::{cut, width, wrap};
use crate::ui::draw::{self, BoxLine, BoxSpec, Drawn, Gesture, Hit};
use crate::ui::kit::{self, Col, Surface, Tone};
use crate::ui::{Look, Outcome, Overlay, Picked, Step};

/// The box's inside, while the screen leaves room around it.
const WIDTH: usize = 100;
/// The column of the fields' names.
const NAMES: usize = 15;
/// The most lines the prompt shows.
const PROMPT_LINES: usize = 6;

/// What the box holds, as it hands it back.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Draft {
    pub name: String,
    pub dir: String,
    pub when: String,
    pub tab_policy: TabPolicy,
    pub when_busy: WhenBusy,
    pub when_missed: WhenMissed,
    pub prompt: String,
    pub model: String,
}

impl Default for Draft {
    fn default() -> Draft {
        Draft {
            name: String::new(),
            dir: String::new(),
            when: String::new(),
            tab_policy: TabPolicy::SameTab,
            when_busy: WhenBusy::Wait,
            when_missed: WhenMissed::OnWaking,
            prompt: String::new(),
            model: String::new(),
        }
    }
}

impl Draft {
    /// A routine as the box shows it to change.
    pub fn of(r: &Routine) -> Draft {
        Draft {
            name: r.name.clone(),
            dir: crate::paths::tilde(Path::new(&r.dir)),
            when: r.when.clone(),
            tab_policy: r.tab_policy,
            when_busy: r.when_busy,
            when_missed: r.when_missed,
            prompt: r.prompt.clone(),
            model: r.model.clone(),
        }
    }
}

/// A field with options.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Choice {
    RunsIn,
    IfBusy,
    IfMissed,
}

impl Choice {
    fn label(self) -> &'static str {
        match self {
            Choice::RunsIn => "Runs in",
            Choice::IfBusy => "If it's busy",
            Choice::IfMissed => "If it's missed",
        }
    }

    fn options(self) -> [&'static str; 2] {
        match self {
            Choice::RunsIn => ["The same tab each run", "A new tab each run"],
            Choice::IfBusy => ["Wait for its turn to end, up to 30 minutes", "Skip the run"],
            Choice::IfMissed => ["Run when the computer wakes, up to an hour late", "Skip it"],
        }
    }
}

/// What Tab's focus or the mouse is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Part {
    Name,
    Dir,
    When,
    Pick(Choice),
    /// An option of a field, by its place.
    Option(Choice, usize),
    Prompt,
    Model,
    Save,
    SaveRun,
    Cancel,
    Close,
}

/// A part on a line: the line's index, its first and last column, the part.
type Placed = (usize, u16, u16, Part);

pub struct RoutineForm {
    title: String,
    draft: Draft,
    /// The draft as it opened, to tell whether anything was typed.
    opened: Draft,
    /// Directories to suggest.
    places: Vec<String>,
    /// The schedule kept while When is unchanged.
    kept: Option<Schedule>,
    /// The time When is read against, fixed for a test; `None` reads the
    /// clock.
    fixed: Option<Zoned>,
    focus: Part,
    hover: Option<Part>,
    /// What saving found wrong, under the field it is about.
    wrong: Option<(Part, String)>,
    /// Esc pressed once with something typed.
    leaving: bool,
    drawn: Option<Drawn>,
    parts: Vec<Placed>,
}

impl RoutineForm {
    /// `kept` is the schedule of the routine being changed, which stands
    /// while When is left as it was: read again, "tomorrow 9am" would move
    /// a day on, and a time to come once that has gone by couldn't be saved.
    pub fn new(
        change: bool,
        draft: Draft,
        places: Vec<String>,
        kept: Option<Schedule>,
    ) -> RoutineForm {
        let title = if change {
            format!("Change the routine {}", draft.name)
        } else {
            "New routine".to_string()
        };
        RoutineForm {
            title,
            opened: draft.clone(),
            draft,
            places,
            kept,
            fixed: None,
            focus: Part::Name,
            hover: None,
            wrong: None,
            leaving: false,
            drawn: None,
            parts: Vec::new(),
        }
    }

    #[cfg(test)]
    fn at_time(mut self, now: Zoned) -> RoutineForm {
        self.fixed = Some(now);
        self
    }

    fn now(&self) -> Zoned {
        self.fixed.clone().unwrap_or_else(crate::util::local_now)
    }

    fn stops(look: &Look) -> Vec<Part> {
        let mut out = vec![
            Part::Name,
            Part::Dir,
            Part::When,
            Part::Pick(Choice::RunsIn),
            Part::Pick(Choice::IfBusy),
            Part::Pick(Choice::IfMissed),
            Part::Prompt,
            Part::Model,
            Part::Save,
            Part::SaveRun,
            Part::Cancel,
        ];
        if look.close {
            out.push(Part::Close);
        }
        out
    }

    fn step_focus(&mut self, by: isize, look: &Look) {
        let stops = Self::stops(look);
        let at = stops.iter().position(|p| *p == self.focus).unwrap_or(0) as isize;
        self.focus = stops[(at + by).rem_euclid(stops.len() as isize) as usize];
    }

    fn field(&mut self) -> Option<&mut String> {
        match self.focus {
            Part::Name => Some(&mut self.draft.name),
            Part::Dir => Some(&mut self.draft.dir),
            Part::When => Some(&mut self.draft.when),
            Part::Prompt => Some(&mut self.draft.prompt),
            Part::Model => Some(&mut self.draft.model),
            _ => None,
        }
    }

    fn chosen(&self, c: Choice) -> usize {
        match c {
            Choice::RunsIn => (self.draft.tab_policy == TabPolicy::NewTab) as usize,
            Choice::IfBusy => (self.draft.when_busy == WhenBusy::Skip) as usize,
            Choice::IfMissed => (self.draft.when_missed == WhenMissed::Skip) as usize,
        }
    }

    fn choose(&mut self, c: Choice, i: usize) {
        match c {
            Choice::RunsIn => {
                self.draft.tab_policy = [TabPolicy::SameTab, TabPolicy::NewTab][i.min(1)]
            }
            Choice::IfBusy => self.draft.when_busy = [WhenBusy::Wait, WhenBusy::Skip][i.min(1)],
            Choice::IfMissed => {
                self.draft.when_missed = [WhenMissed::OnWaking, WhenMissed::Skip][i.min(1)]
            }
        }
    }

    /// The directory suggested after what is typed: the first of the
    /// places that starts with it and is longer.
    fn suggestion(&self) -> Option<&str> {
        let typed = self.draft.dir.trim();
        if typed.is_empty() {
            return self.places.first().map(String::as_str);
        }
        self.places.iter().map(String::as_str).find(|p| {
            let shown = crate::paths::tilde(Path::new(p));
            (p.starts_with(typed) || shown.starts_with(typed)) && *p != typed && shown != typed
        })
    }

    fn take_suggestion(&mut self) -> bool {
        let Some(s) = self.suggestion().map(|s| crate::paths::tilde(Path::new(s))) else {
            return false;
        };
        self.draft.dir = s;
        true
    }

    /// When it runs: the schedule kept while When is as it was, else what
    /// is typed, read.
    fn schedule(&self) -> Result<Schedule, String> {
        match &self.kept {
            Some(k) if self.draft.when.trim() == self.opened.when.trim() => Ok(k.clone()),
            _ => Schedule::parse(&self.draft.when, &self.now()),
        }
    }

    /// What is wrong with the draft, by the field it is about, or none.
    fn check(&self) -> Option<(Part, String)> {
        if self.draft.name.trim().is_empty() {
            return Some((Part::Name, "Give it a name.".into()));
        }
        let typed = self.draft.dir.trim();
        let dir = match typed.strip_prefix('~') {
            Some(rest) => crate::paths::home().join(rest.trim_start_matches('/')),
            None => typed.into(),
        };
        if typed.is_empty() || !dir.is_dir() {
            return Some((Part::Dir, "Choose a directory that is there.".into()));
        }
        // Read against nothing that stays: the server's own directory
        // changes with where it was started from.
        if !dir.is_absolute() {
            return Some((Part::Dir, "Give its whole path, or one from ~.".into()));
        }
        if let Err(e) = self.schedule() {
            return Some((Part::When, e));
        }
        if self.draft.prompt.trim().is_empty() {
            return Some((Part::Prompt, "Write the prompt it runs.".into()));
        }
        None
    }

    fn save(&mut self, run_now: bool) -> Step {
        if let Some((part, why)) = self.check() {
            self.focus = part;
            self.wrong = Some((part, why));
            return Step::Redraw;
        }
        Step::Close(Outcome::Picked(Picked {
            index: usize::from(run_now),
            value: serde_json::to_string(&self.draft).unwrap_or_default(),
            query: String::new(),
        }))
    }

    fn cancel(&mut self) -> Step {
        if self.draft != self.opened && !self.leaving {
            self.leaving = true;
            return Step::Redraw;
        }
        Step::Close(Outcome::Dismissed)
    }

    fn press(&mut self, part: Part) -> Step {
        match part {
            Part::Option(c, i) => {
                self.choose(c, i);
                self.focus = Part::Pick(c);
                Step::Redraw
            }
            Part::Save => self.save(false),
            Part::SaveRun => self.save(true),
            Part::Cancel | Part::Close => Step::Close(Outcome::Dismissed),
            part => {
                self.focus = part;
                Step::Redraw
            }
        }
    }

    /// The box's lines, and where each part on them is.
    fn lines(&self, inner: usize, look: &Look) -> (Vec<BoxLine>, Vec<Placed>) {
        let theme = &look.theme;
        let mut lines: Vec<BoxLine> = Vec::new();
        let mut parts: Vec<Placed> = Vec::new();
        let cols = kit::columns(
            &[
                Col::Fixed {
                    min: NAMES,
                    natural: NAMES,
                    cap: NAMES,
                },
                Col::Stretch {
                    weight: 1,
                    min: 20,
                    max: None,
                },
            ],
            inner.saturating_sub(1),
            1,
            &[],
        );
        let (fx, fw) = cols[1].unwrap_or((NAMES + 1, inner.saturating_sub(NAMES + 2)));
        let fx = fx + 1;
        let name_of = |text: &str, on: bool| -> Vec<Span<'static>> {
            let tone = if on { Tone::Accent } else { Tone::Dim };
            vec![
                Span::raw(" "),
                Span::styled(cut(text, NAMES), tone.bold(theme)),
            ]
        };
        // A row: the field's name, then `spans` at the fields' column.
        let at_field = |name: &str, on: bool, spans: Vec<Span<'static>>| -> Vec<Span<'static>> {
            let mut line = name_of(name, on);
            kit::place(&mut line, spans, (fx, fw));
            line
        };
        let under = |spans: Vec<Span<'static>>| -> Vec<Span<'static>> {
            let mut line = Vec::new();
            kit::place(&mut line, spans, (fx, fw));
            line
        };
        let wrong_under = |part: Part, lines: &mut Vec<BoxLine>| {
            if let Some((p, why)) = &self.wrong
                && *p == part
            {
                lines.push(BoxLine::spans(under(vec![Span::styled(
                    why.clone(),
                    Tone::Failed.style(theme),
                )])));
            }
        };
        let text_field = |text: &str, placeholder: &str, part: Part, right: Vec<Span<'static>>| {
            let focused = self.focus == part;
            input(text, placeholder, fw, focused, right, look)
        };

        // Name.
        parts.push((lines.len(), 0, inner as u16, Part::Name));
        lines.push(BoxLine::spans(at_field(
            "Name",
            self.focus == Part::Name,
            text_field(&self.draft.name, "Morning triage", Part::Name, Vec::new()),
        )));
        wrong_under(Part::Name, &mut lines);

        // Directory, with its suggestion.
        let suggestion = self
            .suggestion()
            .map(|s| crate::paths::tilde(Path::new(s)))
            .filter(|_| self.focus == Part::Dir);
        let right = match &suggestion {
            Some(_) => kit::hints(&[("→", "takes it")], theme),
            None => Vec::new(),
        };
        parts.push((lines.len(), 0, inner as u16, Part::Dir));
        let mut dir_line = text_field(&self.draft.dir, "~/code/api", Part::Dir, right);
        if let Some(s) = &suggestion {
            // The rest of the suggestion, dim after the cursor.
            let typed = self.draft.dir.trim();
            let rest = s.strip_prefix(typed).unwrap_or(s);
            if let Some(at) = dir_line.iter().position(|sp| sp.content == "▏") {
                dir_line.insert(
                    at + 1,
                    Span::styled(rest.to_string(), Tone::Faint.style(theme)),
                );
                // Taken back off the padding, so the line keeps its width.
                let extra = width(rest);
                if let Some(pad) = dir_line
                    .iter_mut()
                    .rev()
                    .find(|sp| sp.content.chars().all(|c| c == ' ') && width(&sp.content) > extra)
                {
                    let n = width(&pad.content) - extra;
                    pad.content = " ".repeat(n).into();
                }
            }
        }
        lines.push(BoxLine::spans(at_field(
            "Directory",
            self.focus == Part::Dir,
            dir_line,
        )));
        wrong_under(Part::Dir, &mut lines);

        // When, read back as it is typed.
        parts.push((lines.len(), 0, inner as u16, Part::When));
        lines.push(BoxLine::spans(at_field(
            "When",
            self.focus == Part::When,
            text_field(&self.draft.when, "weekdays at 8:45", Part::When, Vec::new()),
        )));
        let now = self.now();
        if !self.draft.when.trim().is_empty() {
            match self.schedule() {
                Ok(s) => {
                    lines.push(BoxLine::spans(under(vec![
                        Span::styled("✓ ", Tone::Done.bold(theme)),
                        Span::styled(s.describe(&now), Tone::Text.style(theme)),
                    ])));
                    let mut chips = vec![Span::styled("next ", Tone::Dim.style(theme))];
                    for at in s.next(now.timestamp().as_second(), 3, now.time_zone()) {
                        chips.extend(kit::chip(
                            &when_words(at, &now),
                            Tone::Working,
                            false,
                            theme,
                        ));
                    }
                    lines.push(BoxLine::spans(under(chips)));
                }
                Err(e) if self.wrong.as_ref().is_none_or(|(p, _)| *p != Part::When) => {
                    lines.push(BoxLine::spans(under(vec![Span::styled(
                        e,
                        Tone::Needs.style(theme),
                    )])));
                }
                Err(_) => {}
            }
        }
        wrong_under(Part::When, &mut lines);
        // The examples go first on a narrow box.
        if fw >= 60 {
            lines.push(BoxLine::spans(under(vec![Span::styled(
                EXAMPLES,
                Tone::Faint.style(theme),
            )])));
        }
        lines.push(BoxLine::blank());

        // The options.
        for c in [Choice::RunsIn, Choice::IfBusy, Choice::IfMissed] {
            let on = self.focus == Part::Pick(c);
            let chosen = self.chosen(c);
            let mut spans = Vec::new();
            let mut hits = Vec::new();
            for (i, words) in c.options().iter().enumerate() {
                let x = fx + kit::spans_width(&spans);
                let lit = self.hover == Some(Part::Option(c, i));
                let (dot, style) = if i == chosen {
                    (
                        "● ",
                        if on {
                            Style::new().fg(theme.bright).add_modifier(Modifier::BOLD)
                        } else {
                            Tone::Text.bold(theme)
                        },
                    )
                } else if lit {
                    ("○ ", Tone::Text.style(theme))
                } else {
                    ("○ ", Tone::Dim.style(theme))
                };
                let dot_tone = if i == chosen { Tone::Accent } else { Tone::Dim };
                let piece = vec![
                    Span::styled(dot, dot_tone.bold(theme)),
                    Span::styled(words.to_string(), style),
                    Span::raw("   "),
                ];
                let w = kit::spans_width(&piece);
                hits.push((x, x + w.saturating_sub(4), Part::Option(c, i)));
                spans.extend(piece);
            }
            // Two that don't fit side by side go one under the other.
            if kit::spans_width(&spans) > fw {
                for (i, words) in c.options().iter().enumerate() {
                    let dot = if i == chosen { "● " } else { "○ " };
                    let lit = self.hover == Some(Part::Option(c, i));
                    let tone = if i == chosen || lit {
                        Tone::Text
                    } else {
                        Tone::Dim
                    };
                    let row = vec![
                        Span::styled(dot, Tone::Accent.bold(theme)),
                        Span::styled(words.to_string(), tone.style(theme)),
                    ];
                    let line = if i == 0 {
                        at_field(c.label(), on, row)
                    } else {
                        under(row)
                    };
                    parts.push((lines.len(), fx as u16, inner as u16, Part::Option(c, i)));
                    lines.push(BoxLine::spans(line).clickable());
                }
                continue;
            }
            for (x0, x1, part) in hits {
                parts.push((lines.len(), x0 as u16, x1 as u16, part));
            }
            lines.push(BoxLine::spans(at_field(c.label(), on, spans)).clickable());
        }
        lines.push(BoxLine::blank());

        // The prompt, several lines on the panel.
        let panel = Surface::Panel.color(theme);
        let focused = self.focus == Part::Prompt;
        let text_w = fw.saturating_sub(4);
        let mut shown: Vec<String> = Vec::new();
        for l in self.draft.prompt.split('\n') {
            if l.is_empty() {
                shown.push(String::new());
            } else {
                shown.extend(wrap(l, text_w.max(10)));
            }
        }
        if focused && let Some(last) = shown.last_mut() {
            last.push('▏');
        }
        if self.draft.prompt.is_empty() {
            shown = vec![if focused {
                "▏Read the new issues and sort them by area.".into()
            } else {
                "Read the new issues and sort them by area.".into()
            }];
        }
        let skip = shown.len().saturating_sub(PROMPT_LINES);
        let height = shown.len().clamp(3, PROMPT_LINES);
        for row in 0..height {
            let text = shown.get(skip + row).cloned().unwrap_or_default();
            let style = if self.draft.prompt.is_empty() {
                Tone::Faint.style(theme)
            } else {
                Tone::Text.style(theme)
            };
            let body = kit::fit(
                vec![
                    Span::styled(" ", Style::new().bg(panel)),
                    Span::styled(text, style.bg(panel)),
                ],
                fw,
                Style::new().bg(panel),
            );
            let line = if row == 0 {
                at_field("Prompt", focused, body)
            } else if row == 1 {
                let mut l = vec![
                    Span::raw(" "),
                    Span::styled(cut("system prompt", NAMES), Tone::Faint.style(theme)),
                ];
                kit::place(&mut l, body, (fx, fw));
                l
            } else {
                under(body)
            };
            parts.push((lines.len(), 0, inner as u16, Part::Prompt));
            lines.push(BoxLine::spans(line));
        }
        wrong_under(Part::Prompt, &mut lines);

        // Model.
        parts.push((lines.len(), 0, inner as u16, Part::Model));
        lines.push(BoxLine::spans(at_field(
            "Model",
            self.focus == Part::Model,
            text_field(
                &self.draft.model,
                "Claude Code's default (opus, sonnet, haiku)",
                Part::Model,
                Vec::new(),
            ),
        )));

        // The keys.
        lines.push(BoxLine::spans(vec![kit::rule(inner, theme)]));
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
        cap("M-↵", "Save", Part::Save, &mut caps);
        cap("M-r", "Save and run now", Part::SaveRun, &mut caps);
        cap("esc", "Cancel", Part::Cancel, &mut caps);
        lines.push(BoxLine::spans(caps).clickable());
        if self.leaving {
            lines.push(BoxLine::text(
                " Esc again discards what you typed; anything else goes back to it.",
                Tone::Needs.style(theme),
            ));
        }
        (lines, parts)
    }
}

/// A field: the kit's input, its cursor only while it has the focus.
fn input(
    text: &str,
    placeholder: &str,
    w: usize,
    focused: bool,
    right: Vec<Span<'static>>,
    look: &Look,
) -> Vec<Span<'static>> {
    kit::input_beside(text, placeholder, w, right, &look.theme, false)
        .into_iter()
        .map(|s| {
            if !focused && s.content == "▏" {
                Span::styled(" ", s.style)
            } else if !focused && s.content.starts_with("› ") {
                Span::styled(
                    s.content,
                    Tone::Dim.style(&look.theme).add_modifier(Modifier::BOLD),
                )
            } else {
                s
            }
        })
        .collect()
}

impl Overlay for RoutineForm {
    fn drawn(&self) -> Option<&crate::ui::draw::Drawn> {
        self.drawn.as_ref()
    }

    fn kind(&self) -> &'static str {
        "routine"
    }

    fn stands_out(&self) -> Option<ratatui::layout::Rect> {
        self.drawn.as_ref().map(|d| d.outer)
    }

    fn draw(&mut self, buf: &mut Buffer, look: &Look) {
        let inner = (buf.area.width as usize).saturating_sub(6).min(WIDTH);
        let (lines, parts) = self.lines(inner, look);
        self.parts = parts;
        let mut spec = BoxSpec::new(&self.title, &lines);
        spec.keys = Some(
            "tab next field  ←→ an option  ↵ next (a line in Prompt)  M-↵ save  M-r save and run  esc cancel",
        );
        spec.width = inner as u16;
        spec.close_lit = self.focus == Part::Close || self.hover == Some(Part::Close);
        self.drawn = Some(draw::draw_box(buf, &spec, look));
    }

    fn key(&mut self, key: Key, look: &Look) -> Step {
        let k = key.normalized();
        if k.code != KeyCode::Esc {
            self.leaving = false;
        }
        match k.code {
            KeyCode::Esc => return self.cancel(),
            KeyCode::Char('c') if k.mods == Mods::CTRL => return Step::Close(Outcome::Dismissed),
            KeyCode::Enter if k.mods.alt => return self.save(false),
            KeyCode::Char('r') if k.mods == Mods::ALT => return self.save(true),
            KeyCode::Tab => {
                self.step_focus(1, look);
                return Step::Redraw;
            }
            KeyCode::BackTab => {
                self.step_focus(-1, look);
                return Step::Redraw;
            }
            KeyCode::Enter => {
                return match self.focus {
                    Part::Prompt => {
                        self.draft.prompt.push('\n');
                        Step::Redraw
                    }
                    Part::Save | Part::SaveRun | Part::Cancel | Part::Close => {
                        self.press(self.focus)
                    }
                    _ => {
                        self.step_focus(1, look);
                        Step::Redraw
                    }
                };
            }
            _ => {}
        }
        if let Part::Pick(c) = self.focus {
            return match k.code {
                KeyCode::Left | KeyCode::Char('h') | KeyCode::Char('1') => {
                    self.choose(c, 0);
                    Step::Redraw
                }
                KeyCode::Right | KeyCode::Char('l') | KeyCode::Char('2') => {
                    self.choose(c, 1);
                    Step::Redraw
                }
                KeyCode::Char(' ') => {
                    self.choose(c, 1 - self.chosen(c));
                    Step::Redraw
                }
                _ => Step::Same,
            };
        }
        if matches!(
            self.focus,
            Part::Save | Part::SaveRun | Part::Cancel | Part::Close
        ) {
            return match k.code {
                KeyCode::Char(' ') => self.press(self.focus),
                _ => Step::Same,
            };
        }
        if self.focus == Part::Dir && k.code == KeyCode::Right && self.take_suggestion() {
            return Step::Redraw;
        }
        let Some(text) = self.field() else {
            return Step::Same;
        };
        let changed = match k.code {
            KeyCode::Backspace => {
                text.pop();
                true
            }
            KeyCode::Char('u') if k.mods == Mods::CTRL => {
                text.clear();
                true
            }
            KeyCode::Char(c) if !k.mods.ctrl && !k.mods.alt => {
                text.push(c);
                true
            }
            _ => false,
        };
        if !changed {
            return Step::Same;
        }
        if self.wrong.as_ref().is_some_and(|(p, _)| *p == self.focus) {
            self.wrong = None;
        }
        Step::Redraw
    }

    fn mouse(&mut self, mouse: Mouse, look: &Look) -> Step {
        let Some(drawn) = &self.drawn else {
            return Step::Same;
        };
        let hit = drawn.hit(mouse.col, mouse.row);
        let part = match hit {
            Hit::Close => Some(Part::Close),
            Hit::Line(line, x) => self
                .parts
                .iter()
                .filter(|(l, x0, x1, _)| *l == line && x >= *x0 && x <= *x1)
                // The narrowest: an option over its field's line.
                .min_by_key(|(_, x0, x1, _)| x1 - x0)
                .map(|p| p.3),
            _ => None,
        };
        match draw::gesture(&mouse) {
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
        let multi = self.focus == Part::Prompt;
        let Some(field) = self.field() else {
            return Step::Same;
        };
        if multi {
            field.push_str(&text.replace("\r\n", "\n").replace('\r', "\n"));
        } else {
            field.push_str(&text.replace(['\n', '\r'], " "));
        }
        Step::Redraw
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> Key {
        Key::new(code, Mods::NONE)
    }

    fn now() -> Zoned {
        "2026-11-04T14:30:00[America/New_York]".parse().unwrap()
    }

    fn text_of(f: &RoutineForm, inner: usize) -> String {
        let (lines, _) = f.lines(inner, &Look::default());
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

    fn type_in(f: &mut RoutineForm, text: &str) {
        for c in text.chars() {
            f.key(key(KeyCode::Char(c)), &Look::default());
        }
    }

    #[test]
    fn its_fields_read_the_schedule_back_and_m_enter_saves_it() {
        let dir = std::env::temp_dir().to_string_lossy().into_owned();
        let mut f =
            RoutineForm::new(false, Draft::default(), vec![dir.clone()], None).at_time(now());
        let look = Look::default();
        let text = text_of(&f, WIDTH);
        for field in [
            "Name",
            "Directory",
            "When",
            "Runs in",
            "If it's busy",
            "If it's missed",
            "Prompt",
            "Model",
        ] {
            assert!(text.contains(field), "{field}: {text}");
        }
        assert!(text.contains("● The same tab each run"), "{text}");
        assert!(text.contains("Save and run now"), "{text}");
        assert!(
            text.contains("weekdays at 8:45 · mondays 10:00"),
            "the examples: {text}"
        );

        type_in(&mut f, "Triage");
        f.key(key(KeyCode::Enter), &look);
        type_in(&mut f, &dir);
        f.key(key(KeyCode::Tab), &look);
        type_in(&mut f, "weekdays at 8:45");
        let text = text_of(&f, WIDTH);
        assert!(text.contains("✓ Monday to Friday at 8:45 AM"), "{text}");
        assert!(text.contains("tomorrow 8:45 AM"), "the next runs: {text}");
        for _ in 0.."8:45".len() {
            f.key(key(KeyCode::Backspace), &look);
        }
        type_in(&mut f, "xx");
        let text = text_of(&f, WIDTH);
        assert!(text.contains("isn't a schedule I can read"), "{text}");
        for _ in 0..2 {
            f.key(key(KeyCode::Backspace), &look);
        }
        type_in(&mut f, "8:45");

        // Saving without a prompt says so, at the prompt.
        let alt_enter = Key::new(KeyCode::Enter, Mods::ALT);
        assert_eq!(f.key(alt_enter, &look), Step::Redraw);
        assert!(text_of(&f, WIDTH).contains("Write the prompt it runs."));
        assert_eq!(f.focus, Part::Prompt);
        type_in(&mut f, "Sort them.");
        f.key(key(KeyCode::Enter), &look);
        type_in(&mut f, "By area.");
        // Skip it when busy.
        f.focus = Part::Pick(Choice::IfBusy);
        f.key(key(KeyCode::Right), &look);
        match f.key(alt_enter, &look) {
            Step::Close(Outcome::Picked(p)) => {
                assert_eq!(p.index, 0);
                let d: Draft = serde_json::from_str(&p.value).unwrap();
                assert_eq!(d.name, "Triage");
                assert_eq!(d.when, "weekdays at 8:45");
                assert_eq!(d.prompt, "Sort them.\nBy area.");
                assert_eq!(d.when_busy, WhenBusy::Skip);
                assert_eq!(d.tab_policy, TabPolicy::SameTab);
            }
            other => panic!("{other:?}"),
        }
        let alt_r = Key::new(KeyCode::Char('r'), Mods::ALT);
        assert!(matches!(f.key(alt_r, &look), Step::Close(Outcome::Picked(p)) if p.index == 1));
    }

    #[test]
    fn an_unchanged_when_keeps_its_schedule_even_one_gone_by() {
        let look = Look::default();
        let gone = Schedule::Once {
            at: now().timestamp().as_second() - 3600,
        };
        let dir = std::env::temp_dir().to_string_lossy().into_owned();
        let draft = Draft {
            name: "Ship notes".into(),
            dir,
            when: "tomorrow 9am".into(),
            prompt: "Write them.".into(),
            ..Draft::default()
        };
        let mut f = RoutineForm::new(true, draft, Vec::new(), Some(gone)).at_time(now());
        assert!(
            text_of(&f, WIDTH).contains("✓ Once, Wednesday, November 4"),
            "{}",
            text_of(&f, WIDTH)
        );
        let alt_enter = Key::new(KeyCode::Enter, Mods::ALT);
        assert!(matches!(
            f.key(alt_enter, &look),
            Step::Close(Outcome::Picked(_))
        ));
        // Typed again, it is read against now.
        f.focus = Part::When;
        f.key(key(KeyCode::Backspace), &look);
        f.key(key(KeyCode::Backspace), &look);
        type_in(&mut f, "AM");
        assert!(text_of(&f, WIDTH).contains("✓ Once, Thursday, November 5 at 9:00 AM"));
    }

    #[test]
    fn esc_asks_once_anything_was_typed_and_a_narrow_box_drops_the_examples() {
        let look = Look::default();
        let mut f = RoutineForm::new(false, Draft::default(), Vec::new(), None).at_time(now());
        assert_eq!(
            f.key(key(KeyCode::Esc), &look),
            Step::Close(Outcome::Dismissed)
        );
        type_in(&mut f, "x");
        assert_eq!(f.key(key(KeyCode::Esc), &look), Step::Redraw);
        assert!(text_of(&f, WIDTH).contains("Esc again discards"));
        assert_eq!(
            f.key(key(KeyCode::Esc), &look),
            Step::Close(Outcome::Dismissed)
        );
        let text = text_of(&f, 60);
        assert!(!text.contains("mondays 10:00"), "{text}");
        assert!(text.contains("Prompt"), "{text}");
    }
}
