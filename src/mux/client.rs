//! The attached terminal: raw mode, the alternate screen, mouse, focus and
//! paste reporting on; what is typed goes to the server, what the server
//! draws goes to the terminal; a resize is sent as it happens. However it
//! ends (detached, the server gone, a signal), the terminal is put back as
//! it was.
//!
//! STUB: the signatures are the contract; step 2 fills them in.

use std::process::ExitCode;

use anyhow::Result;

/// Attaches this terminal to `session` (the server's default when `None`),
/// starting the server when none runs, until the server says to end.
pub fn attach(session: Option<String>) -> Result<ExitCode> {
    let _ = session;
    todo!("client::attach")
}
