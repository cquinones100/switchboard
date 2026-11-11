//! `switchboard landing [--json] [--refresh] [<dir>]`: how the repository at
//! `<dir>` (the current directory by default) lands its work, `pr` (through
//! a pull request) or `direct` (straight on its default branch), on the
//! first line, then, on the second, a direct landing's steps as JSON
//! (`test`, `testIn`, `squash`, `review`, `beforePush`, `afterRebase`) with where the answer came
//! from (`source`), why, and the repository. `--json` prints both as one
//! object; `--refresh` asks GitHub and Claude again rather than taking the
//! answer kept from the last day. It fails when repos.json can't be read.
//! queue-and-clean asks it which way to land a branch with no PR, and how.
//! `target` is the branch a direct landing goes onto when it isn't the
//! default branch: the one autopilot cut the branch from, which `wtn
//! --target` recorded as `branch.<branch>.wtnTarget`, with the remote branch
//! it tracks (`remote` and `remoteBranch`, null when it tracks none, and
//! then the landing only moves the local branch); null for the default
//! branch.
//! The answer is src/repos.rs's. Ported from bin/landing.

use std::path::PathBuf;
use std::process::ExitCode;

use serde_json::{Value, json};

use crate::cmd::worktree_from_note::tracked;
use crate::{repos, worktree};

/// The landing's target, as the module's opening says.
fn target_at(dir: &std::path::Path) -> Value {
    let Ok(branch) = worktree::git_at(dir, &["rev-parse", "--abbrev-ref", "HEAD"]) else {
        return Value::Null;
    };
    let Some(target) = worktree::git_at(
        dir,
        &["config", "--get", &format!("branch.{branch}.wtnTarget")],
    )
    .ok()
    .filter(|t| !t.is_empty()) else {
        return Value::Null;
    };
    let (remote, remote_branch) = match tracked(dir, &target) {
        Some((r, b)) => (json!(r), json!(b)),
        None => (Value::Null, Value::Null),
    };
    json!({ "branch": target, "remote": remote, "remoteBranch": remote_branch })
}

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
                map.insert("target".into(), target_at(&dir));
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
