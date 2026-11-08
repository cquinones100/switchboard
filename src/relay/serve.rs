//! The server's side of the relay (tmux/slack-relay-open.sh): M-S opens the
//! relay in a tab of its own, or goes to that tab if it is already running,
//! and the palette's "Stop the Slack relay" stops it. Each runs
//! `switchboard slack-relay start|stop --say` in the background, which
//! reaches the server back over its socket, as the binding's script ran
//! `slack-relay` and said what went wrong in a box (say.sh); the server
//! never waits on it.

use crate::mux::server::Server;

impl Server {
    /// M-S (`start`), or stopping the relay from the palette.
    pub fn slack_relay(&mut self, start: bool) {
        let Ok(exe) = std::env::current_exe() else {
            return;
        };
        if tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        let socket = crate::mux::socket::socket_path();
        let spawned = tokio::process::Command::new(exe)
            .args(["slack-relay", if start { "start" } else { "stop" }, "--say"])
            .env("SWITCHBOARD", socket)
            .env_remove("SWITCHBOARD_PANE")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
        if let Ok(mut child) = spawned {
            tokio::spawn(async move {
                let _ = child.wait().await;
            });
        }
    }
}
