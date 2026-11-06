//! `shape-batch`: opens one tab per idea or ticket and runs `shape` in each,
//! the way `wt-batch` does for `wt`. Ported from bin/shape-batch.
//!
//! ```text
//! shape-batch "phase label is unclear" "results load slowly" ABC-123
//! shape-batch "ABC-123, ABC-124"
//! shape-batch --continue "ABC-123, ABC-124"
//! shape-batch < ideas.txt
//! ```
//!
//! Each argument is one thing to shape, since an idea is free text full of
//! spaces and commas. An argument that is nothing but ticket identifiers or
//! Linear URLs is split as `wt-batch` splits them. With no arguments, ideas
//! are read from stdin, one per line. `--continue` carries straight on
//! shaping each ticket (`shape --continue`).
//!
//! The tabs go at the end of the tab list, in the order given, and focus
//! stays where it is. Each tab is named "Shaping: ..." by `shape` itself
//! once it starts, so /wt can later clear that name when it hands the tab
//! over; the tab gets no name of its own here, since one given when it opens
//! would stand over that one.
//!
//! Each tab's command is an interactive shell running `shape <item>`
//! (`worktree::tab_command`): the item is its own argument, never typed or
//! parsed as shell, and when Claude exits the tab carries on as an ordinary
//! shell.

use std::io::{IsTerminal, Read};
use std::process::ExitCode;
use std::sync::LazyLock;

use regex::Regex;

use crate::mux::socket;
use crate::worktree::{self, fail};

fn usage() -> ExitCode {
    eprintln!(
        "Usage: shape-batch [--continue] <ideas-or-tickets...>\n       shape-batch < ideas.txt\n\nEach argument is one thing to shape; quote ideas that contain spaces.\nExamples:\n  shape-batch \"phase label is unclear\" \"results load slowly\" ABC-123\n  shape-batch \"ABC-123, ABC-124\"\n  shape-batch < ideas.txt"
    );
    ExitCode::from(1)
}

static TICKET: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:[A-Za-z][A-Za-z0-9]*-\d+|https?://\S*/issue/[A-Za-z][A-Za-z0-9]*-\d+\S*)$")
        .unwrap()
});

/// A line break and the space around it, made one space, so an item is one
/// line of the tab's command.
static LINE_BREAK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s*\n\s*").unwrap());

/// "ABC-1, ABC-2" or "ABC-1 https://linear.app/x/issue/ABC-2/title" becomes
/// one item per ticket. Anything with a single word that isn't a ticket in
/// it is an idea and stays whole.
pub fn items_from(arg: &str) -> Vec<String> {
    let tokens: Vec<&str> = arg
        .split(|c: char| c.is_whitespace() || c == ',')
        .filter(|t| !t.is_empty())
        .collect();
    if !tokens.is_empty() && tokens.iter().all(|t| TICKET.is_match(t)) {
        tokens.into_iter().map(str::to_string).collect()
    } else {
        vec![arg.to_string()]
    }
}

/// Every item, worked out before any tab opens, each once, in order.
pub fn items_of(inputs: &[String]) -> Vec<String> {
    let mut items: Vec<String> = Vec::new();
    for item in inputs.iter().flat_map(|i| items_from(i)) {
        let item = item.trim().to_string();
        if !item.is_empty() && !items.contains(&item) {
            items.push(item);
        }
    }
    items
}

pub fn main(args: Vec<String>) -> ExitCode {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        return usage();
    }
    let carry_on = args.iter().any(|a| a == "--continue");
    let argv: Vec<String> = args.into_iter().filter(|a| a != "--continue").collect();
    let inputs = if argv.is_empty() && !std::io::stdin().is_terminal() {
        let mut text = String::new();
        let _ = std::io::stdin().read_to_string(&mut text);
        text.lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(str::to_string)
            .collect()
    } else {
        argv
    };
    if inputs.is_empty() {
        return usage();
    }
    if !socket::in_pane() {
        return fail("not inside Switchboard");
    }
    let items = items_of(&inputs);
    if items.is_empty() {
        return usage();
    }
    let cwd = std::env::current_dir()
        .map(|d| d.to_string_lossy().into_owned())
        .unwrap_or_else(|_| ".".into());
    let script = format!(
        "{}{} \"$@\"",
        worktree::self_command("shape"),
        if carry_on { " --continue" } else { "" }
    );
    for item in items {
        let one_line = LINE_BREAK.replace_all(&item, " ").into_owned();
        let command = worktree::tab_command(&script, "shape", &[one_line]);
        let mut ask: Vec<&str> = vec!["new-tab", "-d", "-E", "-c", &cwd, "--"];
        ask.extend(command.iter().map(String::as_str));
        let tab = match socket::ask_ok(&ask) {
            Ok(out) => out.split_whitespace().next().unwrap_or("").to_string(),
            Err(e) => return fail(&format!("couldn't open a tab: {e}")),
        };
        let shown = if item.chars().count() > 60 {
            format!("{}...", item.chars().take(57).collect::<String>())
        } else {
            item.clone()
        };
        eprintln!("Shaping {shown} in window {tab}");
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
    fn lists_of_tickets_are_split_and_ideas_kept_whole() {
        assert_eq!(
            items_of(&args(&[
                "ABC-1, https://linear.app/x/issue/ABC-2/title",
                "results load slowly, again",
                "ABC-1",
                "  ",
            ])),
            args(&[
                "ABC-1",
                "https://linear.app/x/issue/ABC-2/title",
                "results load slowly, again"
            ])
        );
    }
}
