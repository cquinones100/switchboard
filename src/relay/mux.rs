//! The tabs, as the relay reaches them: the remote front-end protocol
//! (src/remote/protocol.rs), which the phone app speaks too, under the
//! names the relay's parts use.

pub use crate::remote::protocol::{
    How, PaneInfo, Remote as Mux, Server, TabStage, Window, server_idle_ms, windows_of,
};
