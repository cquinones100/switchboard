//! A tab past 9 by its number's digits: `M-1` then `6` goes to tab 16,
//! `M-1` `2` `3` to tab 123 (new, no tmux version had it, where `M-0` asked
//! for the number in a box). The server holds a client's digits while an
//! open tab's number could still grow from them, each digit typed within
//! Settings' `tab_digit_delay` taken for the number rather than the pane,
//! and goes to the tab they name once the wait passes or another key comes
//! (`Server::tab_digit_input`). With no tab whose number is longer and starts
//! with them, nothing waits, so with tabs 1 to 9 `M-1` goes at once.

use std::time::Duration;

/// How long to wait for the next digit when Settings' `tab_digit_delay`
/// can't be read.
const DEFAULT_DELAY: u64 = 500;

/// Whether an open tab's number starts with `digits` and is longer, so a
/// further digit could still name it.
pub fn could_grow(digits: &str, open: impl IntoIterator<Item = usize>) -> bool {
    open.into_iter().any(|n| {
        let n = n.to_string();
        n.len() > digits.len() && n.starts_with(digits)
    })
}

/// How long to wait for the next digit: Settings' `tab_digit_delay`.
pub fn delay() -> Duration {
    let ms = crate::settings::cached("tab_digit_delay")
        .and_then(|v| v.as_str().and_then(|s| s.parse().ok()).or(v.as_u64()))
        .unwrap_or(DEFAULT_DELAY);
    Duration::from_millis(ms)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waits_only_for_a_longer_number_that_starts_with_the_digits() {
        assert!(could_grow("1", [1, 16]));
        assert!(could_grow("1", [123]));
        assert!(could_grow("12", [123]));
        assert!(!could_grow("1", 1..=9));
        assert!(!could_grow("2", [1, 16]));
        assert!(!could_grow("16", [1, 16]));
        assert!(!could_grow("1", []));
    }
}
