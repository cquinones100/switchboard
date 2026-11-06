//! `worktree-done [--list] [--all] [--paths]`: reads every worktree of the
//! repository, looks up the Linear ticket each is named after, and offers
//! the ones whose ticket is finished (completed or canceled, whatever the
//! team's workflow calls those states) to remove. The answer to "which of
//! these forty directories can I delete", and the deleting. Ported from
//! `bin/worktree-done`.
//!
//! Removal is `git worktree remove` without `--force`, so a worktree holding
//! uncommitted work refuses to go rather than losing it, and the branch
//! always survives: only the checkout is removed.
//!
//! ```text
//! worktree-done            pick finished worktrees to remove
//! worktree-done --list     print them instead, removing nothing
//! worktree-done --all      print every worktree grouped by state (implies --list)
//! worktree-done --paths    print only the finished worktrees' paths, for pipes
//! ```
//!
//! The script picked with fzf; with no fzf here, the finished worktrees are
//! listed numbered and the choice is typed on the terminal (`/dev/tty`, not
//! stdin, which may be a pipe even with a terminal to draw on, and a piped
//! answer is not consent): numbers and ranges, `a` for all, Enter for none.
//! Taking every one at once asks again before anything goes. Without a
//! terminal it prints rather than asks, since there is nothing to ask on and
//! silently removing nothing would look like a bug. Status goes to stderr so
//! `--paths` stays pipeable.

use std::collections::{BTreeMap, HashMap};
use std::io::{BufRead, BufReader, IsTerminal, Write};
use std::path::Path;
use std::process::{Command, ExitCode};
use std::sync::LazyLock;

use regex::Regex;
use serde_json::{Value, json};

use crate::integrations::linear;
use crate::util;
use crate::worktree::{self, Worktree};

/// Linear's own words for "this issue is over"; every other type (triage,
/// backlog, unstarted, started) means the work is still live.
const DONE_TYPES: [&str; 2] = ["completed", "canceled"];

/// Comfortably under Linear's 250 page cap, so one query per chunk always
/// gives every match rather than silently paginating.
const CHUNK_SIZE: usize = 100;

fn usage() -> ExitCode {
    eprintln!(
        "Usage: worktree-done [--list] [--all] [--paths]\n\n\
         Lists finished worktrees and removes the ones you pick.\n\n  \
         --list   print them instead, removing nothing\n  \
         --all    print every worktree grouped by ticket state (implies --list)\n  \
         --paths  print only the finished worktrees' paths, one per line\n\n\
         When asked which to remove:\n  \
         1 3-5      those, by number\n  \
         a          all of them (asks again first)\n  \
         Enter      none\n"
    );
    ExitCode::from(1)
}

/// A ticket as Linear gives it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Issue {
    pub identifier: String,
    pub title: String,
    pub state_name: String,
    pub state_type: String,
}

static IDENTIFIER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?:^|[^a-z0-9])([a-z][a-z0-9]*)-(\d+)(?:[^a-z0-9]|$)").unwrap());
static PR_PREFIX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^pr-\d+-").unwrap());

/// The ticket a name carries, as its team key and number: anchored on a
/// word boundary so the number belongs to the key beside it, and with a
/// `wtpr` directory's `pr-<n>-` taken off first, so a PR number is never
/// read as team "PR".
pub fn find_identifier(text: &str) -> Option<(String, u64)> {
    let lower = text.to_lowercase();
    let cleaned = PR_PREFIX.replace(&lower, "");
    let m = IDENTIFIER.captures(&cleaned)?;
    Some((m[1].to_uppercase(), m[2].parse().ok()?))
}

/// The branch first, then the directory: `wt` names both after the
/// ticket, but `wtpr` names the directory `pr-<n>-<branch>` and only the
/// branch half can carry a ticket.
fn identifier_for(w: &Worktree) -> Option<(String, u64)> {
    w.branch
        .as_deref()
        .and_then(find_identifier)
        .or_else(|| find_identifier(&w.path.file_name()?.to_string_lossy()))
}

const ISSUES: &str = "query Issues($filters: [IssueFilter!]) { \
     issues(filter: { or: $filters }, first: 150) { \
     nodes { identifier title url state { name type } } } }";

/// One request per chunk rather than one per ticket: forty worktrees is
/// forty round trips otherwise. Keyed by identifier, since the filter is an
/// unordered `or` and tickets that no longer exist (or the key can't see)
/// don't come back at all.
fn fetch_issues(identifiers: &[(String, u64)]) -> Result<HashMap<String, Issue>, String> {
    util::block_on(async {
        let mut found = HashMap::new();
        for batch in identifiers.chunks(CHUNK_SIZE) {
            let filters: Vec<Value> = batch
                .iter()
                .map(|(key, number)| {
                    json!({ "and": [{ "number": { "eq": number } }, { "team": { "key": { "eq": key } } }] })
                })
                .collect();
            let data = linear::graphql(
                ISSUES,
                json!({ "filters": filters }),
                "worktree-done: tickets",
            )
            .await
            .map_err(|e| e.to_string())?;
            for node in data
                .pointer("/issues/nodes")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let text = |p: &str| {
                    node.pointer(p)
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string()
                };
                let issue = Issue {
                    identifier: text("/identifier"),
                    title: text("/title"),
                    state_name: text("/state/name"),
                    state_type: text("/state/type"),
                };
                found.insert(issue.identifier.clone(), issue);
            }
        }
        Ok(found)
    })
}

/// A text cut to `width` characters, an ellipsis at its end when cut.
fn truncate(text: &str, width: usize) -> String {
    let count = text.chars().count();
    if count <= width {
        return text.to_string();
    }
    let mut cut: String = text.chars().take(width.saturating_sub(1)).collect();
    cut.push('…');
    cut
}

struct Entry {
    worktree: Worktree,
    issue: Option<Issue>,
}

impl Entry {
    fn done(&self) -> bool {
        self.issue
            .as_ref()
            .is_some_and(|i| DONE_TYPES.contains(&i.state_type.as_str()))
    }
}

fn relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .map(|r| r.display().to_string())
        .unwrap_or_else(|_| path.display().to_string())
}

/// The terminal's width for the listing, as stdout's columns, 100 when it
/// has none (a pty with no size reports 0, which is no width to lay
/// anything out against).
fn columns() -> usize {
    if !std::io::stdout().is_terminal() {
        return 100;
    }
    match crossterm::terminal::size() {
        Ok((cols, _)) if cols > 0 => cols as usize,
        _ => 100,
    }
}

/// Prints a group. Paths are dimmed rather than aligned under the title:
/// they run to seventy characters, and indenting them past an identifier
/// and a state would push most of them off the edge.
fn print_group(title: &str, entries: &[&Entry], root: &Path) {
    if entries.is_empty() {
        return;
    }
    let width = columns().saturating_sub(2).max(40);
    let tty = std::io::stdout().is_terminal();
    let dim = |t: String| if tty { format!("\x1b[2m{t}\x1b[0m") } else { t };
    let with: Vec<&Issue> = entries.iter().filter_map(|e| e.issue.as_ref()).collect();
    let id_w = with.iter().map(|i| i.identifier.len()).max().unwrap_or(0);
    let state_w = with
        .iter()
        .map(|i| i.state_name.chars().count())
        .max()
        .unwrap_or(0);
    println!("\n{title} ({})", entries.len());
    for e in entries {
        let rel = relative(root, &e.worktree.path);
        // Nothing to say about a worktree with no ticket but where it is.
        let Some(issue) = &e.issue else {
            println!("  {}", truncate(&rel, width - 2));
            continue;
        };
        let columns = format!(
            "{:id_w$}  {:state_w$}  {}",
            issue.identifier, issue.state_name, issue.title
        );
        println!("  {}", truncate(&columns, width - 2));
        println!("{}", dim(format!("    {}", truncate(&rel, width - 4))));
    }
}

/// Numbers and ranges (`1 3-5`, commas too) out of what was typed, each in
/// 1..=count; `None` for anything else.
pub fn parse_choice(text: &str, count: usize) -> Option<Vec<usize>> {
    let text = text.trim();
    if text.eq_ignore_ascii_case("a") || text.eq_ignore_ascii_case("all") {
        return Some((0..count).collect());
    }
    let mut chosen = Vec::new();
    for part in text.split([' ', ',']).filter(|p| !p.is_empty()) {
        let (from, to) = match part.split_once('-') {
            Some((a, b)) => (a.parse::<usize>().ok()?, b.parse::<usize>().ok()?),
            None => {
                let n = part.parse::<usize>().ok()?;
                (n, n)
            }
        };
        if from == 0 || to > count || from > to {
            return None;
        }
        for n in from..=to {
            if !chosen.contains(&(n - 1)) {
                chosen.push(n - 1);
            }
        }
    }
    Some(chosen)
}

/// A line typed on the terminal, the question on stderr; `None` with no
/// terminal, or one closed.
fn ask_tty(question: &str) -> Option<String> {
    let tty = std::fs::File::open("/dev/tty").ok()?;
    eprint!("{question} ");
    let _ = std::io::stderr().flush();
    let mut line = String::new();
    match BufReader::new(tty).read_line(&mut line) {
        Ok(0) | Err(_) => None,
        Ok(_) => Some(line),
    }
}

/// The finished worktrees chosen to go, by their place in `done`.
fn choose(done: &[&Entry], root: &Path) -> Vec<usize> {
    let id_w = done
        .iter()
        .filter_map(|e| e.issue.as_ref())
        .map(|i| i.identifier.len())
        .max()
        .unwrap_or(0);
    let state_w = done
        .iter()
        .filter_map(|e| e.issue.as_ref())
        .map(|i| i.state_name.chars().count())
        .max()
        .unwrap_or(0);
    let number_w = done.len().to_string().len();
    for (i, e) in done.iter().enumerate() {
        let issue = e
            .issue
            .as_ref()
            .expect("a finished worktree has its ticket");
        eprintln!(
            "  {:>number_w$}. {:id_w$}  {:state_w$}  {}  {}",
            i + 1,
            issue.identifier,
            issue.state_name,
            relative(root, &e.worktree.path),
            issue.title
        );
    }
    loop {
        let Some(answer) =
            ask_tty("Remove which? Numbers or ranges (1 3-5), a for all, Enter for none:")
        else {
            eprintln!("No terminal to ask on — nothing removed.");
            return Vec::new();
        };
        match parse_choice(&answer, done.len()) {
            Some(chosen) => return chosen,
            None => eprintln!("Numbers between 1 and {}, a, or Enter.", done.len()),
        }
    }
}

/// No `--force`, deliberately: git refuses to remove a worktree with
/// uncommitted changes or untracked files, which is the only thing between
/// a mistaken choice and lost work. A refusal is said and the rest go on.
/// Gives how many were left in place.
fn remove(entries: &[&Entry], root: &Path) -> usize {
    let mut failures = 0;
    for e in entries {
        let rel = relative(root, &e.worktree.path);
        eprintln!("Removing {rel}...");
        let ok = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["worktree", "remove"])
            .arg(&e.worktree.path)
            .status()
            .is_ok_and(|s| s.success());
        if !ok {
            eprintln!(
                "  failed — run 'git -C {} worktree remove --force {rel}' if that is intended",
                root.display()
            );
            failures += 1;
        }
    }
    eprintln!(
        "\nRemoved {} of {}{}.",
        entries.len() - failures,
        entries.len(),
        if failures > 0 {
            format!(", {failures} left in place")
        } else {
            String::new()
        }
    );
    failures
}

pub fn main(args: Vec<String>) -> ExitCode {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        return usage();
    }
    if let Some(unknown) = args
        .iter()
        .find(|a| !["--all", "--paths", "--list"].contains(&a.as_str()))
    {
        eprintln!("Unknown argument: {unknown}");
        return usage();
    }
    let show_all = args.iter().any(|a| a == "--all");
    let paths_only = args.iter().any(|a| a == "--paths");
    // --all exists to show the worktrees this won't offer to remove, which
    // is a thing to read rather than act on, so it implies --list.
    let list_only = show_all || args.iter().any(|a| a == "--list");

    let root = match worktree::repo_root() {
        Ok(r) => r,
        Err(e) => return worktree::fail(&e.to_string()),
    };
    // The main clone is never one to clean up: git won't remove it and its
    // branch is usually the trunk.
    let trees: Vec<Worktree> = match worktree::worktrees(&root) {
        Ok(all) => all.into_iter().filter(|w| w.path != root).collect(),
        Err(e) => return worktree::fail(&e.to_string()),
    };
    if trees.is_empty() {
        eprintln!("No worktrees besides the main clone in {}", root.display());
        return ExitCode::SUCCESS;
    }
    let tagged: Vec<(Worktree, Option<(String, u64)>)> = trees
        .into_iter()
        .map(|w| {
            let id = identifier_for(&w);
            (w, id)
        })
        .collect();
    let identified = tagged.iter().filter(|(_, id)| id.is_some()).count();
    eprintln!(
        "Checking {identified} of {} worktree{} against Linear",
        tagged.len(),
        if tagged.len() == 1 { "" } else { "s" }
    );
    // Two worktrees on one ticket (its own, and one made later from its PR)
    // are asked about once.
    let unique: Vec<(String, u64)> = tagged
        .iter()
        .filter_map(|(_, id)| id.clone())
        .map(|(k, n)| (format!("{k}-{n}"), (k, n)))
        .collect::<BTreeMap<_, _>>()
        .into_values()
        .collect();
    let issues = if unique.is_empty() {
        HashMap::new()
    } else {
        match fetch_issues(&unique) {
            Ok(found) => found,
            Err(e) => return worktree::fail(&e),
        }
    };
    let entries: Vec<Entry> = tagged
        .into_iter()
        .map(|(worktree, id)| Entry {
            issue: id.and_then(|(k, n)| issues.get(&format!("{k}-{n}")).cloned()),
            worktree,
        })
        .collect();
    let done: Vec<&Entry> = entries.iter().filter(|e| e.done()).collect();
    if paths_only {
        for e in &done {
            println!("{}", e.worktree.path.display());
        }
        eprintln!("{} finished", done.len());
        return ExitCode::SUCCESS;
    }
    let open: Vec<&Entry> = entries
        .iter()
        .filter(|e| e.issue.is_some() && !e.done())
        .collect();
    let missing: Vec<&Entry> = entries.iter().filter(|e| e.issue.is_none()).collect();
    let summarise = || {
        if !open.is_empty() {
            eprintln!("{} still open (--all to list)", open.len());
        }
        if !missing.is_empty() {
            eprintln!("{} with no ticket found (--all to list)", missing.len());
        }
    };
    if done.is_empty() {
        eprintln!("Nothing finished — every worktree is still live.");
        summarise();
        return ExitCode::SUCCESS;
    }
    if list_only || !std::io::stdout().is_terminal() {
        print_group("Done", &done, &root);
        if show_all {
            print_group("Still open", &open, &root);
            print_group("No ticket found", &missing, &root);
        } else {
            println!();
            summarise();
        }
        return ExitCode::SUCCESS;
    }
    eprintln!("{} finished", done.len());
    summarise();
    let chosen = choose(&done, &root);
    if chosen.is_empty() {
        eprintln!("Nothing selected.");
        return ExitCode::SUCCESS;
    }
    // Taking the whole list at once is the one choice worth a second look;
    // anything less was chosen one by one and speaks for itself. Enter
    // alone is yes, as in every yes/no question here; no terminal is no.
    if chosen.len() == done.len() && done.len() > 1 {
        let yes = ask_tty(&format!(
            "Remove all {} finished worktrees? [Y/n]",
            done.len()
        ))
        .is_some_and(|a| {
            let a = a.trim().to_lowercase();
            a.is_empty() || a == "y" || a == "yes"
        });
        if !yes {
            eprintln!("Cancelled.");
            return ExitCode::SUCCESS;
        }
    }
    let selected: Vec<&Entry> = chosen.iter().map(|i| done[*i]).collect();
    if remove(&selected, &root) > 0 {
        return ExitCode::from(1);
    }
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn a_ticket_is_read_from_a_branch_or_a_directory_never_a_pr_number() {
        assert_eq!(find_identifier("abc-12-fix-it"), Some(("ABC".into(), 12)));
        assert_eq!(find_identifier("pr-7-eng-3-x"), Some(("ENG".into(), 3)));
        assert_eq!(find_identifier("pr-7-feature"), None);
        assert_eq!(find_identifier("me/ENG-44"), Some(("ENG".into(), 44)));
        let w = Worktree {
            path: PathBuf::from("/r/.claude/worktrees/pr-9-abc-5-thing"),
            branch: Some("feature".into()),
            ..Worktree::default()
        };
        assert_eq!(identifier_for(&w), Some(("ABC".into(), 5)));
    }

    #[test]
    fn choices_by_number_range_or_all() {
        assert_eq!(parse_choice("1 3-4", 5), Some(vec![0, 2, 3]));
        assert_eq!(parse_choice("2,2", 3), Some(vec![1]));
        assert_eq!(parse_choice("a", 3), Some(vec![0, 1, 2]));
        assert_eq!(parse_choice("", 3), Some(vec![]));
        assert_eq!(parse_choice("4", 3), None);
        assert_eq!(parse_choice("x", 3), None);
    }

    #[test]
    fn long_text_is_cut_with_an_ellipsis() {
        assert_eq!(truncate("abcdef", 4), "abc…");
        assert_eq!(truncate("abc", 4), "abc");
    }
}
