//! Keeping the computer awake on the server (src/awake): the check on every
//! tick, from the sidebar's model of each tab and autopilot's count of its
//! open tabs, both already worked out from what the server holds; the hold
//! taken and let go off the server's task, since taking it starts
//! `caffeinate`; M-p's "Keep the computer awake until I say"; `list`'s
//! `awake`; and the hold let go before an upgrade's exec.

use std::time::Instant;

use serde_json::{Value, json};

use super::{Step, Why};
use crate::mux::server::{ClientId, Event, Server};
use crate::platform::KeepAwake;
use crate::settings;
use crate::sidebar::model::TabState;

/// What the work off the task answers.
pub enum AwakeEvent {
    /// The hold taken, or `None` where it couldn't be.
    Taken(Option<KeepAwake>),
}

/// A hold let go off the task: letting `caffeinate` go waits for it to end.
fn let_go(hold: KeepAwake) {
    match tokio::runtime::Handle::try_current() {
        Ok(runtime) => {
            runtime.spawn_blocking(move || drop(hold));
        }
        Err(_) => drop(hold),
    }
}

impl Server {
    /// On every tick, and when it is held or let go by hand: what holds the
    /// computer awake counted, and the hold taken or let go.
    pub fn awake_check(&mut self) {
        let by_hand = self.awake.by_hand;
        let counted = settings::is_on("keep_awake") && !by_hand;
        let working = if counted { self.awake_working() } else { 0 };
        let autopilot = if counted { self.awake_autopilot() } else { 0 };
        let why = super::why(by_hand, working, autopilot);
        match self.awake.step(why, Instant::now()) {
            Step::Take => self.awake_take(why),
            Step::LetGo => {
                if let Some(hold) = self.awake.hold.take() {
                    let_go(hold);
                    self.awake_redraw();
                }
            }
            Step::Stay => {}
        }
        // Said once, the first time work holds it (cheap once shown).
        if self.awake.holding() && matches!(why, Some(Why::Working(_) | Why::Autopilot(_))) {
            self.tip("awake", None);
        }
    }

    /// The tabs whose Claude works: in the foreground, or with background
    /// work or watches running, between turns (a turn that ended over its
    /// watches too, which stays Done until seen) or while it waits on you,
    /// whose work the sidebar's model leaves out, as the tab shows the
    /// question; not a /loop between its ticks, which may wait for hours.
    fn awake_working(&self) -> usize {
        let runs_work = |pane: Option<crate::mux::PaneId>| {
            pane.and_then(|p| self.sidebar.panes.get(&p))
                .is_some_and(|r| r.session.is_some() && r.work.count > 0)
        };
        self.sidebar_all_tabs()
            .iter()
            .filter(|t| {
                t.state == TabState::Working || t.background > 0 || runs_work(t.claude_pane)
            })
            .count()
    }

    /// Autopilot's tabs open and not landed, while it is on and not paused.
    fn awake_autopilot(&self) -> usize {
        let mode = &self.autopilot.mode;
        if settings::is_on("autopilot") && mode.on && !mode.paused {
            self.autopilot.pilot.counts.1
        } else {
            0
        }
    }

    /// The hold taken off the task, answered with `AwakeEvent::Taken`.
    fn awake_take(&mut self, why: Option<Why>) {
        if tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        let words = format!(
            "Switchboard keeps it awake: {}",
            why.map_or_else(String::new, Why::words)
        );
        let tx = self.tx.clone();
        tokio::task::spawn_blocking(move || {
            let hold = crate::platform::keep_awake(&words);
            let _ = tx.send(Event::Awake(AwakeEvent::Taken(hold)));
        });
    }

    /// What the work off the task answered.
    pub fn awake_event(&mut self, event: AwakeEvent) {
        match event {
            AwakeEvent::Taken(hold) => match self.awake.taken(hold) {
                Ok(true) => self.awake_redraw(),
                Ok(false) => eprintln!(
                    "switchboard server: couldn't keep the computer awake ({} said no)",
                    crate::platform::NAME
                ),
                Err(not_wanted) => let_go(not_wanted),
            },
        }
    }

    /// The ☕ on the status line, or the sidebar's top line, drawn again.
    fn awake_redraw(&mut self) {
        for c in self.clients.values_mut() {
            c.dirty = true;
        }
    }

    /// M-p's "Keep the computer awake until I say": held whatever runs,
    /// or, pressed again, back to Settings' rule.
    pub fn awake_by_hand(&mut self, client: ClientId) {
        self.awake.by_hand = !self.awake.by_hand;
        self.awake_check();
        let text = if self.awake.by_hand {
            "Keeping the computer awake until you say; the same command in M-p lets it sleep again. The display still sleeps."
        } else if settings::is_on("keep_awake") {
            "The computer stays awake only while tabs work again, and sleeps two minutes after they're idle."
        } else {
            "The computer sleeps on its own schedule again."
        };
        self.say(Some(client), text);
    }

    /// `list`'s `awake`: whether the computer is held awake now, what holds
    /// it (none in the grace after everything went idle), and whether by
    /// hand.
    pub fn awake_json(&self) -> Value {
        json!({
            "holding": self.awake.holding(),
            "why": self.awake.why.map(Why::words),
            "by_hand": self.awake.by_hand,
        })
    }

    /// Before an upgrade's exec: the hold let go now, since `caffeinate -w`
    /// waits on this process, which the new build carries on as, and that
    /// build takes its own on its first tick (this one's next, when the
    /// exec fails).
    pub fn awake_hand_over(&mut self) {
        self.awake.held = false;
        self.awake.idle_since = None;
        drop(self.awake.hold.take());
    }
}
