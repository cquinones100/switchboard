//! The messages between a client (an attached terminal, a command, a hook)
//! and the server, over the server's Unix socket. Each is a frame: a tag
//! byte, a big-endian u32 length, then the payload, which is the raw bytes
//! for `Input` and `Output` (the bulk of the traffic) and JSON for the rest.
//!
//! STUB: the signatures are the contract; step 2 fills them in.

use std::io;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// The longest frame read, so a broken peer can't make the server allocate
/// without end.
pub const MAX_FRAME: usize = 64 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClientMsg {
    /// The first message of a terminal attaching: the session to attach to
    /// (made when it isn't there), the terminal's size and what it is.
    Attach {
        session: Option<String>,
        cols: u16,
        rows: u16,
        term: String,
    },
    /// Bytes typed.
    Input(Vec<u8>),
    Resize {
        cols: u16,
        rows: u16,
    },
    /// The client is leaving; the server keeps everything running.
    Detach,
    /// The first message of a command run from a shell or a hook
    /// (`switchboard hook ...`, `switchboard tab ...`): its arguments, the
    /// directory it ran in, the pane it ran in (`SWITCHBOARD_PANE`) and its
    /// stdin when it was given some.
    Command {
        args: Vec<String>,
        cwd: PathBuf,
        pane: Option<String>,
        stdin: Option<Vec<u8>>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ServerMsg {
    /// Bytes to write to the terminal.
    Output(Vec<u8>),
    /// The client should end, with this said once the terminal is back.
    Exit { message: Option<String> },
    /// A command's result.
    Reply {
        code: i32,
        stdout: Vec<u8>,
        stderr: Vec<u8>,
    },
}

/// A frame's bytes for a client message.
pub fn encode_client(msg: &ClientMsg) -> Vec<u8> {
    let _ = msg;
    todo!("proto::encode_client")
}

/// A frame's bytes for a server message.
pub fn encode_server(msg: &ServerMsg) -> Vec<u8> {
    let _ = msg;
    todo!("proto::encode_server")
}

/// Reads one client message; `Ok(None)` at a clean end of the stream.
pub fn read_client(r: &mut impl io::Read) -> io::Result<Option<ClientMsg>> {
    let _ = r;
    todo!("proto::read_client")
}

/// Reads one server message; `Ok(None)` at a clean end of the stream.
pub fn read_server(r: &mut impl io::Read) -> io::Result<Option<ServerMsg>> {
    let _ = r;
    todo!("proto::read_server")
}

/// As `read_client`, on the server's runtime.
pub async fn read_client_async(
    r: &mut (impl tokio::io::AsyncRead + Unpin),
) -> io::Result<Option<ClientMsg>> {
    let _ = r;
    todo!("proto::read_client_async")
}
