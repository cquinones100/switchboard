//! `switchboard-notes`: Notes alone, in any terminal, on `$NOTES_DIR` or
//! `~/Notes`, with no Switchboard running. It answers for the workspace
//! itself (`host::Alone`): there is no tab to go to, no work to start on an
//! item and nothing that opens a link or shows a picture, so those are
//! left out, and its files are its own.

use std::process::ExitCode;
use std::sync::Arc;

use switchboard_plugin_api::{Host, Plugin};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "-V" || a == "--version") {
        println!("switchboard-notes {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }
    // A pipe closed by what reads it (`| head`) ends the program quietly.
    // SAFETY: setting a signal's disposition back to its default.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
    let host = Arc::new(switchboard_notes::host::Alone::new());
    // Its folders are its owner's alone before anything is kept in them:
    // the palette's picks and the app's socket name your notes.
    if let Some(folders) = host.folders() {
        switchboard_plugin_api::sdk::make_private(&[folders.state, folders.cache]);
    }
    switchboard_notes::Notes.run("switchboard-notes", args, host)
}
