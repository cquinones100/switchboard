//! `land-lock`: The landing lock and queue. Ported from `claude/skills/queue-and-clean/scripts/land-lock.js`.

use std::process::ExitCode;

pub fn main(_args: Vec<String>) -> ExitCode {
    super::not_yet("land-lock")
}
