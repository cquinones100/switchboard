//! Keeping the computer awake while tabs work, which nothing in the tmux
//! version did: a Mac left alone while autopilot's tabs worked slept, and
//! their work stopped until you were back, unless a `caffeinate` you ran in
//! a tab of its own held it, whether anything worked or not.
//!
//! On every tick the server counts the tabs whose Claude is working, in the
//! foreground or between turns with background work or watches running (a
//! /loop between its ticks counts for nothing, since it may wait for hours),
//! and, while autopilot is on and not paused, its tabs open and not landed.
//! While any count (and Settings' `keep_awake` is on), or while M-p's "Keep
//! the computer awake until I say" holds it by hand, the server holds
//! `platform::keep_awake` (`caffeinate -is` on macOS, a logind inhibitor on
//! Linux): the computer doesn't sleep when idle, though the display does,
//! and a laptop closed with no display attached sleeps anyway. Once nothing
//! counts it lets go after `GRACE`, so the moment between one tool call and
//! the next, or between a loop's rounds, doesn't let go and take it again;
//! a hold that couldn't be taken isn't asked for again until then.
//!
//! The rule is here, on plain data; `serve` puts it on the server: the
//! counts on its tick, the hold taken and let go off its task (taking it
//! starts a process), the palette's command, the ☕ on the status line (the
//! sidebar's top line with UX 2.0) while it holds, its tip, `list`'s
//! `awake`, and the hold let go before an upgrade's exec, since
//! `caffeinate -w` waits on the server's pid, which the new build keeps.

pub mod serve;

use std::time::{Duration, Instant};

/// How long after everything is idle the hold is let go.
pub const GRACE: Duration = Duration::from_secs(120);

/// What holds the computer awake.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Why {
    /// M-p's "Keep the computer awake until I say".
    ByHand,
    /// This many tabs' Claude working.
    Working(usize),
    /// Autopilot on, with this many of its tabs open and not landed.
    Autopilot(usize),
}

impl Why {
    /// In words, after "keeping the computer awake: ".
    pub fn words(self) -> String {
        let tabs = |n: usize| format!("{n} {}", if n == 1 { "tab" } else { "tabs" });
        match self {
            Why::ByHand => "until you say".to_string(),
            Why::Working(n) => format!("{} working", tabs(n)),
            Why::Autopilot(n) => format!("autopilot has {} open", tabs(n)),
        }
    }
}

/// What holds it now, if anything: by hand first, since that holds until
/// you say, then the tabs working, then autopilot's.
pub fn why(by_hand: bool, working: usize, autopilot: usize) -> Option<Why> {
    if by_hand {
        Some(Why::ByHand)
    } else if working > 0 {
        Some(Why::Working(working))
    } else if autopilot > 0 {
        Some(Why::Autopilot(autopilot))
    } else {
        None
    }
}

/// What a check asks of the hold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Take,
    LetGo,
    Stay,
}

/// What the server keeps of it.
pub struct Awake {
    /// Held by hand, until pressed again; handed over in an upgrade.
    pub by_hand: bool,
    /// What holds it now; `None` while nothing does, the grace too.
    pub why: Option<Why>,
    /// Asked for and not let go since, taken or not.
    held: bool,
    /// The hold, once taken: `None` while it is being taken, or where it
    /// couldn't be.
    hold: Option<crate::platform::KeepAwake>,
    /// Since when nothing has held it, while it is still held.
    idle_since: Option<Instant>,
    grace: Duration,
}

impl Default for Awake {
    /// `GRACE`, or `SWITCHBOARD_AWAKE_GRACE_MS` in its place, which the
    /// tests set.
    fn default() -> Awake {
        let grace = std::env::var("SWITCHBOARD_AWAKE_GRACE_MS")
            .ok()
            .and_then(|ms| ms.trim().parse().ok())
            .map_or(GRACE, Duration::from_millis);
        Awake {
            by_hand: false,
            why: None,
            held: false,
            hold: None,
            idle_since: None,
            grace,
        }
    }
}

impl Awake {
    /// Whether the computer is held awake now.
    pub fn holding(&self) -> bool {
        self.hold.is_some()
    }

    /// The check at `now`, with `why` holding it or nothing: taken when
    /// something holds it and it isn't yet, let go once nothing has for
    /// the grace.
    pub fn step(&mut self, why: Option<Why>, now: Instant) -> Step {
        self.why = why;
        if why.is_some() {
            self.idle_since = None;
            if self.held {
                return Step::Stay;
            }
            self.held = true;
            return Step::Take;
        }
        if !self.held {
            return Step::Stay;
        }
        let since = *self.idle_since.get_or_insert(now);
        if now.duration_since(since) < self.grace {
            return Step::Stay;
        }
        self.held = false;
        self.idle_since = None;
        Step::LetGo
    }

    /// The hold taken off the task: kept while it is still wanted and none
    /// is kept yet, else given back to be let go. Gives whether it was kept.
    fn taken(
        &mut self,
        hold: Option<crate::platform::KeepAwake>,
    ) -> Result<bool, crate::platform::KeepAwake> {
        match hold {
            Some(h) if !self.held || self.hold.is_some() => Err(h),
            Some(h) => {
                self.hold = Some(h);
                Ok(true)
            }
            None => Ok(false),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn awake() -> Awake {
        Awake {
            grace: Duration::from_secs(120),
            ..Awake::default()
        }
    }

    #[test]
    fn by_hand_comes_first_then_the_tabs_working_then_autopilot() {
        assert_eq!(why(false, 0, 0), None);
        assert_eq!(why(true, 3, 2), Some(Why::ByHand));
        assert_eq!(why(false, 3, 2), Some(Why::Working(3)));
        assert_eq!(why(false, 0, 2), Some(Why::Autopilot(2)));
        assert_eq!(Why::Working(1).words(), "1 tab working");
        assert_eq!(Why::Working(3).words(), "3 tabs working");
        assert_eq!(Why::Autopilot(1).words(), "autopilot has 1 tab open");
        assert_eq!(Why::ByHand.words(), "until you say");
    }

    #[test]
    fn it_is_taken_once_and_let_go_only_after_the_grace() {
        let mut a = awake();
        let t = Instant::now();
        assert_eq!(a.step(None, t), Step::Stay, "nothing to hold");
        assert_eq!(a.step(Some(Why::Working(1)), t), Step::Take);
        assert_eq!(a.step(Some(Why::Working(2)), t), Step::Stay, "once");
        // A moment between tool calls keeps it.
        assert_eq!(a.step(None, t + Duration::from_secs(1)), Step::Stay);
        assert_eq!(
            a.step(Some(Why::Working(1)), t + Duration::from_secs(5)),
            Step::Stay
        );
        // Idle again: the grace counts from now.
        let idle = t + Duration::from_secs(10);
        assert_eq!(a.step(None, idle), Step::Stay);
        assert_eq!(a.step(None, idle + Duration::from_secs(119)), Step::Stay);
        assert_eq!(a.step(None, idle + Duration::from_secs(120)), Step::LetGo);
        assert_eq!(
            a.step(None, idle + Duration::from_secs(200)),
            Step::Stay,
            "let go once"
        );
        assert_eq!(
            a.step(Some(Why::ByHand), idle + Duration::from_secs(300)),
            Step::Take
        );
    }

    #[test]
    fn a_hold_is_kept_only_while_it_is_wanted_and_none_is_yet() {
        let hold = crate::platform::KeepAwake::stand_in;
        let mut a = awake();
        let t = Instant::now();
        assert!(a.taken(Some(hold())).is_err(), "not asked for");
        a.step(Some(Why::Working(1)), t);
        assert!(matches!(a.taken(None), Ok(false)), "couldn't be taken");
        assert!(!a.holding());
        assert!(matches!(a.taken(Some(hold())), Ok(true)));
        assert!(a.holding());
        assert!(a.taken(Some(hold())).is_err(), "one is enough");
    }
}
