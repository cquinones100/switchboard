//! Notes' "Work on this item" box (UX 2.0 board 36), which Enter on an item
//! whose plan shows beside the note, and `gw`, open in place of
//! `work_on_item`'s chain of boxes (src/cmd/work.rs) with UX 2.0 on. No
//! tmux-version file is behind it; the tmux version's wtn-open.zsh asked
//! the repository and the branch in turn.
//!
//! One list moved with ↑↓, drawn as autopilot's settings box is (board 35):
//! the item's checkbox and first line with its plan's chip; the
//! repositories as radio rows, the note's last one marked "the note's" and
//! chosen first; "Lands on", a field holding the branch last picked for the
//! repository, its suggestion dim after what is typed (→ takes it), how it
//! lands under it; what Start opens (the tab's number, the worktree wtn
//! names, wtn on the item and its plan), or that a worktree has the item
//! already; then "▶ Start it", the row it opens on, and, while autopilot
//! works the note, "Next in autopilot's queue". With a tab on the item
//! already, it says so in place of the rows, its one row "Go to it".
//!
//! Choosing another repository closes it with `REPO`, so the server reads
//! that one's branches, landing and worktrees off its task and opens it
//! again; Start, Next and Go to it close it with theirs, the choices as
//! JSON (`Chosen`). Rows are laid out with `kit::columns`, so it fills any
//! width as board 45 says: 78 columns inside while the screen leaves room,
//! a repository's name fixed and its path stretching, cut from its start,
//! "the note's" at the right edge.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;
use serde::{Deserialize, Serialize};

use crate::mux::input::{Key, KeyCode, Mods, Mouse};
use crate::ui::draw::{self, BoxLine, BoxSpec, Drawn, Gesture, Hit};
use crate::ui::kit::{self, Col, Surface, Tone};
use crate::ui::pointer::Pointer;
use crate::ui::{Look, Outcome, Overlay, Picked, Step};

/// Its inside, while the screen leaves room around it.
const WIDTH: usize = 78;

/// What it closed with, as `Picked::index`.
pub const START: usize = 0;
pub const NEXT: usize = 1;
pub const REPO: usize = 2;
pub const GO_TO: usize = 3;
/// "Another directory…": a repository typed in, which the server asks for.
pub const OTHER: usize = 4;

/// How settled the item's plan is, as its chip says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlanChip {
    Settled,
    Choice,
    None,
}

/// A repository offered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Repo {
    /// Its folder's name.
    pub name: String,
    /// Its path, `~/` for the home directory.
    pub path: String,
}

/// What the box shows, read off the server's task as it opens.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Context {
    /// The item's first line.
    pub item: String,
    pub plan: PlanChip,
    pub repos: Vec<Repo>,
    /// The one chosen.
    pub chosen: usize,
    /// The note's last repository, which "the note's" marks.
    pub note_repo: Option<usize>,
    /// The branch it lands on as it opens: the one last picked for the
    /// chosen repository, empty for its default branch.
    pub target: String,
    /// The chosen repository's branches, its default one first.
    pub branches: Vec<String>,
    /// Its default branch's name, for the words under the field.
    pub default_branch: String,
    /// How the chosen repository lands: "direct" or "pr", and what said so
    /// in words (", as repos.json says"), or why it can't tell.
    pub landing: Result<(String, String), String>,
    /// The number the tab Start opens takes.
    pub tab: usize,
    /// The worktree wtn makes for the item.
    pub worktree: String,
    /// A worktree of the chosen repository has the item already.
    pub claimed: bool,
    /// Autopilot works the note, so the item can be put next instead.
    pub autopilot: bool,
    /// A tab on the item already: its number and name, and whether its
    /// Claude waits on you.
    pub linked: Option<(usize, String, bool)>,
}

/// What it closed with, as `Picked::value`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Chosen {
    pub repo: usize,
    pub target: String,
}

/// A row of the list, which ↑↓ and Tab move between.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    Repo(usize),
    /// "Another directory…", after the repositories.
    Other,
    Target,
    Start,
    Next,
    GoTo,
    Close,
}

pub struct WorkItemBox {
    ctx: Context,
    target: String,
    focus: Part,
    hover: Option<Part>,
    drawn: Option<Drawn>,
    /// Each part's line in the box.
    parts: Vec<(usize, Part)>,
    /// The first repository shown, when they don't all fit.
    repo_top: usize,
    /// The focus the window last followed, so the wheel moves it until the
    /// focus moves again.
    followed: Option<Part>,
}

impl WorkItemBox {
    /// The box, its focus on "▶ Start it" (or "Go to it").
    pub fn new(ctx: Context) -> WorkItemBox {
        let target = ctx.target.clone();
        let focus = if ctx.linked.is_some() {
            Part::GoTo
        } else {
            Part::Start
        };
        WorkItemBox {
            ctx,
            target,
            focus,
            hover: None,
            drawn: None,
            parts: Vec::new(),
            repo_top: 0,
            followed: None,
        }
    }

    /// The box reopened on another repository, `focus` on its row.
    pub fn with_focus(mut self, focus: Part) -> WorkItemBox {
        self.focus = focus;
        self
    }

    fn stops(&self, look: &Look) -> Vec<Part> {
        let mut out = Vec::new();
        if self.ctx.linked.is_some() {
            out.push(Part::GoTo);
        } else {
            out.extend((0..self.ctx.repos.len()).map(Part::Repo));
            out.push(Part::Other);
            out.push(Part::Target);
            out.push(Part::Start);
            if self.ctx.autopilot {
                out.push(Part::Next);
            }
        }
        if look.close {
            out.push(Part::Close);
        }
        out
    }

    fn step_focus(&mut self, by: isize, look: &Look) {
        let stops = self.stops(look);
        let at = stops.iter().position(|p| *p == self.focus).unwrap_or(0) as isize;
        self.focus = stops[(at + by).rem_euclid(stops.len() as isize) as usize];
    }

    fn close(&self, index: usize, repo: usize) -> Step {
        let chosen = Chosen {
            repo,
            target: self.target.trim().to_string(),
        };
        Step::Close(Outcome::Picked(Picked {
            index,
            value: serde_json::to_string(&chosen).unwrap_or_default(),
            query: String::new(),
        }))
    }

    fn press(&mut self, part: Part) -> Step {
        // A click on the field it is typing in takes the suggestion, as →
        // does.
        if part == Part::Target
            && self.focus == Part::Target
            && let Some(rest) = self.suggestion()
        {
            self.target.push_str(&rest);
            return Step::Redraw;
        }
        self.focus = part;
        match part {
            Part::Repo(i) if i != self.ctx.chosen => self.close(REPO, i),
            Part::Repo(_) | Part::Target => Step::Redraw,
            Part::Other => self.close(OTHER, self.ctx.chosen),
            Part::Start => self.close(START, self.ctx.chosen),
            Part::Next => self.close(NEXT, self.ctx.chosen),
            Part::GoTo => self.close(GO_TO, self.ctx.chosen),
            Part::Close => Step::Close(Outcome::Dismissed),
        }
    }

    /// The rest of the suggested branch for what is typed.
    fn suggestion(&self) -> Option<String> {
        let typed = &self.target;
        self.ctx
            .branches
            .iter()
            .find(|b| b.starts_with(typed.as_str()) && b.len() > typed.len())
            .map(|b| b[typed.len()..].to_string())
    }

    /// How the branch typed lands, in words.
    fn lands(&self) -> String {
        let branch = match self.target.trim() {
            "" => self.ctx.default_branch.as_str(),
            t => t,
        };
        let branch = if branch.is_empty() {
            "the default branch"
        } else {
            branch
        };
        match &self.ctx.landing {
            Ok((how, said)) if how == "direct" => {
                format!("straight on {branch} through queue-and-clean{said}")
            }
            Ok((_, said)) => format!("through a PR to {branch}{said}"),
            Err(e) => format!("couldn't tell how it lands: {e}"),
        }
    }

    /// The box's lines `inner` wide, and where each part is.
    pub fn lines(&self, inner: usize, look: &Look) -> (Vec<BoxLine>, Vec<(usize, Part)>) {
        self.lines_in(inner, None, look)
    }

    /// `lines`, showing only `window`'s repositories (the first and how
    /// many) when given, their count on the label.
    fn lines_in(
        &self,
        inner: usize,
        window: Option<(usize, usize)>,
        look: &Look,
    ) -> (Vec<BoxLine>, Vec<(usize, Part)>) {
        let theme = &look.theme;
        let ctx = &self.ctx;
        let mut lines: Vec<BoxLine> = Vec::new();
        let mut parts = Vec::new();
        let cols = inner.saturating_sub(1);
        let chosen_fill = Some(Surface::Chosen.color(theme));
        let lit = |p: Part| self.focus == p || self.hover == Some(p);

        // The item, with its plan's chip.
        let (chip, tone) = match ctx.plan {
            PlanChip::Settled => ("plan settled", Tone::Done),
            PlanChip::Choice => ("needs a choice", Tone::Needs),
            PlanChip::None => ("no plan", Tone::Faint),
        };
        let chip = if ctx.plan == PlanChip::None {
            vec![Span::styled(chip, tone.style(theme))]
        } else {
            kit::chip(chip, tone, false, theme)
        };
        let at = kit::columns(
            &[
                Col::Stretch {
                    weight: 1,
                    min: 10,
                    max: None,
                },
                Col::Right {
                    natural: kit::spans_width(&chip),
                    cap: 20,
                },
            ],
            cols,
            2,
            &[1],
        );
        let mut head = vec![Span::raw(" ")];
        if let Some(a) = at[0] {
            kit::place(
                &mut head,
                vec![
                    Span::styled("☐ ", Tone::Dim.style(theme)),
                    Span::styled(ctx.item.clone(), Tone::Text.bold(theme)),
                ],
                (a.0 + 1, a.1),
            );
        }
        if let Some(a) = at[1] {
            kit::place(&mut head, chip, (a.0 + 1, a.1));
        }
        lines.push(BoxLine::spans(head));
        lines.push(BoxLine::blank());

        if let Some((number, name, waits)) = &ctx.linked {
            let (mark, tone) = if *waits {
                ("●", Tone::Needs)
            } else {
                ("⠹", Tone::Working)
            };
            lines.push(BoxLine::spans(vec![
                Span::raw(" "),
                Span::styled("A tab works on it already: ", Tone::Dim.style(theme)),
                Span::styled(mark, tone.bold(theme)),
                Span::raw(" "),
                Span::styled(number.to_string(), Tone::Text.bold(theme)),
                Span::raw(" "),
                Span::styled(name.clone(), Tone::Text.style(theme)),
            ]));
            lines.push(BoxLine::blank());
            parts.push((lines.len(), Part::GoTo));
            lines.push(self.action("Go to it", "", lit(Part::GoTo), inner, theme));
            return (lines, parts);
        }

        // The repositories, the chosen one's radio filled.
        let count = window.map(|_| ctx.repos.len());
        lines.push(BoxLine::spans(kit::label(
            "Repository",
            Tone::Dim,
            count,
            cols,
            theme,
        )));
        let (first, shown) = window.unwrap_or((0, ctx.repos.len()));
        let name_w = kit::natural(ctx.repos.iter().map(|r| r.name.as_str()));
        let theirs = "the note's";
        let repo_cols = kit::columns(
            &[
                Col::Fixed {
                    min: 1,
                    natural: 1,
                    cap: 1,
                },
                Col::Fixed {
                    min: 4,
                    natural: name_w,
                    cap: 24,
                },
                Col::Stretch {
                    weight: 1,
                    min: 6,
                    max: None,
                },
                Col::Right {
                    natural: draw::width(theirs),
                    cap: draw::width(theirs),
                },
            ],
            cols,
            2,
            &[2, 3],
        );
        for (i, repo) in ctx.repos.iter().enumerate().skip(first).take(shown) {
            let on = i == ctx.chosen;
            let mut line = vec![Span::raw(" ")];
            let cells = [
                vec![Span::styled(
                    if on { "●" } else { "○" },
                    if on {
                        Tone::Accent.bold(theme)
                    } else {
                        Tone::Dim.style(theme)
                    },
                )],
                vec![Span::styled(repo.name.clone(), Tone::Text.bold(theme))],
                vec![Span::styled(
                    cut_start(&repo.path, repo_cols[2].map_or(0, |c| c.1)),
                    Tone::Dim.style(theme),
                )],
                if ctx.note_repo == Some(i) {
                    vec![Span::styled(theirs, Tone::Dim.style(theme))]
                } else {
                    Vec::new()
                },
            ];
            for (cell, place) in cells.into_iter().zip(&repo_cols) {
                if let Some((x, w)) = place {
                    kit::place(&mut line, cell, (x + 1, *w));
                }
            }
            parts.push((lines.len(), Part::Repo(i)));
            lines.push(
                BoxLine::spans(line)
                    .fill(lit(Part::Repo(i)).then_some(chosen_fill).flatten())
                    .clickable(),
            );
        }
        parts.push((lines.len(), Part::Other));
        let lit_other = lit(Part::Other);
        lines.push(
            BoxLine::spans(vec![
                Span::raw(" "),
                Span::styled("+", Tone::Dim.style(theme)),
                Span::raw("  "),
                Span::styled(
                    "Another directory…",
                    if lit_other {
                        Tone::Text.bold(theme)
                    } else {
                        Tone::Dim.style(theme)
                    },
                ),
            ])
            .fill(lit_other.then_some(chosen_fill).flatten())
            .clickable(),
        );
        lines.push(BoxLine::blank());

        // Lands on: the branch, its suggestion, how it lands.
        lines.push(BoxLine::spans(kit::label(
            "Lands on",
            Tone::Dim,
            None,
            cols,
            theme,
        )));
        parts.push((lines.len(), Part::Target));
        lines.push(BoxLine::spans(self.field(cols, look)).clickable());
        lines.push(BoxLine::spans(vec![
            Span::raw("   "),
            Span::styled(
                draw::cut(&self.lands(), cols.saturating_sub(3)),
                Tone::Dim.style(theme),
            ),
        ]));
        lines.push(BoxLine::blank());

        // What Start opens.
        let opens = if ctx.claimed {
            format!(
                "A worktree of {} has this item already.",
                ctx.repos.get(ctx.chosen).map_or("", |r| r.path.as_str())
            )
        } else {
            format!(
                "Opens tab {} in a new worktree, {}, running wtn on this item{}.",
                ctx.tab,
                ctx.worktree,
                if ctx.plan == PlanChip::None {
                    ""
                } else {
                    " and its plan"
                }
            )
        };
        let tone = if ctx.claimed { Tone::Needs } else { Tone::Dim };
        for row in draw::wrap(&opens, cols.saturating_sub(1)) {
            lines.push(BoxLine::spans(vec![
                Span::raw(" "),
                Span::styled(row, tone.style(theme)),
            ]));
        }
        lines.push(BoxLine::blank());

        parts.push((lines.len(), Part::Start));
        lines.push(self.action(
            "▶ Start it",
            &format!("in tab {}, now", ctx.tab),
            lit(Part::Start),
            inner,
            theme,
        ));
        if ctx.autopilot {
            parts.push((lines.len(), Part::Next));
            lines.push(self.action(
                "  Next in autopilot's queue",
                "autopilot works this note",
                lit(Part::Next),
                inner,
                theme,
            ));
        }
        (lines, parts)
    }

    /// A row that does something: its words, and what it says of itself at
    /// the right, dim; on the chosen surface when lit.
    fn action(
        &self,
        words: &str,
        right: &str,
        lit: bool,
        inner: usize,
        theme: &crate::ui::Theme,
    ) -> BoxLine {
        let cols = inner.saturating_sub(1);
        let at = kit::columns(
            &[
                Col::Stretch {
                    weight: 1,
                    min: 8,
                    max: None,
                },
                Col::Right {
                    natural: draw::width(right),
                    cap: 30,
                },
            ],
            cols,
            2,
            &[1],
        );
        let style = if lit {
            Style::new().fg(theme.bright).add_modifier(Modifier::BOLD)
        } else {
            Tone::Text.style(theme)
        };
        let mut line = vec![Span::raw(" ")];
        if let Some(a) = at[0] {
            kit::place(
                &mut line,
                vec![Span::styled(words.to_string(), style)],
                (a.0 + 1, a.1),
            );
        }
        if let Some(a) = at[1] {
            kit::place(
                &mut line,
                vec![Span::styled(right.to_string(), Tone::Dim.style(theme))],
                (a.0 + 1, a.1),
            );
        }
        BoxLine::spans(line)
            .fill(lit.then_some(Surface::Chosen.color(theme)))
            .clickable()
    }

    /// The Lands on field: what is typed, the suggestion's rest dim after
    /// it, "→ take the suggestion" at its right while there is one.
    fn field(&self, cols: usize, look: &Look) -> Vec<Span<'static>> {
        let theme = &look.theme;
        let focused = self.focus == Part::Target;
        let mut typed = vec![
            Span::styled(
                "› ",
                if focused {
                    Tone::Accent.bold(theme)
                } else {
                    Tone::Dim.bold(theme)
                },
            ),
            Span::styled(self.target.clone(), Tone::Text.style(theme)),
        ];
        if focused {
            typed.push(Span::styled("▏", Tone::Accent.style(theme)));
        }
        let suggestion = self.suggestion();
        let mut right = Vec::new();
        match &suggestion {
            Some(rest) => {
                typed.push(Span::styled(rest.clone(), Tone::Faint.style(theme)));
                right = kit::hints(&[("→", "take the suggestion")], theme);
            }
            None if self.target.is_empty() => {
                typed.push(Span::styled("the default branch", Tone::Faint.style(theme)));
            }
            None => {}
        }
        let inner = cols.saturating_sub(3);
        let room = inner.saturating_sub(kit::spans_width(&right) + 1);
        let mut line = kit::fit(typed, room, Style::new());
        if inner > kit::spans_width(&line) + kit::spans_width(&right) {
            line.extend(right);
        }
        let bg = if focused || self.hover == Some(Part::Target) {
            Surface::Chosen.color(theme)
        } else {
            Surface::Panel.color(theme)
        };
        let mut out = vec![Span::raw(" "), Span::styled(" ", Style::new().bg(bg))];
        out.extend(
            kit::fit(line, inner, Style::new())
                .into_iter()
                .map(|s| Span::styled(s.content, s.style.bg(bg))),
        );
        out.push(Span::styled(" ", Style::new().bg(bg)));
        out
    }

    fn part_at(&self, hit: Hit) -> Option<Part> {
        match hit {
            Hit::Close => Some(Part::Close),
            Hit::Line(line, _) => self.parts.iter().find(|(l, _)| *l == line).map(|p| p.1),
            _ => None,
        }
    }
}

/// `text` cut from its start with … to `cols` columns, so a path keeps
/// its end.
fn cut_start(text: &str, cols: usize) -> String {
    if draw::width(text) <= cols {
        return text.to_string();
    }
    if cols == 0 {
        return String::new();
    }
    let mut tail: Vec<char> = Vec::new();
    let mut used = 1;
    for c in text.chars().rev() {
        let w = draw::width(&c.to_string());
        if used + w > cols {
            break;
        }
        used += w;
        tail.push(c);
    }
    tail.reverse();
    format!("…{}", tail.into_iter().collect::<String>())
}

impl Overlay for WorkItemBox {
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
        "work-on-item"
    }

    fn stands_out(&self) -> Option<Rect> {
        self.drawn.as_ref().map(|d| d.outer)
    }

    fn draw(&mut self, buf: &mut Buffer, look: &Look) {
        let inner = (buf.area.width as usize).saturating_sub(6).min(WIDTH);
        let (mut lines, mut parts) = self.lines(inner, look);
        // Too tall for the screen (draw_box keeps a row clear above and
        // below, and two for the keys): the repositories in a window kept
        // around the one with the focus, so the rows after them stay in
        // sight.
        let fits = (buf.area.height as usize).saturating_sub(6);
        let n = self.ctx.repos.len();
        if lines.len() > fits && n > 1 {
            let room = n.saturating_sub(lines.len() - fits).max(1);
            let moved = self.followed != Some(self.focus);
            self.followed = Some(self.focus);
            if let (true, Part::Repo(i)) = (moved, self.focus) {
                if i < self.repo_top {
                    self.repo_top = i;
                } else if i >= self.repo_top + room {
                    self.repo_top = i + 1 - room;
                }
            }
            self.repo_top = self.repo_top.min(n - room);
            (lines, parts) = self.lines_in(inner, Some((self.repo_top, room)), look);
        }
        self.parts = parts;
        let mut spec = BoxSpec::new("Work on this item", &lines);
        spec.keys = Some("↑↓ row  ↵ choose  → take the suggestion  esc cancel");
        spec.width = inner as u16;
        spec.close_lit = self.focus == Part::Close || self.hover == Some(Part::Close);
        self.drawn = Some(draw::draw_box(buf, &spec, look));
    }

    fn key(&mut self, key: Key, look: &Look) -> Step {
        let k = key.normalized();
        match k.code {
            KeyCode::Esc => return Step::Close(Outcome::Dismissed),
            KeyCode::Char('c') if k.mods == Mods::CTRL => return Step::Close(Outcome::Dismissed),
            KeyCode::Tab | KeyCode::Down => {
                self.step_focus(1, look);
                return Step::Redraw;
            }
            KeyCode::BackTab | KeyCode::Up => {
                self.step_focus(-1, look);
                return Step::Redraw;
            }
            KeyCode::Enter => return self.press(self.focus),
            _ => {}
        }
        if self.focus != Part::Target {
            return match k.code {
                KeyCode::Char(' ') => self.press(self.focus),
                _ => Step::Same,
            };
        }
        match k.code {
            KeyCode::Right => match self.suggestion() {
                Some(rest) => {
                    self.target.push_str(&rest);
                    Step::Redraw
                }
                None => Step::Same,
            },
            KeyCode::Backspace => {
                self.target.pop();
                Step::Redraw
            }
            KeyCode::Char('u') if k.mods == Mods::CTRL => {
                self.target.clear();
                Step::Redraw
            }
            KeyCode::Char(c) if !k.mods.ctrl && !k.mods.alt => {
                self.target.push(c);
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
            Gesture::WheelDown => {
                self.repo_top += 1;
                Step::Redraw
            }
            Gesture::WheelUp => {
                self.repo_top = self.repo_top.saturating_sub(1);
                Step::Redraw
            }
            Gesture::Click if look.mouse => match part {
                Some(p) => self.press(p),
                None if hit == Hit::Outside => Step::Close(Outcome::Dismissed),
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
        if self.focus != Part::Target {
            return Step::Same;
        }
        self.target.push_str(text.trim());
        Step::Redraw
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn ctx() -> Context {
        Context {
            item: "cycle through idle tabs with M-A".into(),
            plan: PlanChip::Settled,
            repos: vec![
                Repo {
                    name: "switchboard".into(),
                    path: "~/switchboard".into(),
                },
                Repo {
                    name: "web-app".into(),
                    path: "~/code/web-app".into(),
                },
            ],
            chosen: 0,
            note_repo: Some(0),
            target: String::new(),
            branches: vec!["main".into(), "release".into()],
            default_branch: "main".into(),
            landing: Ok(("direct".into(), ", as repos.json says".into())),
            tab: 10,
            worktree: "note-cycle-through-idle-tabs-with-m-a".into(),
            claimed: false,
            autopilot: true,
            linked: None,
        }
    }

    fn text(b: &WorkItemBox, inner: usize) -> String {
        let (lines, _) = b.lines(inner, &Look::default());
        lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn key(code: KeyCode) -> Key {
        Key::new(code, Mods::NONE)
    }

    #[test]
    fn the_box_shows_the_item_its_repositories_how_it_lands_and_what_start_opens() {
        let b = WorkItemBox::new(ctx());
        let shown = text(&b, WIDTH);
        assert!(
            shown.contains("☐ cycle through idle tabs with M-A"),
            "{shown}"
        );
        assert!(shown.contains("plan settled"), "{shown}");
        assert!(shown.contains("REPOSITORY"), "{shown}");
        let first = shown.lines().find(|l| l.contains("switchboard")).unwrap();
        assert!(
            first.contains('●') && first.ends_with("the note's"),
            "{first}"
        );
        let second = shown.lines().find(|l| l.contains("web-app")).unwrap();
        assert!(
            second.contains('○') && !second.contains("the note's"),
            "{second}"
        );
        assert!(shown.contains("straight on main through queue-and-clean, as repos.json says"));
        let flat = shown.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(flat.contains(
            "Opens tab 10 in a new worktree, note-cycle-through-idle-tabs-with-m-a, running wtn on this item and its plan."
        ), "{shown}");
        assert!(shown.contains("▶ Start it"), "{shown}");
        assert!(shown.contains("Next in autopilot's queue"), "{shown}");
        assert_eq!(b.focus, Part::Start);

        let mut off = ctx();
        off.autopilot = false;
        assert!(!text(&WorkItemBox::new(off), WIDTH).contains("autopilot's queue"));
    }

    #[test]
    fn enter_starts_it_and_another_repository_reopens_the_box_on_it() {
        let look = Look::default();
        let mut b = WorkItemBox::new(ctx());
        let Step::Close(Outcome::Picked(p)) = b.key(key(KeyCode::Enter), &look) else {
            panic!("Enter should start it");
        };
        assert_eq!(p.index, START);
        let chosen: Chosen = serde_json::from_str(&p.value).unwrap();
        assert_eq!(
            chosen,
            Chosen {
                repo: 0,
                target: String::new()
            }
        );

        // ↑ to the field, then the second repository, Enter.
        let mut b = WorkItemBox::new(ctx());
        b.key(key(KeyCode::Up), &look);
        assert_eq!(b.focus, Part::Target);
        b.key(key(KeyCode::Right), &look);
        assert_eq!(b.target, "main");
        b.key(key(KeyCode::Up), &look);
        assert_eq!(b.focus, Part::Other);
        b.key(key(KeyCode::Up), &look);
        let Step::Close(Outcome::Picked(p)) = b.key(key(KeyCode::Enter), &look) else {
            panic!("Enter on another repository should reopen the box");
        };
        assert_eq!(p.index, REPO);
        let chosen: Chosen = serde_json::from_str(&p.value).unwrap();
        assert_eq!(
            chosen,
            Chosen {
                repo: 1,
                target: "main".into()
            }
        );

        // ↓ from Start to Next.
        let mut b = WorkItemBox::new(ctx());
        b.key(key(KeyCode::Down), &look);
        let Step::Close(Outcome::Picked(p)) = b.key(key(KeyCode::Enter), &look) else {
            panic!("Enter on Next should close it");
        };
        assert_eq!(p.index, NEXT);
    }

    #[test]
    fn a_click_on_the_field_being_typed_in_takes_the_suggestion() {
        let mut b = WorkItemBox::new(ctx());
        b.press(Part::Target);
        assert_eq!(b.target, "");
        b.press(Part::Target);
        assert_eq!(b.target, "main");
    }

    #[test]
    fn another_directory_closes_it_to_ask_for_one() {
        let look = Look::default();
        let mut b = WorkItemBox::new(ctx());
        assert!(text(&b, WIDTH).contains("Another directory…"));
        b.focus = Part::Other;
        let Step::Close(Outcome::Picked(p)) = b.key(key(KeyCode::Enter), &look) else {
            panic!("Enter should ask for a directory");
        };
        assert_eq!(p.index, OTHER);
    }

    #[test]
    fn many_repositories_scroll_so_start_stays_in_sight() {
        let mut c = ctx();
        c.repos = (0..40)
            .map(|i| Repo {
                name: format!("repo{i}"),
                path: format!("~/code/repo{i}"),
            })
            .collect();
        let mut b = WorkItemBox::new(c);
        let draw = |b: &mut WorkItemBox| {
            let mut buf = Buffer::empty(Rect::new(0, 0, 120, 40));
            b.draw(&mut buf, &Look::default());
            (0..40)
                .map(|y| {
                    (0..120)
                        .map(|x| buf[(x, y)].symbol().to_string())
                        .collect::<String>()
                })
                .collect::<Vec<_>>()
                .join("\n")
        };
        let shown = draw(&mut b);
        assert!(shown.contains("▶ Start it"), "{shown}");
        assert!(
            shown.contains("REPOSITORY") && shown.contains("40"),
            "{shown}"
        );
        assert!(
            shown.contains("repo0 ") && !shown.contains("repo39"),
            "{shown}"
        );
        b.focus = Part::Repo(39);
        let shown = draw(&mut b);
        assert!(
            shown.contains("repo39") && shown.contains("▶ Start it"),
            "{shown}"
        );
        // With the focus on the first, the wheel still moves the window.
        b.focus = Part::Repo(0);
        draw(&mut b);
        b.repo_top = 40;
        let shown = draw(&mut b);
        assert!(shown.contains("repo39"), "{shown}");
    }

    #[test]
    fn with_a_tab_on_the_item_it_offers_only_to_go_there() {
        let mut c = ctx();
        c.linked = Some((4, "idle tabs".into(), true));
        let b = WorkItemBox::new(c);
        let shown = text(&b, WIDTH);
        assert!(
            shown.contains("A tab works on it already: ● 4 idle tabs"),
            "{shown}"
        );
        assert!(shown.contains("Go to it"));
        assert!(!shown.contains("REPOSITORY"));
        assert_eq!(b.focus, Part::GoTo);
    }

    #[test]
    fn a_claimed_item_and_a_pr_landing_say_so() {
        let mut c = ctx();
        c.claimed = true;
        c.landing = Ok(("pr".into(), String::new()));
        let shown = text(&WorkItemBox::new(c), WIDTH);
        assert!(shown.contains("A worktree of ~/switchboard has this item already."));
        assert!(shown.contains("through a PR to main"));
    }

    #[test]
    fn a_narrow_box_cuts_a_path_from_its_start_and_keeps_the_note_s_at_the_edge() {
        let mut c = ctx();
        c.repos[0].path = "~/a/very/long/path/to/the/switchboard/clone".into();
        let shown = text(&WorkItemBox::new(c), 50);
        let row = shown.lines().find(|l| l.contains("switchboard")).unwrap();
        assert!(row.contains("…"), "{row}");
        assert!(row.ends_with("the note's"), "{row}");
        assert!(draw::width(row) <= 50, "{row}");
    }
}
