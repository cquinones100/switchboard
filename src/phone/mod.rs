//! The phone app: `switchboard phone`, a web app for answering and
//! messaging your Claude tabs from your phone, served from this binary on
//! your tailnet alone. The server's side is `serve`, the commands it asks
//! over the socket.
pub mod cli;
pub mod config;
pub mod guard;
pub mod log;
pub mod pairing;
#[cfg(feature = "net")]
pub mod push;
pub mod serve;
pub mod tailnet;
pub mod turns;
#[cfg(feature = "net")]
pub mod web;
