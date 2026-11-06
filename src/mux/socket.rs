//! Where the server listens, and reaching it: connecting, starting it when
//! none answers, and making sure only one listens.
//!
//! STUB: the signatures are the contract; step 2 fills them in.

use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::Result;

/// The server's socket: `SWITCHBOARD` (set in every pane, so a command run in
/// one reaches the server that runs it), else `<state dir>/sock`.
pub fn socket_path() -> PathBuf {
    todo!("socket::socket_path")
}

/// A connection to the running server.
pub fn connect() -> std::io::Result<UnixStream> {
    todo!("socket::connect")
}

/// A connection to the running server, starting one (`switchboard server`,
/// detached in a session of its own, its output to `<state dir>/server.log`)
/// when none answers, and waiting up to a few seconds for it to listen.
pub fn connect_or_start() -> Result<UnixStream> {
    todo!("socket::connect_or_start")
}

/// The server's listening socket, for `switchboard server`: refuses when
/// another server answers on it, takes the place of a socket left by one
/// that died, and is readable only by its owner.
pub fn bind() -> Result<tokio::net::UnixListener> {
    todo!("socket::bind")
}

/// Runs a command in the server (`ClientMsg::Command`), passing stdin when
/// it isn't a terminal, printing the reply's output and giving its code.
pub fn run_command(args: Vec<String>) -> ExitCode {
    let _ = args;
    todo!("socket::run_command")
}
