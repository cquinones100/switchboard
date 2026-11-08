//! `wt-batch <tickets...>`: opens one tab per Linear ticket and runs `wt
//! <ticket>` in each, so a handful of tickets can be started in one go.
//! Ported from bin/wt-batch. Tickets are separated by commas, spaces, or
//! both, and full Linear issue URLs work in place of identifiers.
//!
//! ```text
//! wt-batch ABC-123 ABC-124
//! wt-batch "ABC-123, ABC-124"
//! wt-batch ABC-123, https://linear.app/team/issue/ABC-124/some-title
//! ```
//!
//! The tabs go at the end of the tab list, in the order given, and focus
//! stays where it is; the tabs fill in behind you. Each is named from its
//! directory until `wt` has moved it into the ticket's worktree, and then
//! after the ticket, as the server names every tab; tmux's version named the
//! window after the ticket first, which here would stand over the ticket's
//! title for good.
//!
//! `wt` runs in each new tab's interactive shell, for the same reason `wt` is
//! a shell function in the first place: the shell has to be the thing that
//! cd's, so it stays in the worktree after Claude exits and new panes split
//! from it open there too. The tab's shell defines `wt` from
//! `worktree-from-ticket init` and runs it as its command
//! (`worktree::tab_running`), where tmux's version typed `wt <ticket>` into
//! a shell that might not have started yet.

use std::process::ExitCode;

use crate::mux::socket;
use crate::worktree::{self, fail};

fn usage() -> ExitCode {
    eprintln!(
        "Usage: wt-batch <tickets...>\n\nTickets may be separated by commas, spaces, or both.\nExamples:\n  wt-batch ABC-123 ABC-124\n  wt-batch \"ABC-123, ABC-124\""
    );
    ExitCode::from(1)
}

/// Every ticket given, as its identifier in capitals, each once, in order;
/// the first that isn't a ticket is the error. Every token is checked before
/// any tab opens, so a typo in the third ticket does not leave the first two
/// half-started, and only an identifier ever reaches the tab's shell.
pub fn tickets_of(args: &[String]) -> Result<Vec<String>, String> {
    let mut tickets: Vec<String> = Vec::new();
    for token in args
        .iter()
        .flat_map(|a| a.split(|c: char| c.is_whitespace() || c == ','))
        .filter(|t| !t.is_empty())
    {
        let ticket = worktree::ticket_from(token)
            .ok_or_else(|| format!("could not parse a Linear ticket identifier from \"{token}\""))?
            .to_uppercase();
        if !tickets.contains(&ticket) {
            tickets.push(ticket);
        }
    }
    Ok(tickets)
}

pub fn main(args: Vec<String>) -> ExitCode {
    if args.is_empty() || args.iter().any(|a| a == "--help" || a == "-h") {
        return usage();
    }
    if !socket::in_pane() {
        return fail("not inside Switchboard");
    }
    let tickets = match tickets_of(&args) {
        Ok(tickets) => tickets,
        Err(why) => return fail(&why),
    };
    if tickets.is_empty() {
        return usage();
    }
    let cwd = std::env::current_dir()
        .map(|d| d.to_string_lossy().into_owned())
        .unwrap_or_else(|_| ".".into());
    for ticket in tickets {
        let command =
            worktree::tab_running("worktree-from-ticket", "wt", std::slice::from_ref(&ticket));
        let mut ask: Vec<&str> = vec!["new-tab", "-d", "-E", "-c", &cwd, "--"];
        ask.extend(command.iter().map(String::as_str));
        match socket::ask_ok(&ask) {
            Ok(out) => eprintln!(
                "Opened {ticket} in window {}",
                out.split_whitespace().next().unwrap_or("")
            ),
            Err(e) => return fail(&format!("couldn't open a tab for {ticket}: {e}")),
        }
    }
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn tickets_are_split_on_commas_and_spaces_and_given_once() {
        assert_eq!(
            tickets_of(&args(&[
                "abc-1,",
                "https://linear.app/t/issue/ABC-2/title, ABC-1"
            ])),
            Ok(args(&["ABC-1", "ABC-2"]))
        );
        assert_eq!(
            tickets_of(&args(&["ABC-1", "$(rm -rf ~)"])),
            Err("could not parse a Linear ticket identifier from \"$(rm\"".into())
        );
    }
}
