//! The request audit: every request Switchboard makes to a service outside
//! this computer, counted as it is made, for mission control's Usage view.
//! Each is one line, `<epoch>\t<service>\t<protocol>\t<what made it>`,
//! appended to `~/.local/state/switchboard/requests/<day>.tsv`, a file a day,
//! kept `KEEP_DAYS`. The service is who answers (GitHub, Linear, Slack,
//! Anthropic, ...); the protocol how it is reached: "GraphQL" and "REST" over
//! HTTPS, "WebSocket", "iCal" over HTTPS, "MCP" (a Claude connector, reached
//! through a headless `claude -p`, which is counted too, as the Anthropic API
//! under "claude -p", once a run however many turns it takes), "git" and
//! "HTTPS" for a plain page. Counting never fails what it counts.

use std::fs;
use std::path::PathBuf;
use std::sync::LazyLock;

use indexmap::IndexMap;
use regex::Regex;

use crate::paths;
use crate::util;

pub const KEEP_DAYS: i64 = 14;

fn dir() -> PathBuf {
    paths::state_dir().join("requests")
}

fn file_for(day: &jiff::Zoned) -> PathBuf {
    dir().join(format!("{}.tsv", util::ymd(day)))
}

fn clean(text: &str) -> String {
    text.replace(['\t', '\n'], " ")
}

// A host on this computer (a page served locally, read in the link view) is
// no service outside it, so it isn't counted.
static LOCAL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(localhost|127\.\d+\.\d+\.\d+|\[?::1\]?|0\.0\.0\.0)$").unwrap()
});

/// Counts `n` requests to `service` over `protocol`, made by `source`.
pub fn count(service: &str, protocol: &str, source: &str) {
    count_n(service, protocol, source, 1);
}

/// Counts `n` requests at once (a connector called several times in a run).
/// Nothing is counted in the library's own unit tests, whose home is yours:
/// a test's stand-in for Claude or a service is no request of yours.
pub fn count_n(service: &str, protocol: &str, source: &str, n: usize) {
    if LOCAL.is_match(service) || cfg!(test) {
        return;
    }
    let _ = (|| -> std::io::Result<()> {
        // Once a day across every process that counts, Rust's and the
        // shell's: .pruned holds the day it last ran.
        let marker = dir().join(".pruned");
        let today = util::ymd(&util::local_now());
        let last = fs::read_to_string(&marker)
            .map(|t| t.trim().to_string())
            .unwrap_or_default();
        if last != today {
            prune_at(&util::local_now());
            fs::create_dir_all(dir())?;
            fs::write(&marker, format!("{today}\n"))?;
        }
        fs::create_dir_all(dir())?;
        let line = format!(
            "{}\t{}\t{}\t{}\n",
            util::now_secs(),
            clean(service),
            clean(protocol),
            clean(source)
        );
        util::append_quietly(file_for(&util::local_now()), line.repeat(n.max(1)));
        Ok(())
    })();
}

/// One service's count for a day (or a week).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ServiceCount {
    pub service: String,
    pub total: usize,
    /// Each protocol's count, in the order first seen.
    pub protocols: IndexMap<String, usize>,
    /// Its count in each hour of the day (24), or each day of the week (7,
    /// the oldest first).
    pub by_time: Vec<usize>,
    /// Each source's count (what made the requests: the PR board, the tab
    /// names, CI), in the order first seen.
    pub sources: IndexMap<String, usize>,
}

/// A day's counts (or a week's): the busiest service first, and each
/// protocol's count in the order first seen.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Tally {
    pub total: usize,
    pub services: Vec<ServiceCount>,
    pub protocols: IndexMap<String, usize>,
}

/// Today's counts.
pub fn tally() -> Tally {
    tally_on(&util::local_now())
}

/// Counts taken in, a line at a time, into `buckets` slots of time.
struct Counting {
    buckets: usize,
    total: usize,
    services: IndexMap<String, ServiceCount>,
    protocols: IndexMap<String, usize>,
}

impl Counting {
    fn new(buckets: usize) -> Counting {
        Counting {
            buckets,
            total: 0,
            services: IndexMap::new(),
            protocols: IndexMap::new(),
        }
    }

    /// Takes in a file's lines, each in the slot `bucket` gives its time.
    fn take(&mut self, text: &str, bucket: impl Fn(i64) -> usize) {
        for line in text.split('\n') {
            let mut fields = line.split('\t');
            let (Some(at), Some(service), Some(protocol)) =
                (fields.next(), fields.next(), fields.next())
            else {
                continue;
            };
            if service.is_empty() || protocol.is_empty() {
                continue;
            }
            let source = fields.next().unwrap_or("").trim();
            let source = if source.is_empty() { "unknown" } else { source };
            self.total += 1;
            let buckets = self.buckets;
            // One service whatever its case (an older build counted git's
            // fetches as "github"), named as written with a capital.
            let entry = self
                .services
                .entry(service.to_lowercase())
                .or_insert_with(|| ServiceCount {
                    service: service.to_string(),
                    total: 0,
                    protocols: IndexMap::new(),
                    by_time: vec![0; buckets],
                    sources: IndexMap::new(),
                });
            if entry.service == entry.service.to_lowercase() {
                entry.service = service.to_string();
            }
            entry.total += 1;
            *entry.protocols.entry(protocol.to_string()).or_insert(0) += 1;
            *entry.sources.entry(source.to_string()).or_insert(0) += 1;
            if let Ok(at) = at.trim().parse::<i64>() {
                let slot = bucket(at).min(buckets.saturating_sub(1));
                if let Some(n) = entry.by_time.get_mut(slot) {
                    *n += 1;
                }
            }
            *self.protocols.entry(protocol.to_string()).or_insert(0) += 1;
        }
    }

    fn done(self) -> Tally {
        let mut services: Vec<ServiceCount> = self.services.into_values().collect();
        // A stable sort, so services with the same count keep the order first seen.
        services.sort_by_key(|s| std::cmp::Reverse(s.total));
        Tally {
            total: self.total,
            services,
            protocols: self.protocols,
        }
    }
}

/// The counts of the day `day` falls on, each service's by hour.
pub fn tally_on(day: &jiff::Zoned) -> Tally {
    let text = fs::read_to_string(file_for(day)).unwrap_or_default();
    let mut counting = Counting::new(24);
    counting.take(&text, |at| {
        util::local_at_ms(at * 1000).hour().max(0) as usize
    });
    counting.done()
}

/// The counts of the seven days to `now`'s, each service's by day, the
/// oldest first.
pub fn tally_week(now: &jiff::Zoned) -> Tally {
    let mut counting = Counting::new(7);
    // By the calendar, so a day of 23 or 25 hours is still one day.
    for back in (0..7).rev() {
        let Ok(day) = now.date().checked_sub(jiff::Span::new().days(back)) else {
            continue;
        };
        let name = format!("{:04}-{:02}-{:02}.tsv", day.year(), day.month(), day.day());
        let text = fs::read_to_string(dir().join(name)).unwrap_or_default();
        let slot = (6 - back) as usize;
        counting.take(&text, |_| slot);
    }
    counting.done()
}

/// The name of the file today's requests are counted in, for the view to
/// say where they come from.
pub fn file_name_today() -> String {
    format!("{}.tsv", util::ymd(&util::local_now()))
}

/// Days past `KEEP_DAYS` go.
pub fn prune() {
    prune_at(&util::local_now());
}

/// Days more than `KEEP_DAYS` before `now` go.
pub fn prune_at(now: &jiff::Zoned) {
    let then = now.timestamp().as_millisecond() - KEEP_DAYS * 86_400_000;
    let oldest = util::ymd(&util::local_at_ms(then));
    let Ok(entries) = fs::read_dir(dir()) else {
        return;
    };
    let day = Regex::new(r"^\d{4}-\d{2}-\d{2}\.tsv$").unwrap();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if day.is_match(&name) && name[..10] < *oldest {
            let _ = fs::remove_file(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosts_here_are_local() {
        for host in [
            "localhost",
            "127.0.0.1",
            "::1",
            "[::1]",
            "0.0.0.0",
            "LOCALHOST",
        ] {
            assert!(LOCAL.is_match(host), "{host}");
        }
        assert!(!LOCAL.is_match("github.com"));
    }

    #[test]
    fn a_day_is_counted_by_service_protocol_hour_and_source() {
        let at = |h: i8, m: i8| {
            jiff::civil::date(2026, 11, 11)
                .at(h, m, 0, 0)
                .to_zoned(jiff::tz::TimeZone::system())
                .unwrap()
                .timestamp()
                .as_second()
        };
        let text = format!(
            "{}\tGitHub\tGraphQL\tPR board\n{}\tGitHub\tREST\tCI\n{}\tGitHub\tGraphQL\tPR board\n{}\tLinear\tGraphQL\tticket view\nbroken line\n",
            at(9, 5),
            at(9, 40),
            at(14, 0),
            at(14, 30)
        );
        let mut counting = Counting::new(24);
        counting.take(&text, |at| {
            util::local_at_ms(at * 1000).hour().max(0) as usize
        });
        let day = counting.done();
        assert_eq!(day.total, 4);
        assert_eq!(day.protocols["GraphQL"], 3);
        let github = &day.services[0];
        assert_eq!(github.service, "GitHub");
        assert_eq!(github.by_time[9], 2);
        assert_eq!(github.by_time[14], 1);
        assert_eq!(github.sources["PR board"], 2);
        assert_eq!(github.sources["CI"], 1);
        assert_eq!(day.services[1].by_time[14], 1);
    }

    #[test]
    fn a_service_counted_in_two_cases_is_one() {
        let text = "1\tgithub\tgit\trestart\n2\tGitHub\tREST\tCI\n3\tgithub\tgit\trestart\n";
        let mut counting = Counting::new(24);
        counting.take(text, |_| 0);
        let day = counting.done();
        assert_eq!(day.services.len(), 1);
        assert_eq!(day.services[0].service, "GitHub");
        assert_eq!(day.services[0].total, 3);
        assert_eq!(day.services[0].protocols["git"], 2);
    }

    #[test]
    fn tabs_and_newlines_are_spaces() {
        assert_eq!(clean("a\tb\nc"), "a b c");
    }
}
