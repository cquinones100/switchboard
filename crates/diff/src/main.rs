//! `switchboard-diff`: the diff alone, in any terminal and any repository,
//! with no Switchboard running. It answers for the workspace itself
//! (`host::Alone`): the branch is compared with the default branch (or the branch it was
//! cut from, when it tracks one), the
//! story is told by `claude -p` directly, and its files are its own.

use std::process::ExitCode;
use std::sync::Arc;

use switchboard_plugin_api::{Host, Plugin};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "-V" || a == "--version") {
        println!("switchboard-diff {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }
    // A pipe closed by what reads it (`| head`) ends the program quietly.
    // SAFETY: setting a signal's disposition back to its default.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
    let host = Arc::new(switchboard_diff::host::Alone::new());
    // Its folders are its owner's alone before anything is kept in them,
    // one made open to other users tightened: a story holds what the
    // repository does, and the folders its modules make inside are reached
    // only through these.
    if let Some(folders) = host.folders() {
        switchboard_plugin_api::sdk::make_private(&[folders.state, folders.cache]);
    }
    switchboard_diff::Diff.run("switchboard-diff", args, host)
}
