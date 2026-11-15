//! When autopilot should be done with its note's ready items, for the
//! Autopilot view's heading ("done by about 3:40 PM · 6 left"). Plain
//! arithmetic on what autopilot already keeps: how long its recent items
//! took from opening to landing, its slots, its tabs still working and the
//! items it would take next; no model call and no file read. New with this
//! feature, with no tmux-version file behind it.

use jiff::Zoned;

/// The time an item is taken to need until three have landed (seconds).
pub const FALLBACK: i64 = 30 * 60;

/// How many of the latest landings the typical time is taken from.
pub const RECENT: usize = 20;

/// The least a working item is still given (seconds), so one past its
/// typical time isn't counted done.
const AT_LEAST: i64 = 60;

/// What is left of the note, read off the server's task with the queue:
/// what autopilot would take, in order; each item held by open items it
/// builds on (`After:`), with every one of them; and how many wait on you (an open
/// choice it won't take, a design board to review).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Left {
    pub ready: Vec<String>,
    pub after: Vec<(String, Vec<String>)>,
    pub on_you: usize,
}

impl Left {
    /// How many items autopilot will still take, and how many wait on
    /// you, with `working` the items of its tabs still at work: an item
    /// held only by ones it will take or ones at work counts as one it will
    /// take, through a chain of them; the rest wait on you.
    pub fn counts(&self, working: &[String]) -> (usize, usize) {
        let mut done: std::collections::HashSet<&str> = self
            .ready
            .iter()
            .chain(working)
            .map(String::as_str)
            .collect();
        let mut held: Vec<&(String, Vec<String>)> = self.after.iter().collect();
        loop {
            let before = held.len();
            held.retain(|(text, on)| {
                let free = on.iter().all(|o| done.contains(o.as_str()));
                if free {
                    done.insert(text.as_str());
                }
                !free
            });
            if held.len() == before {
                break;
            }
        }
        let after = self.after.len() - held.len();
        (self.ready.len() + after, self.on_you + held.len())
    }
}

fn median(times: &[i64]) -> i64 {
    let mut sorted = times.to_vec();
    sorted.sort_unstable();
    let n = sorted.len();
    if n % 2 == 1 {
        sorted[n / 2]
    } else {
        (sorted[n / 2 - 1] + sorted[n / 2]) / 2
    }
}

/// How long an item typically takes from opening to landing (seconds),
/// from `took`, each landed item's time, oldest landing first: the median
/// of the last `RECENT`, leaving out any over three times the median of
/// all of them (an item that waited hours on you); `FALLBACK` until three
/// have landed.
pub fn typical(took: &[i64]) -> i64 {
    let took: Vec<i64> = took.iter().copied().filter(|t| *t > 0).collect();
    if took.len() < 3 {
        return FALLBACK;
    }
    let limit = median(&took) * 3;
    let recent: Vec<i64> = took
        .iter()
        .copied()
        .filter(|t| *t <= limit)
        .rev()
        .take(RECENT)
        .collect();
    median(&recent)
}

/// When the last of the work should end (seconds), or none with nothing
/// working or ready: each working item, by when it opened, ends at that
/// plus `typical` (at least a minute from `now`), and each of the `ready`
/// items takes the first of `slots` to come free for `typical`.
pub fn estimate(now: i64, slots: u32, working: &[i64], ready: usize, typical: i64) -> Option<i64> {
    if working.is_empty() && ready == 0 {
        return None;
    }
    let slots = slots.max(1) as usize;
    let mut ends: Vec<i64> = working
        .iter()
        .map(|start| (start + typical).max(now + AT_LEAST))
        .collect();
    ends.sort_unstable();
    // More working than its slots: the earliest end without a ready item
    // taking their place.
    let over = ends.len().saturating_sub(slots);
    let mut free = ends.split_off(over);
    let first_ends = ends.last().copied().unwrap_or(now);
    free.resize(slots.max(free.len()), now);
    for _ in 0..ready {
        free.sort_unstable();
        free[0] += typical;
    }
    Some(free.into_iter().fold(first_ends, i64::max))
}

/// The heading's words: "done by about <when> · <n> left", and
/// "· <n> wait on you" when some are held on you; just the latter with
/// nothing to estimate, and nothing at all with neither.
pub fn words(end: Option<i64>, left: usize, on_you: usize, now: &Zoned) -> String {
    let mut out = Vec::new();
    if let Some(end) = end {
        out.push(format!(
            "done by about {} · {left} left",
            crate::ui::when::stamp(end * 1000, now)
        ));
    }
    if on_you > 0 {
        out.push(format!("{on_you} wait on you"));
    }
    out.join(" · ")
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: i64 = 60;

    #[test]
    fn the_typical_time_leaves_out_items_that_waited_on_you() {
        let took = [30, 34, 40, 700, 20, 800].map(|m| m * MIN);
        // The median of all is 37 minutes; 700 and 800 are over three times it.
        assert_eq!(typical(&took), 32 * MIN);
    }

    #[test]
    fn fewer_than_three_landed_takes_half_an_hour() {
        assert_eq!(typical(&[]), FALLBACK);
        assert_eq!(typical(&[10 * MIN, 12 * MIN]), FALLBACK);
        assert_eq!(FALLBACK, 30 * MIN);
    }

    #[test]
    fn only_the_latest_twenty_count() {
        // All 45 have a median of 40 minutes; the latest twenty, 20.
        let mut took = vec![40 * MIN; 25];
        took.extend(vec![20 * MIN; RECENT]);
        assert_eq!(typical(&took), 20 * MIN);
    }

    #[test]
    fn two_slots_one_half_done_and_three_ready() {
        let now = 1_000_000;
        let working = [now - 15 * MIN];
        // Free now and at +15: +0→30, +15→45, +30→60.
        assert_eq!(
            estimate(now, 2, &working, 3, 30 * MIN),
            Some(now + 60 * MIN)
        );
    }

    #[test]
    fn an_item_added_moves_it_later() {
        let now = 1_000_000;
        let working = [now - 15 * MIN];
        let before = estimate(now, 2, &working, 3, 30 * MIN).unwrap();
        let after = estimate(now, 2, &working, 4, 30 * MIN).unwrap();
        assert_eq!(after - before, 15 * MIN);
    }

    #[test]
    fn nothing_working_or_ready_has_no_estimate() {
        assert_eq!(estimate(0, 2, &[], 0, 30 * MIN), None);
    }

    #[test]
    fn a_working_item_past_its_time_is_given_a_minute() {
        let now = 1_000_000;
        assert_eq!(
            estimate(now, 1, &[now - 90 * MIN], 0, 30 * MIN),
            Some(now + MIN)
        );
    }

    #[test]
    fn more_working_than_slots_waits_for_them_to_end() {
        let now = 0;
        // Three working in two slots, ending at 10, 20 and 30: the ready
        // item starts when the third would leave a slot, at 20.
        let working = [-20 * MIN, -10 * MIN, 0];
        assert_eq!(estimate(now, 2, &working, 1, 30 * MIN), Some(50 * MIN));
    }

    #[test]
    fn an_item_after_one_it_will_take_counts_and_one_held_on_you_does_not() {
        let s = |t: &str| t.to_string();
        let left = Left {
            ready: vec![s("A")],
            after: vec![
                (s("C"), vec![s("B")]),
                (s("B"), vec![s("A")]),
                (s("D"), vec![s("Working")]),
                (s("E"), vec![s("Needs a choice")]),
                (s("F"), vec![s("A"), s("Needs a choice")]),
            ],
            on_you: 1,
        };
        // B after A, C after B, D after a tab at work; E after one held, and
        // F after A and one held.
        assert_eq!(left.counts(&[s("Working")]), (4, 3));
        assert_eq!(left.counts(&[]), (3, 4));
    }

    #[test]
    fn the_words_name_the_time_the_count_and_what_waits_on_you() {
        let now = jiff::civil::date(2026, 11, 11)
            .at(13, 0, 0, 0)
            .to_zoned(jiff::tz::TimeZone::UTC)
            .unwrap();
        let secs = now.timestamp().as_second();
        assert_eq!(
            words(Some(secs + 160 * MIN), 6, 2, &now),
            "done by about 3:40 PM · 6 left · 2 wait on you"
        );
        assert_eq!(
            words(Some(secs + 12 * 60 * MIN), 1, 0, &now),
            "done by about Tomorrow 1:00 AM · 1 left"
        );
        assert_eq!(words(None, 0, 2, &now), "2 wait on you");
        assert_eq!(words(None, 0, 0, &now), "");
    }
}
