//! `switchboard autopilot-duplicate '<which>'`: what a tab autopilot opened
//! runs when its Claude finds the item or issue it was opened for already
//! started or done elsewhere (another item, issue, PR or tab), as its first
//! prompt tells it to (work_tab.rs's `duplicates_first`). It marks
//! its own tab `@autopilot_refused` with "Duplicate of <which>", which
//! autopilot reads to skip the item with that reason, close the tab and, for
//! an issue, comment that it is one (src/autopilot/serve.rs). That
//! option is one of Switchboard's own marks, which `switchboard set` typed in
//! a pane may not set, so this is the one narrow way a tab's Claude sets it:
//! only on its own tab, only in one autopilot opened. No tmux-version file is
//! behind it.

use std::process::ExitCode;

use crate::mux::socket;

const USAGE: &str = "Usage: switchboard autopilot-duplicate '<which item, issue, PR or tab already has this work>'\n";

pub fn main(args: Vec<String>) -> ExitCode {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        eprint!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    let which = crate::worktree::one_line(&args.join(" "));
    let which = which.trim();
    if which.is_empty() {
        eprint!("{USAGE}");
        return ExitCode::from(2);
    }
    if !crate::cmd::work_tab::in_autopilot_tab() {
        eprintln!(
            "autopilot-duplicate: this tab isn't autopilot's (never opened by it, or made yours since)"
        );
        return ExitCode::from(1);
    }
    let why = format!("Duplicate of {which}");
    match socket::ask(&["set", crate::autopilot::REFUSED, &why]) {
        Ok((0, _, _)) => {
            println!("{why}: autopilot skips this item and closes the tab");
            ExitCode::SUCCESS
        }
        Ok((_, _, err)) => {
            eprintln!("autopilot-duplicate: {}", err.trim());
            ExitCode::from(1)
        }
        Err(e) => {
            eprintln!("autopilot-duplicate: {e}");
            ExitCode::from(1)
        }
    }
}
