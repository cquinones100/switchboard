//! `switchboard` alone: the workspace, started or attached to (step 2 of
//! PORTING.md, the multiplexer).

use std::process::ExitCode;

pub fn main(_args: Vec<String>) -> ExitCode {
    super::not_yet("the workspace")
}
