//! `slack-updates-save`: Keep what is new for you on Slack for mission
//! control's Slack view, which reads it from `slack.json` in the state
//! directory. Ported from `claude/skills/slack-updates/scripts/save.js`; the
//! saving is `day::slack::save`, which the server's own look calls too.
//!
//!   switchboard slack-updates-save --since   prints when to look back to:
//!                                            ten minutes before the last
//!                                            look, or an hour ago the first
//!                                            time (ISO 8601, and the Unix
//!                                            time), then the Unix time to
//!                                            look for your own messages from
//!   switchboard slack-updates-save < new.json  adds what this look found
//!
//! new.json: `{ "items": [{ "kind": "mention" | "dm" | "thread" | "channel",
//! "channel": "C…", "where": "#general", "user": "U…", "from": "Ada",
//! "text": "…", "ts": "1761660000.000100", "thread": "…", "link":
//! "https://….slack.com/archives/…" }], "replied": [{ "channel": "C…",
//! "ts": "…", "thread": "…" }] }`, replied being what you wrote yourself.

use std::io::Read;
use std::process::ExitCode;

use serde_json::Value;

use crate::day::{Dirs, ical, slack};
use crate::util;

pub fn main(args: Vec<String>) -> ExitCode {
    let dirs = Dirs::here();
    let now = util::now_ms();
    if args.first().map(String::as_str) == Some("--since") {
        let (since, mine) = slack::since(&dirs.state, now);
        let iso = jiff::Timestamp::from_millisecond(since)
            .map(ical::iso)
            .unwrap_or_default();
        println!("{iso} {} {}", since.div_euclid(1000), mine.div_euclid(1000));
        return ExitCode::SUCCESS;
    }
    let mut input = String::new();
    let found: Value = match std::io::stdin()
        .read_to_string(&mut input)
        .map_err(|e| e.to_string())
        .and_then(|_| serde_json::from_str(&input).map_err(|e| e.to_string()))
    {
        Ok(v) => v,
        Err(e) => {
            eprintln!("Not JSON: {e}");
            return ExitCode::FAILURE;
        }
    };
    match slack::save(&dirs.state, &dirs.relay, &found, now) {
        Ok(said) => {
            println!("{said}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("Couldn't save it: {e}");
            ExitCode::FAILURE
        }
    }
}
