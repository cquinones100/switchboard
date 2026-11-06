//! `worktree-from-ticket [path] <linear-ticket-id-or-url> [--prompt <text>]`:
//! a git worktree on a new branch named after a Linear ticket, with Claude
//! opened in it, told to work on the ticket. Ported from
//! `bin/worktree-from-ticket`.
//!
//! `<path>` defaults to `.claude/worktrees`, resolved against the main
//! clone, so it works from inside an existing worktree. The ticket comes
//! from Linear's API (`integrations::linear`), with the key the rest of
//! Switchboard uses.
//!
//! ```text
//! worktree-from-ticket ABC-123        creates the worktree, opens claude
//! worktree-from-ticket init zsh       prints the `wt` shell function
//! ```
//!
//! The bare command can't change your shell's directory (no child process
//! can), so `init` prints a `wt` function that runs it with `--no-claude
//! --cd-file`, cd's into the worktree it names, and starts Claude itself.
//! Doing the cd first means the shell sits in the worktree for the whole
//! session, so new panes and splits open there too. The path comes back
//! through the cd-file rather than stdout because `$( )` would make stdout a
//! pipe, and anything interactive downstream would see no terminal. tmux's
//! function named the tabs again once it had cd'd; Switchboard's server
//! names every tab on its own, so the function doesn't.

use std::path::Path;
use std::process::ExitCode;

use serde_json::{Value, json};

use crate::integrations::linear;
use crate::util;
use crate::worktree::{self, Given, Purpose};

fn usage() -> ExitCode {
    eprintln!(
        "Usage: worktree-from-ticket [path] <linear-ticket-id-or-url> [--prompt <text>]\n       \
         worktree-from-ticket init <zsh|bash>\n\n\
         Examples:\n  \
         worktree-from-ticket ABC-123\n  \
         worktree-from-ticket .worktrees ABC-123\n  \
         worktree-from-ticket ABC-123 --prompt \"start with the migration\"\n\n\
         Path defaults to {} under the repo root.\n\
         --prompt adds to the prompt Claude starts with. The wt function also takes\n\
         it from WT_PROMPT, which it clears, so /wt can hand its text over.",
        worktree::DEFAULT_WORKTREE_DIR
    );
    ExitCode::from(1)
}

/// The `wt` function, for `eval "$(worktree-from-ticket init zsh)"`. Named
/// `wt` rather than after the command so it can't shadow it and recurse.
/// WT_PROMPT, which /wt sets on the pane it respawns, is added to the prompt
/// and cleared, so a later `wt` in the same shell doesn't see it again.
/// `rc`, not `status`: `status` is a read-only special variable in zsh.
pub fn shell_function() -> String {
    format!(
        r#"wt() {{
  local cdfile rc dir prompt
  local -a extra
  extra=()
  [ -n "${{WT_PROMPT:-}}" ] && extra=(--prompt "$WT_PROMPT")
  unset WT_PROMPT
  cdfile=$(mktemp) || return 1
  {} --no-claude --cd-file "$cdfile" "${{extra[@]}}" "$@"
  rc=$?
  if [ $rc -ne 0 ]; then
    rm -f "$cdfile"
    return $rc
  fi
  dir=$(sed -n 1p "$cdfile")
  prompt=$(sed -n 2p "$cdfile")
  rm -f "$cdfile"
  if [ ! -d "$dir" ]; then
    echo "wt: expected a worktree directory, got: $dir" >&2
    return 1
  fi
  cd "$dir" || return 1
  claude "$prompt"
}}"#,
        worktree::self_command("worktree-from-ticket")
    )
}

/// A ticket's team key and number, from its identifier or an issue URL.
pub fn parse_identifier(input: &str) -> Option<(String, u64)> {
    let raw = worktree::ticket_from(input)?;
    let (key, number) = raw.rsplit_once('-')?;
    Some((key.to_uppercase(), number.parse().ok()?))
}

/// What Linear says of the ticket, as far as the worktree needs it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Issue {
    pub identifier: String,
    pub title: String,
    pub branch_name: Option<String>,
    /// Its state's name ("In Progress") and type (one of Linear's fixed set:
    /// triage, backlog, unstarted, started, completed, canceled, duplicate).
    pub state: Option<(String, String)>,
}

const ISSUE: &str = "query IssueByIdentifier($number: Float!, $key: String!) { \
     issues(filter: { number: { eq: $number }, team: { key: { eq: $key } } }) { \
     nodes { identifier title branchName state { name type } } } }";

fn fetch_issue(key: &str, number: u64) -> Result<Issue, String> {
    let data = util::block_on(linear::graphql(
        ISSUE,
        json!({ "number": number, "key": key }),
        "wt: the ticket",
    ))
    .map_err(|e| e.to_string())?;
    let node = data
        .pointer("/issues/nodes/0")
        .filter(|n| n.is_object())
        .ok_or_else(|| format!("no Linear issue found for {key}-{number}"))?;
    let text = |p: &str| node.pointer(p).and_then(Value::as_str).map(str::to_string);
    Ok(Issue {
        identifier: text("/identifier").unwrap_or_else(|| format!("{key}-{number}")),
        title: text("/title").unwrap_or_default(),
        branch_name: text("/branchName").filter(|b| !b.is_empty()),
        state: text("/state/name").map(|n| (n, text("/state/type").unwrap_or_default())),
    })
}

/// The prompt Claude starts with. The state goes in as a fact rather than
/// something the session has to look up, keyed on its type, not its name: a
/// workspace can call its completed state anything ("Shipped"), while the
/// type is one of Linear's fixed set, whose closed ones are completed,
/// canceled and duplicate. The clause is dropped if Linear gives no state.
pub fn prompt_for(issue: &Issue, extra: &str) -> String {
    let state_clause = issue
        .state
        .as_ref()
        .map(|(name, _)| format!(", which is in the \"{name}\" state"))
        .unwrap_or_default();
    let closed = issue
        .state
        .as_ref()
        .is_some_and(|(_, kind)| matches!(kind.as_str(), "completed" | "canceled" | "duplicate"));
    let mut prompt = format!("Work on ticket {}{state_clause}. ", issue.identifier);
    if closed {
        prompt.push_str(
            "That state is closed, so before doing anything else tell me and confirm whether we want to reopen it; \
             if we do, ask what needs to be done and whether the ticket should be updated to cover the new work. ",
        );
    }
    prompt.push_str(
        "Before changing anything, read the ticket and compare it against the current state of the codebase: \
         is it still relevant, is it still necessary, is the approach it outlines sound, and how could it be improved? \
         Where the ticket gets a detail wrong — a wrong type, the wrong target directory, a file that turns out to be \
         unused — build what the codebase actually needs rather than stopping to ask, then update the ticket so it \
         matches what you built. Keep that out of the PR description: it describes the work, not the journey. \
         Stop and wait for my answer only when the correction is one of scope: widening the work to another file, \
         deleting a caller, changing an API. \
         Then work on the ticket. Don't run any tests, lint, or other checks when done. \
         When the work is done, run the /review-loop skill. \
         Push and create a PR when done. \
         Your final message has only these parts. A ticket check: \"Unchanged.\" if you didn't edit the ticket, \
         otherwise the edits you made to it, as tersely as possible. Then \"PR pushed\" or \"PR not pushed\", \
         and if not pushed, why. Then, only if the review loop stopped at its round cap, say so, \
         since that means it needs to run again. \
         Then \"Preview:\" and how to try the work in the PR's preview environment, then \"Prod:\" and how to check it \
         in production once it is deployed, each a few terse steps saying where to go and what to look for, changing \
         nothing real in production; \"None.\" for either when the work can't be seen there. \
         Don't summarize what changed, because I review the PR itself; \
         don't mention the review loop otherwise; and don't mention that tests, lint, or checks weren't run.",
    );
    if !extra.is_empty() {
        prompt.push_str(&format!(" Also, from me for this ticket: {extra}"));
    }
    prompt
}

pub fn main(args: Vec<String>) -> ExitCode {
    let given = match Given::parse(&args, &["--prompt"]) {
        Ok(g) => g,
        Err(flag) if flag == "--prompt" => return worktree::fail("--prompt requires text"),
        Err(flag) => return worktree::fail(&format!("{flag} requires a path")),
    };
    match worktree::init_shell(&given.positional) {
        Some(true) => {
            println!("{}", shell_function());
            return ExitCode::SUCCESS;
        }
        Some(false) => {
            return worktree::fail("init requires a shell: worktree-from-ticket init <zsh|bash>");
        }
        None => {}
    }
    if given.help {
        return usage();
    }
    let extra = worktree::one_line(&given.all("--prompt").join(" "));
    // One positional is the ticket; two means a path came first.
    let (target, ticket) = match given.positional.as_slice() {
        [ticket] => (worktree::DEFAULT_WORKTREE_DIR, ticket.as_str()),
        [path, ticket, ..] => (path.as_str(), ticket.as_str()),
        [] => return usage(),
    };
    if !worktree::looks_like_ticket(ticket) {
        return usage();
    }
    let Some((key, number)) = parse_identifier(ticket) else {
        return worktree::fail(&format!(
            "could not parse a Linear ticket identifier from \"{ticket}\""
        ));
    };
    let issue = match fetch_issue(&key, number) {
        Ok(issue) => issue,
        Err(e) => return worktree::fail(&e),
    };
    let branch = issue.branch_name.clone().unwrap_or_else(|| {
        format!(
            "{}-{}",
            issue.identifier.to_lowercase(),
            worktree::slugify(&issue.title)
        )
    });
    let root = match worktree::repo_root() {
        Ok(root) => root,
        Err(e) => return worktree::fail(&e.to_string()),
    };
    let path = worktree::normalize(&root.join(target).join(&branch));

    eprintln!("Ticket:   {} — {}", issue.identifier, issue.title);
    eprintln!(
        "Status:   {}",
        issue
            .state
            .as_ref()
            .map(|(n, _)| n.as_str())
            .unwrap_or("unknown")
    );
    eprintln!("Branch:   {branch}");
    eprintln!("Worktree: {}", path.display());

    let purpose = Purpose {
        work: "ticket",
        command: "wt",
        start: None,
    };
    if let Err(e) = worktree::ensure_worktree(&path, &branch, &purpose) {
        return worktree::fail(&e.to_string());
    }
    if let Err(e) = worktree::link_local_settings(&root, &path) {
        return worktree::fail(&e.to_string());
    }
    let prompt = prompt_for(&issue, &extra);
    // Read by the `wt` function: line 1 the worktree, line 2 the prompt, so
    // the function and the standalone path can't drift apart.
    if let Some(cd_file) = &given.cd_file
        && let Err(e) = worktree::write_cd_file(cd_file, &format!("{}\n{prompt}\n", path.display()))
    {
        return worktree::fail(&e.to_string());
    }
    if given.no_claude {
        return ExitCode::SUCCESS;
    }
    worktree::run_in("claude", &[prompt], Path::new(&path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiers_from_ids_and_urls() {
        assert_eq!(parse_identifier("abc-12"), Some(("ABC".into(), 12)));
        assert_eq!(
            parse_identifier("https://linear.app/w/issue/ENG-7/some-title"),
            Some(("ENG".into(), 7))
        );
        assert_eq!(parse_identifier("12"), None);
    }

    #[test]
    fn a_closed_ticket_is_raised_first_and_the_text_added_last() {
        let issue = Issue {
            identifier: "ENG-1".into(),
            title: "t".into(),
            branch_name: None,
            state: Some(("Shipped".into(), "completed".into())),
        };
        let prompt = prompt_for(&issue, "start small");
        assert!(prompt.starts_with(
            "Work on ticket ENG-1, which is in the \"Shipped\" state. That state is closed"
        ));
        assert!(prompt.ends_with("Also, from me for this ticket: start small"));
        let open = Issue {
            state: None,
            ..issue
        };
        assert!(prompt_for(&open, "").starts_with("Work on ticket ENG-1. Before changing"));
    }
}
