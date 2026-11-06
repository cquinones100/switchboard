//! M-a's walk through what needs you, in the sidebar's order: the tabs under
//! Needs you (pinned and reminders come due first, then waiting, then done),
//! then the notifications while there are any (`needs_notices` setting).
//! What tmux/agent-state.sh's `next` did with the list the sidebar daemon
//! wrote (`writeNeeds`), kept in `@agent_walk_window`, `@agent_walk_rest`,
//! `@agent_walk_after` and `@agent_notices_from`; here the same rules on the
//! sidebar's own list, with nothing that reads or acts, so each can be
//! tested on its own.
//!
//! Each press goes to the stop after this one, wrapping at the end, so
//! pressing it again and again walks the whole list; a tab stays waiting
//! after a visit until you answer it, so "the most urgent tab" would keep
//! coming back to the same one. A done tab leaves the list once seen, so
//! pressed again from it, the walk carries on with the stops that were after
//! it (`rest`). From a tab that only finished, or one not in the list, a tab
//! pinned or waiting on you comes first, before the next finished one; the
//! next press from there picks up where the finished ones were left
//! (`after`), so the walk still reaches every one.

use crate::mux::TabId;

/// A place the walk can go: a tab, or the notifications.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Place {
    Tab(TabId),
    Notices,
}

/// Why a stop needs you, which says whether it comes first from a finished
/// tab.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Why {
    Pinned,
    Reminder,
    Waiting,
    Done,
    Notices,
}

impl Why {
    /// Pinned, a reminder come due, or waiting on you: before the next
    /// finished tab.
    fn urgent(self) -> bool {
        matches!(self, Why::Pinned | Why::Reminder | Why::Waiting)
    }
}

/// One stop on the walk.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stop {
    pub place: Place,
    pub why: Why,
}

/// Where the walk got to.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Walk {
    /// The place it last went to.
    window: Option<Place>,
    /// The stops that were after it then.
    rest: Vec<Place>,
    /// Where the finished ones were left when it went to an urgent one.
    after: Option<Place>,
    /// The tab the notifications were last opened from, which counts as
    /// being at them while you are still there.
    notices_from: Option<TabId>,
}

/// What a press of M-a comes to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Next {
    /// Nothing needs you.
    Nothing,
    /// Only the tab you are at does.
    OnlyHere,
    /// Go to this stop (its place in the list).
    Go(usize),
}

impl Walk {
    /// The stop to go to from the tab `here`, keeping where the walk got to.
    pub fn next(&mut self, stops: &[Stop], here: TabId) -> Next {
        let n = stops.len();
        if n == 0 {
            return Next::Nothing;
        }
        let window_here = here;
        // At the notifications, while still in the tab they were opened from.
        let here = if self.window == Some(Place::Notices) && self.notices_from == Some(here) {
            Place::Notices
        } else {
            Place::Tab(here)
        };
        let walked_here = self.window == Some(here);
        let find = |place: Place| stops.iter().position(|s| s.place == place);
        let at = stops.iter().rposition(|s| s.place == here);
        let urgent = (0..n).find(|&i| Some(i) != at && stops[i].why.urgent());
        // Where the finished ones were left, kept whether or not the urgent
        // one is still listed (answered, it leaves).
        let after = self
            .after
            .take()
            .filter(|_| walked_here)
            .and_then(find)
            .filter(|&i| Some(i) != at);
        // Where the walk would go on from a stop that left the list (seen,
        // it clears): the first of the rest it noted still listed.
        let rest = if at.is_none() && walked_here {
            self.rest.iter().find_map(|p| find(*p))
        } else {
            None
        };
        let from_finished = at.is_none_or(|i| matches!(stops[i].why, Why::Done | Why::Notices));
        let to = match urgent {
            Some(u) if from_finished => {
                if let Some(i) = at {
                    self.after = Some(stops[(i + 1) % n].place);
                } else if let Some(i) = after {
                    self.after = Some(stops[i].place);
                } else if let Some(i) = rest.filter(|&i| i != u) {
                    self.after = Some(stops[i].place);
                }
                u
            }
            _ => {
                if let Some(i) = after {
                    i
                } else if let Some(i) = at {
                    // The notifications alone open again whatever was last
                    // done with them.
                    if n == 1 && stops[0].place != Place::Notices {
                        return Next::OnlyHere;
                    }
                    (i + 1) % n
                } else {
                    rest.unwrap_or(0)
                }
            }
        };
        self.window = Some(stops[to].place);
        self.rest = stops[to + 1..].iter().map(|s| s.place).collect();
        if stops[to].place == Place::Notices {
            self.notices_from = Some(window_here);
        }
        Next::Go(to)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tab(n: u32, why: Why) -> Stop {
        Stop {
            place: Place::Tab(TabId(n)),
            why,
        }
    }

    const NOTICES: Stop = Stop {
        place: Place::Notices,
        why: Why::Notices,
    };

    /// Presses M-a from `from`, giving where it went.
    fn press(walk: &mut Walk, stops: &[Stop], from: u32) -> Option<Place> {
        match walk.next(stops, TabId(from)) {
            Next::Go(i) => Some(stops[i].place),
            _ => None,
        }
    }

    fn at(n: u32) -> Option<Place> {
        Some(Place::Tab(TabId(n)))
    }

    #[test]
    fn from_a_finished_tab_a_waiting_one_comes_before_the_next_finished() {
        let (done_a, done_b, waiting, done_c) = (1, 2, 3, 4);
        let mut w = Walk::default();
        let stops = [
            tab(waiting, Why::Waiting),
            tab(done_a, Why::Done),
            tab(done_b, Why::Done),
        ];
        assert_eq!(press(&mut w, &stops, done_a), at(waiting));
        assert_eq!(
            press(&mut w, &stops, waiting),
            at(done_b),
            "on after the one it left"
        );
        assert_eq!(press(&mut w, &stops, done_b), at(waiting));
        assert_eq!(press(&mut w, &stops, waiting), at(done_a), "and round");

        // Answered, the waiting tab leaves: the walk picks up after the
        // finished one it left.
        assert_eq!(press(&mut w, &stops, done_a), at(waiting));
        let answered = [tab(done_a, Why::Done), tab(done_b, Why::Done)];
        assert_eq!(press(&mut w, &answered, waiting), at(done_b));

        // A finished tab that clears once seen, mid-walk: the detour to a
        // waiting tab goes on from the rest of the walk after it.
        let three = [
            tab(done_a, Why::Done),
            tab(done_b, Why::Done),
            tab(done_c, Why::Done),
        ];
        assert_eq!(press(&mut w, &three, done_a), at(done_b));
        let seen = [
            tab(waiting, Why::Waiting),
            tab(done_a, Why::Done),
            tab(done_c, Why::Done),
        ];
        assert_eq!(press(&mut w, &seen, done_b), at(waiting));
        assert_eq!(
            press(&mut w, &seen, waiting),
            at(done_c),
            "not back to done_a"
        );
    }

    #[test]
    fn the_notifications_come_last_and_the_walk_goes_round_from_them() {
        let (done_a, done_b, waiting) = (1, 2, 3);
        let mut w = Walk::default();
        let stops = [tab(done_a, Why::Done), NOTICES];
        w.window = Some(Place::Tab(TabId(done_a)));
        assert_eq!(
            press(&mut w, &stops, done_a),
            Some(Place::Notices),
            "from the last finished tab, the notifications"
        );
        let with_waiting = [tab(waiting, Why::Waiting), NOTICES];
        assert_eq!(press(&mut w, &with_waiting, done_b), at(waiting));

        // Opened in the same tab's sidebar, the next press from there goes
        // on round, not to them again.
        let mut w = Walk::default();
        let stops = [tab(done_a, Why::Done), tab(done_b, Why::Done), NOTICES];
        w.window = Some(Place::Tab(TabId(done_b)));
        assert_eq!(press(&mut w, &stops, done_b), Some(Place::Notices));
        assert_eq!(press(&mut w, &stops, done_b), at(done_a));

        // The notifications all that needs you: every press opens them.
        let alone = [NOTICES];
        assert_eq!(press(&mut w, &alone, done_b), Some(Place::Notices));
        assert_eq!(press(&mut w, &alone, done_b), Some(Place::Notices));
    }

    #[test]
    fn nothing_or_only_this_tab_goes_nowhere() {
        let mut w = Walk::default();
        assert_eq!(w.next(&[], TabId(1)), Next::Nothing);
        assert_eq!(w.next(&[tab(1, Why::Waiting)], TabId(1)), Next::OnlyHere);
        assert_eq!(w.next(&[tab(1, Why::Waiting)], TabId(2)), Next::Go(0));
        // Pinned tabs lead the walk as they lead Needs you.
        let stops = [tab(5, Why::Pinned), tab(6, Why::Waiting), tab(7, Why::Done)];
        let mut w = Walk::default();
        assert_eq!(press(&mut w, &stops, 1), at(5));
        assert_eq!(press(&mut w, &stops, 5), at(6));
        assert_eq!(press(&mut w, &stops, 6), at(7));
    }
}
