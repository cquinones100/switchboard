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
pub fn count_n(service: &str, protocol: &str, source: &str, n: usize) {
    if LOCAL.is_match(service) {
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

/// One service's count for a day.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServiceCount {
    pub service: String,
    pub total: usize,
    /// Each protocol's count, in the order first seen.
    pub protocols: IndexMap<String, usize>,
}

/// A day's counts: the busiest service first, and each protocol's count in the
/// order first seen.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Tally {
    pub total: usize,
    pub services: Vec<ServiceCount>,
    pub protocols: IndexMap<String, usize>,
}

/// Today's counts.
pub fn tally() -> Tally {
    tally_on(&util::local_now())
}

/// The counts of the day `day` falls on.
pub fn tally_on(day: &jiff::Zoned) -> Tally {
    let text = fs::read_to_string(file_for(day)).unwrap_or_default();
    let mut services: IndexMap<String, ServiceCount> = IndexMap::new();
    let mut protocols: IndexMap<String, usize> = IndexMap::new();
    let mut total = 0;
    for line in text.split('\n') {
        let mut fields = line.split('\t');
        let (_, Some(service), Some(protocol)) = (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        if service.is_empty() || protocol.is_empty() {
            continue;
        }
        total += 1;
        let entry = services
            .entry(service.to_string())
            .or_insert_with(|| ServiceCount {
                service: service.to_string(),
                total: 0,
                protocols: IndexMap::new(),
            });
        entry.total += 1;
        *entry.protocols.entry(protocol.to_string()).or_insert(0) += 1;
        *protocols.entry(protocol.to_string()).or_insert(0) += 1;
    }
    let mut services: Vec<ServiceCount> = services.into_values().collect();
    // A stable sort, so services with the same count keep the order first seen.
    services.sort_by_key(|s| std::cmp::Reverse(s.total));
    Tally {
        total,
        services,
        protocols,
    }
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
    fn tabs_and_newlines_are_spaces() {
        assert_eq!(clean("a\tb\nc"), "a b c");
    }
}
