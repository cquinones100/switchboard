//! Whether you're at the computer, the one way everything here tells: the
//! Slack relay, to hold its notices while you're at the computer, and the
//! server, to pause its lookups while you're away. Each decides how
//! long idle counts as away for it; what idle is, is decided here, from
//! `platform::presence` (macOS's input idle time and lock screen, the Linux
//! desktop's) and the last key pressed in the workspace's clients
//! (`Desktop::with_keys`), whichever is the more recent.
//!
//! The desktop sees only its own keyboard and mouse, so a key pressed in a
//! client attached over SSH never reaches its idle time, and with only the
//! desktop's reading the workspace counted you away the whole time you
//! worked in it from another computer. A locked screen is away at once,
//! however recently a key was pressed, unless one is pressed in a client
//! after the lock was seen: the lock screen takes the computer's own keys,
//! so that one came from somewhere else.

use std::time::Instant;

pub use crate::platform::Presence;

/// Milliseconds since the last input, or `None` when it can't be read.
pub fn read_idle_ms() -> Option<f64> {
    read_presence().idle_ms
}

/// Whether the lock screen is up, or `None` when it can't be read.
pub fn read_locked() -> Option<bool> {
    read_presence().locked
}

/// Both at once. Blocking: on a runtime, use `read_presence_async`.
pub fn read_presence() -> Presence {
    crate::platform::presence()
}

/// Both at once, off the runtime's thread.
pub async fn read_presence_async() -> Presence {
    tokio::task::spawn_blocking(read_presence)
        .await
        .unwrap_or_default()
}

/// Away, by a reading and how long idle counts as away for the one asking.
pub fn away_by(p: &Presence, after_ms: f64) -> bool {
    p.locked == Some(true) || p.idle_ms.unwrap_or(0.0) >= after_ms
}

/// What the desktop last said, and since when it has said the screen is
/// locked, for folding in the keys pressed in the workspace's clients.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Desktop {
    pub reading: Presence,
    /// When a reading first said the screen is locked, while they still
    /// say so; a reading that can't tell leaves it as it was.
    locked_since: Option<Instant>,
}

impl Desktop {
    /// Takes a new reading.
    pub fn take(&mut self, reading: Presence) {
        self.take_at(reading, Instant::now());
    }

    fn take_at(&mut self, reading: Presence, now: Instant) {
        match reading.locked {
            Some(true) => self.locked_since = self.locked_since.or(Some(now)),
            Some(false) => self.locked_since = None,
            None => {}
        }
        self.reading = reading;
    }

    /// The last reading with the last key pressed in one of the
    /// workspace's clients, `typed_ms` ago, folded in: idle is the shorter
    /// of the desktop's and the key's (the key's alone where the desktop
    /// can't say), and a key pressed since the lock was seen unlocks it. A
    /// key from before the lock leaves it locked, as one pressed at the
    /// computer just before it was locked was.
    pub fn with_keys(&self, typed_ms: Option<f64>) -> Presence {
        self.with_keys_at(typed_ms, Instant::now())
    }

    /// The last reading with what the workspace's server says folded in:
    /// `since_ms` is how long since a key was pressed in one of its clients
    /// when `typed`, and how long since the server started when none has
    /// been. A key is folded in as `with_keys` does; the server's start is
    /// no key, so it counts only where the desktop can't say how long
    /// you've been idle, which would otherwise read as none at all.
    pub fn with_workspace(&self, since_ms: f64, typed: bool) -> Presence {
        let mut p = self.with_keys(typed.then_some(since_ms));
        p.idle_ms = p.idle_ms.or(Some(since_ms));
        p
    }

    fn with_keys_at(&self, typed_ms: Option<f64>, now: Instant) -> Presence {
        let mut p = self.reading;
        let Some(typed) = typed_ms else {
            return p;
        };
        let locked_for_ms = self
            .locked_since
            .filter(|_| p.locked == Some(true))
            .map(|at| now.saturating_duration_since(at).as_secs_f64() * 1000.0);
        if locked_for_ms.is_some_and(|locked| typed >= locked) {
            p.idle_ms = p.idle_ms.or(Some(typed));
            return p;
        }
        p.idle_ms = Some(p.idle_ms.map_or(typed, |idle| idle.min(typed)));
        if p.locked == Some(true) {
            p.locked = Some(false);
        }
        p
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn locked_is_away_at_once_and_unknown_is_here() {
        assert!(away_by(
            &Presence {
                idle_ms: Some(0.0),
                locked: Some(true)
            },
            60_000.0
        ));
        assert!(!away_by(
            &Presence {
                idle_ms: None,
                locked: None
            },
            60_000.0
        ));
        assert!(away_by(
            &Presence {
                idle_ms: Some(61_000.0),
                locked: Some(false)
            },
            60_000.0
        ));
    }

    const AFTER: f64 = 600_000.0;

    fn reading(idle_ms: Option<f64>, locked: Option<bool>) -> Presence {
        Presence { idle_ms, locked }
    }

    #[test]
    fn a_key_typed_over_ssh_counts_though_the_desktop_has_been_idle_for_days() {
        let mut d = Desktop::default();
        d.take(reading(Some(3.0 * 86_400_000.0), Some(false)));
        let p = d.with_keys(Some(5_000.0));
        assert_eq!(p, reading(Some(5_000.0), Some(false)));
        assert!(!away_by(&p, AFTER));
        assert!(away_by(&d.with_keys(Some(AFTER)), AFTER), "keys stopped");
    }

    #[test]
    fn the_desktops_input_counts_when_no_key_was_pressed_in_the_workspace() {
        let mut d = Desktop::default();
        d.take(reading(Some(2_000.0), Some(false)));
        assert_eq!(
            d.with_keys(Some(3_600_000.0)),
            reading(Some(2_000.0), Some(false))
        );
    }

    #[test]
    fn where_the_desktop_cant_say_the_last_key_is_idle() {
        let mut d = Desktop::default();
        d.take(reading(None, None));
        assert_eq!(d.with_keys(Some(7_000.0)), reading(Some(7_000.0), None));
        assert_eq!(d.with_keys(None), reading(None, None));
    }

    #[test]
    fn a_server_starting_is_no_key_but_stands_in_where_the_desktop_cant_say() {
        let mut d = Desktop::default();
        d.take(reading(Some(3.0 * 3_600_000.0), Some(false)));
        let p = d.with_workspace(2_000.0, false);
        assert_eq!(p, reading(Some(3.0 * 3_600_000.0), Some(false)));
        assert!(away_by(&p, AFTER), "started with nobody at the computer");
        assert!(!away_by(&d.with_workspace(2_000.0, true), AFTER), "a key");

        d.take(reading(Some(3.0 * 3_600_000.0), Some(true)));
        assert_eq!(d.with_workspace(0.0, false).locked, Some(true));

        d.take(reading(None, None));
        assert_eq!(
            d.with_workspace(7_000.0, false),
            reading(Some(7_000.0), None)
        );
    }

    #[test]
    fn a_key_pressed_before_the_lock_was_seen_leaves_it_locked() {
        let start = Instant::now();
        let mut d = Desktop::default();
        d.take_at(reading(Some(1_000.0), Some(true)), start);
        let later = start + Duration::from_secs(30);
        let p = d.with_keys_at(Some(31_000.0), later);
        assert_eq!(p, reading(Some(1_000.0), Some(true)));
        assert!(away_by(&p, AFTER));
    }

    #[test]
    fn a_key_pressed_after_the_lock_was_seen_came_from_another_computer() {
        let start = Instant::now();
        let mut d = Desktop::default();
        d.take_at(reading(Some(86_400_000.0), Some(true)), start);
        let later = start + Duration::from_secs(30);
        let p = d.with_keys_at(Some(10_000.0), later);
        assert_eq!(p, reading(Some(10_000.0), Some(false)));
        assert!(!away_by(&p, AFTER));
    }

    #[test]
    fn a_reading_that_cant_tell_the_lock_keeps_when_it_was_seen() {
        let start = Instant::now();
        let mut d = Desktop::default();
        d.take_at(reading(Some(0.0), Some(true)), start);
        d.take_at(reading(Some(0.0), None), start + Duration::from_secs(60));
        d.take_at(
            reading(Some(0.0), Some(true)),
            start + Duration::from_secs(120),
        );
        // Typed 90s ago: after the lock was first seen, before it was seen
        // again.
        let p = d.with_keys_at(Some(90_000.0), start + Duration::from_secs(180));
        assert_eq!(p.locked, Some(false));
        d.take_at(
            reading(Some(0.0), Some(false)),
            start + Duration::from_secs(200),
        );
        d.take_at(
            reading(Some(0.0), Some(true)),
            start + Duration::from_secs(240),
        );
        let p = d.with_keys_at(Some(90_000.0), start + Duration::from_secs(250));
        assert_eq!(p.locked, Some(true), "unlocked and locked again since");
    }
}
