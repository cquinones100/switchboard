//! `switchboard`: every program Switchboard has, in one binary. Run as
//! `switchboard <command>`, or through a link named after the command (a
//! `wt-jump` linked to the binary), it runs that command; alone, it starts
//! the workspace or attaches to it. Under any name that isn't a command's,
//! it is `switchboard`: install.sh and `switchboard update` run a new
//! binary once under a temporary name before putting it in place.

use std::path::Path;
use std::process::ExitCode;

fn main() -> ExitCode {
    let mut args = std::env::args_os().map(|a| a.to_string_lossy().into_owned());
    let argv0 = args.next().unwrap_or_default();
    let called = Path::new(&argv0)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let rest: Vec<String> = args.collect();
    // A link named after a command runs it; any other name is switchboard.
    let link = called != "switchboard" && switchboard::cmd::is_command(&called);
    let command = if link {
        Some(called.as_str())
    } else {
        rest.first().map(String::as_str)
    };
    // A command that prints, piped into something that stops reading
    // (`| head`), ends quietly as other programs do, rather than panicking
    // on the broken pipe Rust's ignored SIGPIPE turns into an error. Not the
    // server, the attached client or a hook, which must outlive a peer gone.
    if !matches!(
        command,
        None | Some("server" | "hook" | "slack-relay" | "-s")
    ) {
        // SAFETY: setting a signal's disposition back to its default.
        unsafe {
            libc::signal(libc::SIGPIPE, libc::SIG_DFL);
        }
    }
    if !link {
        let mut rest = rest.into_iter();
        let name = rest.next();
        // `switchboard -s <session>` is the workspace too.
        match name.as_deref() {
            Some("-s") => switchboard::cmd::run(
                None,
                std::iter::once("-s".to_string()).chain(rest).collect(),
            ),
            _ => switchboard::cmd::run(name.as_deref(), rest.collect()),
        }
    } else {
        switchboard::cmd::run(Some(&called), rest)
    }
}
