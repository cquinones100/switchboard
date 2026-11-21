//! The story told as a talk, read outside in (boards 159 to 163): the
//! diff's Talk view, first in the views' switch while diff_story_talk is
//! on, and where M-d opens.
//!
//! It reads at one of four zooms, `−` and `+` stepping out and in from
//! wherever the reader is: Gist (the change in a sentence, the example
//! from the change's own tests as a Before and an After card, and the flow
//! it takes, each step a box lit where the example passes, its value over
//! the arrow after it), Flow (the same flow over each step's How and Why
//! and the example's value there, the step to read closely flagged), Steps
//! (a slide a step: the flow down the left, the step's claim, the example
//! going in and coming out, its hunk with notes in the margin, and its
//! Decided, Instead and Watch folded) and Code (the same step with its
//! notes open, every hunk of it, and the test the example comes from with
//! its case lit). The slides are the gist and then a step each, along the
//! example's path first and the steps off it after, `←` and `→` going
//! between them and a row of dots at the foot showing where.
//!
//! The steps are the story's pages, but for the side work; the flow is
//! What runs (story.rs's `runs`), only the steps the example passes
//! through while there is one, else every step, and a page's number among
//! the steps is the one the reader is told ("Step 2 of 5"). A story with
//! no example (no test of the change gave one) says so, its flow drawn
//! with no values and no step lit.
//!
//! Everything is laid out from the story and the diff's text alone, on
//! each draw, reading and starting nothing: the diff is split once, as
//! the view first needs it. The flow is wider than the pane when it has
//! many steps: it scrolls sideways (`h` and `l` onto a step out of sight
//! bring it in, Shift with the wheel moves it), a scroll bar under it
//! showing where.

use std::sync::Arc;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line as Spans, Span};

use super::reading::{self, Does, HunkLine, Mode, PROSE, counted};
use super::select::Pane;
use super::{Act, App, Focus, Hit};
use crate::story::{self, Kind, Story};
use switchboard_ui::Theme;
use switchboard_ui::draw::{cut, put_line, width, wrap};
use switchboard_ui::kit::{self, Surface, Tone};

/// How closely the talk is read.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Zoom {
    #[default]
    Gist,
    Flow,
    Steps,
    Code,
}

const ZOOMS: [(&str, Zoom); 4] = [
    ("Gist", Zoom::Gist),
    ("Flow", Zoom::Flow),
    ("Steps", Zoom::Steps),
    ("Code", Zoom::Code),
];

/// The columns a Steps slide's flow takes at its left, and the narrowest
/// pane that keeps it.
const SIDE: usize = 30;
const SIDE_FROM: usize = 110;
/// How far Shift and the wheel move the flow sideways.
const SIDEWAYS: usize = 8;
/// The narrowest a Before or After card is beside the other.
const CARD: usize = 40;

/// Something of the Talk view a click lands on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TalkHit {
    /// A zoom of the switch, and `−` and `+`.
    Zoom(Zoom),
    Out,
    In,
    /// A dot of the slides, by its place (0 the gist).
    Dot(usize),
    /// A step, by its page: a box of the flow, or a step of Flow's list.
    Step(usize),
    /// The Notes of a Steps slide, folded or open.
    Notes,
    /// The example's test, shown in the files.
    Test,
    /// A hunk's file, shown in the files.
    File(usize),
}

/// What has Tab's focus in the Talk view: what is read, the zoom's
/// switch, or the slides' dots.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Part {
    #[default]
    Content,
    Zoom,
    Slides,
}

/// The Talk view's own state: the zoom, the slide (0 the gist), the step
/// chosen on the flow by its page, the first row shown and the flow's
/// first column, whether the chosen step is brought into sight on the
/// next draw, a Steps slide's notes open, the part with Tab's focus, and
/// the diff split once for its hunks.
#[derive(Clone, Debug, Default)]
pub(super) struct Talker {
    pub(super) zoom: Zoom,
    pub(super) slide: usize,
    pub(super) at: usize,
    pub(super) top: usize,
    pub(super) left: usize,
    follow: bool,
    pub(super) notes: bool,
    pub(super) part: Part,
    /// The flow's width and the content's rows as last drawn, and the
    /// columns and rows it had, for the keys that scroll.
    shown: (usize, usize, usize, usize),
    /// The rows of the files and the test shown, by what a click on each
    /// shows, as last drawn: what Enter at Code shows.
    heads: Vec<(usize, TalkHit)>,
    split: Option<Arc<(story::Files, story::Hunks)>>,
}

/// A row of the talk: its spans, what a click on a part of it does (by
/// its first column and width), and whether it is the flow's, which
/// scrolls sideways, the flow's scroll bar, or on the chosen surface.
#[derive(Clone, Debug, Default)]
struct TRow {
    spans: Vec<Span<'static>>,
    hits: Vec<(TalkHit, usize, usize)>,
    wide: bool,
    bar: bool,
    chosen: bool,
}

impl TRow {
    fn of(spans: Vec<Span<'static>>) -> TRow {
        TRow {
            spans,
            ..TRow::default()
        }
    }

    fn doing(spans: Vec<Span<'static>>, hit: TalkHit) -> TRow {
        let w = kit::spans_width(&spans);
        TRow {
            spans,
            hits: vec![(hit, 0, w.max(1))],
            ..TRow::default()
        }
    }
}

/// The talk laid out: its rows, a Steps slide's flow at the left, the
/// flow's width and the chosen step's columns in it, and the rows the
/// chosen step takes.
#[derive(Default)]
struct Laid {
    rows: Vec<TRow>,
    side: Vec<TRow>,
    flow_width: usize,
    chosen_x: Option<(usize, usize)>,
    chosen_rows: Option<(usize, usize)>,
}

/// A hunk's lines, and the margin notes on them by line.
type Noted = (Vec<HunkLine>, Vec<(usize, String)>);

/// A box of the flow: its name and the page it is on.
struct FlowBox {
    name: String,
    page: usize,
}

/// The steps: the pages of every chapter but the side work, in reading
/// order.
pub(super) fn steps(story: &Story) -> Vec<usize> {
    story
        .chapters
        .iter()
        .filter(|c| c.kind != Kind::Side)
        .flat_map(|c| c.pages.iter().copied())
        .collect()
}

/// Whether the example passes through page `n`.
fn passes(story: &Story, n: usize) -> bool {
    story.example.is_some() && story.pages.get(n - 1).is_some_and(|p| p.passes)
}

/// The flow's boxes: What runs on the steps, those the example passes
/// through while there is one; with nothing said to run, a box a step.
fn flow(story: &Story) -> Vec<FlowBox> {
    let steps = steps(story);
    let runs: Vec<FlowBox> = story
        .runs
        .iter()
        .filter(|r| steps.contains(&r.page))
        .filter(|r| story.example.is_none() || passes(story, r.page))
        .map(|r| FlowBox {
            name: r.step.clone(),
            page: r.page,
        })
        .collect();
    if !runs.is_empty() || !story.runs.is_empty() {
        return runs;
    }
    steps
        .into_iter()
        .filter(|&n| story.example.is_none() || passes(story, n))
        .map(|n| FlowBox {
            name: story.pages[n - 1].title.clone(),
            page: n,
        })
        .collect()
}

/// The slides after the gist, a step each: along the example's path in
/// the order it runs, then the steps off it in reading order.
pub(super) fn slides(story: &Story) -> Vec<usize> {
    let mut out: Vec<usize> = Vec::new();
    for b in flow(story) {
        if !out.contains(&b.page) {
            out.push(b.page);
        }
    }
    for n in steps(story) {
        if !out.contains(&n) {
            out.push(n);
        }
    }
    out
}

/// Page `n`'s number among the steps, and how many there are.
fn step_number(story: &Story, n: usize) -> (usize, usize) {
    let steps = steps(story);
    let k = steps.iter().position(|&m| m == n).map_or(0, |i| i + 1);
    (k, steps.len())
}

/// Page `n`'s own hunks, by path in the order its files are touched.
fn own_hunks(story: &Story, n: usize) -> Vec<(String, story::HunkNote)> {
    let Some(page) = story.pages.get(n - 1) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for path in &page.touches {
        for h in story.hunks.get(path).map(Vec::as_slice).unwrap_or(&[]) {
            if h.page == n {
                out.push((path.clone(), h.clone()));
            }
        }
    }
    out
}

/// The hunk a Steps slide of page `n` shows, among its own: the first
/// with a note in the margin, else its first.
fn shown_hunk(story: &Story, n: usize) -> usize {
    own_hunks(story, n)
        .iter()
        .position(|(_, h)| story.margins.iter().any(|m| m.hunk == h.id))
        .unwrap_or(0)
}

/// Whether page `n` is new in the change: every hunk of it adds a file.
fn is_new(story: &Story, n: usize) -> bool {
    let hunks = own_hunks(story, n);
    !hunks.is_empty()
        && hunks
            .iter()
            .all(|(_, h)| h.old_start == 0 && h.old_count == 0)
}

/// The columns from `left` on of a row of spans, `w` of them.
fn columns_of(spans: &[Span<'static>], left: usize, w: usize) -> Vec<Span<'static>> {
    let mut out: Vec<Span<'static>> = Vec::new();
    let mut at = 0;
    for span in spans {
        let mut text = String::new();
        for c in span.content.chars() {
            let cw = width(c.encode_utf8(&mut [0; 4]));
            if at >= left && at + cw <= left + w {
                text.push(c);
            }
            at += cw;
        }
        if !text.is_empty() {
            out.push(Span::styled(text, span.style));
        }
        if at >= left + w {
            break;
        }
    }
    out
}

/// A horizontal scroll bar `w` wide for `total` columns of which `w`
/// show from `left`: a faint track, the thumb in the text's tone.
pub(super) fn sideways_bar(
    total: usize,
    left: usize,
    w: usize,
    theme: &Theme,
) -> Vec<Span<'static>> {
    let Some((at, len)) = kit::thumb(total, left, w, w) else {
        return Vec::new();
    };
    vec![
        Span::styled("─".repeat(at), Tone::Faint.style(theme)),
        Span::styled("━".repeat(len), Tone::Text.style(theme)),
        Span::styled("─".repeat(w - at - len), Tone::Faint.style(theme)),
    ]
}

/// Prose wrapped at `cols` under a label, the label's width kept empty on
/// the lines after the first.
fn labeled(
    label: Span<'static>,
    text: &str,
    tone: Style,
    cols: usize,
    indent: usize,
) -> Vec<Vec<Span<'static>>> {
    let lead = indent + width(&label.content);
    wrap(text, cols.saturating_sub(lead).max(10))
        .into_iter()
        .enumerate()
        .map(|(i, line)| {
            let first = if i == 0 {
                label.clone()
            } else {
                Span::raw(" ".repeat(width(&label.content)))
            };
            vec![
                Span::raw(" ".repeat(indent)),
                first,
                Span::styled(line, tone),
            ]
        })
        .collect()
}

/// Where a margin note goes in a hunk: the first line whose code is the
/// note's, else the first holding it.
fn margin_line(lines: &[HunkLine], code: &str) -> Option<usize> {
    let code = code.trim();
    if code.is_empty() {
        return None;
    }
    lines
        .iter()
        .position(|(_, text, ..)| text.trim() == code)
        .or_else(|| {
            (width(code) >= 4)
                .then(|| lines.iter().position(|(_, text, ..)| text.contains(code)))
                .flatten()
        })
}

/// Where a test's case is in a hunk: the line naming it, through the line
/// that closes it (no deeper than it, starting with a closing bracket),
/// twelve lines at most.
fn case_lines(lines: &[HunkLine], test: &str) -> Option<(usize, usize)> {
    let name = test
        .trim()
        .trim_matches(|c| c == '"' || c == '\'' || c == '`');
    if name.is_empty() {
        return None;
    }
    let start = lines
        .iter()
        .position(|(mark, text, ..)| *mark != '-' && text.contains(name))?;
    let depth = |t: &str| t.len() - t.trim_start().len();
    let first = depth(&lines[start].1);
    let mut end = start;
    for (i, (mark, text, ..)) in lines.iter().enumerate().skip(start + 1).take(11) {
        if *mark == '-' {
            continue;
        }
        end = i;
        let t = text.trim_start();
        if depth(text) <= first && (t.starts_with('}') || t.starts_with(')') || t == "end") {
            break;
        }
    }
    Some((start, end))
}

impl App {
    /// Whether the Talk view is on the screen.
    pub(super) fn on_talk(&self) -> bool {
        self.mapped() && self.opts.talk && self.mode == Mode::Talk
    }

    /// The page the talk is on: the slide's step, else the step chosen on
    /// the flow, else the first slide's.
    pub(super) fn talk_page(&self) -> usize {
        let Some(story) = self.told_map() else {
            return 0;
        };
        let slides = slides(story);
        match self.talker.slide {
            0 if self.talker.at > 0 => self.talker.at,
            0 => slides.first().copied().unwrap_or(0),
            k => slides.get(k - 1).copied().unwrap_or(0),
        }
    }

    /// The story's example kept only where its test is in the change: its
    /// name on a line its file adds or keeps in one of its hunks, or its
    /// file one of tests (an edited test's name may sit past its hunk's
    /// context). One in no such file is none, so the talk never follows
    /// an example made up from the code.
    pub(super) fn check_example(&self, story: &mut Story) {
        let Some(example) = &story.example else {
            return;
        };
        let (files, hunks) = self
            .change
            .as_ref()
            .map(|c| story::split(&c.told_part().runs()))
            .unwrap_or_default();
        let text = files.get(&example.file).map(Vec::as_slice).unwrap_or(&[]);
        let found = hunks.get(&example.file).is_some_and(|list| {
            list.iter()
                .any(|h| case_lines(&reading::hunk_lines(text, h), &example.test).is_some())
        });
        let tests = crate::order::kind(&example.file) == crate::order::Kind::Tests;
        if !found && !tests {
            story.example = None;
        }
    }

    /// `g t`, and Talk in the switch and the menu: the talk where it was
    /// left, its gist the first time.
    pub(super) fn open_talk_view(&mut self) {
        if !self.mapped() {
            return self.no_map();
        }
        if !self.opts.talk {
            return self.say(
                "No talk for this diff: Settings' \"Open the diff as a talk\" (diff_story_talk) is off.",
            );
        }
        if self.mode == Mode::Files {
            self.files_place = Some(self.place());
        }
        self.mode = Mode::Talk;
        self.keep_mode();
        self.filtering = false;
        self.path_filter.clear();
        self.list_visual = None;
        self.visual = None;
        self.focus = Focus::List;
        if self.alone.take().is_some() {
            self.whole_at = None;
            self.lines.clear();
            self.sort();
            self.show_at(0);
        }
    }

    /// The diff split for its hunks, once a story: its colors asked for
    /// as it is.
    fn talk_split(&mut self) -> Arc<(story::Files, story::Hunks)> {
        if let Some(split) = &self.talker.split {
            return split.clone();
        }
        let split = Arc::new(
            self.change
                .as_ref()
                .map(|c| story::split(&c.told_part().runs()))
                .unwrap_or_default(),
        );
        self.talker.split = Some(split.clone());
        self.ask_hunk_colors();
        split
    }

    // ── Moving ──────────────────────────────────────────────────────────────

    /// The talk at a zoom: Steps and Code on the slide shown, else the
    /// step chosen on the flow, else the first; Gist and Flow on the
    /// flow, the step of the slide left chosen on it.
    pub(super) fn set_zoom(&mut self, zoom: Zoom) {
        let Some(story) = self.told_map() else {
            return;
        };
        let slides = slides(story);
        let t = &mut self.talker;
        match zoom {
            Zoom::Gist | Zoom::Flow => {
                if t.slide > 0 {
                    t.at = slides.get(t.slide - 1).copied().unwrap_or(0);
                    t.slide = 0;
                    t.follow = true;
                }
            }
            Zoom::Steps | Zoom::Code if t.slide == 0 => {
                let i = slides.iter().position(|&n| n == t.at).unwrap_or(0);
                t.slide = if slides.is_empty() { 0 } else { i + 1 };
            }
            Zoom::Steps | Zoom::Code => {}
        }
        if t.zoom != zoom {
            t.top = 0;
        }
        t.zoom = if t.slide == 0 && matches!(zoom, Zoom::Steps | Zoom::Code) {
            // A story of no steps has nothing to zoom in on.
            Zoom::Flow
        } else {
            zoom
        };
        self.mark_slide();
    }

    /// `+` and `−`: a zoom in or out.
    fn zoom_step(&mut self, step: i32) {
        let i = ZOOMS
            .iter()
            .position(|(_, z)| *z == self.talker.zoom)
            .unwrap_or(0) as i32;
        let to = (i + step).clamp(0, ZOOMS.len() as i32 - 1) as usize;
        self.set_zoom(ZOOMS[to].1);
    }

    /// `→` and `←`: the next slide or the one before; from the gist, the
    /// first step at the Steps zoom, and back to it from the first.
    pub(super) fn slide_step(&mut self, step: i32) {
        let Some(story) = self.told_map() else {
            return;
        };
        let last = slides(story).len() as i32;
        let to = (self.talker.slide as i32 + step).clamp(0, last) as usize;
        self.go_to_slide(to);
    }

    fn go_to_slide(&mut self, to: usize) {
        let Some(story) = self.told_map() else {
            return;
        };
        let slides = slides(story);
        let t = &mut self.talker;
        if to == 0 {
            if t.slide > 0 {
                t.at = slides.get(t.slide - 1).copied().unwrap_or(0);
                t.follow = true;
            }
            t.slide = 0;
            if matches!(t.zoom, Zoom::Steps | Zoom::Code) {
                t.zoom = Zoom::Gist;
            }
        } else {
            t.slide = to.min(slides.len());
            t.at = slides.get(t.slide.wrapping_sub(1)).copied().unwrap_or(0);
            if matches!(t.zoom, Zoom::Gist | Zoom::Flow) {
                t.zoom = Zoom::Steps;
            }
        }
        t.top = 0;
        self.mark_slide();
    }

    /// The step shown on a slide is read, as the Story view counts it.
    fn mark_slide(&mut self) {
        if self.talker.slide > 0 {
            let n = self.talk_page();
            if n > 0 {
                self.mark_read(n);
            }
        }
    }

    /// `h` and `l` on the flow: the step before or after the chosen one,
    /// brought into sight.
    fn flow_step(&mut self, step: i32) {
        let Some(story) = self.told_map() else {
            return;
        };
        let boxes = flow(story);
        if boxes.is_empty() {
            return;
        }
        let i = boxes
            .iter()
            .position(|b| b.page == self.talker.at)
            .map_or(if step > 0 { -1 } else { 0 }, |i| i as i32);
        let to = (i + step).clamp(0, boxes.len() as i32 - 1) as usize;
        self.talker.at = boxes[to].page;
        self.talker.follow = true;
    }

    /// `j` and `k` in Flow: the next step down its list or the one up,
    /// past the pages that only test, which its Tested line names.
    fn list_step(&mut self, step: i32) {
        let Some(story) = self.told_map() else {
            return;
        };
        let steps: Vec<usize> = steps(story)
            .into_iter()
            .filter(|&n| !reading::only_tests(&story.pages[n - 1]))
            .collect();
        if steps.is_empty() {
            return;
        }
        let i = steps
            .iter()
            .position(|&n| n == self.talker.at)
            .map_or(if step > 0 { -1 } else { 0 }, |i| i as i32);
        let to = (i + step).clamp(0, steps.len() as i32 - 1) as usize;
        self.talker.at = steps[to];
        self.talker.follow = true;
    }

    /// Enter on a step: its slide, at the Steps zoom.
    fn open_step(&mut self, n: usize) {
        let Some(story) = self.told_map() else {
            return;
        };
        if let Some(i) = slides(story).iter().position(|&m| m == n) {
            self.talker.zoom = Zoom::Steps;
            self.go_to_slide(i + 1);
        }
    }

    fn talk_scroll(&mut self, by: isize) {
        let (_, total, _, h) = self.talker.shown;
        let last = total.saturating_sub(h.max(1));
        self.talker.top = self.talker.top.saturating_add_signed(by).min(last);
    }

    /// The flow moved sideways, Shift with the wheel, only at Gist and
    /// Flow and while it is wider than the pane; whether it moved.
    pub(super) fn talk_sideways(&mut self, right: bool) -> bool {
        let (flow, _, cols, _) = self.talker.shown;
        let last = flow.saturating_sub(cols);
        if last == 0 || !matches!(self.talker.zoom, Zoom::Gist | Zoom::Flow) {
            return false;
        }
        let t = &mut self.talker;
        t.follow = false;
        t.left = if right {
            (t.left + SIDEWAYS).min(last)
        } else {
            t.left.saturating_sub(SIDEWAYS)
        };
        true
    }

    /// Shift with the wheel, or a trackpad sideways: the talk's flow, the
    /// map or What runs moved, where one is shown.
    pub(super) fn scroll_sideways(&mut self, right: bool, (x, y): (u16, u16)) -> bool {
        if self.answer.is_some() || self.ask_box.is_some() || self.menu.is_some() {
            return false;
        }
        if self.on_talk() {
            return self.talk_sideways(right);
        } else if self.on_map() {
            self.map_sideways(right);
        } else if self.on_story() && self.runs_shown() {
            // Only over What runs, and only while it is wider than its pane.
            let area = self.diff_area();
            let over = x >= area.x && y >= area.y && y < area.y + reading::RUNS_ROWS;
            return over && self.runs_sideways(right);
        } else {
            return false;
        }
        true
    }

    pub(super) fn talk_wheel(&mut self, down: bool) {
        self.talk_scroll(if down { 3 } else { -3 });
    }

    /// The step's hunk `hunk` (of `own_hunks`), or the example's test
    /// with none, shown in the files.
    fn talk_show(&mut self, hunk: Option<usize>) {
        let Some(story) = self.told_map() else {
            return;
        };
        let target = if let Some(i) = hunk {
            let Some((path, hunk)) = own_hunks(story, self.talk_page()).into_iter().nth(i) else {
                return self.say("This step has no change of its own to show.");
            };
            Does::File {
                path,
                run: hunk.run,
            }
        } else {
            let Some(example) = &story.example else {
                return;
            };
            let run = story
                .hunks
                .get(&example.file)
                .and_then(|h| h.first())
                .map_or(1, |h| h.run);
            Does::File {
                path: example.file.clone(),
                run,
            }
        };
        self.reader.target = Some(target);
        self.show_target();
    }

    // ── The keys and clicks ─────────────────────────────────────────────────

    /// A key in the Talk view; `None` for one the diff takes anywhere.
    pub(super) fn talk_key(&mut self, key: KeyEvent) -> Option<Act> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        // A c or p typed after ] or [ from the habit of ]c and ]p adds
        // nothing (c would open the commits).
        let after_bracket = std::mem::take(&mut self.reader.after_bracket);
        if after_bracket && !ctrl && matches!(key.code, KeyCode::Char('c' | 'p')) {
            return Some(Act::Same);
        }
        let zoom = self.talker.zoom;
        let part = self.talker.part;
        let h = self.talker.shown.3.max(1) as isize;
        match key.code {
            KeyCode::Tab => {
                self.talker.part = match part {
                    Part::Content => Part::Zoom,
                    Part::Zoom => Part::Slides,
                    Part::Slides => Part::Content,
                };
            }
            KeyCode::BackTab => {
                self.talker.part = match part {
                    Part::Content => Part::Slides,
                    Part::Zoom => Part::Content,
                    Part::Slides => Part::Zoom,
                };
            }
            KeyCode::Esc if part != Part::Content => self.talker.part = Part::Content,
            KeyCode::Esc => return Some(Act::Quit),
            KeyCode::Char('-' | '−' | '_') if !ctrl => self.zoom_step(-1),
            KeyCode::Char('+' | '=') if !ctrl => self.zoom_step(1),
            // The zoom's switch, with Tab's focus: across it.
            KeyCode::Char('h') | KeyCode::Left if part == Part::Zoom && !ctrl => self.zoom_step(-1),
            KeyCode::Char('l') | KeyCode::Right if part == Part::Zoom && !ctrl => self.zoom_step(1),
            KeyCode::Char('[') if !ctrl => {
                self.reader.after_bracket = true;
                self.slide_step(-1);
            }
            KeyCode::Char(']') if !ctrl => {
                self.reader.after_bracket = true;
                self.slide_step(1);
            }
            KeyCode::Left if !ctrl => self.slide_step(-1),
            KeyCode::Right if !ctrl => self.slide_step(1),
            KeyCode::Char('t') if !ctrl => match self.told_map().map(|s| s.example.is_some()) {
                Some(true) => self.talk_show(None),
                _ => self.say("This talk has no example, so no test to show."),
            },
            KeyCode::Char('h') if part == Part::Slides && !ctrl => self.slide_step(-1),
            KeyCode::Char('l') if part == Part::Slides && !ctrl => self.slide_step(1),
            // Along the flow at Gist and Flow, a slide at a time beyond.
            KeyCode::Char('h') if !ctrl && matches!(zoom, Zoom::Gist | Zoom::Flow) => {
                self.flow_step(-1)
            }
            KeyCode::Char('l') if !ctrl && matches!(zoom, Zoom::Gist | Zoom::Flow) => {
                self.flow_step(1)
            }
            KeyCode::Char('h') if !ctrl => self.slide_step(-1),
            KeyCode::Char('l') if !ctrl => self.slide_step(1),
            KeyCode::Char('j') | KeyCode::Down if !ctrl && zoom == Zoom::Flow => self.list_step(1),
            KeyCode::Char('k') | KeyCode::Up if !ctrl && zoom == Zoom::Flow => self.list_step(-1),
            KeyCode::Char('j') | KeyCode::Down if !ctrl => self.talk_scroll(1),
            KeyCode::Char('k') | KeyCode::Up if !ctrl => self.talk_scroll(-1),
            KeyCode::Char('J') => self.talk_scroll(3),
            KeyCode::Char('K') => self.talk_scroll(-3),
            KeyCode::Char('d') if ctrl => self.talk_scroll(h / 2),
            KeyCode::Char('u') if ctrl => self.talk_scroll(-(h / 2)),
            KeyCode::PageDown => self.talk_scroll(h),
            KeyCode::PageUp => self.talk_scroll(-h),
            KeyCode::Char('G') | KeyCode::End => self.talk_scroll(isize::MAX / 2),
            KeyCode::Home => self.talk_scroll(isize::MIN / 2),
            KeyCode::Char('n') if !ctrl && zoom == Zoom::Steps => {
                self.talker.notes = !self.talker.notes;
            }
            KeyCode::Enter | KeyCode::Char('o' | ' ') if !ctrl && part == Part::Slides => {
                self.talker.part = Part::Content;
            }
            KeyCode::Enter | KeyCode::Char('o' | ' ') if !ctrl && part == Part::Zoom => {
                self.talker.part = Part::Content;
            }
            KeyCode::Enter | KeyCode::Char('o' | ' ')
                if !ctrl && matches!(zoom, Zoom::Gist | Zoom::Flow) =>
            {
                let n = match self.talker.at {
                    0 => self.talk_page(),
                    n => n,
                };
                self.open_step(n);
            }
            KeyCode::Enter | KeyCode::Char('o') if !ctrl => {
                match (zoom, self.told_map()) {
                    (Zoom::Steps, Some(story)) => {
                        let shown = shown_hunk(story, self.talk_page());
                        self.talk_show(Some(shown));
                    }
                    // At Code, the file of the hunk or test in view.
                    _ => {
                        let (top, h) = (self.talker.top, self.talker.shown.3);
                        let heads = &self.talker.heads;
                        let head = heads
                            .iter()
                            .rev()
                            .find(|(row, _)| *row <= top + h / 2)
                            .or(heads.first())
                            .map(|(_, hit)| *hit);
                        match head {
                            Some(TalkHit::Test) => self.talk_show(None),
                            Some(TalkHit::File(i)) => self.talk_show(Some(i)),
                            _ => self.talk_show(Some(0)),
                        }
                    }
                }
            }
            KeyCode::Char('m') if key.modifiers.is_empty() => self.open_menu(None),
            KeyCode::Char('q' | 'y' | 'g' | 'R' | 'c' | 'E') if !ctrl => return None,
            KeyCode::Char('w') if ctrl => return None,
            KeyCode::F(10) => return None,
            _ => return Some(Act::Same),
        }
        Some(Act::Redraw)
    }

    /// A click on something of the Talk view.
    pub(super) fn talk_press(&mut self, hit: TalkHit) -> Act {
        match hit {
            TalkHit::Zoom(z) => {
                self.talker.part = Part::Zoom;
                self.set_zoom(z);
            }
            TalkHit::Out => self.zoom_step(-1),
            TalkHit::In => self.zoom_step(1),
            TalkHit::Dot(i) => {
                self.talker.part = Part::Slides;
                self.go_to_slide(i);
            }
            // A step opens as the mouse is let go, once chosen.
            TalkHit::Step(n) if self.talker.at == n && self.talker.slide == 0 => {
                self.open_on_up = Some(Hit::Talk(hit));
            }
            // On a slide, a step of its flow opens that step's slide.
            TalkHit::Step(n) if self.talker.slide > 0 => {
                self.talker.part = Part::Content;
                self.open_step(n);
            }
            TalkHit::Step(n) => {
                self.talker.part = Part::Content;
                self.talker.at = n;
                self.talker.follow = true;
            }
            TalkHit::Notes => self.talker.notes = !self.talker.notes,
            TalkHit::Test | TalkHit::File(_) => self.open_on_up = Some(Hit::Talk(hit)),
        }
        Act::Redraw
    }

    /// The mouse let go where it was pressed.
    pub(super) fn talk_release(&mut self, hit: TalkHit) {
        match hit {
            TalkHit::Step(n) => self.open_step(n),
            TalkHit::Test => self.talk_show(None),
            TalkHit::File(i) => self.talk_show(Some(i)),
            _ => {}
        }
    }

    // ── Drawing ─────────────────────────────────────────────────────────────

    /// The Talk view across the diff's whole width: the zoom's row over a
    /// rule, what the zoom shows, and the slides' dots at the foot.
    pub(super) fn draw_talk(&mut self, buf: &mut Buffer) {
        let theme = self.theme();
        let area = self.diff_area();
        let Some(story) = self.told_map().cloned() else {
            return;
        };
        let slides = slides(&story);
        if self.talker.slide > slides.len() {
            self.talker.slide = slides.len();
        }
        let (x0, cols) = (area.x + 1, area.width.saturating_sub(2) as usize);
        self.draw_zoom_row(buf, &story, (x0, area.y, cols), &theme);
        put_line(
            buf,
            area.x,
            area.y + 1,
            area.width,
            &Spans::from(kit::rule(area.width as usize, &theme)),
        );
        let h = (area.height as usize).saturating_sub(4);
        let side = self.talker.zoom == Zoom::Steps && self.talker.slide > 0 && cols >= SIDE_FROM;
        let (cx, ccols) = if side {
            (x0 + SIDE as u16 + 2, cols.saturating_sub(SIDE + 2))
        } else {
            (x0, cols)
        };
        let split = self.talk_split();
        let laid = self.lay_talk(&story, &split, ccols, &theme);
        // The chosen step brought into sight, along the flow and down.
        if std::mem::take(&mut self.talker.follow) {
            if let Some((x, w)) = laid.chosen_x {
                let t = &mut self.talker;
                if x < t.left {
                    t.left = x;
                } else if x + w > t.left + ccols {
                    t.left = (x + w).saturating_sub(ccols);
                }
            }
            if let Some((from, to)) = laid.chosen_rows {
                let t = &mut self.talker;
                if from < t.top {
                    t.top = from;
                } else if to >= t.top + h {
                    t.top = (to + 1).saturating_sub(h);
                }
            }
        }
        let t = &mut self.talker;
        t.left = t.left.min(laid.flow_width.saturating_sub(ccols));
        t.top = t.top.min(laid.rows.len().saturating_sub(h.max(1)));
        t.shown = (laid.flow_width, laid.rows.len(), ccols, h);
        t.heads = laid
            .rows
            .iter()
            .enumerate()
            .filter_map(|(i, r)| match r.hits.first() {
                Some((hit @ (TalkHit::File(_) | TalkHit::Test), ..)) => Some((i, *hit)),
                _ => None,
            })
            .collect();
        let (top, left) = (t.top, t.left);
        let y0 = area.y + 2;
        if side {
            let sx = x0;
            for y in y0..y0 + h as u16 {
                buf.set_style(
                    Rect::new(sx.saturating_sub(1), y, SIDE as u16 + 1, 1),
                    Style::new().bg(Surface::Raised.color(&theme)),
                );
                put_line(
                    buf,
                    sx + SIDE as u16,
                    y,
                    1,
                    &Spans::from(kit::rule_v(&theme)),
                );
            }
            for (i, row) in laid.side.iter().take(h).enumerate() {
                let y = y0 + i as u16;
                if row.chosen {
                    buf.set_style(
                        Rect::new(sx.saturating_sub(1), y, SIDE as u16 + 1, 1),
                        Style::new().bg(Surface::Chosen.color(&theme)),
                    );
                    put_line(
                        buf,
                        sx.saturating_sub(1),
                        y,
                        1,
                        &Spans::from(Span::styled("▌", Tone::Accent.style(&theme))),
                    );
                }
                put_line(buf, sx, y, SIDE as u16, &Spans::from(row.spans.clone()));
                for (hit, hx, hw) in &row.hits {
                    self.hits.push((
                        Hit::Talk(*hit),
                        Rect::new(sx + *hx as u16, y, (*hw).min(SIDE) as u16, 1),
                    ));
                }
            }
        }
        for (i, row) in laid.rows.iter().enumerate().skip(top).take(h) {
            let y = y0 + (i - top) as u16;
            let line = Rect::new(cx, y, ccols as u16, 1);
            let lit = row
                .hits
                .first()
                .is_some_and(|(hit, ..)| self.lit == Some(Hit::Talk(*hit)));
            if row.chosen {
                buf.set_style(line, Style::new().bg(Surface::Chosen.color(&theme)));
            } else if lit && !row.wide {
                buf.set_style(line, Style::new().bg(Surface::Raised.color(&theme)));
            }
            if row.bar {
                if kit::scroll_bars() && laid.flow_width > ccols {
                    let bar = sideways_bar(laid.flow_width, left, ccols, &theme);
                    put_line(buf, cx, y, ccols as u16, &Spans::from(bar));
                }
                continue;
            }
            let (spans, shift) = if row.wide {
                (columns_of(&row.spans, left, ccols), left)
            } else {
                (row.spans.clone(), 0)
            };
            put_line(buf, cx, y, ccols as u16, &Spans::from(spans));
            for (hit, hx, hw) in &row.hits {
                let (from, to) = (hx.max(&shift) - shift, (hx + hw).saturating_sub(shift));
                let to = to.min(ccols);
                if from < to {
                    self.hits.push((
                        Hit::Talk(*hit),
                        Rect::new(cx + from as u16, y, (to - from) as u16, 1),
                    ));
                }
            }
            self.spot(
                Pane::Head,
                Rect::new(cx, y, ccols as u16, 1),
                cx,
                i64::from(y),
                false,
            );
        }
        self.draw_dots(
            buf,
            slides.len(),
            (x0, area.bottom().saturating_sub(1), cols),
            &theme,
        );
    }

    /// The zoom's row: `−`, the switch of Gist, Flow, Steps and Code, `+`,
    /// and at the right which slide or how many steps.
    fn draw_zoom_row(
        &mut self,
        buf: &mut Buffer,
        story: &Story,
        (x0, y, cols): (u16, u16, usize),
        theme: &Theme,
    ) {
        let focused = self.talker.part == Part::Zoom;
        let mut x = x0;
        let label = Span::styled(
            "Zoom ",
            if focused {
                Tone::Accent.bold(theme)
            } else {
                Tone::Dim.style(theme)
            },
        );
        put_line(buf, x, y, 5, &Spans::from(label));
        x += 5;
        let sign = |app: &mut App, buf: &mut Buffer, x: u16, text: &str, hit: TalkHit| {
            let style = if app.lit == Some(Hit::Talk(hit)) {
                Style::new().fg(theme.bright).add_modifier(Modifier::BOLD)
            } else {
                Tone::Accent.bold(theme)
            };
            put_line(
                buf,
                x,
                y,
                1,
                &Spans::from(Span::styled(text.to_string(), style)),
            );
            app.hits.push((Hit::Talk(hit), Rect::new(x, y, 1, 1)));
        };
        sign(self, buf, x, "−", TalkHit::Out);
        x += 2;
        let parts: Vec<&str> = ZOOMS.iter().map(|(l, _)| *l).collect();
        let chosen = ZOOMS.iter().position(|(_, z)| *z == self.talker.zoom);
        let lit = ZOOMS
            .iter()
            .position(|(_, z)| self.lit == Some(Hit::Talk(TalkHit::Zoom(*z))));
        let (spans, places) = kit::choice_switch(&parts, chosen, lit, &[], false, theme);
        let w = kit::spans_width(&spans) as u16;
        put_line(buf, x, y, w, &Spans::from(spans));
        for ((_, z), (at, pw)) in ZOOMS.iter().zip(places) {
            self.hits.push((
                Hit::Talk(TalkHit::Zoom(*z)),
                Rect::new(x + at as u16, y, pw as u16, 1),
            ));
        }
        x += w + 1;
        sign(self, buf, x, "+", TalkHit::In);
        let (k, of) = (self.talker.slide + 1, slides(story).len() + 1);
        let right = match self.talker.zoom {
            Zoom::Flow => counted(steps(story).len(), "step"),
            _ => format!("slide {k} of {of}"),
        };
        let rx = (x0 as usize + cols).saturating_sub(width(&right)) as u16;
        if rx > x + 2 {
            put_line(
                buf,
                rx,
                y,
                width(&right) as u16,
                &Spans::from(Span::styled(right, Tone::Dim.style(theme))),
            );
        }
    }

    /// The slides' dots at the foot, the one shown lit, each a click.
    fn draw_dots(
        &mut self,
        buf: &mut Buffer,
        n: usize,
        (x0, y, cols): (u16, u16, usize),
        theme: &Theme,
    ) {
        let focused = self.talker.part == Part::Slides;
        let mut x = x0;
        let label = Span::styled(
            "Slides ",
            if focused {
                Tone::Accent.bold(theme)
            } else {
                Tone::Dim.style(theme)
            },
        );
        put_line(buf, x, y, 7, &Spans::from(label));
        x += 7;
        for i in 0..=n {
            if (x - x0) as usize + 2 > cols {
                break;
            }
            let on = i == self.talker.slide;
            let lit = self.lit == Some(Hit::Talk(TalkHit::Dot(i)));
            let style = if on {
                Tone::Accent.bold(theme)
            } else if lit {
                Style::new().fg(theme.bright)
            } else {
                Tone::Faint.style(theme)
            };
            put_line(
                buf,
                x,
                y,
                1,
                &Spans::from(Span::styled(if on { "●" } else { "○" }, style)),
            );
            self.hits
                .push((Hit::Talk(TalkHit::Dot(i)), Rect::new(x, y, 1, 1)));
            x += 2;
        }
    }

    /// What the zoom shows, laid out for `cols` columns.
    fn lay_talk(
        &self,
        story: &Story,
        split: &(story::Files, story::Hunks),
        cols: usize,
        theme: &Theme,
    ) -> Laid {
        let slide = self.talker.slide;
        let page = self.talk_page();
        match self.talker.zoom {
            Zoom::Gist => self.lay_gist(story, cols, theme),
            Zoom::Flow => self.lay_flow(story, cols, theme),
            Zoom::Steps if slide > 0 => self.lay_step(story, split, page, cols, theme),
            Zoom::Code if slide > 0 => self.lay_code(story, split, page, cols, theme),
            _ => self.lay_gist(story, cols, theme),
        }
    }

    /// The flow drawn left to right: a box a step, its name and its
    /// number's chip with whether it is new or changed, lit where the
    /// example passes, the chosen one on the chosen surface; an arrow
    /// between two, the example's value over it. Four rows that scroll
    /// sideways and the scroll bar's under them; its width and the chosen
    /// box's columns.
    fn flow_rows(
        &self,
        story: &Story,
        theme: &Theme,
    ) -> (Vec<TRow>, usize, Option<(usize, usize)>) {
        let boxes = flow(story);
        let mut rows: Vec<TRow> = (0..4)
            .map(|_| TRow {
                wide: true,
                ..TRow::default()
            })
            .collect();
        let mut x = 0;
        let mut chosen_x = None;
        for (i, b) in boxes.iter().enumerate() {
            let lit = passes(story, b.page);
            let chosen = self.talker.at == b.page;
            let hover = self.lit == Some(Hit::Talk(TalkHit::Step(b.page)));
            let (k, _) = step_number(story, b.page);
            let mut chips = kit::chip(
                &format!("step {k}"),
                if lit { Tone::Accent } else { Tone::Dim },
                false,
                theme,
            );
            chips.extend(kit::chip(
                if is_new(story, b.page) {
                    "new"
                } else {
                    "changed"
                },
                if is_new(story, b.page) {
                    Tone::Done
                } else {
                    Tone::Working
                },
                false,
                theme,
            ));
            let name = cut(&b.name, 32);
            let inner = (width(&name) + 2).max(kit::spans_width(&chips) + 1);
            let w = inner + 2;
            let edge = if chosen || hover {
                Style::new().fg(theme.bright)
            } else if lit {
                Tone::Accent.style(theme)
            } else {
                Style::new().fg(kit::border(theme))
            };
            let fill = if chosen {
                Style::new().bg(Surface::Chosen.color(theme))
            } else {
                Style::new()
            };
            let name_style = if lit || chosen {
                Tone::Text.bold(theme)
            } else {
                Tone::Dim.bold(theme)
            };
            rows[0]
                .spans
                .push(Span::styled(format!("╭{}╮", "─".repeat(inner)), edge));
            rows[1].spans.extend([
                Span::styled("│", edge),
                Span::styled(
                    format!(" {name}{}", " ".repeat(inner - width(&name) - 1)),
                    name_style.patch(fill),
                ),
                Span::styled("│", edge),
            ]);
            let chips_w = kit::spans_width(&chips);
            rows[2].spans.push(Span::styled("│ ", edge));
            rows[2].spans.extend(chips);
            rows[2].spans.push(Span::styled(
                format!("{}│", " ".repeat(inner - 1 - chips_w)),
                edge,
            ));
            rows[3]
                .spans
                .push(Span::styled(format!("╰{}╯", "─".repeat(inner)), edge));
            for row in rows.iter_mut() {
                row.hits.push((TalkHit::Step(b.page), x, w));
            }
            if chosen {
                chosen_x = Some((x, w));
            }
            x += w;
            if i + 1 < boxes.len() {
                let value = if story.example.is_some() {
                    story.pages[b.page - 1].value.clone()
                } else {
                    String::new()
                };
                let aw = (width(&value) + 4).max(6);
                let on = lit && passes(story, boxes[i + 1].page);
                let line = if on {
                    Tone::Accent.style(theme)
                } else {
                    Tone::Faint.style(theme)
                };
                rows[0].spans.push(Span::styled(
                    kit::centered(&value, aw),
                    Tone::Accent.bold(theme),
                ));
                rows[1]
                    .spans
                    .push(Span::styled(format!(" {}▶ ", "─".repeat(aw - 3)), line));
                rows[2].spans.push(Span::raw(" ".repeat(aw)));
                rows[3].spans.push(Span::raw(" ".repeat(aw)));
                x += aw;
            }
        }
        rows.push(TRow {
            bar: true,
            ..TRow::default()
        });
        (rows, x, chosen_x)
    }

    /// What needs no reading, folded to one line.
    fn folded_rows(&self, story: &Story, cols: usize, theme: &Theme) -> Vec<TRow> {
        if story.skip.is_empty() {
            return Vec::new();
        }
        labeled(
            Span::styled("Folded, needing no reading  ", Tone::Dim.bold(theme)),
            &story.skip.join(" · "),
            Tone::Dim.style(theme),
            cols.min(PROSE * 2),
            0,
        )
        .into_iter()
        .map(TRow::of)
        .collect()
    }

    /// The gist, as boards 159 and 163 draw it.
    fn lay_gist(&self, story: &Story, cols: usize, theme: &Theme) -> Laid {
        let prose = cols.min(PROSE);
        let mut rows: Vec<TRow> = Vec::new();
        let blank = || TRow::default();
        rows.push(TRow::of(vec![Span::styled(
            "THE GIST",
            Tone::Accent.bold(theme),
        )]));
        let gist = if story.gist.is_empty() {
            reading::first_sentence(&story.after)
        } else {
            story.gist.clone()
        };
        for line in wrap(&gist, prose) {
            rows.push(TRow::of(vec![Span::styled(line, Tone::Text.bold(theme))]));
        }
        let n = steps(story).len();
        rows.push(TRow::of(vec![Span::styled(
            format!(
                "{} · +{} −{} in {}",
                counted(n, "step"),
                self.totals.added,
                self.totals.deleted,
                counted(self.totals.files, "file")
            ),
            Tone::Dim.style(theme),
        )]));
        rows.push(blank());
        match &story.example {
            Some(example) => {
                rows.push(TRow::doing(
                    kit::fit(
                        vec![
                            Span::styled("ONE EXAMPLE", Tone::Dim.bold(theme)),
                            Span::styled("   from ", Tone::Dim.style(theme)),
                            Span::styled(example.file.clone(), Tone::Accent.style(theme)),
                            Span::styled(
                                format!(" · \"{}\"", example.test),
                                Tone::Dim.style(theme),
                            ),
                        ],
                        cols,
                        Style::new(),
                    ),
                    TalkHit::Test,
                ));
                // The two cards as a pair: side by side while each holds
                // `CARD` columns, one over the other below that.
                let pair = cols.min(PROSE * 2 + 5);
                for spans in
                    reading::sides_boxes(&example.input, &example.output, pair, CARD * 2 + 5, theme)
                {
                    rows.push(TRow::of(spans));
                }
            }
            None => {
                let sunken = Surface::Sunken.color(theme);
                let panel = |spans: Vec<Span<'static>>| {
                    let mut spans = kit::fit(spans, cols, Style::new().bg(sunken));
                    for s in &mut spans {
                        s.style = s.style.bg(sunken);
                    }
                    TRow::of(spans)
                };
                rows.push(panel(vec![Span::styled(
                    " NO EXAMPLE",
                    Tone::Dim.bold(theme),
                )]));
                for line in wrap(
                    "No test in this change sets an input and what it expects, so the talk shows no example rather than make one up.",
                    prose.saturating_sub(2),
                ) {
                    rows.push(panel(vec![Span::styled(
                        format!(" {line}"),
                        Tone::Dim.style(theme),
                    )]));
                }
            }
        }
        rows.push(blank());
        let mut how = vec![Span::styled("HOW IT GETS THERE", Tone::Dim.bold(theme))];
        if let Some(e) = &story.example {
            let name = if e.name.is_empty() {
                "the example"
            } else {
                &e.name
            };
            how.push(Span::styled(
                format!("   following {name}"),
                Tone::Dim.style(theme),
            ));
        }
        rows.push(TRow::of(kit::fit(how, cols, Style::new())));
        rows.push(blank());
        let (flow_rows, flow_width, chosen_x) = self.flow_rows(story, theme);
        rows.extend(flow_rows);
        // The steps off the example's path, named apart.
        if story.example.is_some() {
            let off: Vec<String> = story
                .runs
                .iter()
                .filter(|r| steps(story).contains(&r.page) && !passes(story, r.page))
                .map(|r| {
                    let (k, _) = step_number(story, r.page);
                    format!("{} (step {k}, {})", r.step, story.pages[r.page - 1].title)
                })
                .collect();
            if !off.is_empty() {
                for spans in labeled(
                    Span::styled("Off this example's path: ", Tone::Dim.style(theme)),
                    &off.join(" · "),
                    Tone::Accent.style(theme),
                    cols.min(PROSE * 2),
                    0,
                ) {
                    rows.push(TRow::of(spans));
                }
            }
        }
        rows.extend(self.folded_rows(story, cols, theme));
        rows.push(blank());
        let first = slides(story).first().map(|&n| step_number(story, n).0);
        let mut hint = Vec::new();
        if let Some(k) = first {
            hint.extend([
                Span::styled("→", Tone::Accent.bold(theme)),
                Span::styled(
                    format!(" starts the talk at step {k}   "),
                    Tone::Faint.style(theme),
                ),
            ]);
        }
        hint.extend([
            Span::styled("+", Tone::Accent.bold(theme)),
            Span::styled(" zooms in to the flow   ", Tone::Faint.style(theme)),
            Span::styled("Enter", Tone::Accent.bold(theme)),
            Span::styled(" on a step opens its slide", Tone::Faint.style(theme)),
        ]);
        rows.push(TRow::of(kit::fit(hint, cols, Style::new())));
        Laid {
            rows,
            flow_width,
            chosen_x,
            ..Laid::default()
        }
    }

    /// The flow over each step, as board 160 draws it.
    fn lay_flow(&self, story: &Story, cols: usize, theme: &Theme) -> Laid {
        let prose = cols.min(PROSE);
        let mut rows: Vec<TRow> = Vec::new();
        let mut head = vec![Span::styled("HOW IT GETS THERE", Tone::Dim.bold(theme))];
        if let Some(e) = &story.example {
            let name = if e.name.is_empty() {
                "the example"
            } else {
                &e.name
            };
            head.push(Span::styled(
                format!("   following {name}, from {}", e.file),
                Tone::Dim.style(theme),
            ));
        }
        rows.push(TRow::of(kit::fit(head, cols, Style::new())));
        let (flow_rows, flow_width, chosen_x) = self.flow_rows(story, theme);
        rows.extend(flow_rows);
        rows.push(TRow::of(vec![kit::rule(cols, theme)]));
        let name = story.example.as_ref().map(|e| {
            if e.name.is_empty() {
                "the example".to_string()
            } else {
                e.name.clone()
            }
        });
        let mut chosen_rows = None;
        let mut tested: Vec<(String, usize)> = Vec::new();
        for n in steps(story) {
            let Some(page) = story.pages.get(n - 1) else {
                continue;
            };
            let (k, _) = step_number(story, n);
            if reading::only_tests(page) {
                tested.push((reading::tested_name(&page.title), k));
                continue;
            }
            let from = rows.len();
            let chosen = self.talker.at == n;
            let mut block: Vec<TRow> = vec![TRow::default()];
            let mut head = vec![
                Span::styled(format!("{k}  "), Tone::Accent.bold(theme)),
                Span::styled(page.title.clone(), Tone::Text.bold(theme)),
                Span::raw("  "),
            ];
            if !page.closely.is_empty() {
                head.extend(kit::chip(
                    &format!("⚑ read closely: {}", page.closely),
                    Tone::Needs,
                    false,
                    theme,
                ));
            }
            block.push(TRow::of(kit::fit(head, cols, Style::new())));
            let how = if page.how.is_empty() {
                &page.claim
            } else {
                &page.how
            };
            // The example's value beside How where the pane holds both,
            // else on a row of its own under Why.
            let value = name
                .as_ref()
                .filter(|_| passes(story, n) && !page.value.is_empty())
                .map(|name| {
                    vec![
                        Span::styled(format!("{name}  "), Tone::Faint.style(theme)),
                        Span::styled(page.value.clone(), Tone::Accent.style(theme)),
                    ]
                });
            let value_w = value.as_ref().map_or(0, |v| kit::spans_width(v));
            let beside = value.is_some() && cols >= 3 + prose + 2 + value_w;
            let mut texts: Vec<Vec<Span<'static>>> = Vec::new();
            for (label, text, tone) in [
                ("How  ", how.as_str(), Tone::Text),
                ("Why  ", page.why.as_str(), Tone::Dim),
            ] {
                if !text.is_empty() {
                    texts.extend(labeled(
                        Span::styled(label, Tone::Dim.bold(theme)),
                        text,
                        tone.style(theme),
                        prose,
                        3,
                    ));
                }
            }
            match value {
                Some(value) if beside => {
                    if texts.is_empty() {
                        texts.push(Vec::new());
                    }
                    let first = &mut texts[0];
                    let used = kit::spans_width(first);
                    first.push(Span::raw(" ".repeat((3 + prose + 2).saturating_sub(used))));
                    first.extend(value);
                }
                Some(value) => {
                    let mut row = vec![
                        Span::raw("   "),
                        Span::styled("Ex   ", Tone::Dim.bold(theme)),
                    ];
                    row.extend(value);
                    texts.push(kit::fit(row, cols, Style::new()));
                }
                None => {}
            }
            block.extend(texts.into_iter().map(TRow::of));
            for (i, row) in block.iter_mut().enumerate() {
                row.hits.push((TalkHit::Step(n), 0, cols));
                row.chosen = chosen && i > 0;
            }
            rows.extend(block);
            if chosen {
                chosen_rows = Some((from + 1, rows.len() - 1));
            }
        }
        rows.push(TRow::default());
        if !tested.is_empty() {
            rows.extend(
                reading::tested_rows(&tested, prose, theme)
                    .into_iter()
                    .map(TRow::of),
            );
        }
        rows.extend(self.folded_rows(story, cols, theme));
        Laid {
            rows,
            flow_width,
            chosen_x,
            chosen_rows,
            ..Laid::default()
        }
    }

    /// A Steps slide's flow down the left, this step lit, the example's
    /// value under each.
    fn side_rows(&self, story: &Story, n: usize, theme: &Theme) -> Vec<TRow> {
        let mut rows = vec![
            TRow::of(vec![Span::styled("THE FLOW", Tone::Dim.bold(theme))]),
            TRow::default(),
        ];
        let boxes = flow(story);
        for (i, b) in boxes.iter().enumerate() {
            let on = b.page == n;
            let (k, _) = step_number(story, b.page);
            rows.push(TRow {
                chosen: on,
                ..TRow::doing(
                    kit::fit(
                        vec![
                            Span::styled(
                                format!("{k}  "),
                                if on {
                                    Tone::Accent.bold(theme)
                                } else {
                                    Tone::Dim.bold(theme)
                                },
                            ),
                            Span::styled(
                                b.name.clone(),
                                if on {
                                    Tone::Text.bold(theme)
                                } else {
                                    Tone::Dim.style(theme)
                                },
                            ),
                        ],
                        SIDE,
                        Style::new(),
                    ),
                    TalkHit::Step(b.page),
                )
            });
            let value = story
                .pages
                .get(b.page - 1)
                .filter(|_| story.example.is_some())
                .map_or(String::new(), |p| p.value.clone());
            rows.push(TRow {
                chosen: on,
                ..TRow::of(vec![Span::styled(
                    format!("   {}", cut(&value, SIDE - 3)),
                    if on {
                        Tone::Accent.style(theme)
                    } else {
                        Tone::Faint.style(theme)
                    },
                )])
            });
            if i + 1 < boxes.len() {
                rows.push(TRow::of(vec![Span::styled(
                    "│",
                    Style::new().fg(kit::border(theme)),
                )]));
            }
        }
        rows
    }

    /// A step's slide's head: "Step k of N", whether it is new, and the
    /// chip of why it is read closely.
    fn step_head(
        &self,
        story: &Story,
        n: usize,
        cols: usize,
        title: bool,
        theme: &Theme,
    ) -> Vec<TRow> {
        let page = &story.pages[n - 1];
        let (k, of) = step_number(story, n);
        let mut head = vec![Span::styled(
            format!("Step {k} of {of}"),
            Tone::Accent.bold(theme),
        )];
        if title {
            head.push(Span::styled(
                format!("   {}", page.title),
                Tone::Text.bold(theme),
            ));
        } else {
            head.push(Span::styled(
                if is_new(story, n) {
                    "   new in this change"
                } else {
                    "   changed in this change"
                },
                Tone::Dim.style(theme),
            ));
        }
        let mut rows = Vec::new();
        if page.closely.is_empty() {
            rows.push(TRow::of(kit::fit(head, cols, Style::new())));
            return rows;
        }
        let chip = kit::chip(
            &format!("⚑ read closely: {}", page.closely),
            Tone::Needs,
            false,
            theme,
        );
        let (hw, cw) = (kit::spans_width(&head), kit::spans_width(&chip));
        if hw + 2 + cw <= cols {
            head.push(Span::raw(" ".repeat(cols - hw - cw)));
            head.extend(chip);
            rows.push(TRow::of(head));
        } else {
            rows.push(TRow::of(kit::fit(head, cols, Style::new())));
            rows.push(TRow::of(kit::fit(chip, cols, Style::new())));
        }
        rows
    }

    /// A hunk's rows: its lines numbered and marked on their surface, a
    /// line's margin note right of it (`◀` and the note) where the pane
    /// holds both, else on a row of its own under the line; the lines
    /// `lit` on the chosen surface.
    #[allow(clippy::too_many_arguments)]
    fn hunk_rows(
        &self,
        path: &str,
        lines: &[HunkLine],
        notes: &[(usize, String)],
        lit: Option<(usize, usize)>,
        window: (usize, usize),
        cols: usize,
        theme: &Theme,
    ) -> Vec<TRow> {
        let (removed, _, added, _) = self.palette();
        let colors = self
            .reader
            .colors
            .get(&reading::hunk_key(path, lines))
            .and_then(Option::as_ref);
        const LEAD: usize = 7;
        let longest = lines
            .iter()
            .map(|(_, t, ..)| width(&t.replace('\t', "    ")))
            .max()
            .unwrap_or(0)
            + LEAD;
        // The notes start two columns past the longest line, the code's
        // surface running to them.
        let block = longest.min(cols);
        let beside = |note: &str| block + 2 + 2 + width(note) <= cols;
        let mut rows = Vec::new();
        for (i, (mark, text, old, new)) in lines.iter().enumerate().take(window.1).skip(window.0) {
            if *mark == '\\' {
                rows.push(TRow::of(vec![Span::styled(
                    format!("      ⏎ {text}"),
                    Tone::Faint.style(theme),
                )]));
                continue;
            }
            let note = notes.iter().find(|(l, _)| *l == i).map(|(_, n)| n.as_str());
            let is_lit = lit.is_some_and(|(a, b)| (a..=b).contains(&i));
            let bg = if is_lit {
                Surface::Chosen.color(theme)
            } else {
                match mark {
                    '+' => added,
                    '-' => removed,
                    _ => Surface::Sunken.color(theme),
                }
            };
            let number = match mark {
                '-' => old,
                _ => new,
            }
            .map_or(String::new(), |l| l.to_string());
            let runs = colors
                .and_then(|c| c.get(i))
                .and_then(|(o, n)| if *mark == '-' { o.as_ref() } else { n.as_ref() });
            let (code, code_colors) = reading::with_colors(text, runs);
            let room = if note.is_some_and(beside) {
                block
            } else {
                cols
            };
            let mut from = 0;
            let pieces = reading::pieces(&code, room.saturating_sub(LEAD).max(1));
            let last = pieces.len() - 1;
            for (k, piece) in pieces.into_iter().enumerate() {
                let fill = Style::new().bg(bg);
                let mut spans = vec![
                    Span::styled(
                        format!("{:>4} ", if k == 0 { number.as_str() } else { "" }),
                        Tone::Faint.style(theme).bg(bg),
                    ),
                    Span::styled(
                        format!("{} ", if k == 0 { *mark } else { ' ' }),
                        match mark {
                            '+' => Tone::Done.style(theme),
                            '-' => Tone::Failed.style(theme),
                            _ => Tone::Faint.style(theme),
                        }
                        .bg(bg),
                    ),
                ];
                spans.extend(reading::colored_spans(
                    &piece,
                    &code_colors,
                    from,
                    Tone::Text.style(theme).bg(bg),
                ));
                from += piece.len();
                let used = kit::spans_width(&spans);
                let surface_to = if note.is_some_and(beside) {
                    block
                } else {
                    cols
                };
                spans.push(Span::styled(
                    " ".repeat(surface_to.saturating_sub(used)),
                    fill,
                ));
                if k == last
                    && let Some(note) = note.filter(|n| beside(n))
                {
                    spans.extend([
                        Span::raw("  "),
                        Span::styled("◀ ", Tone::Accent.bold(theme)),
                        Span::styled(note.to_string(), Tone::Dim.style(theme)),
                    ]);
                }
                rows.push(TRow::of(spans));
            }
            if let Some(note) = note.filter(|n| !beside(n)) {
                for (j, line) in wrap(note, cols.saturating_sub(LEAD + 2).max(10))
                    .into_iter()
                    .enumerate()
                {
                    rows.push(TRow::of(vec![
                        Span::raw(" ".repeat(LEAD)),
                        Span::styled(if j == 0 { "◀ " } else { "  " }, Tone::Accent.bold(theme)),
                        Span::styled(line, Tone::Dim.style(theme)),
                    ]));
                }
            }
        }
        rows
    }

    /// A hunk of the story's by its id, with its lines and the margin
    /// notes on them.
    fn hunk_with_notes(
        story: &Story,
        split: &(story::Files, story::Hunks),
        path: &str,
        id: &str,
    ) -> Option<Noted> {
        let (files, hunks) = split;
        let hunk = hunks.get(path)?.iter().find(|h| h.id == id)?.clone();
        let text = files.get(path).map(Vec::as_slice).unwrap_or(&[]);
        let lines = reading::hunk_lines(text, &hunk);
        let notes = story
            .margins
            .iter()
            .filter(|m| m.hunk == id)
            .filter_map(|m| Some((margin_line(&lines, &m.line)?, m.note.clone())))
            .collect();
        Some((lines, notes))
    }

    /// A file's header over its hunk, the step's `i`th: its path and how
    /// much it changes, a click showing that hunk's file in the files.
    fn file_head(&self, story: &Story, i: usize, path: &str, cols: usize, theme: &Theme) -> TRow {
        let entry = self.change.as_ref().and_then(|c| {
            c.told_part()
                .entries
                .iter()
                .find(|e| e.path == path)
                .cloned()
        });
        let (a, d) = entry
            .as_ref()
            .map_or((0, 0), |e| (e.additions, e.deletions));
        let status = match entry.map(|e| e.status) {
            Some('A') => "   new file",
            Some('D') => "   deleted",
            _ => "",
        };
        let _ = story;
        TRow::doing(
            kit::fit(
                vec![
                    Span::styled(path.to_string(), Tone::Text.bold(theme)),
                    Span::styled(format!("   +{a}"), Tone::Done.style(theme)),
                    Span::styled(format!(" −{d}"), Tone::Failed.style(theme)),
                    Span::styled(status, Tone::Dim.style(theme)),
                ],
                cols,
                Style::new(),
            ),
            TalkHit::File(i),
        )
    }

    /// Decided, Instead and Watch on their surface, as the Story view's
    /// column draws them.
    fn notes_rows(page: &story::Page, cols: usize, theme: &Theme) -> Vec<TRow> {
        let prose = cols.min(PROSE);
        let sunken = Surface::Sunken.color(theme);
        let mut rows = Vec::new();
        for (label, text, tone) in [
            ("Decided", &page.decided, Tone::Done),
            ("Instead", &page.instead, Tone::Dim),
            ("Watch", &page.watch, Tone::Needs),
        ] {
            if text.is_empty() {
                continue;
            }
            for mut spans in labeled(
                Span::styled(format!(" {label:<8} "), tone.bold(theme)),
                text,
                if label == "Instead" {
                    Tone::Dim.style(theme)
                } else {
                    Tone::Text.style(theme)
                },
                prose,
                0,
            ) {
                for s in &mut spans {
                    s.style = s.style.bg(sunken);
                }
                let used = kit::spans_width(&spans);
                spans.push(Span::styled(
                    " ".repeat(prose.saturating_sub(used)),
                    Style::new().bg(sunken),
                ));
                rows.push(TRow::of(spans));
            }
        }
        rows
    }

    /// The example's value going into step `n` and coming out: what the
    /// step before it on the path made of it, else its name.
    fn in_out(&self, story: &Story, n: usize, cols: usize, theme: &Theme) -> Vec<TRow> {
        let Some(example) = &story.example else {
            return Vec::new();
        };
        let page = &story.pages[n - 1];
        if !passes(story, n) || page.value.is_empty() {
            return Vec::new();
        }
        let path: Vec<usize> = slides(story)
            .into_iter()
            .filter(|&m| passes(story, m))
            .collect();
        let name = if example.name.is_empty() {
            "The example".to_string()
        } else {
            example.name.clone()
        };
        let input = path
            .iter()
            .position(|&m| m == n)
            .and_then(|i| i.checked_sub(1))
            .and_then(|i| story.pages.get(path[i] - 1))
            .map_or_else(
                || reading::first_sentence(&example.input),
                |p| p.value.clone(),
            );
        let raised = Surface::Raised.color(theme);
        let pad = " ".repeat(width(&name));
        let mut rows = Vec::new();
        for (lead, label, text, style) in [
            (name.as_str(), "   in  ", input, Tone::Text.style(theme)),
            (
                pad.as_str(),
                "   out ",
                page.value.clone(),
                Tone::Accent.bold(theme),
            ),
        ] {
            let mut spans = vec![
                Span::styled(format!(" {lead}"), Tone::Faint.style(theme)),
                Span::styled(label, Tone::Faint.style(theme)),
                Span::styled(text, style),
            ];
            spans = kit::fit(spans, cols.min(PROSE), Style::new());
            for s in &mut spans {
                s.style = s.style.bg(raised);
            }
            let used = kit::spans_width(&spans);
            spans.push(Span::styled(
                " ".repeat(cols.min(PROSE).saturating_sub(used)),
                Style::new().bg(raised),
            ));
            rows.push(TRow::of(spans));
        }
        rows
    }

    /// A step's slide, as board 161 draws it.
    fn lay_step(
        &self,
        story: &Story,
        split: &(story::Files, story::Hunks),
        n: usize,
        cols: usize,
        theme: &Theme,
    ) -> Laid {
        let Some(page) = story.pages.get(n.wrapping_sub(1)) else {
            return Laid::default();
        };
        let prose = cols.min(PROSE);
        let mut rows = self.step_head(story, n, cols, false, theme);
        rows.push(TRow::default());
        for line in wrap(&page.title, prose) {
            rows.push(TRow::of(vec![Span::styled(line, Tone::Text.bold(theme))]));
        }
        let claim = if page.claim.is_empty() {
            &page.why
        } else {
            &page.claim
        };
        for line in wrap(claim, prose) {
            rows.push(TRow::of(vec![Span::styled(line, Tone::Text.style(theme))]));
        }
        let in_out = self.in_out(story, n, cols, theme);
        if !in_out.is_empty() {
            rows.push(TRow::default());
            rows.extend(in_out);
        }
        // Its hunk: the one with a note in the margin, else its first.
        let hunks = own_hunks(story, n);
        let at = shown_hunk(story, n);
        if let Some((path, note)) = hunks.get(at)
            && let Some((lines, notes)) = Self::hunk_with_notes(story, split, path, &note.id)
        {
            rows.push(TRow::default());
            rows.push(self.file_head(story, at, path, cols, theme));
            rows.extend(self.hunk_rows(path, &lines, &notes, None, (0, lines.len()), cols, theme));
            if hunks.len() > 1 {
                rows.push(TRow::of(vec![Span::styled(
                    format!(
                        "      {} more of this step at the Code zoom",
                        counted(hunks.len() - 1, "hunk")
                    ),
                    Tone::Faint.style(theme),
                )]));
            }
        }
        let notes = Self::notes_rows(page, cols, theme);
        if !notes.is_empty() {
            rows.push(TRow::default());
            let (mark, rest) = if self.talker.notes {
                ("▾ Notes", "")
            } else {
                ("▸ Notes", "   Decided · Instead · Watch")
            };
            rows.push(TRow::doing(
                vec![
                    Span::styled(mark, Tone::Accent.bold(theme)),
                    Span::styled(rest, Tone::Dim.style(theme)),
                ],
                TalkHit::Notes,
            ));
            if self.talker.notes {
                rows.extend(notes);
            }
        }
        if let Some(example) = &story.example {
            rows.push(TRow::default());
            rows.push(TRow::doing(
                kit::fit(
                    vec![
                        Span::styled("Its test: ", Tone::Dim.style(theme)),
                        Span::styled(example.file.clone(), Tone::Accent.style(theme)),
                        Span::styled(
                            format!(" · \"{}\"   ▸", example.test),
                            Tone::Dim.style(theme),
                        ),
                    ],
                    cols,
                    Style::new(),
                ),
                TalkHit::Test,
            ));
        }
        Laid {
            rows,
            side: self.side_rows(story, n, theme),
            ..Laid::default()
        }
    }

    /// A step's code, as board 162 draws it: its notes open, every hunk
    /// of it with its margin notes, and the test the example comes from
    /// with its case lit.
    fn lay_code(
        &self,
        story: &Story,
        split: &(story::Files, story::Hunks),
        n: usize,
        cols: usize,
        theme: &Theme,
    ) -> Laid {
        let Some(page) = story.pages.get(n.wrapping_sub(1)) else {
            return Laid::default();
        };
        let mut rows = self.step_head(story, n, cols, true, theme);
        let notes = Self::notes_rows(page, cols, theme);
        if !notes.is_empty() {
            rows.push(TRow::default());
            rows.extend(notes);
        }
        let mut last_path = String::new();
        for (i, (path, note)) in own_hunks(story, n).into_iter().enumerate() {
            let Some((lines, notes)) = Self::hunk_with_notes(story, split, &path, &note.id) else {
                continue;
            };
            rows.push(TRow::default());
            if path != last_path {
                rows.push(self.file_head(story, i, &path, cols, theme));
                last_path = path.clone();
            }
            rows.extend(self.hunk_rows(&path, &lines, &notes, None, (0, lines.len()), cols, theme));
        }
        if let Some(example) = &story.example {
            rows.push(TRow::default());
            let (files, hunks) = split;
            let text = files.get(&example.file).map(Vec::as_slice).unwrap_or(&[]);
            let found = hunks.get(&example.file).and_then(|list| {
                list.iter().find_map(|h| {
                    let lines = reading::hunk_lines(text, h);
                    let case = case_lines(&lines, &example.test)?;
                    Some((lines, case))
                })
            });
            let entry = self.change.as_ref().and_then(|c| {
                c.told_part()
                    .entries
                    .iter()
                    .find(|e| e.path == example.file)
                    .cloned()
            });
            let (a, d) = entry.map_or((0, 0), |e| (e.additions, e.deletions));
            rows.push(TRow::doing(
                kit::fit(
                    vec![
                        Span::styled(example.file.clone(), Tone::Text.bold(theme)),
                        Span::styled(format!("   +{a} −{d}"), Tone::Dim.style(theme)),
                        Span::styled("   the example's test", Tone::Accent.style(theme)),
                    ],
                    cols,
                    Style::new(),
                ),
                TalkHit::Test,
            ));
            match found {
                Some((lines, (from, to))) => {
                    let name = if example.name.is_empty() {
                        "the example".to_string()
                    } else {
                        example.name.clone()
                    };
                    let window = if lines.len() <= 24 {
                        (0, lines.len())
                    } else {
                        (from.saturating_sub(3), (to + 4).min(lines.len()))
                    };
                    let notes = vec![(from, format!("{name}, in and out"))];
                    rows.extend(self.hunk_rows(
                        &example.file,
                        &lines,
                        &notes,
                        Some((from, to)),
                        window,
                        cols,
                        theme,
                    ));
                }
                None => rows.push(TRow::of(vec![Span::styled(
                    format!(
                        "      \"{}\" isn't among this diff's changes: t or a click on its file shows it",
                        example.test
                    ),
                    Tone::Faint.style(theme),
                )])),
            }
        }
        Laid {
            rows,
            ..Laid::default()
        }
    }

    /// The Talk view's footer keys.
    pub(super) fn talk_keys(&self) -> Vec<(&'static str, &'static str)> {
        match (self.talker.part, self.talker.zoom) {
            (Part::Zoom, _) => vec![("←→", "zoom"), ("tab", "slides"), ("esc", "back")],
            (Part::Slides, _) => vec![("←→", "slides"), ("tab", "the talk"), ("esc", "back")],
            (_, Zoom::Gist) => vec![
                ("←→", "slides"),
                ("−+", "zoom"),
                ("hl", "step"),
                ("↵", "open a step"),
            ],
            (_, Zoom::Flow) => vec![
                ("↑↓", "step"),
                ("↵", "its slide"),
                ("−+", "zoom"),
                ("←→", "slides"),
            ],
            (_, Zoom::Steps) => vec![
                ("←→", "slides"),
                ("−+", "zoom"),
                ("n", "notes"),
                ("↵", "the file"),
                ("t", "its test"),
            ],
            (_, Zoom::Code) => vec![
                ("↑↓", "scroll"),
                ("−+", "zoom"),
                ("←→", "slides"),
                ("↵", "the whole file"),
            ],
        }
    }
}
