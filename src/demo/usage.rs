//! The demo's made-up usage, so mission control's Usage view (board 24 of
//! the UX 2.0 canvas) has a day and a week to chart: what the statusline
//! would have logged of each session's spend (`usage/spent-<day>.tsv` and
//! today's `usage/spend-<session>`, src/usage/charts.rs), which demo tab ran
//! each session (`usage/sessions-tabs.tsv`), the rate limits as the
//! statusline saves them (`usage/limits`), and the request audit by hour
//! (`requests/<day>.tsv`, src/requests.rs). New with the demo drawing as
//! Ghostty does, with no tmux-version file behind it.
//!
//! Everything is timed from when the demo is laid out, as the meetings are:
//! today's spend and requests run from a few hours before then up to it,
//! and the six days before are working days of spend and requests, lighter
//! at the weekend. The amounts come from a fixed seed, so every demo shows
//! the same shape. A tab is named in the tabs' file by its number, name and
//! color, the color worked out from its directory as the tab's own is, with
//! no tab id or server, so the view shows it by those whatever the server's
//! process is; an earlier tab, closed since, has spend on the week alone.

use std::fmt::Write as _;
use std::fs;
use std::path::Path;

use anyhow::Result;
use serde_json::json;

use crate::tabs::color::{Shade, accent_color_of};
use crate::util;

const HOUR_MS: i64 = 3_600_000;
const DAY_MS: i64 = 24 * HOUR_MS;

/// How long the rate limits' windows are, and how full each is.
const FIVE_HOURS_SECS: i64 = 5 * 3600;
const WEEK_SECS: i64 = 7 * 86_400;
const FIVE_HOUR_USED: f64 = 46.0;
const WEEK_USED: f64 = 31.0;

/// The services the request audit counts, each with its protocol, what
/// made the requests, and about how many an hour.
const REQUESTS: &[(&str, &str, &str, u64)] = &[
    ("GitHub", "GraphQL", "PR board", 24),
    ("GitHub", "REST", "CI", 14),
    ("GitHub", "git", "fetch", 4),
    ("Linear", "GraphQL", "ticket view", 10),
    ("Slack", "MCP", "slack_search_public", 3),
    ("Anthropic", "claude -p", "turn summary", 5),
];

/// A small, fixed sequence of numbers between 0 and 1.
struct Seed(u64);

impl Seed {
    fn next(&mut self) -> f64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// A tab the made-up spend ran in: its number, name, color and how much of
/// the day's spend it takes.
struct Tab {
    index: usize,
    name: String,
    color: String,
    weight: f64,
}

/// The demo's tabs that run Claude, as `super::tabs` opens them (its home
/// tab is 1), and one closed before today.
fn tabs(home: &Path) -> Vec<Tab> {
    let weights = [1.0, 0.7, 0.35];
    let mut out: Vec<Tab> = super::tabs(home, Path::new("switchboard"))
        .into_iter()
        .filter(|(_, _, program)| program.first().is_some_and(|p| p == "claude"))
        .zip(weights)
        .enumerate()
        .map(|(i, ((name, dir, _), weight))| Tab {
            index: i + 2,
            name,
            color: accent_color_of(&dir, Shade::of(None)).hex(),
            weight,
        })
        .collect();
    out.push(Tab {
        index: out.len() + 3,
        name: "Fix the avatar upload on Safari".into(),
        color: "#9ad1b5".into(),
        weight: 0.5,
    });
    out
}

/// Lays out the usage files under the state directory, `now_ms` being
/// when the demo is laid out.
pub fn lay_out(state: &Path, home: &Path, now_ms: i64) -> Result<()> {
    let usage = state.join("usage");
    let requests = state.join("requests");
    fs::create_dir_all(&usage)?;
    fs::create_dir_all(&requests)?;
    let tabs = tabs(home);
    let now = util::local_at_ms(now_ms);
    let midnight = now
        .start_of_day()
        .map_or(now_ms - 12 * HOUR_MS, |z| z.timestamp().as_millisecond());
    let mut seed = Seed(0x5eed);
    let mut tabs_file = String::new();

    // Today, by quarter hour from up to five hours before now, the open
    // tabs taking turns: the first busiest early, the last late.
    let today = util::ymd(&now);
    let start = (now_ms - 5 * HOUR_MS).max(midnight);
    let mut log = String::new();
    let open = &tabs[..tabs.len() - 1];
    for (t, tab) in open.iter().enumerate() {
        let session = format!("demo-today-{}", tab.index);
        let mut total = 0.0;
        let mut at = start + t as i64 * 20 * 60_000;
        while at < now_ms {
            let through = (at - start) as f64 / (now_ms - start).max(1) as f64;
            let lean = if t == 0 { 1.4 - through } else { 0.5 + through };
            let dollars = (tab.weight * lean * (0.2 + 0.6 * seed.next())).max(0.01);
            total += dollars;
            writeln!(log, "{}\t{session}\t{dollars:.4}", at / 1000)?;
            at += 15 * 60_000 + (seed.next() * 10.0 * 60_000.0) as i64;
        }
        util::write_atomic(
            usage.join(format!("spend-{session}")),
            format!("{today}\t{total:.4}\t{total:.4}\t\t2"),
        )?;
        writeln!(
            tabs_file,
            "{today}\t{session}\t{}\t{}\t{}\t\t0",
            tab.index, tab.name, tab.color
        )?;
    }
    util::write_atomic(usage.join(format!("spent-{today}.tsv")), &log)?;
    util::write_atomic(
        requests.join(format!("{today}.tsv")),
        requests_between(start, now_ms, 1.0, &mut seed)?,
    )?;

    // The six days before, from nine to six, each tab a session a day.
    for back in 1..=6 {
        let day_start = midnight - back * DAY_MS + 12 * HOUR_MS;
        let day = util::local_at_ms(day_start);
        let ymd = util::ymd(&day);
        let weekend = matches!(
            day.weekday(),
            jiff::civil::Weekday::Saturday | jiff::civil::Weekday::Sunday
        );
        let busy = if weekend { 0.15 } else { 1.0 };
        let (from, to) = (day_start - 3 * HOUR_MS, day_start + 6 * HOUR_MS);
        let mut log = String::new();
        for tab in &tabs {
            // The closed tab ran early in the week, the dark mode tab only
            // these last days.
            let ran = if tab.index > open.len() + 1 {
                back >= 3
            } else {
                tab.index != 3 || back <= 3
            };
            if !ran {
                continue;
            }
            let session = format!("demo-{back}-{}", tab.index);
            let mut at = from + (seed.next() * HOUR_MS as f64) as i64;
            while at < to {
                let dollars = (tab.weight * busy * (0.3 + 0.9 * seed.next())).max(0.01);
                writeln!(log, "{}\t{session}\t{dollars:.4}", at / 1000)?;
                at += 25 * 60_000 + (seed.next() * 30.0 * 60_000.0) as i64;
            }
            writeln!(
                tabs_file,
                "{ymd}\t{session}\t{}\t{}\t{}\t\t0",
                tab.index, tab.name, tab.color
            )?;
        }
        util::write_atomic(usage.join(format!("spent-{ymd}.tsv")), &log)?;
        util::write_atomic(
            requests.join(format!("{ymd}.tsv")),
            requests_between(from, to, busy, &mut seed)?,
        )?;
    }
    util::write_atomic(usage.join("sessions-tabs.tsv"), &tabs_file)?;
    limits(state, now_ms, now_ms)
}

/// The request audit's lines from `from` to `to`, each service about its
/// count an hour times `busy`.
fn requests_between(from: i64, to: i64, busy: f64, seed: &mut Seed) -> Result<String> {
    let mut out = String::new();
    let mut hour = from;
    while hour < to {
        for (service, protocol, source, per_hour) in REQUESTS {
            let n = (*per_hour as f64 * busy * (0.5 + seed.next())).round() as i64;
            for _ in 0..n {
                let at = hour + (seed.next() * HOUR_MS as f64) as i64;
                if at < to {
                    writeln!(out, "{}\t{service}\t{protocol}\t{source}", at / 1000)?;
                }
            }
        }
        hour += HOUR_MS;
    }
    Ok(out)
}

/// The rate limits as the statusline saves them, each window resetting on
/// a step of its length from `laid_out`, the first still to come at `now_ms`,
/// so the view always has them; written again with the day's data.
pub fn limits(state: &Path, laid_out: i64, now_ms: i64) -> Result<()> {
    let next = |window: i64| {
        let base = laid_out / 1000 + window * 2 / 5;
        let now = now_ms / 1000;
        base + (now - base).max(0) / window * window + if now >= base { window } else { 0 }
    };
    util::write_atomic(
        state.join("usage/limits"),
        util::json_compact(&json!({
            "five_hour": { "used_percentage": FIVE_HOUR_USED, "resets_at": next(FIVE_HOURS_SECS) },
            "seven_day": { "used_percentage": WEEK_USED, "resets_at": next(WEEK_SECS) },
        })),
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_limits_reset_still_to_come_whenever_they_are_written() {
        let dir = std::env::temp_dir().join(format!("demo-usage-{}", std::process::id()));
        fs::create_dir_all(dir.join("usage")).unwrap();
        let laid_out = 1_700_000_000_000;
        for later in [0, HOUR_MS, 3 * HOUR_MS, 26 * HOUR_MS, 9 * DAY_MS] {
            let now = laid_out + later;
            limits(&dir, laid_out, now).unwrap();
            let v = util::read_json(dir.join("usage/limits")).unwrap();
            for (key, window) in [("five_hour", FIVE_HOURS_SECS), ("seven_day", WEEK_SECS)] {
                let resets = v[key]["resets_at"].as_i64().unwrap();
                assert!(resets > now / 1000, "{key} at +{later}");
                assert!(resets <= now / 1000 + window, "{key} at +{later}");
            }
        }
        let _ = fs::remove_dir_all(&dir);
    }
}
