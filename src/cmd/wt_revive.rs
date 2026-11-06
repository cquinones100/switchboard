//! `wt-revive`: The worktrees no tab has open. Ported from `bin/wt-revive`.

use std::process::ExitCode;

pub fn main(_args: Vec<String>) -> ExitCode {
    super::not_yet("wt-revive")
}
