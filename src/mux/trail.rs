//! The tabs a session has visited, for M-{ and M-} to go back and forward
//! through as a browser's buttons do (new, no tmux version had it). Every
//! switch puts the tab left on `back` and clears `forward`, however it came;
//! a step back or forward moves the current tab to the other side instead.
//! A tab that closes is taken out of both, and each keeps at most `CAP`.

use serde::{Deserialize, Serialize};

use super::TabId;

/// The tabs one way, nearest first, each with how many steps away it is.
pub type Steps = Vec<(TabId, usize)>;

/// How many tabs each way are kept.
pub const CAP: usize = 50;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Trail {
    /// The tabs before, the latest last.
    #[serde(default)]
    pub back: Vec<TabId>,
    /// The tabs after, the nearest last.
    #[serde(default)]
    pub forward: Vec<TabId>,
}

impl Trail {
    pub fn is_empty(&self) -> bool {
        self.back.is_empty() && self.forward.is_empty()
    }

    /// A switch away from `left` that wasn't a step back or forward.
    pub fn visited(&mut self, left: TabId) {
        push(&mut self.back, left);
        self.forward.clear();
    }

    /// The tab to go back to from `current`, which goes on `forward`; none
    /// when there is nothing before.
    pub fn back(&mut self, current: TabId) -> Option<TabId> {
        let to = pop_other(&mut self.back, current)?;
        push(&mut self.forward, current);
        Some(to)
    }

    /// The tab to go forward to from `current`, which goes on `back`.
    pub fn forward(&mut self, current: TabId) -> Option<TabId> {
        let to = pop_other(&mut self.forward, current)?;
        push(&mut self.back, current);
        Some(to)
    }

    /// The tab a step back from `current` would go to, without taking it.
    pub fn peek_back(&self, current: TabId) -> Option<TabId> {
        self.back.iter().rev().copied().find(|t| *t != current)
    }

    /// The tab a step forward from `current` would go to.
    pub fn peek_forward(&self, current: TabId) -> Option<TabId> {
        self.forward.iter().rev().copied().find(|t| *t != current)
    }

    /// Every step each way from `current`, nearest first, with how many
    /// steps away it is, as stepping again and again would go: the back
    /// ones, then the forward ones.
    pub fn steps(&self, current: TabId) -> (Steps, Steps) {
        let way = |list: &[TabId]| {
            let mut list = list.to_vec();
            let mut out: Steps = Vec::new();
            let mut at = current;
            while let Some(t) = pop_other(&mut list, at) {
                out.push((t, out.len() + 1));
                at = t;
            }
            out
        };
        (way(&self.back), way(&self.forward))
    }

    /// A tab closed, or not in the session any more.
    pub fn forget(&mut self, tab: TabId) {
        self.back.retain(|t| *t != tab);
        self.forward.retain(|t| *t != tab);
    }

    /// Only the tabs `keep` holds, for a trail read back from a handover.
    pub fn keeping(mut self, keep: &[TabId]) -> Trail {
        self.back.retain(|t| keep.contains(t));
        self.forward.retain(|t| keep.contains(t));
        self
    }
}

fn push(list: &mut Vec<TabId>, tab: TabId) {
    if list.last() != Some(&tab) {
        list.push(tab);
    }
    if list.len() > CAP {
        list.drain(..list.len() - CAP);
    }
}

/// The latest tab of `list` that isn't `current`, the ones that are dropped.
fn pop_other(list: &mut Vec<TabId>, current: TabId) -> Option<TabId> {
    while let Some(t) = list.pop() {
        if t != current {
            return Some(t);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: TabId = TabId(1);
    const B: TabId = TabId(2);
    const C: TabId = TabId(3);

    #[test]
    fn back_and_forward_retrace_the_visits() {
        let mut t = Trail::default();
        t.visited(A);
        t.visited(B);
        assert_eq!(t.back(C), Some(B));
        assert_eq!(t.back(B), Some(A));
        assert_eq!(t.back(A), None);
        assert_eq!(t.forward(A), Some(B));
        assert_eq!(t.forward(B), Some(C));
        assert_eq!(t.forward(C), None);
    }

    #[test]
    fn a_new_visit_after_going_back_clears_forward() {
        let mut t = Trail::default();
        t.visited(A);
        t.visited(B);
        assert_eq!(t.back(C), Some(B));
        t.visited(B);
        assert_eq!(t.forward(A), None);
        assert_eq!(t.back(A), Some(B));
    }

    #[test]
    fn peeking_and_steps_take_nothing_off_the_trail() {
        let mut t = Trail::default();
        t.visited(A);
        t.visited(B);
        assert_eq!(t.back(C), Some(B));
        assert_eq!(t.peek_back(B), Some(A));
        assert_eq!(t.peek_forward(B), Some(C));
        assert_eq!(t.steps(B), (vec![(A, 1)], vec![(C, 1)]));
        assert_eq!(t.back(B), Some(A));
        assert_eq!(t.steps(A), (vec![], vec![(B, 1), (C, 2)]));
    }

    #[test]
    fn a_closed_tab_is_skipped() {
        let mut t = Trail::default();
        t.visited(A);
        t.visited(B);
        t.forget(B);
        assert_eq!(t.back(C), Some(A));
        assert_eq!(t.back(A), None);
    }

    #[test]
    fn each_way_keeps_at_most_the_cap() {
        let mut t = Trail::default();
        for n in 0..(CAP as u32 + 10) {
            t.visited(TabId(n));
        }
        assert_eq!(t.back.len(), CAP);
        assert_eq!(t.back[0], TabId(10));
    }

    #[test]
    fn keeping_drops_tabs_gone() {
        let t = Trail {
            back: vec![A, B],
            forward: vec![C],
        };
        assert_eq!(
            t.keeping(&[A, C]),
            Trail {
                back: vec![A],
                forward: vec![C]
            }
        );
    }
}
