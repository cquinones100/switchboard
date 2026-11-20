//! `switchboard tickets [<dir>]`: where the repository at `<dir>` (the
//! current directory by default) keeps its work, which `/shape` files new
//! work in, on the first line: `github` (its issues), `linear`, or `notes`
//! (a note's checklist with a plan note each), repos.json's `tickets`
//! (`repos::tickets_at`), the one place it is decided. It has no default:
//! while it isn't set, the command prints nothing, says on stderr how to set
//! it (`repos::unset_words`) and exits 1, and `/shape` stops on it. Then,
//! when `tickets` is `github` and its `issueLabel` is set, a second line
//! `autopilot-label <label>`, the label an issue carries for autopilot to
//! take it, whether or not autopilot works the repository now, so an issue
//! `/shape` files joins autopilot's queue. It reads repos.json and runs git,
//! and fails when repos.json can't be read or `<dir>` isn't in a
//! repository. No tmux-version file is behind it.

use std::path::PathBuf;
use std::process::ExitCode;

/// What it prints for a repository whose `tickets` and `issueLabel` are
/// these: the lines, or `None` while `tickets` isn't set.
pub fn lines(tickets: Option<&str>, label: Option<&str>) -> Option<Vec<String>> {
    let tickets = tickets?;
    let mut out = vec![tickets.to_string()];
    if let (Some(label), "github") = (label, tickets) {
        out.push(format!("autopilot-label {label}"));
    }
    Some(out)
}

pub fn main(args: Vec<String>) -> ExitCode {
    if let Some(bad) = args.iter().find(|a| a.starts_with('-')) {
        eprintln!("tickets: unknown option {bad}\nusage: switchboard tickets [<dir>]");
        return ExitCode::from(2);
    }
    let dir = args
        .first()
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    let read =
        crate::repos::tickets_at(&dir).and_then(|t| Ok((t, crate::repos::issue_label_at(&dir)?)));
    let (tickets, label) = match read {
        Ok(read) => read,
        Err(e) => {
            eprintln!("tickets: {e}");
            return ExitCode::from(1);
        }
    };
    match lines(tickets.as_deref(), label.as_deref()) {
        Some(lines) => {
            for line in lines {
                println!("{line}");
            }
            ExitCode::SUCCESS
        }
        None => {
            let name = crate::repos::repo_at(&dir)
                .or_else(|| crate::repos::configured_at(&dir))
                .unwrap_or_else(|| "This repository".into());
            eprintln!("tickets: {}", crate::repos::unset_words(&name));
            ExitCode::from(1)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_label_is_named_for_github_alone_and_nothing_while_unset() {
        assert_eq!(lines(None, Some("ready")), None);
        assert_eq!(
            lines(Some("github"), Some("ready")).unwrap(),
            ["github", "autopilot-label ready"]
        );
        assert_eq!(lines(Some("github"), None).unwrap(), ["github"]);
        assert_eq!(lines(Some("notes"), Some("ready")).unwrap(), ["notes"]);
        assert_eq!(lines(Some("linear"), None).unwrap(), ["linear"]);
    }
}
