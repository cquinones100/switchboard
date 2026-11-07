//! `switchboard landing [--json] [--refresh] [<dir>]`: how the repository at
//! `<dir>` (the current directory by default) lands its work, `pr` (through
//! a pull request) or `direct` (straight on its default branch), on the
//! first line, then, on the second, a direct landing's steps as JSON
//! (`test`, `squash`, `review`, `beforePush`) with where the answer came
//! from (`source`), why, and the repository. `--json` prints both as one
//! object; `--refresh` asks GitHub and Claude again rather than taking the
//! answer kept from the last day. It fails when repos.json can't be read.
//! queue-and-clean asks it which way to land a branch with no PR, and how.
//! The answer is src/repos.rs's. Ported from bin/landing.

use std::path::PathBuf;
use std::process::ExitCode;

use serde_json::{Value, json};

use crate::repos;

pub fn main(args: Vec<String>) -> ExitCode {
    let as_json = args.iter().any(|a| a == "--json");
    let refresh = args.iter().any(|a| a == "--refresh");
    if let Some(bad) = args
        .iter()
        .find(|a| a.starts_with('-') && !["--json", "--refresh"].contains(&a.as_str()))
    {
        eprintln!(
            "landing: unknown option {bad}\nusage: switchboard landing [--json] [--refresh] [<dir>]"
        );
        return ExitCode::from(2);
    }
    let dir = args
        .iter()
        .find(|a| !a.starts_with('-'))
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    match repos::landing_for(&dir, refresh) {
        Ok(answer) => {
            let mut rest = answer.steps.json();
            if let Value::Object(map) = &mut rest {
                map.insert("source".into(), json!(answer.source));
                map.insert("why".into(), json!(answer.why));
                map.insert("repo".into(), json!(answer.repo));
            }
            if as_json {
                let mut all = serde_json::Map::new();
                all.insert("landing".into(), json!(answer.landing));
                if let Value::Object(map) = rest {
                    all.extend(map);
                }
                println!("{}", Value::Object(all));
            } else {
                println!("{}\n{rest}", answer.landing);
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("landing: {e}");
            ExitCode::from(1)
        }
    }
}
