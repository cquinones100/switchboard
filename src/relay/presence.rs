//! Whether you're at the computer (lib/slack-relay/presence.js), read the
//! one way everything here reads it (src/presence.rs): you count as away
//! once nothing has been typed or clicked for `AWAY_AFTER_MS`, or at once
//! when the screen is locked. It is read every `CHECK_MS`, in the
//! background, so asking costs nothing.
//!
//! The lines the relay puts in the main DM on its own (a tab waiting on
//! you, a dialog, a stall, a failure) are what notify you, and while you're
//! at the computer the workspace already shows them. So `deliver` sends one
//! at once only when you're away, and otherwise holds it, in order, until
//! you have been away `AWAY_AFTER_MS`. Everything else goes into the tabs'
//! threads at once, so they are always complete, and replies to what you
//! send from Slack never wait. Held lines live in memory, so a relay that
//! stops drops them.
//!
//! Writing to the relay from Slack also counts as away, for
//! `SLACK_AWAY_MS` from your latest message: you're on Slack, most likely on
//! your phone, even if the computer was touched a moment ago. Unless its
//! keyboard or mouse was used within `TYPED_HERE_MS` of the message
//! arriving, which means you wrote it in Slack on the computer itself.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;

use futures_util::future::LocalBoxFuture;

use super::Relay;
use crate::presence::{Presence as Reading, away_by};
use crate::util;

pub const AWAY_AFTER_MS: f64 = 30_000.0;
pub const CHECK_MS: u64 = 5_000;
pub const SLACK_AWAY_MS: i64 = 5 * 60_000;
pub const TYPED_HERE_MS: f64 = 5_000.0;

/// A notice held while you're at the computer.
pub type Held = LocalBoxFuture<'static, anyhow::Result<()>>;

/// What is known of you: the last reading, until when Slack counts you as
/// away, and the notices held meanwhile. `read` reads the computer; the
/// tests give it readings of their own. Until the first reading, you count
/// as at the computer.
pub struct Presence {
    pub reading: Cell<Reading>,
    pub slack_until: Cell<i64>,
    pub held: RefCell<VecDeque<Held>>,
    flushing: Cell<bool>,
    pub read: Box<dyn Fn() -> LocalBoxFuture<'static, Reading>>,
}

impl Presence {
    /// The computer's own readings.
    pub fn new() -> Presence {
        Presence::reading_with(Box::new(
            || Box::pin(crate::presence::read_presence_async()),
        ))
    }

    pub fn reading_with(read: Box<dyn Fn() -> LocalBoxFuture<'static, Reading>>) -> Presence {
        Presence {
            reading: Cell::new(Reading {
                idle_ms: Some(0.0),
                locked: Some(false),
            }),
            slack_until: Cell::new(0),
            held: RefCell::new(VecDeque::new()),
            flushing: Cell::new(false),
            read,
        }
    }

    pub fn away(&self) -> bool {
        away_by(&self.reading.get(), AWAY_AFTER_MS) || util::now_ms() < self.slack_until.get()
    }
}

impl Default for Presence {
    fn default() -> Presence {
        Presence::new()
    }
}

/// Sends a notice now when you're away, else holds it.
pub async fn deliver(cx: &Relay, send: Held) {
    if cx.presence.away() {
        if let Err(e) = send.await {
            cx.log(&format!("held message failed: {e}"));
        }
        return;
    }
    let held = {
        let mut held = cx.presence.held.borrow_mut();
        held.push_back(send);
        held.len()
    };
    cx.log(&format!("at the computer: holding a notice ({held} held)"));
}

/// Sends what is held, in order, one flush at a time.
pub async fn flush(cx: &Relay) {
    if cx.presence.flushing.replace(true) {
        return;
    }
    let count = cx.presence.held.borrow().len();
    if count > 0 {
        cx.log(&format!(
            "away: sending {count} held message{}",
            if count == 1 { "" } else { "s" }
        ));
    }
    loop {
        let next = cx.presence.held.borrow_mut().pop_front();
        let Some(send) = next else {
            break;
        };
        if let Err(e) = send.await {
            cx.log(&format!("held message failed: {e}"));
        }
    }
    cx.presence.flushing.set(false);
}

/// A message from you in Slack has just arrived.
pub async fn from_slack(cx: &Relay) {
    let reading = (cx.presence.read)().await;
    if reading.idle_ms.is_some_and(|idle| idle < TYPED_HERE_MS) {
        return;
    }
    if util::now_ms() >= cx.presence.slack_until.get() {
        cx.log("away: you wrote from Slack, not the computer");
    }
    cx.presence.slack_until.set(util::now_ms() + SLACK_AWAY_MS);
    flush(cx).await;
}

/// One reading: noted, each change logged with why, so a notice that did
/// or didn't reach the phone can be explained from the log, and what is
/// held sent once you're away.
pub async fn check(cx: &Relay) {
    let fresh = (cx.presence.read)().await;
    let was = cx.presence.away();
    let mut reading = cx.presence.reading.get();
    if fresh.idle_ms.is_some() {
        reading.idle_ms = fresh.idle_ms;
    }
    if fresh.locked.is_some() {
        reading.locked = fresh.locked;
    }
    cx.presence.reading.set(reading);
    let now = cx.presence.away();
    if now != was {
        let why = if reading.locked == Some(true) {
            "screen locked".to_string()
        } else {
            format!(
                "idle {}s",
                (reading.idle_ms.unwrap_or(0.0) / 1000.0).round() as i64
            )
        };
        cx.log(&if now {
            format!("away ({why})")
        } else {
            format!("at the computer ({why})")
        });
    }
    if now {
        flush(cx).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::relay::testing::Rig;

    #[test]
    fn a_locked_screen_is_away_at_once_however_recently_a_key_was_pressed() {
        let rig = Rig::new();
        rig.at_computer();
        assert!(!rig.cx.presence.away());
        rig.set_reading(Some(0.0), Some(true));
        rig.block_on(check(&rig.cx));
        assert!(rig.cx.presence.away());
    }

    #[test]
    fn writing_from_slack_counts_as_away_unless_you_wrote_it_here() {
        let rig = Rig::new();
        rig.at_computer();
        rig.set_reading(Some(1_000.0), Some(false));
        rig.block_on(from_slack(&rig.cx));
        assert!(
            !rig.cx.presence.away(),
            "typed on the computer a moment ago"
        );
        rig.set_reading(Some(10_000.0), Some(false));
        rig.block_on(from_slack(&rig.cx));
        assert!(rig.cx.presence.away(), "on Slack for a while");
    }
}
