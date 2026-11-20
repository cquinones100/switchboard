//! `worktree-from-ticket [path] [<work>] [--prompt <text>]` (`wt`): a git
//! worktree on a new branch for one piece of work, from any of a
//! repository's ticket sources, with Claude opened in it, told to work on
//! it. Ported from `bin/worktree-from-ticket`; it took over a note's items
//! and GitHub issues from the command that started them before, so it is
//! the one command that starts work.
//!
//! The work is any of:
//!
//! - a Linear ticket, its ID (`ABC-12`) or link, asked of Linear's API
//!   (`integrations::linear`), here;
//! - a GitHub issue, `#12`, its link, `--issue <number>` (autopilot passes
//!   `<owner>/<repo>#<number>`), or `12` in a repository whose `tickets`
//!   in repos.json is `github` (worktree_from_issue.rs); `12` where it is
//!   `linear` is the ticket of that number in linear.json's `linearTeam`;
//! - a note's checklist item, `--note <note> --item <line>`
//!   (worktree_from_note.rs), `--note` alone asking which item.
//!
//! With none, it asks (src/ui/alone.rs, the workspace's picker in the
//! shell's own terminal): the repository's open work from where its
//! `tickets` keeps it, Linear's tickets assigned to you newest first,
//! GitHub's open issues as M-p's "Start work on a GitHub issue" lists them
//! (yours first, each group newest first), or a note's unticked items as
//! M-I lists them, the note asked first; where `tickets` isn't set, which source first when more
//! than one is set up (`linear` and `github` each by its setting, notes by
//! a notes directory). Outside a repository it asks which repository, as
//! M-W lists them; for a Linear ticket only once the ticket is found, the
//! repositories whose `tickets` is `linear` first.
//!
//! `<path>` defaults to `.claude/worktrees`, resolved against the main
//! clone, so it works from inside an existing worktree.
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
//!
//! A ticket's branch someone else started (a teammate's, on origin) holds
//! their `CLAUDE.md`, settings and hooks, so Claude isn't started on it
//! unless `--trusted` says it is yours to open
//! (`trust::branch_yours_or_listed`).

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::LazyLock;

use regex::Regex;
use serde_json::{Value, json};

use crate::cmd::worktree_from_note::{self as note, failed_in_autopilot};
use crate::integrations::{github, linear};
use crate::ui::{Menu, MenuOption, PickItem, Picker, alone};
use crate::util;
use crate::worktree::{self, Fetched, Given, Purpose};

fn usage() -> ExitCode {
    eprintln!(
        "Usage: worktree-from-ticket [--trusted] [path] [<work>] [--prompt <text>] [--target <branch>]\n       \
         worktree-from-ticket [path] --note <note> [--item <line>] [--prompt <text>] [--target <branch>]\n       \
         worktree-from-ticket [path] --issue <number> [--prompt <text>] [--target <branch>]\n       \
         worktree-from-ticket init <zsh|bash>\n\n\
         <work> is a Linear ticket (ABC-12, or its link), a GitHub issue (#12, or its\n\
         link), or a number alone, from where repos.json's tickets keeps the work:\n\
         github, the issue; linear, the ticket in linear.json's linearTeam. With none,\n\
         it lists the repository's open work to pick from.\n\n\
         Examples:\n  \
         worktree-from-ticket ABC-123\n  \
         worktree-from-ticket '#12'\n  \
         worktree-from-ticket --note \"Switchboard Notes\" --item 12\n  \
         worktree-from-ticket .worktrees ABC-123 --prompt \"start with the migration\"\n\n\
         Path defaults to {} under the repo root.\n\
         --trusted opens a branch someone else started on the ticket as your own.\n\
         --prompt adds to the prompt Claude starts with. The wt function also takes\n\
         it from WT_PROMPT, and the note from WT_NOTE, clearing both, so /wt can hand\n\
         them over.\n\
         --target cuts an item's or an issue's branch from that local branch rather\n\
         than the default branch, and records it as branch.<branch>.wtnTarget, which\n\
         queue-and-clean lands it on. It is refused in a repository that lands\n\
         through PRs, which go to the default branch.\n\
         --issue starts one of the repository's open GitHub issues; in a tab\n\
         autopilot opened, only one assigned to you and carrying its label.",
        worktree::DEFAULT_WORKTREE_DIR
    );
    ExitCode::from(1)
}

/// The `wt` function, for `eval "$(worktree-from-ticket init zsh)"`. Named
/// `wt` rather than after the command so it can't shadow it and recurse.
/// WT_PROMPT and WT_NOTE, which /wt sets on the pane it respawns (and M-I's
/// tabs, the note's path), are passed on and cleared, so a later `wt` in the
/// same shell doesn't see them again and a note's path never has to be
/// quoted into a command. The prompt is every line of the cd-file after the
/// first, since an item it carries can run to several. The settings Claude
/// starts with (`push_rule` in worktree_from_note.rs), when the command
/// writes any, come back in a file of their own. `rc`, not `status`:
/// `status` is a read-only special variable in zsh.
///
/// With it comes a `claude` function: in one of Switchboard's panes, each
/// Claude it starts takes Switchboard's plugin as Settings has it now
/// (`claude-plugin-dirs`), since a shell open from before `claude_plugin`
/// changed keeps the value its pane started with. Only Switchboard's entry
/// changes; one process a launch, none outside the workspace.
pub fn shell_function() -> String {
    format!(
        r#"wt() {{
  local cdfile settingsfile settings rc dir prompt
  local -a extra
  extra=()
  [ -n "${{WT_PROMPT:-}}" ] && extra+=(--prompt "$WT_PROMPT")
  [ -n "${{WT_NOTE:-}}" ] && extra+=(--note "$WT_NOTE")
  unset WT_PROMPT WT_NOTE
  cdfile=$(mktemp) || return 1
  settingsfile=$(mktemp) || {{ rm -f "$cdfile"; return 1; }}
  {} --no-claude --cd-file "$cdfile" --settings-file "$settingsfile" "${{extra[@]}}" "$@"
  rc=$?
  if [ $rc -ne 0 ]; then
    rm -f "$cdfile" "$settingsfile"
    return $rc
  fi
  dir=$(sed -n 1p "$cdfile")
  prompt=$(sed -n '2,$p' "$cdfile")
  settings=$(cat "$settingsfile")
  rm -f "$cdfile" "$settingsfile"
  if [ ! -d "$dir" ]; then
    echo "wt: expected a worktree directory, got: $dir" >&2
    return 1
  fi
  cd "$dir" || return 1
  if [ -n "$settings" ]; then
    claude --settings "$settings" "$prompt"
  else
    claude "$prompt"
  fi
}}

claude() {{
  if [ -n "${{SWITCHBOARD:-}}" ]; then
    local dirs
    if dirs=$({} 2>/dev/null); then
      if [ -n "$dirs" ]; then export CLAUDE_CODE_PLUGIN_DIRS="$dirs"; else unset CLAUDE_CODE_PLUGIN_DIRS; fi
    fi
  fi
  command claude "$@"
}}"#,
        worktree::self_command("worktree-from-ticket"),
        worktree::self_command("claude-plugin-dirs")
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

/// Where a new ticket branch starts, whether it tracks origin's, and, when
/// origin couldn't be reached, which copy of its default branch it is
/// ("origin/main, last updated 2 hours ago").
struct Start {
    commit: String,
    tracks: bool,
    offline: Option<String>,
}

/// The ticket's branch on origin, fetched, when someone started it before
/// (a teammate, or another computer), tracking it; else origin's default
/// branch as it is now (`worktree::default_start`), never the main clone's
/// checked-out HEAD, whose unpushed commits would land with the ticket's. A
/// fetch of the default branch that fails is tried twice more
/// (`worktree::FETCH_RETRIES_S`), then the branch starts from origin's
/// default branch as last fetched, since a failed fetch is most often a
/// moment's network; with none fetched before, it stops. It stops too when
/// the ticket's own fetch failed rather than found no branch
/// (`worktree::Fetched`) and the clone has a copy of that branch, so an
/// outage never hides work already started, nor checks out commits whose
/// authors couldn't be checked against origin.
fn ticket_start(root: &Path, branch: &str) -> Result<Start, String> {
    const SOURCE: &str = "wt: fetch the base";
    let has_origin = worktree::remotes(root).iter().any(|r| r == "origin");
    let tracking = || Start {
        commit: format!("refs/remotes/origin/{branch}"),
        tracks: true,
        offline: None,
    };
    let mut ticket = if has_origin {
        worktree::fetch_branch_answer(root, "origin", branch, SOURCE)
    } else {
        Fetched::NoSuchBranch
    };
    if ticket == Fetched::Yes {
        return Ok(tracking());
    }
    let name = worktree::origin_head(root).unwrap_or_else(|| "main".into());
    let fetched = worktree::with_retries(
        &worktree::fetch_waits(),
        || eprintln!("Fetching origin's {name} again"),
        || worktree::default_start(root, SOURCE),
    );
    // The ticket's fetch failed rather than finding no branch: ask again
    // once origin answers.
    if ticket == Fetched::Failed && fetched.is_ok() {
        ticket = worktree::fetch_branch_answer(root, "origin", branch, SOURCE);
        if ticket == Fetched::Yes {
            return Ok(tracking());
        }
    }
    // Origin never answered for it, and the clone has a copy: the ticket was
    // started, and what was pushed since, and by whom, can't be told.
    let cached = format!("refs/remotes/origin/{branch}");
    if ticket == Fetched::Failed
        && worktree::git_ok(root, &["show-ref", "--verify", "--quiet", &cached])
    {
        return Err(format!(
            "couldn't reach origin for {branch}, which the ticket was started on: run wt again once origin is back"
        ));
    }
    let (commit, offline) = match fetched {
        Ok(commit) => (commit, None),
        Err(e) => {
            let (commit, words) = worktree::last_fetched(root, None).ok_or(e)?;
            (commit, Some(words))
        }
    };
    Ok(Start {
        commit,
        tracks: false,
        offline,
    })
}

pub fn main(args: Vec<String>) -> ExitCode {
    // A program you run asks the services whatever Settings says.
    crate::integrations::run_by_you();
    let mut given = match Given::parse(
        &args,
        &[
            "--prompt",
            "--note",
            "--item",
            "--target",
            "--settings-file",
            "--issue",
        ],
    ) {
        Ok(g) => g,
        Err(flag) if flag == "--prompt" => return worktree::fail("--prompt requires text"),
        Err(flag) if flag == "--cd-file" => return worktree::fail("--cd-file requires a path"),
        Err(flag) => return worktree::fail(&format!("{flag} requires a value")),
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
    let trusted = given.positional.iter().any(|a| a == "--trusted");
    given.positional.retain(|a| a != "--trusted");
    let mut made = false;
    match start(&given, trusted, &mut made) {
        Ok(code) => code,
        Err(e) => {
            // Once its worktree is made the work is claimed, so a failure
            // after it waits on you in the tab rather than closing it.
            if !made {
                failed_in_autopilot(&e);
            }
            worktree::fail(&e)
        }
    }
}

/// The work `wt` is given as a word of its own rather than by a flag.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Asked {
    /// `12`, whose source is the repository's `tickets`.
    Number(u64),
    /// A GitHub issue: `#12` (its number), or its link
    /// (`<owner>/<repo>#<number>`).
    Issue(String),
    /// A Linear ticket's ID or link, as written.
    Ticket(String),
}

static ISSUE_LINK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^https?://github\.com/([A-Za-z0-9._-]+)/([A-Za-z0-9._-]+)/issues/(\d+)/?(?:[?#].*)?$",
    )
    .unwrap()
});

impl Asked {
    fn parse(text: &str) -> Option<Asked> {
        let text = text.trim();
        let is_number = |t: &str| !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit());
        if is_number(text) {
            return text.parse().ok().map(Asked::Number);
        }
        if let Some(digits) = text.strip_prefix('#').filter(|d| is_number(d)) {
            return Some(Asked::Issue(digits.to_string()));
        }
        if let Some(m) = ISSUE_LINK.captures(text) {
            return Some(Asked::Issue(format!("{}/{}#{}", &m[1], &m[2], &m[3])));
        }
        worktree::looks_like_ticket(text).then(|| Asked::Ticket(text.to_string()))
    }
}

/// Starts the work `given` names, or the one picked when it names none.
/// `made` is set once its worktree is made, which claims it.
fn start(given: &Given, trusted: bool, made: &mut bool) -> Result<ExitCode, String> {
    if let Some(number) = given.last("--issue") {
        if given.positional.len() > 1 {
            return Ok(usage());
        }
        in_a_repository()?;
        return super::worktree_from_issue::run(given, number, made);
    }
    if let Some(note_name) = given.last("--note") {
        if given.positional.len() > 1 {
            return Ok(usage());
        }
        let item = match given.last("--item") {
            Some(item) if !item.is_empty() && item.bytes().all(|b| b.is_ascii_digit()) => {
                item.to_string()
            }
            Some(item) => {
                return Err(format!(
                    "--item takes the item's line number, not \"{item}\""
                ));
            }
            None => {
                let root = in_a_repository()?;
                match pick_item(&root, note_name)? {
                    Some(line) => line.to_string(),
                    None => return Ok(ExitCode::from(1)),
                }
            }
        };
        in_a_repository()?;
        return note::run(given, note_name, &item, made);
    }
    if given.last("--item").is_some() {
        return Ok(usage());
    }
    // The last word is the work, and one before it the path.
    let mut rest = given.clone();
    let asked = match given.positional.as_slice() {
        [] => None,
        [.., work] if given.positional.len() <= 2 => match Asked::parse(work) {
            Some(asked) => {
                rest.positional.pop();
                Some(asked)
            }
            None => return Ok(usage()),
        },
        _ => return Ok(usage()),
    };
    match asked {
        Some(Asked::Ticket(ticket)) => linear_ticket(&rest, &ticket, trusted),
        Some(Asked::Issue(issue)) => {
            in_a_repository()?;
            super::worktree_from_issue::run(&rest, &issue, made)
        }
        Some(Asked::Number(n)) => {
            let root = in_a_repository()?;
            by_provider(&rest, &root, n, trusted, made)
        }
        None => {
            let root = in_a_repository()?;
            picked(&rest, &root, trusted, made)
        }
    }
}

/// The repository's name, for what `wt` says about it.
fn name_of(root: &Path) -> String {
    crate::repos::repo_at(root)
        .or_else(|| crate::repos::configured_at(root))
        .unwrap_or_else(|| "This repository".into())
}

/// `wt <number>`: the work of that number from where the repository's
/// `tickets` in repos.json keeps it, a GitHub issue, or a Linear ticket of
/// linear.json's `linearTeam`; refused where `tickets` isn't set, whose
/// number could be either, or is `notes`, whose items have no numbers.
fn by_provider(
    rest: &Given,
    root: &Path,
    n: u64,
    trusted: bool,
    made: &mut bool,
) -> Result<ExitCode, String> {
    let name = name_of(root);
    match crate::repos::tickets_at(root)?.as_deref() {
        Some("github") => super::worktree_from_issue::run(rest, &n.to_string(), made),
        Some("linear") => {
            let team = util::read_json(crate::files::file("linear.json"))
                .and_then(|v| Some(v.get("linearTeam")?.as_str()?.trim().to_string()))
                .filter(|t| !t.is_empty())
                .ok_or_else(|| {
                    format!(
                        "{name} keeps its work in Linear, and a number alone needs its team: set linearTeam in {}, or give the ticket's ID (ABC-{n})",
                        crate::files::shown("linear.json")
                    )
                })?;
            linear_ticket(rest, &format!("{team}-{n}"), trusted)
        }
        Some(_) => Err(format!(
            "{name} keeps its work in a note's checklist, which has no numbers: run wt alone to pick an item, or give a GitHub issue as #{n}"
        )),
        None => Err(format!(
            "{name} has no ticket provider, so {n} could be a GitHub issue or a Linear ticket: give the issue as #{n} or the ticket's ID, or set its Tickets in Settings (\"tickets\": \"github\" or \"linear\" in {})",
            crate::files::shown("repos.json")
        )),
    }
}

// ── Asking ──────────────────────────────────────────────────────────────────

/// The terminal to ask in: a box drawn there only when what is typed comes
/// from one, never for a program run with its input elsewhere (a script,
/// a test), which a box would wait on forever.
fn can_ask() -> Result<(), String> {
    use std::io::IsTerminal;
    if std::io::stdin().is_terminal() {
        Ok(())
    } else {
        Err("no work given, and no terminal to ask in: name the work (see --help)".into())
    }
}

/// The repository the work starts in: the one here, else one picked.
fn in_a_repository() -> Result<PathBuf, String> {
    pick_repository(false)
}

/// The repository here, else one picked as M-W lists them, `linear_first`
/// putting those whose `tickets` is `linear` first. The program goes on
/// from it, as if run there.
fn pick_repository(linear_first: bool) -> Result<PathBuf, String> {
    if let Ok(root) = worktree::repo_root() {
        return Ok(root);
    }
    can_ask().map_err(|_| "not in a git repository".to_string())?;
    let here = std::env::current_dir().unwrap_or_else(|_| crate::paths::home());
    let repos = crate::cmd::work::repositories(&here, linear_first);
    let items = repos
        .iter()
        .map(|d| PickItem::new(crate::paths::tilde(d), d.to_string_lossy()))
        .collect();
    let picker = Picker::new("Which repository", items)
        .named("pick-dir")
        .prompt("Repository > ")
        .exact()
        .keep_order();
    let Some(picked) = alone::pick(picker)? else {
        return Err("no repository picked".into());
    };
    let root = PathBuf::from(&picked.value);
    std::env::set_current_dir(&root).map_err(|e| format!("{}: {e}", root.display()))?;
    worktree::repo_root().map_err(|e| e.to_string())
}

/// The sources `wt` alone offers where the repository's `tickets` isn't
/// set: each service its setting turns on (GitHub only for a repository on
/// it), and the notes when there is a notes directory.
pub fn sources_offered(root: &Path) -> Vec<&'static str> {
    let mut out = Vec::new();
    if crate::settings::is_on("linear") {
        out.push("linear");
    }
    if crate::settings::is_on("github") && crate::sources::repo::origin_of(root).is_some() {
        out.push("github");
    }
    if note::notes_dir().is_dir() {
        out.push("notes");
    }
    out
}

/// What the question of which source names a source by.
pub fn source_words(source: &str) -> &'static str {
    match source {
        "linear" => "A Linear ticket assigned to you",
        "github" => "A GitHub issue",
        _ => "A note's checklist item",
    }
}

/// `wt` alone: the repository's open work from where it keeps it, picked.
fn picked(rest: &Given, root: &Path, trusted: bool, made: &mut bool) -> Result<ExitCode, String> {
    can_ask()?;
    let source = match crate::repos::tickets_at(root)? {
        Some(source) => source,
        None => {
            let offered = sources_offered(root);
            match offered.as_slice() {
                [] => {
                    return Err(format!(
                        "{} has no ticket source: turn on Linear or GitHub in Settings, or keep a note's checklist",
                        name_of(root)
                    ));
                }
                [one] => one.to_string(),
                _ => {
                    let options = offered
                        .iter()
                        .map(|s| MenuOption::new(source_words(s)))
                        .collect();
                    match alone::choose(Menu::new("Work on what?", options))? {
                        Some(at) => offered[at].to_string(),
                        None => return Ok(ExitCode::from(1)),
                    }
                }
            }
        }
    };
    match source.as_str() {
        "linear" => match pick_ticket()? {
            Some(ticket) => linear_ticket(rest, &ticket, trusted),
            None => Ok(ExitCode::from(1)),
        },
        "github" => match pick_issue(root)? {
            Some(issue) => super::worktree_from_issue::run(rest, &issue, made),
            None => Ok(ExitCode::from(1)),
        },
        _ => {
            let Some(note_name) = pick_note()? else {
                return Ok(ExitCode::from(1));
            };
            match pick_item(root, &note_name)? {
                Some(line) => note::run(rest, &note_name, &line.to_string(), made),
                None => Ok(ExitCode::from(1)),
            }
        }
    }
}

const ASSIGNED: &str = "query { viewer { assignedIssues(first: 100, orderBy: createdAt, \
     filter: { state: { type: { nin: [\"completed\", \"canceled\", \"duplicate\"] } } }) { \
     nodes { identifier title createdAt state { type } } } } }";

/// The open Linear tickets assigned to you, newest first: their IDs and
/// titles, the titles without control characters, since someone else
/// wrote them.
pub async fn assigned_tickets(source: &str) -> anyhow::Result<Vec<(String, String)>> {
    let data = linear::graphql(ASSIGNED, json!({}), source).await?;
    Ok(assigned_of(&data))
}

/// `assigned_tickets` from Linear's answer.
fn assigned_of(data: &Value) -> Vec<(String, String)> {
    let mut tickets: Vec<(String, String, String)> = data
        .pointer("/viewer/assignedIssues/nodes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|n| {
            let kind = n.pointer("/state/type").and_then(Value::as_str);
            !matches!(kind, Some("completed" | "canceled" | "duplicate"))
        })
        .filter_map(|n| {
            let text = |k: &str| n.get(k).and_then(Value::as_str).map(str::to_string);
            let title: String = text("title")?.chars().filter(|c| !c.is_control()).collect();
            Some((
                text("createdAt").unwrap_or_default(),
                text("identifier")?,
                title,
            ))
        })
        .collect();
    tickets.sort_by(|a, b| b.0.cmp(&a.0));
    tickets
        .into_iter()
        .map(|(_, id, title)| (id, title))
        .collect()
}

/// Which Linear ticket, from those assigned to you, or one typed in.
fn pick_ticket() -> Result<Option<String>, String> {
    let tickets =
        util::block_on(assigned_tickets("wt: your tickets")).map_err(|e| e.to_string())?;
    let items = tickets
        .iter()
        .map(|(id, title)| PickItem::new(format!("{id} {title}"), id.clone()))
        .collect();
    let title = if tickets.is_empty() {
        "No open tickets assigned to you: a ticket's ID to work on"
    } else {
        "Work on a Linear ticket"
    };
    let picker = Picker::new(title, items)
        .named("tickets")
        .prompt("Ticket: ")
        .keep_order()
        .free();
    let Some(picked) = alone::pick(picker)? else {
        return Ok(None);
    };
    let value = picked.value.trim();
    if !worktree::looks_like_ticket(value) {
        return Err(format!("not a ticket ID or Linear link: {value}"));
    }
    Ok(Some(value.to_string()))
}

/// Which GitHub issue, from the repository's open ones, those assigned to
/// you first, as M-p's "Start work on a GitHub issue" lists them, or one
/// typed in, as `<owner>/<repo>#<number>`.
fn pick_issue(root: &Path) -> Result<Option<String>, String> {
    let (repo, issues) = util::block_on(async {
        let repo = github::repo_of(root).await?;
        let issues = github::open_issues(&repo, "wt: your issues").await?;
        anyhow::Ok((repo, issues))
    })
    .map_err(|e| format!("couldn't list the open issues: {e}"))?;
    let items: Vec<PickItem> = issues
        .iter()
        .map(|i| {
            let title: String = i.title.chars().filter(|c| !c.is_control()).collect();
            PickItem {
                text: format!("#{} {title}", i.number),
                tags: if i.yours {
                    "yours".into()
                } else {
                    String::new()
                },
                value: i.number.to_string(),
                ..PickItem::default()
            }
        })
        .collect();
    let title = if items.is_empty() {
        "No open issues: an issue number to work on"
    } else {
        "Work on a GitHub issue"
    };
    let picker = Picker::new(title, items)
        .named("work-on-issue")
        .prompt("Issue: ")
        .keep_order()
        .free();
    let Some(picked) = alone::pick(picker)? else {
        return Ok(None);
    };
    let typed = picked.value.trim();
    let number: u64 = typed
        .strip_prefix('#')
        .unwrap_or(typed)
        .parse()
        .map_err(|_| format!("not an issue number: {typed}"))?;
    Ok(Some(format!("{repo}#{number}")))
}

/// Which note, the most recently changed first, as M-I asks: its path
/// within the notes directory.
fn pick_note() -> Result<Option<String>, String> {
    let notes = note::notes_dir();
    let names = note::notes_newest_first(&notes);
    if names.is_empty() {
        return Err(format!("no notes in {}", crate::paths::tilde(&notes)));
    }
    let items = names
        .iter()
        .map(|n| PickItem::new(n.clone(), n.clone()))
        .collect();
    let picker = Picker::new("Which note", items)
        .named("notes")
        .prompt("note> ")
        .keep_order();
    Ok(alone::pick(picker)?.map(|p| format!("{}.md", p.value)))
}

/// Which of the note's unticked items no worktree has yet, as M-I lists
/// them: its line.
fn pick_item(root: &Path, note_name: &str) -> Result<Option<usize>, String> {
    can_ask()?;
    // Its real path, as `note::run` reads it, since a note's file is
    // found by its real path (a notes folder may be a link).
    let given = note::notes_dir();
    let dir = given
        .canonicalize()
        .map_err(|_| format!("no notes directory at {}", given.display()))?;
    let file = note::note_file(&dir, note_name)?;
    let name = file
        .strip_prefix(&dir)
        .map(|r| r.to_string_lossy().into_owned())
        .unwrap_or_else(|_| note_name.to_string());
    let text = std::fs::read_to_string(&file).map_err(|e| e.to_string())?;
    let title = name.strip_suffix(".md").unwrap_or(&name).to_string();
    if note::items(&text).is_empty() {
        return Err(format!("{title} has no unticked items"));
    }
    let items = note::unclaimed(root, &name, &text);
    if items.is_empty() {
        return Err(format!(
            "every unticked item of {title} has a worktree already"
        ));
    }
    let previews = if crate::settings::is_on("wt_item_preview") {
        items
            .iter()
            .map(|i| note::item_text(&text, i.line))
            .collect()
    } else {
        Vec::new()
    };
    let short = Path::new(&title)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let picker = Picker::new(format!("Work on {short}'s items"), note::item_rows(&items))
        .named("items")
        .prompt(format!("{short}> "))
        .previews(previews);
    Ok(alone::pick(picker)?.and_then(|p| p.value.parse().ok()))
}

// ── A Linear ticket ─────────────────────────────────────────────────────────

/// A Linear ticket's worktree, in the repository here, else, once the
/// ticket is found, in one picked, those whose `tickets` is `linear` first.
fn linear_ticket(given: &Given, ticket: &str, trusted: bool) -> Result<ExitCode, String> {
    if given.last("--target").is_some() {
        return Err("--target takes a note's item or a GitHub issue, not a Linear ticket".into());
    }
    let extra = worktree::one_line(&given.all("--prompt").join(" "));
    let target = given
        .positional
        .first()
        .map(String::as_str)
        .unwrap_or(worktree::DEFAULT_WORKTREE_DIR);
    let (key, number) = parse_identifier(ticket)
        .ok_or_else(|| format!("could not parse a Linear ticket identifier from \"{ticket}\""))?;
    let issue = fetch_issue(&key, number)?;
    let branch = issue.branch_name.clone().unwrap_or_else(|| {
        format!(
            "{}-{}",
            issue.identifier.to_lowercase(),
            worktree::slugify(&issue.title)
        )
    });
    eprintln!("Ticket:   {} — {}", issue.identifier, issue.title);
    let root = pick_repository(true)?;
    let path = worktree::normalize(&root.join(target).join(&branch));

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

    // A worktree or local branch already there is the ticket's own work,
    // reused as it is, so only a new branch needs a start point.
    let is_new = !path.exists() && !worktree::branch_exists(&root, &branch);
    let start = if is_new {
        Some(ticket_start(&root, &branch)?)
    } else {
        None
    };
    let purpose = Purpose {
        work: "ticket",
        command: "wt",
        start: start.as_ref().map(|s| s.commit.as_str()),
    };
    worktree::ensure_worktree(&path, &branch, &purpose).map_err(|e| e.to_string())?;
    if start.as_ref().is_some_and(|s| s.tracks) {
        worktree::set_upstream(&root, &branch, "origin", &branch);
    }
    // A teammate's start on the ticket is their code: Claude loads its
    // CLAUDE.md, settings and hooks only once you say so (src/trust.rs).
    // Against the default branch as it is now; offline, as last fetched.
    if !trusted && start.as_ref().is_none_or(|s| s.tracks) {
        let _ = worktree::default_start(&root, "wt: fetch the base");
    }
    if !trusted {
        crate::trust::branch_yours_or_listed(&crate::paths::home(), &path)?;
    }
    crate::cmd::clone_build::start(&path);
    worktree::link_local_settings(&root, &path).map_err(|e| e.to_string())?;
    let mut prompt = prompt_for(&issue, &extra);
    if let Some(words) = start.as_ref().and_then(|s| s.offline.as_ref()) {
        // Offline, wt couldn't ask whether someone pushed the ticket's
        // branch since the clone last fetched; whose code it holds is the
        // trust check's to say, so Claude asks rather than takes it in.
        let catch_up = format!(
            "if origin has a branch named {branch}, the ticket was started elsewhere: ask me before building on it; otherwise rebase onto origin's default branch"
        );
        prompt.push_str(&format!(
            " This branch starts from {words}, since origin couldn't be reached: before your first commit, fetch origin and {catch_up}."
        ));
    }
    // Read by the `wt` function: line 1 the worktree, then the prompt, so
    // the function and the standalone path can't drift apart.
    if let Some(cd_file) = &given.cd_file {
        worktree::write_cd_file(cd_file, &format!("{}\n{prompt}\n", path.display()))
            .map_err(|e| e.to_string())?;
    }
    if given.no_claude {
        return Ok(ExitCode::SUCCESS);
    }
    Ok(worktree::run_in("claude", &[prompt], Path::new(&path)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn work_asked_by_its_number_an_issues_number_or_link_or_a_linear_ticket() {
        assert_eq!(Asked::parse("12"), Some(Asked::Number(12)));
        assert_eq!(Asked::parse("#12"), Some(Asked::Issue("12".into())));
        assert_eq!(
            Asked::parse("https://github.com/acme/widgets/issues/12"),
            Some(Asked::Issue("acme/widgets#12".into()))
        );
        assert_eq!(Asked::parse("ABC-12"), Some(Asked::Ticket("ABC-12".into())));
        assert_eq!(Asked::parse(".worktrees"), None, "a path stays a path");
        assert_eq!(Asked::parse("#"), None);
        assert_eq!(
            Asked::parse("https://github.com/acme/widgets/pull/12"),
            None
        );
    }

    #[test]
    fn your_open_tickets_newest_first_without_closed_ones_or_control_characters() {
        let data = json!({ "viewer": { "assignedIssues": { "nodes": [
            { "identifier": "ENG-1", "title": "Old", "createdAt": "2026-01-01T00:00:00Z", "state": { "type": "started" } },
            { "identifier": "ENG-2", "title": "New\u{1b}[2J", "createdAt": "2026-02-01T00:00:00Z", "state": { "type": "unstarted" } },
            { "identifier": "ENG-3", "title": "Done", "createdAt": "2026-03-01T00:00:00Z", "state": { "type": "completed" } },
        ] } } });
        assert_eq!(
            assigned_of(&data),
            vec![
                ("ENG-2".to_string(), "New[2J".to_string()),
                ("ENG-1".to_string(), "Old".to_string())
            ]
        );
    }

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
