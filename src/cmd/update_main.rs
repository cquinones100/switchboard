//! `update-main`: Bring the main clone's default branch up to date. Ported from `bin/update-main`.

use std::process::ExitCode;

pub fn main(_args: Vec<String>) -> ExitCode {
    super::not_yet("update-main")
}
