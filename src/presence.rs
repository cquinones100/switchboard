//! Whether you're at the computer, the one way everything here tells: the
//! Slack relay, to hold its notices while you're at the computer, and the
//! server, to pause its lookups while you're away. Each decides how
//! long idle counts as away for it; what idle is, is decided here, from
//! `platform::presence`: macOS's input idle time and lock screen, the Linux
//! desktop's; where there is no desktop to ask, the server (and the relay,
//! asking the server) goes by the last key pressed in its clients.
//! A locked screen is away at once, however recently a key was pressed.

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

#[cfg(test)]
mod tests {
    use super::*;

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
}
