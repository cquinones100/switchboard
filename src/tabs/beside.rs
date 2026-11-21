//! What a new tab opens beside its pane (Settings' `tab_opens_beside`):
//! with its lone pane narrow enough to leave room beside it, as the views
//! beside a narrow pane need (`Server::spare_beside`), the first it has of
//! its PR (the PR view, as M-v opens it), its ticket (the ticket view, as
//! M-i opens it) and what M-e opens for it (its plan or note, with an app
//! of Notes in each tab, since a shared one would be taken from another
//! tab), the focus left in its pane. New with the lone pane's cap, with no
//! tmux-version file behind it.
//!
//! A tab's PR and ticket are known a moment after it opens, once its name
//! lookup answers, and a plan or note once whatever opened it sets its
//! options, so the choice is made again as those arrive (the lookup's
//! answer, a tab option set, the PRs its Claude opened), a view coming
//! earlier in that order taking the place of the one opened before it.
//! What was opened is kept as the tab's `@tab_beside` (`State`), so it
//! survives an upgrade; once the side is no longer as this left it (the
//! view closed, or another pane opened in the tab), the option goes and the
//! tab is left as you set it. A tab restored after a restart has the view
//! its layout saved (`persist`) and loses the option, so nothing opens in
//! it again.
//!
//! The choice reads what the server holds, the tab's ticket from its
//! branch as M-i finds it, never on the drawing path.

use crate::mux::server::Server;
use crate::mux::{PaneId, TabId};

/// The tab option keeping what a new tab opened beside its pane.
pub const OPTION: &str = "@tab_beside";
/// The note a new tab opened beside its pane, so another chosen since (its
/// plan found, a note linked) takes its place.
pub const NOTE_OPTION: &str = "@tab_beside_note";

/// What a tab can open beside its pane, in the order it is preferred.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    Pr,
    Ticket,
    Note,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Kind::Pr => "pr",
            Kind::Ticket => "ticket",
            Kind::Note => "note",
        }
    }

    fn of(name: &str) -> Option<Kind> {
        [Kind::Pr, Kind::Ticket, Kind::Note]
            .into_iter()
            .find(|k| k.name() == name)
    }
}

/// What a tab opened beside its pane so far: nothing yet, or a view of a
/// kind in its pane.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Nothing,
    Opened(Kind, PaneId),
}

impl State {
    pub fn parse(value: &str) -> Option<State> {
        if value == "none" {
            return Some(State::Nothing);
        }
        let (kind, pane) = value.split_once(' ')?;
        Some(State::Opened(Kind::of(kind)?, pane.parse().ok()?))
    }

    pub fn value(self) -> String {
        match self {
            State::Nothing => "none".into(),
            State::Opened(kind, pane) => format!("{} {pane}", kind.name()),
        }
    }
}

/// The view to open, given what the tab has and what is open: the first
/// it has, when it comes before what is open.
pub fn wanted(has: &[Kind], open: Option<Kind>) -> Option<Kind> {
    let best = has.iter().copied().min()?;
    open.is_none_or(|open| best < open).then_some(best)
}

impl Server {
    /// A tab just opened: marked to open a view beside its pane, the choice
    /// made once whoever opened it has set its options.
    pub(crate) fn beside_on_open(&mut self, tab: TabId) {
        if !crate::settings::is_on("tab_opens_beside") {
            return;
        }
        if let Some(t) = self.tabs.get_mut(&tab) {
            t.opts.insert(OPTION.into(), State::Nothing.value());
        }
        self.on_task_soon(move |server| server.beside_check(tab));
    }

    /// A tab left as you set it: nothing more opens beside its pane.
    pub(crate) fn beside_done(&mut self, tab: TabId) {
        if let Some(t) = self.tabs.get_mut(&tab) {
            t.opts.remove(OPTION);
            t.opts.remove(NOTE_OPTION);
        }
    }

    /// Opens beside a new tab's pane the first view it has, in place of one
    /// opened before that comes after it; leaves the tab alone for good
    /// once its side isn't as this left it.
    pub(crate) fn beside_check(&mut self, tab: TabId) {
        let Some(state) = self
            .tabs
            .get(&tab)
            .and_then(|t| t.opts.get(OPTION))
            .map(|v| State::parse(v))
        else {
            return;
        };
        let Some(t) = self.tabs.get(&tab) else {
            return;
        };
        let opened_by_hand =
            t.opts.contains_key("@notes") || t.opts.contains_key(crate::sidebar::notices::MARK);
        let panes = t.layout.panes();
        let left_as_set = match state {
            Some(State::Nothing) => panes.len() == 1,
            // A view you have gone into is yours to close.
            Some(State::Opened(_, view)) => {
                panes.len() == 2
                    && panes.contains(&view)
                    && t.active != view
                    && t.last_pane != Some(view)
            }
            None => false,
        };
        let pane = panes.iter().copied().find(|p| match state {
            Some(State::Opened(_, view)) => *p != view,
            _ => true,
        });
        // The diff or lazygit in the pane's place is the tab changed by hand.
        let in_place = pane
            .and_then(|p| self.panes.get(&p))
            .is_some_and(|s| s.opts.contains_key(crate::diff::in_place::IN_PLACE));
        let choosing = left_as_set
            && !opened_by_hand
            && !in_place
            && crate::settings::is_on("tab_opens_beside");
        // A tab with no room as it opens is left as it is, not opened later.
        let room = matches!(state, Some(State::Opened(..))) || self.spare_beside(tab).is_some();
        let (Some(pane), true, true) = (pane, choosing, room) else {
            self.beside_done(tab);
            return;
        };
        let open = match state {
            Some(State::Opened(kind, _)) => Some(kind),
            _ => None,
        };
        let dir = self
            .tab_path(tab)
            .map(std::path::Path::to_path_buf)
            .or_else(|| self.pane_dir(pane));
        let mut has = Vec::new();
        if self.tab_current_pr(tab).is_some() && crate::settings::is_on("github") {
            has.push(Kind::Pr);
        }
        let ticket = dir
            .as_deref()
            .and_then(|d| self.tab_ticket_at(tab, d))
            .filter(|t| crate::settings::is_on(t.service()));
        if ticket.is_some() {
            has.push(Kind::Ticket);
        }
        // An autopilot tab's plan still being looked for (`find_tab_plan`,
        // which checks again once it answers) comes before its other notes.
        let plan_pending = self.plans_looked_for.contains(&tab);
        let note = (crate::notes::serve::per_tab() && !plan_pending)
            .then(|| self.note_for(tab))
            .flatten();
        if note.is_some() {
            has.push(Kind::Note);
        }
        // An autopilot tab's plan found since its note opened takes its place;
        // a note you chose for the tab is already in the Notes open there.
        let plan = self.tab_opt(tab, crate::notes::serve::PLAN_NOTE);
        let other_note = open == Some(Kind::Note)
            && note.is_some()
            && note == plan
            && note.as_deref() != self.tab_opt(tab, NOTE_OPTION).as_deref();
        let Some(kind) = wanted(&has, open).or(other_note.then_some(Kind::Note)) else {
            return;
        };
        if let Some(State::Opened(old, view)) = state {
            if old == Kind::Note {
                self.set_tab_opt(tab, "@notes_beside", None);
            }
            self.kill_pane(view);
            self.set_tab_opt(tab, OPTION, Some(&State::Nothing.value()));
        }
        if self.spare_beside(tab).is_none() {
            self.beside_done(tab);
            return;
        }
        // The view opens, and the keys stay with the tab's own pane,
        // Claude's or the shell's, without bringing the tab to the front.
        self.opening_beside = true;
        let view = match kind {
            Kind::Pr => {
                self.open_pr_view(tab, pane, dir.as_deref().unwrap_or(&crate::paths::home()))
            }
            Kind::Ticket => ticket.and_then(|found| self.open_ticket_view(pane, found)),
            Kind::Note => self.open_tab_notes(None, pane, tab),
        };
        self.focus_pane(pane);
        self.opening_beside = false;
        if let Some(view) = view {
            self.set_tab_opt(tab, OPTION, Some(&State::Opened(kind, view).value()));
            let shown = note.filter(|_| kind == Kind::Note);
            self.set_tab_opt(tab, NOTE_OPTION, shown.as_deref());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_state_reads_back_as_it_was_kept() {
        for state in [State::Nothing, State::Opened(Kind::Ticket, PaneId(12))] {
            assert_eq!(State::parse(&state.value()), Some(state));
        }
        assert_eq!(State::Opened(Kind::Pr, PaneId(3)).value(), "pr %3");
        assert_eq!(State::parse("diff %3"), None);
    }

    #[test]
    fn the_first_the_tab_has_opens_and_replaces_only_what_comes_after_it() {
        use Kind::*;
        assert_eq!(wanted(&[Note, Pr, Ticket], None), Some(Pr));
        assert_eq!(wanted(&[Note], None), Some(Note));
        assert_eq!(wanted(&[], None), None);
        assert_eq!(wanted(&[Ticket, Pr], Some(Note)), Some(Pr));
        assert_eq!(wanted(&[Ticket], Some(Note)), Some(Ticket));
        assert_eq!(wanted(&[Ticket, Note], Some(Ticket)), None);
        assert_eq!(wanted(&[Ticket], Some(Pr)), None);
    }
}
