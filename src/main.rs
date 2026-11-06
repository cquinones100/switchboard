//! `switchboard`: every program Switchboard has, in one binary. Run as
//! `switchboard <command>`, or through a link named after the command
//! (`bin/wt-jump` links here), it runs that command; alone, it starts the
//! workspace or attaches to it.

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
    if called == "switchboard" || called.is_empty() {
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
