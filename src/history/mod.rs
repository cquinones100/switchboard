//! A tab's history (M-h): UX 2.0's board 6, a timeline under the tab's
//! screen of its Claude session from its start to now, with lanes for its
//! state, your turns, commits, review rounds and context, and the screen
//! above showing the pane as it was at the moment chosen. New with UX 2.0,
//! with no tmux-version file behind it, since tmux kept nothing of a pane
//! but its scrollback.
//!
//! Two sources make it. What the server sees as it happens is kept here,
//! per pane, in memory alone: each change of the pane's Claude state (from
//! the hooks, `Histories::state`) and, while the pane changes, a snapshot
//! of its screen at most every `SHOT_EVERY` (`Histories::shoot`, on the
//! tick), as the rows `Pane::styled_lines` gives, so a snapshot is a few
//! kilobytes. Nothing of it is written to disk, so nothing that was on the
//! screen lands in a file, and it is gone when the server ends; the oldest
//! snapshot goes once a pane has `SHOTS` of them. The rest (your turns,
//! commits, review rounds, your answers at questions, compactions, the
//! context) is read from the session's transcript off the server's task
//! (src/history/transcript.rs), so those lanes reach back to the session's
//! start whenever the server started.
//!
//! The span is the Claude session's: a new session in the pane (a start,
//! `/clear` or a resume, not a compaction) starts the pane's history again.
//!
//! The `tab_history` setting turns it off: nothing is kept and M-h says so.

pub mod serve;
pub mod timeline;
pub mod transcript;
pub mod view;

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;

use ratatui::text::Span;

use crate::agent::State;
use crate::mux::PaneId;
use transcript::Lanes;

/// The least time between two snapshots of a pane, in seconds.
pub const SHOT_EVERY: i64 = 15;
/// How long after a moment the screen it shows may have been taken: a
/// change of state is snapshotted on the next tick (two seconds), so the
/// screen of a wait, a commit or a turn moved to is the one taken just
/// after it.
pub const SETTLE: i64 = 5;
/// The most snapshots kept of a pane: about six hours of one that never
/// stops changing.
pub const SHOTS: usize = 1500;

/// A pane's screen at a moment, as `Pane::styled_lines` gives it.
#[derive(Clone, Debug, PartialEq)]
pub struct Shot {
    pub at: i64,
    pub lines: Vec<Vec<Span<'static>>>,
}

/// What is kept of one pane's Claude session.
#[derive(Clone, Debug, Default)]
pub struct Record {
    pub session: String,
    pub transcript: Option<PathBuf>,
    /// When the record began: the span's start when the transcript says
    /// nothing earlier.
    pub began: i64,
    /// Each change of state, in order, `None` being idle.
    pub states: Vec<(i64, Option<State>)>,
    pub shots: VecDeque<Shot>,
    /// The pane has drawn something since its last snapshot.
    touched: bool,
    /// Its state changed since its last snapshot.
    changed: bool,
    /// The transcript as read so far, and where its next read starts.
    pub lanes: Lanes,
    pub offset: u64,
    /// The session's context window, in tokens, as the last read found it.
    pub window: u64,
    /// A read of the transcript is under way.
    pub reading: bool,
}

impl Record {
    fn new(session: &str, now: i64) -> Record {
        Record {
            session: session.to_string(),
            began: now,
            window: transcript::WINDOW,
            ..Record::default()
        }
    }

    /// Where the span starts: the transcript's first entry, else when the
    /// record began.
    pub fn start(&self) -> i64 {
        self.lanes.first.unwrap_or(self.began).min(self.began)
    }

    /// The screen shown for the moment `at`: the last snapshot taken up to
    /// `SETTLE` after it, but never past the next change of state, so a
    /// question answered at once still shows the question.
    pub fn shot_settled(&self, at: i64) -> Option<&Shot> {
        let next = self
            .states
            .iter()
            .map(|(t, _)| *t)
            .find(|t| *t > at)
            .map_or(i64::MAX, |t| t - 1);
        self.shot_at((at + SETTLE).min(next))
    }

    /// The snapshot showing the screen at `at`: the last taken by then.
    pub fn shot_at(&self, at: i64) -> Option<&Shot> {
        let i = self.shots.partition_point(|s| s.at <= at);
        i.checked_sub(1).and_then(|i| self.shots.get(i))
    }
}

/// Every pane's record.
#[derive(Default)]
pub struct Histories {
    pub records: HashMap<PaneId, Record>,
}

impl Histories {
    pub fn get(&self, pane: PaneId) -> Option<&Record> {
        self.records.get(&pane)
    }

    pub fn get_mut(&mut self, pane: PaneId) -> Option<&mut Record> {
        self.records.get_mut(&pane)
    }

    /// A hook from the pane's Claude named its session and transcript. A
    /// session other than the one kept, or one starting afresh (`fresh`: a
    /// SessionStart that isn't a compaction), starts the pane's history
    /// again.
    pub fn session(
        &mut self,
        pane: PaneId,
        session: &str,
        transcript: Option<PathBuf>,
        fresh: bool,
        now: i64,
    ) {
        if session.is_empty() {
            return;
        }
        let record = self
            .records
            .entry(pane)
            .or_insert_with(|| Record::new(session, now));
        let other = record.session != session;
        if other || (fresh && !record.states.is_empty()) {
            *record = Record::new(session, now);
        }
        if transcript.is_some() && record.transcript != transcript {
            record.transcript = transcript;
            record.lanes = Lanes::default();
            record.offset = 0;
        }
    }

    /// The pane's state now; kept when it changed.
    pub fn state(&mut self, pane: PaneId, state: Option<State>, now: i64) {
        let Some(record) = self.records.get_mut(&pane) else {
            return;
        };
        if record.states.last().map(|(_, s)| *s) != Some(state) {
            // Kept in order, a change seen late never before the last.
            let at = record.states.last().map_or(now, |(t, _)| now.max(*t));
            record.states.push((at, state));
            // A change shows on the screen soon after: the next tick takes
            // it whenever the last snapshot was.
            record.touched = true;
            record.changed = true;
        }
    }

    /// The pane drew something.
    pub fn touched(&mut self, pane: PaneId) {
        if let Some(record) = self.records.get_mut(&pane) {
            record.touched = true;
        }
    }

    /// Something the timeline marks happened (a commit), so the next tick
    /// takes the screen whenever the last snapshot was.
    pub fn marked(&mut self, pane: PaneId) {
        if let Some(record) = self.records.get_mut(&pane) {
            record.touched = true;
            record.changed = true;
        }
    }

    /// The panes due a snapshot: touched since their last, which was at
    /// least `SHOT_EVERY` ago or before their state changed.
    pub fn due(&self, now: i64) -> Vec<PaneId> {
        self.records
            .iter()
            .filter(|(_, r)| {
                r.touched && (r.changed || r.shots.back().is_none_or(|s| now - s.at >= SHOT_EVERY))
            })
            .map(|(p, _)| *p)
            .collect()
    }

    /// Keeps a snapshot of the pane's screen, unless it is the last one
    /// again.
    pub fn shoot(&mut self, pane: PaneId, now: i64, lines: Vec<Vec<Span<'static>>>) {
        let Some(record) = self.records.get_mut(&pane) else {
            return;
        };
        record.touched = false;
        record.changed = false;
        if record.shots.back().is_some_and(|s| s.lines == lines) {
            return;
        }
        record.shots.push_back(Shot { at: now, lines });
        while record.shots.len() > SHOTS {
            record.shots.pop_front();
        }
    }

    pub fn forget(&mut self, pane: PaneId) {
        self.records.remove(&pane);
    }

    /// Forgets everything kept (the setting turned off).
    pub fn clear(&mut self) {
        self.records.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(text: &str) -> Vec<Vec<Span<'static>>> {
        vec![vec![Span::raw(text.to_string())]]
    }

    #[test]
    fn a_change_seen_late_is_kept_in_order() {
        let mut h = Histories::default();
        let p = PaneId(1);
        h.session(p, "s1", None, true, 100);
        h.state(p, Some(State::Waiting), 120);
        h.state(p, None, 110);
        assert_eq!(
            h.get(p).unwrap().states,
            vec![(120, Some(State::Waiting)), (120, None)]
        );
    }

    #[test]
    fn keeps_each_change_of_state_once() {
        let mut h = Histories::default();
        let p = PaneId(1);
        h.session(p, "s1", None, true, 100);
        h.state(p, Some(State::Working), 100);
        h.state(p, Some(State::Working), 110);
        h.state(p, Some(State::Waiting), 120);
        h.state(p, None, 130);
        assert_eq!(
            h.get(p).unwrap().states,
            vec![
                (100, Some(State::Working)),
                (120, Some(State::Waiting)),
                (130, None)
            ]
        );
    }

    #[test]
    fn a_new_session_starts_the_history_again_and_a_compaction_doesnt() {
        let mut h = Histories::default();
        let p = PaneId(1);
        h.session(p, "s1", None, true, 100);
        h.state(p, Some(State::Working), 100);
        h.session(p, "s1", None, false, 200);
        assert_eq!(h.get(p).unwrap().states.len(), 1);
        h.session(p, "s1", None, true, 300);
        assert!(h.get(p).unwrap().states.is_empty(), "/clear in the pane");
        h.state(p, Some(State::Working), 300);
        h.session(p, "s2", None, false, 400);
        let r = h.get(p).unwrap();
        assert_eq!((r.session.as_str(), r.began), ("s2", 400));
        assert!(r.states.is_empty());
    }

    #[test]
    fn snapshots_a_pane_that_changed_at_most_every_so_often() {
        let mut h = Histories::default();
        let p = PaneId(1);
        h.session(p, "s1", None, true, 0);
        assert!(h.due(0).is_empty(), "nothing drawn yet");
        h.touched(p);
        assert_eq!(h.due(0), vec![p]);
        h.shoot(p, 0, line("a"));
        h.touched(p);
        assert!(h.due(SHOT_EVERY - 1).is_empty());
        assert_eq!(h.due(SHOT_EVERY), vec![p]);
        h.shoot(p, SHOT_EVERY, line("a"));
        assert_eq!(h.get(p).unwrap().shots.len(), 1, "the same screen again");
        h.touched(p);
        h.shoot(p, 2 * SHOT_EVERY, line("b"));
        let r = h.get(p).unwrap();
        assert_eq!(r.shot_at(SHOT_EVERY).unwrap().lines, line("a"));
        assert_eq!(r.shot_at(2 * SHOT_EVERY + 5).unwrap().lines, line("b"));
        assert!(r.shot_at(-1).is_none());
    }

    #[test]
    fn the_screen_of_a_moment_settles_but_never_past_the_next_state() {
        let mut h = Histories::default();
        let p = PaneId(1);
        h.session(p, "s1", None, true, 0);
        h.state(p, Some(State::Waiting), 100);
        h.shoot(p, 102, line("question"));
        h.state(p, Some(State::Working), 103);
        h.shoot(p, 104, line("working"));
        let r = h.get(p).unwrap();
        assert_eq!(r.shot_settled(100).unwrap().lines, line("question"));
        assert_eq!(r.shot_settled(103).unwrap().lines, line("working"));
    }

    #[test]
    fn a_commit_is_snapshotted_on_the_next_tick() {
        let mut h = Histories::default();
        let p = PaneId(1);
        h.session(p, "s1", None, true, 0);
        h.touched(p);
        h.shoot(p, 100, line("a"));
        assert!(h.due(106).is_empty());
        h.marked(p);
        assert_eq!(h.due(106), vec![p]);
    }

    #[test]
    fn a_change_of_state_is_snapshotted_on_the_next_tick() {
        let mut h = Histories::default();
        let p = PaneId(1);
        h.session(p, "s1", None, true, 0);
        h.touched(p);
        h.shoot(p, 100, line("a"));
        h.state(p, Some(State::Waiting), 103);
        assert_eq!(h.due(104), vec![p]);
    }

    #[test]
    fn keeps_no_more_than_its_share_of_snapshots() {
        let mut h = Histories::default();
        let p = PaneId(1);
        h.session(p, "s1", None, true, 0);
        for i in 0..SHOTS + 3 {
            h.shoot(p, i as i64 * SHOT_EVERY, line(&i.to_string()));
        }
        let r = h.get(p).unwrap();
        assert_eq!(r.shots.len(), SHOTS);
        assert_eq!(r.shots.front().unwrap().lines, line("3"));
    }
}
