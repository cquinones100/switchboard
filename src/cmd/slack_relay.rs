//! `slack-relay`: the Slack relay (bin/slack-relay). Everything it does is
//! in src/relay; this only hands it the arguments (src/relay/cli.rs).

use std::process::ExitCode;

pub fn main(args: Vec<String>) -> ExitCode {
    crate::relay::cli::main(args)
}
