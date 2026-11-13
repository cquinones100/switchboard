//! The timeline under a tab's screen in its history (board 6): the time
//! axis from the session's start to now, a column for each stretch of it,
//! and under it the lanes: the state (an image at pixel resolution), your
//! turns (▸), commits (•), review rounds (◆) and the context (a chart, an
//! image too, compactions marked by a dashed line), the moment shown marked
//! by a line through them all, and the keys. Drawn from a `Model` the
//! server gives the view, with no file read here.
//!
//! The span fills the timeline's width, so a column is the span over the
//! columns (a little under a minute for an hour and a half on a wide
//! screen). The moment shown is a time, not a column, so it stays put as
//! the span grows; `None` is now.

use jiff::Timestamp;
use jiff::tz::TimeZone;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};

use super::transcript::{Answer, Lanes};
use crate::agent::State;
use crate::mux::compose::Frame;
use crate::ui::Theme;
use crate::ui::draw;
use crate::ui::images::{self, Rule, Segment};
use crate::ui::kit::{self, Tone};

/// The rows the timeline takes: a rule, the axis, a gap, the four lanes,
/// the context's labels and its chart (two rows), and the keys.
pub const ROWS: u16 = 11;
/// The columns of the lanes' names, at the left.
pub const LABELS: u16 = 10;
/// The columns kept free at the keys row's right end, for easy mode's ×.
pub const CLOSE_ROOM: u16 = 5;

/// What the timeline is drawn from.
#[derive(Clone, Debug, Default)]
pub struct Model {
    pub start: i64,
    pub now: i64,
    /// Each change of state the server saw, `None` being idle.
    pub states: Vec<(i64, Option<State>)>,
    pub lanes: Lanes,
    /// The context window, in tokens.
    pub window: u64,
}

/// Where a key moves the moment shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Move {
    /// A column back (h) or on (l).
    Back,
    On,
    /// The turn before (`[`) or after (`]`).
    PrevTurn,
    NextTurn,
    /// The next commit (c) or wait (w), from the first once past the last.
    NextCommit,
    NextWait,
}

impl Model {
    /// The seconds a column stands for, the span over `cols` columns.
    pub fn per_col(&self, cols: u16) -> f64 {
        ((self.now - self.start).max(1) as f64 / f64::from(cols.max(1))).max(1.0)
    }

    /// Where a time falls across the span, from 0 to 1.
    pub fn frac(&self, at: i64) -> f32 {
        let span = (self.now - self.start).max(1) as f32;
        ((at - self.start) as f32 / span).clamp(0.0, 1.0)
    }

    /// The column a time falls in, of `cols`.
    pub fn col(&self, at: i64, cols: u16) -> u16 {
        ((self.frac(at) * f32::from(cols)) as u16).min(cols.saturating_sub(1))
    }

    /// The stretches the server saw it waiting on you, each from and to.
    pub fn waits(&self) -> Vec<(i64, i64)> {
        let mut out = Vec::new();
        for (i, (at, state)) in self.states.iter().enumerate() {
            if *state == Some(State::Waiting) {
                let to = self.states.get(i + 1).map_or(self.now, |(t, _)| *t);
                out.push((*at, to));
            }
        }
        out
    }

    /// The state lane's parts, each change to the next.
    pub fn segments(&self, theme: &Theme) -> Vec<Segment> {
        self.states
            .iter()
            .enumerate()
            .map(|(i, (at, state))| {
                let to = self.states.get(i + 1).map_or(self.now, |(t, _)| *t);
                let color = match state {
                    Some(State::Working) => Tone::Working.fg(theme),
                    Some(State::Waiting) => Tone::Needs.fg(theme),
                    Some(State::Done) => Tone::Done.fg(theme),
                    None => kit::border(theme),
                };
                (self.frac(*at), self.frac(to), color)
            })
            .filter(|(a, b, _)| b > a)
            .collect()
    }

    /// The context used at `samples` moments evenly across the span, as a
    /// share of the window: the last reply's by then, `None` before the
    /// first.
    pub fn context(&self, samples: usize) -> Vec<Option<f32>> {
        let window = self.window.max(1) as f32;
        let span = (self.now - self.start).max(1) as f64;
        (0..samples)
            .map(|i| {
                let at = self.start + (span * (i as f64 + 0.5) / samples.max(1) as f64) as i64;
                let i = self.lanes.context.partition_point(|(t, _)| *t <= at);
                i.checked_sub(1)
                    .map(|i| self.lanes.context[i].1 as f32 / window)
            })
            .collect()
    }

    /// The context's share of the window by `at`, in percent.
    fn percent_at(&self, at: i64) -> Option<u64> {
        let i = self.lanes.context.partition_point(|(t, _)| *t <= at);
        i.checked_sub(1)
            .map(|i| self.lanes.context[i].1 * 100 / self.window.max(1))
    }

    /// The question you were asked at `at`, with what you chose.
    pub fn answer_at(&self, at: i64) -> Option<&Answer> {
        self.lanes
            .answers
            .iter()
            .find(|a| a.asked <= at && at < a.at)
    }

    /// The wait under way at `at`, from and to.
    pub fn wait_at(&self, at: i64) -> Option<(i64, i64)> {
        self.waits().into_iter().find(|(a, b)| *a <= at && at < *b)
    }

    /// Where `how` moves the moment shown from `shown` (`None` being now),
    /// on a timeline `cols` wide.
    pub fn moved(&self, shown: Option<i64>, how: Move, cols: u16) -> Option<i64> {
        let at = shown.unwrap_or(self.now);
        let step = self.per_col(cols).round() as i64;
        let after = |times: &[i64]| -> Option<i64> {
            times
                .iter()
                .copied()
                .find(|t| *t > at)
                .or_else(|| times.first().copied())
                .filter(|t| *t < self.now)
        };
        match how {
            Move::Back => Some((at - step).max(self.start)),
            Move::On => Some(at + step).filter(|t| *t < self.now),
            Move::PrevTurn => self
                .lanes
                .turns
                .iter()
                .rev()
                .copied()
                .find(|t| *t < at)
                .or(shown),
            Move::NextTurn => self.lanes.turns.iter().copied().find(|t| *t > at),
            Move::NextCommit => {
                let times: Vec<i64> = self.lanes.commits.iter().map(|c| c.at).collect();
                after(&times).or(shown)
            }
            Move::NextWait => {
                let times: Vec<i64> = self.waits().iter().map(|(a, _)| *a).collect();
                after(&times).or(shown)
            }
        }
    }
}

/// `13:41`, in the computer's time zone.
pub fn clock(at: i64) -> String {
    Timestamp::from_second(at)
        .map(|t| t.to_zoned(TimeZone::system()).strftime("%H:%M").to_string())
        .unwrap_or_default()
}

/// "viewing 13:41, 51 minutes ago", for the header.
pub fn viewing(at: i64, now: i64) -> String {
    let minutes = (now - at).max(0) / 60;
    let ago = match minutes {
        0 => "under a minute ago".to_string(),
        1 => "a minute ago".to_string(),
        m if m < 120 => format!("{m} minutes ago"),
        m => format!("{} hours ago", m / 60),
    };
    format!("viewing {}, {ago}", clock(at))
}

/// How long, in a few characters: `40s`, `6m`, `1h 5m`.
pub fn span_words(seconds: i64) -> String {
    match seconds.max(0) {
        s if s < 60 => format!("{s}s"),
        s if s < 3600 => format!("{}m", s / 60),
        s => format!("{}h {}m", s / 3600, s % 3600 / 60),
    }
}

/// The minutes between two labels on the axis: the first of these that
/// leaves at least 14 columns between them.
fn label_step(per_col: f64) -> i64 {
    [1, 2, 5, 10, 15, 30, 60, 120, 240, 480, 1440]
        .into_iter()
        .find(|m| (m * 60) as f64 / per_col >= 14.0)
        .unwrap_or(1440)
        * 60
}

fn put(frame: &mut Frame, x: u16, y: u16, room: u16, spans: Vec<Span<'static>>) -> u16 {
    draw::put_line(&mut frame.buf, x, y, room, &Line::from(spans))
}

/// Draws the timeline into `area` (`ROWS` high), the moment shown at
/// `shown` (`None` being now). `viewing`: say which moment is shown at the
/// keys row's end, where no header says it. Gives the lanes' columns, for
/// a click.
pub fn draw_timeline(
    frame: &mut Frame,
    area: Rect,
    model: &Model,
    shown: Option<i64>,
    theme: &Theme,
    viewing_here: bool,
) -> Rect {
    let area = area.intersection(frame.buf.area);
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            frame.buf[(x, y)].reset();
        }
    }
    frame.links.clear_area(area);
    if area.height < ROWS || area.width <= LABELS + 4 {
        return Rect::default();
    }
    let plot = Rect::new(
        area.x + LABELS,
        area.y,
        area.width - LABELS - 2,
        area.height,
    );
    let cols = plot.width;
    let row = |n: u16| area.y + n;
    let dim = Tone::Dim.style(theme);
    let rule = kit::rule(area.width as usize, theme);
    put(frame, area.x, row(0), area.width, vec![rule]);

    // The axis: when it started, a label every so often, now at the end.
    let start = format!("{} started", clock(model.start));
    let now = format!("{} now", clock(model.now));
    let end_at = plot.right().saturating_sub(draw::width(&now) as u16);
    let mut taken = put(frame, plot.x, row(1), cols, vec![Span::styled(start, dim)]) + 2;
    let step = label_step(model.per_col(cols));
    let mut t = (model.start / step + 1) * step;
    while t < model.now {
        let x = plot.x + model.col(t, cols);
        let label = clock(t);
        if x >= taken && x + draw::width(&label) as u16 + 2 <= end_at {
            taken = put(frame, x, row(1), cols, vec![Span::styled(label, dim)]) + 2;
        }
        t += step;
    }
    if end_at >= taken {
        put(frame, end_at, row(1), cols, vec![Span::styled(now, dim)]);
    }

    let lane_name = |frame: &mut Frame, y: u16, name: &str| {
        let x = area.x + LABELS - 2 - draw::width(name) as u16;
        put(
            frame,
            x,
            y,
            LABELS,
            vec![Span::styled(name.to_string(), dim)],
        );
    };
    let at = shown.unwrap_or(model.now);
    let cursor = Rule {
        at: if shown.is_some() {
            (f32::from(model.col(at, cols)) + 0.5) / f32::from(cols)
        } else {
            1.0
        },
        color: Tone::Needs.fg(theme),
        dashed: false,
    };

    lane_name(frame, row(3), "state");
    images::lane(
        frame,
        "history:state",
        Rect::new(plot.x, row(3), cols, 1),
        &model.segments(theme),
        &[cursor],
    );

    lane_name(frame, row(4), "you");
    for turn in &model.lanes.turns {
        let x = plot.x + model.col(*turn, cols);
        put(
            frame,
            x,
            row(4),
            1,
            vec![Span::styled("▸", Tone::Accent.style(theme))],
        );
    }

    // A mark, with its words after it while they fit before the next.
    let marks = |frame: &mut Frame, y: u16, items: Vec<(i64, Span<'static>, String)>| {
        let xs: Vec<u16> = items
            .iter()
            .map(|(t, _, _)| plot.x + model.col(*t, cols))
            .collect();
        for (i, (_, mark, words)) in items.into_iter().enumerate() {
            let x = xs[i];
            let next = xs.get(i + 1).copied().unwrap_or(plot.right());
            put(frame, x, y, 1, vec![mark]);
            let room = next.saturating_sub(x + 3);
            if !words.is_empty() && draw::width(&words) as u16 <= room {
                put(frame, x + 2, y, room, vec![Span::styled(words, dim)]);
            }
        }
    };
    lane_name(frame, row(5), "commits");
    let done = Tone::Done.style(theme);
    marks(
        frame,
        row(5),
        model
            .lanes
            .commits
            .iter()
            .map(|c| {
                (
                    c.at,
                    Span::styled("•", done),
                    c.hash.chars().take(7).collect(),
                )
            })
            .collect(),
    );
    lane_name(frame, row(6), "review");
    let landing = Tone::Landing.style(theme);
    marks(
        frame,
        row(6),
        model
            .lanes
            .rounds
            .iter()
            .map(|r| {
                let words = if r.clean {
                    format!("round {} clean", r.number)
                } else {
                    format!("round {}", r.number)
                };
                (r.at, Span::styled("◆", landing), words)
            })
            .collect(),
    );

    // The context: compactions said over the chart, where it is now at the
    // end, the chart under them.
    for c in &model.lanes.compactions {
        let x = plot.x + model.col(c.at, cols);
        let before = c.before * 100 / model.window.max(1);
        // The first reply after it says what the compaction left.
        let after = model.lanes.context.iter().find(|(t, _)| *t > c.at);
        let words = match after.map(|(_, tokens)| tokens * 100 / model.window.max(1)) {
            Some(after) => format!("compacted {before}% → {after}%"),
            None => format!("compacted at {before}%"),
        };
        put(
            frame,
            x + 1,
            row(7),
            plot.right().saturating_sub(x + 1),
            vec![Span::styled(words, landing)],
        );
    }
    if let Some(p) = model.percent_at(model.now) {
        let words = format!("{p}% now");
        let x = plot.right().saturating_sub(draw::width(&words) as u16);
        put(frame, x, row(7), cols, vec![Span::styled(words, dim)]);
    }
    let mut rules: Vec<Rule> = model
        .lanes
        .compactions
        .iter()
        .map(|c| Rule {
            at: (f32::from(model.col(c.at, cols)) + 0.5) / f32::from(cols),
            color: Tone::Landing.fg(theme),
            dashed: true,
        })
        .collect();
    rules.push(cursor);
    lane_name(frame, row(9), "context");
    images::chart(
        frame,
        "history:context",
        Rect::new(plot.x, row(8), cols, 2),
        &model.context(usize::from(cols) * 4),
        Tone::Working.fg(theme),
        &rules,
    );

    // The moment shown, through the text lanes where nothing is drawn.
    let x = plot.x
        + if shown.is_some() {
            model.col(at, cols)
        } else {
            cols - 1
        };
    for y in [row(2), row(4), row(5), row(6), row(7)] {
        let cell = &mut frame.buf[(x, y)];
        if cell.symbol() == " " {
            cell.set_symbol("│")
                .set_style(Style::new().fg(cursor.color));
        }
    }

    let back = if shown.is_some() {
        "back to now"
    } else {
        "close"
    };
    let hints = kit::hints(
        &[
            ("h/l", "move in time"),
            ("[ ]", "turns"),
            ("c", "next commit"),
            ("w", "next wait"),
            ("Esc", back),
        ],
        theme,
    );
    let end = put(frame, area.x + 1, row(10), area.width - 1, hints);
    if viewing_here && let Some(at) = shown {
        let words = viewing(at, model.now);
        let x = area
            .right()
            .saturating_sub(draw::width(&words) as u16 + CLOSE_ROOM);
        if x > end + 1 {
            put(
                frame,
                x,
                row(10),
                area.width,
                vec![Span::styled(words, Tone::Needs.bold(theme))],
            );
        }
    }
    Rect::new(plot.x, area.y, cols, area.height)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::transcript::{Commit, Compaction, Round};

    fn model() -> Model {
        let mut lanes = Lanes::default();
        lanes.turns = vec![1000, 2200, 3100];
        lanes.commits = vec![
            Commit {
                at: 1900,
                hash: "a1c3e52".into(),
            },
            Commit {
                at: 3400,
                hash: "9c41e07".into(),
            },
        ];
        lanes.rounds = vec![Round {
            at: 2400,
            number: 1,
            clean: true,
        }];
        lanes.answers = vec![Answer {
            asked: 2000,
            at: 2200,
            chose: "1. Fix it".into(),
        }];
        lanes.compactions = vec![Compaction {
            at: 3000,
            before: 132_000,
        }];
        lanes.context = vec![(1100, 20_000), (2900, 132_000), (3050, 36_000)];
        Model {
            start: 1000,
            now: 4000,
            states: vec![
                (1000, Some(State::Working)),
                (2000, Some(State::Waiting)),
                (2200, Some(State::Working)),
                (3600, None),
            ],
            lanes,
            window: 200_000,
        }
    }

    #[test]
    fn a_column_is_the_span_over_the_width() {
        let m = model();
        assert_eq!(m.per_col(100), 30.0);
        assert_eq!(m.col(1000, 100), 0);
        assert_eq!(m.col(2500, 100), 50);
        assert_eq!(m.col(4000, 100), 99);
    }

    #[test]
    fn keys_move_the_moment_through_time() {
        let m = model();
        assert_eq!(m.moved(None, Move::Back, 100), Some(3970));
        assert_eq!(m.moved(Some(3980), Move::On, 100), None, "past now is now");
        assert_eq!(m.moved(Some(1010), Move::Back, 100), Some(1000));
        assert_eq!(m.moved(None, Move::PrevTurn, 100), Some(3100));
        assert_eq!(m.moved(Some(3100), Move::PrevTurn, 100), Some(2200));
        assert_eq!(m.moved(Some(1000), Move::PrevTurn, 100), Some(1000));
        assert_eq!(m.moved(Some(2200), Move::NextTurn, 100), Some(3100));
        assert_eq!(m.moved(Some(3100), Move::NextTurn, 100), None);
        assert_eq!(m.moved(Some(1000), Move::NextCommit, 100), Some(1900));
        assert_eq!(
            m.moved(Some(3500), Move::NextCommit, 100),
            Some(1900),
            "from the first"
        );
        assert_eq!(m.moved(None, Move::NextWait, 100), Some(2000));
    }

    #[test]
    fn reads_waits_answers_and_the_context() {
        let m = model();
        assert_eq!(m.waits(), vec![(2000, 2200)]);
        assert_eq!(m.wait_at(2100), Some((2000, 2200)));
        assert_eq!(m.answer_at(2100).unwrap().chose, "1. Fix it");
        assert!(m.answer_at(2300).is_none());
        let ctx = m.context(3);
        assert_eq!(ctx[0], Some(0.1));
        assert_eq!(ctx[2], Some(0.18));
        assert_eq!(m.percent_at(4000), Some(18));
        let segments = m.segments(&Theme::DARK);
        assert_eq!(segments.len(), 4);
        assert_eq!(segments[3].1, 1.0);
    }

    fn text(frame: &Frame, y: u16) -> String {
        (0..frame.buf.area.width)
            .map(|x| frame.buf[(x, y)].symbol().to_string())
            .collect::<String>()
            .trim_end()
            .to_string()
    }

    #[test]
    fn draws_the_lanes_and_the_moment_shown() {
        let m = model();
        let mut frame = Frame::new(112, ROWS);
        let plot = draw_timeline(
            &mut frame,
            Rect::new(0, 0, 112, ROWS),
            &m,
            Some(2100),
            &Theme::DARK,
            true,
        );
        assert_eq!(plot, Rect::new(LABELS, 0, 100, ROWS));
        assert!(text(&frame, 0).starts_with("───"));
        assert!(text(&frame, 1).ends_with(" now"));
        assert!(text(&frame, 1).contains(" started"));
        assert!(text(&frame, 3).trim_start().starts_with("state"));
        let you = text(&frame, 4);
        assert_eq!(you.matches('▸').count(), 3);
        let commits = text(&frame, 5);
        assert!(commits.contains("• a1c3e52"), "{commits}");
        assert!(text(&frame, 6).contains("◆ round 1 clean"));
        assert!(text(&frame, 7).contains("compacted 66% → 18%"));
        assert!(text(&frame, 7).ends_with("18% now"));
        assert!(text(&frame, 9).trim_start().starts_with("context"));
        let keys = text(&frame, 10);
        assert!(keys.contains("Esc back to now"), "{keys}");
        assert!(keys.contains("viewing "), "{keys}");
        let x = LABELS + m.col(2100, 100);
        assert_eq!(frame.buf[(x, 2)].symbol(), "│", "the moment's line");
        assert!(
            frame.images.is_empty(),
            "text forms where no images are drawn"
        );
    }

    #[test]
    fn the_state_lane_and_the_context_are_images_where_the_terminal_draws_them() {
        let m = model();
        let mut frame = Frame::new(112, ROWS);
        frame.graphics = Some(crate::mux::graphics::Graphics::kitty((8, 16)));
        draw_timeline(
            &mut frame,
            Rect::new(0, 0, 112, ROWS),
            &m,
            None,
            &Theme::DARK,
            false,
        );
        let sizes: Vec<(u16, u16)> = frame.images.iter().map(|i| (i.cols, i.rows)).collect();
        assert_eq!(sizes, vec![(100, 1), (100, 2)]);
        assert!(text(&frame, 10).contains("Esc close"));
    }
}
