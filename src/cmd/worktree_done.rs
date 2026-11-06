//! `worktree-done`: Finish with a worktree. Ported from `bin/worktree-done`.

use std::process::ExitCode;

pub fn main(_args: Vec<String>) -> ExitCode {
    super::not_yet("worktree-done")
}
