//! The phone app: `switchboard phone`, a web app for answering and
//! messaging your Claude tabs from your phone, served from this binary on
//! your tailnet alone. The server's side is `serve`, the commands it asks
//! over the socket.
#[cfg(feature = "net")]
pub mod build;
pub mod cli;
pub mod config;
pub mod drawer;
pub mod guard;
pub mod images;
pub mod log;
pub mod looks;
pub mod mission;
pub mod pairing;
#[cfg(feature = "net")]
pub mod preview;
#[cfg(feature = "net")]
pub mod push;
pub mod serve;
pub mod tab_views;
pub mod tailnet;
pub mod turns;
pub mod views;
#[cfg(feature = "net")]
pub mod web;
