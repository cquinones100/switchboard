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
//! "ts": "…", "thread": "…" }], "threads": [{ "channel": "C…", "thread":
//! "…", "text": "…" }] }`, replied being what you wrote yourself and threads
//! the threads read, each `text` the read-thread tool's answer as it came,
//! kept for the Slack board while Settings' `slack_threads` and `ux2` are on
//! (`day::slack_thread::save_found`).

use std::io::Read;
use std::process::ExitCode;

use serde_json::Value;

use crate::day::{Dirs, ical, slack, slack_thread};
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
            // The threads read are kept, and those whose messages the save
            // let go are let go, whether any were read or not.
            let none = Vec::new();
            let threads = found
                .get("threads")
                .and_then(Value::as_array)
                .unwrap_or(&none);
            let board = crate::settings::is_on("slack_threads") && crate::settings::is_on("ux2");
            if !board {
                println!("{said}");
                return ExitCode::SUCCESS;
            }
            match slack_thread::save_found(&dirs.state, threads, now) {
                Ok(0) => println!("{said}"),
                Ok(n) => println!("{said} Kept {n} threads."),
                Err(e) => println!("{said} Couldn't keep the threads: {e}"),
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("Couldn't save it: {e}");
            ExitCode::FAILURE
        }
    }
}
