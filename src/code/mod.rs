//! A code file a pane names, read beside it: `switchboard code` (`view`),
//! the shared Vim read-only (crates/vim) over the diff's colors
//! (crates/highlight); the paths a pane's text names (`find`); and M-p's
//! "Open a file from this pane in the side panel" on the server (`serve`),
//! behind Settings' `code_view`. New, with no tmux-version file behind it.

use std::process::ExitCode;

pub mod find;
pub mod serve;
pub mod view;

/// `switchboard code <file>[:<line>]`.
pub fn main(args: Vec<String>) -> ExitCode {
    let [arg] = args.as_slice() else {
        eprintln!("Usage: switchboard code <file>[:<line>]");
        return ExitCode::from(2);
    };
    let (file, line) = view::parse_arg(arg);
    view::run(file, line)
}
