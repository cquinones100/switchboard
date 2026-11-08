//! `worktree-from-pr [path] <pr-number-or-url>`: a git worktree on the head
//! branch of a GitHub pull request, with Claude opened in it, told to say
//! where the PR stands before touching it. Ported from `bin/worktree-from-pr`.
//!
//! A fork's PR is set up as `gh pr checkout` sets it up: the fork becomes a
//! remote named after its owner, and the worktree's branch,
//! `<owner>-<branch>`, tracks the fork's, so a push updates the PR. A PR's
//! state doesn't matter: a merged or closed one whose branch the remote has
//! since deleted is opened from the commits GitHub keeps for it
//! (`refs/pull/<n>/head`), on a local branch of the same name with no
//! upstream.
//!
//! The PR is read from GitHub's API (`integrations::github`, its token as the
//! rest of Switchboard finds it) where the script asked `gh pr view`; the
//! repository is the one the clone's remote names as configured, before any
//! `insteadOf` rewrite. Everything after that is plain git. `init zsh`
//! prints the `wtpr` function, which works as `wt` does
//! (worktree_from_ticket.rs).
//!
//! A fork's PR is someone else's code, so by default it is opened
//! `--untrusted` (any PR can be, with the flag; `--trusted` opens a fork's
//! as your own, a collaborator's you know), as bin/worktree-from-pr came to:
//! in `~/.cache/switchboard/pr-review/<owner>-<repo>/<n>`, outside the
//! clone, on a local branch `pr-review/<n>` made from the commits GitHub
//! keeps for it (`refs/pull/<n>/head`), with no remote added. Its own
//! worktree config, which no file in it can change, marks it
//! (`switchboard.untrusted`, src/trust.rs) and turns off git's hooks, every
//! filter your config names (a checkout runs a smudge filter the PR's
//! `.gitattributes` picks), the file-system monitor and submodule recursion
//! (which would fetch from wherever the PR's `.gitmodules` says), so no git
//! command there runs anything of the PR's or yours on its files. Nothing is linked
//! into it, no Claude is started and no shell moved into it (the `wtpr`
//! function reads exit code 3 as that), since its `CLAUDE.md`,
//! `.claude/settings.json`, hooks and skills are the PR's. It prints the
//! author and how long their account has been on GitHub, the PR's size and
//! `pr-risk`'s list of what in it runs without being asked, then how to
//! review it from your main clone. Run again, it moves the worktree to the
//! PR's newest commit. Its title isn't printed: the author wrote it, and a
//! terminal reads escape sequences in it.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use crate::integrations::github::{self, Repo};
use crate::mux::socket;
use crate::worktree::{self, Given, PrDetails, shell_quote};
use crate::{paths, settings, trust, util};

const SOURCE: &str = "wtpr: the PR";
const FETCH: &str = "wtpr: fetch the branch";

fn usage() -> ExitCode {
    eprintln!(
        "Usage: worktree-from-pr [--untrusted | --trusted] [path] <pr-number-or-url>\n       \
         worktree-from-pr init <zsh|bash>\n\n\
         Examples:\n  \
         worktree-from-pr 123\n  \
         worktree-from-pr https://github.com/owner/repo/pull/123\n  \
         worktree-from-pr .worktrees 123\n  \
         worktree-from-pr --untrusted 123     a worktree that runs none of its code (a fork's PR's default)\n  \
         worktree-from-pr --trusted 123       a fork's PR as your own\n\n\
         Path defaults to {} under the repo root.",
        worktree::DEFAULT_WORKTREE_DIR
    );
    ExitCode::from(1)
}

/// The `wtpr` function: runs the command with `--no-claude`, cd's into the
/// worktree and starts Claude there. WTPR_PROMPT, when set, is Claude's
/// first prompt (mission control's CI view starts /ci-loop this way); else
/// the command's own, which asks where the PR stands (`start_prompt`),
/// unless the wtpr_prompt setting is off. Exit code 3 is an untrusted PR's
/// worktree, which no shell or Claude goes into.
pub fn shell_function() -> String {
    format!(
        r#"wtpr() {{
  local cdfile rc dir prompt
  cdfile=$(mktemp) || return 1
  {} --no-claude --cd-file "$cdfile" "$@"
  rc=$?
  if [ $rc -eq 3 ]; then
    rm -f "$cdfile"
    return 0
  fi
  if [ $rc -ne 0 ]; then
    rm -f "$cdfile"
    return $rc
  fi
  dir=$(sed -n 1p "$cdfile")
  prompt=$(sed -n 2p "$cdfile")
  rm -f "$cdfile"
  if [ ! -d "$dir" ]; then
    echo "wtpr: expected a worktree directory, got: $dir" >&2
    return 1
  fi
  cd "$dir" || return 1
  if [ -n "${{WTPR_PROMPT:-}}" ]; then claude "$WTPR_PROMPT"
  elif [ -n "$prompt" ]; then claude "$prompt"
  else claude; fi
}}"#,
        worktree::self_command("worktree-from-pr")
    )
}

/// The prompt Claude starts with in the worktree: a PR picked up is often
/// someone else's or half done, so Claude first says where it stands and
/// waits, rather than starting on it as `wt` does on a ticket. Never its
/// title: the PR's author wrote that, and here it would reach Claude as
/// your own words. Someone else's unless GitHub says it is yours: when it
/// can't say who you are, the PR is treated as someone else's. One line, as
/// the `wtpr` function reads it back.
pub fn start_prompt(pr: &PrDetails, me: Option<&str>) -> String {
    let author = pr.author.as_deref().unwrap_or("");
    let theirs =
        author.is_empty() || me.is_none_or(|me| me.is_empty() || !me.eq_ignore_ascii_case(author));
    let closed = !pr.state.is_empty() && pr.state != "OPEN";
    let by = if author.is_empty() {
        String::new()
    } else {
        format!(" by {author}")
    };
    let parts = [
        format!("Pick up PR #{}{by}.", pr.number),
        if closed {
            format!("It is {}, so say that first.", pr.state.to_lowercase())
        } else {
            String::new()
        },
        "Before changing anything, read its description, its diff, its unresolved review threads and its failing checks, and its Linear ticket if the branch or the description names one.".into(),
        "Then tell me in a few lines where it stands: what's left (review comments to answer, failing checks, merge conflicts, anything the ticket asks for that the PR doesn't do) and what you would do next.".into(),
        "Don't change anything until I say.".into(),
        if theirs {
            let whose = if author.is_empty() {
                "someone else's".to_string()
            } else {
                format!("{author}'s")
            };
            format!("It's {whose} PR, so don't push to it unless I ask.")
        } else {
            String::new()
        },
    ];
    worktree::one_line(
        &parts
            .iter()
            .filter(|p| !p.is_empty())
            .cloned()
            .collect::<Vec<_>>()
            .join(" "),
    )
}

/// The GitHub repository a clone's remote points at, from the URL as
/// configured, before any `insteadOf` rewrite: the name it stands for.
fn remote_repo(root: &Path) -> Option<Repo> {
    let remote = worktree::default_remote(root)?;
    let url = worktree::git_at(root, &["config", "--get", &format!("remote.{remote}.url")]).ok()?;
    github::parse_remote(&url)
}

fn same_repo(a: &Repo, b: &str) -> bool {
    a.to_string().eq_ignore_ascii_case(b)
}

/// The remote of a clone that points at `owner/name` (upstream, in a fork's
/// clone), by its URL as configured.
fn remote_for(root: &Path, repo: &str) -> Option<String> {
    worktree::remotes(root).into_iter().find(|remote| {
        worktree::git_at(root, &["config", "--get", &format!("remote.{remote}.url")])
            .ok()
            .and_then(|url| github::parse_remote(&url))
            .is_some_and(|r| same_repo(&r, repo))
    })
}

/// The repository a bare PR number is a PR of: the one gh would ask
/// (`github::repo_of`: `gh repo set-default`'s, else upstream's when origin
/// is a fork, else origin's), else the first remote's on GitHub.
fn numbered_repo(root: &Path) -> Result<Repo, String> {
    match util::block_on(github::repo_of(root)) {
        Ok(repo) => Ok(repo),
        Err(e) if crate::net::kind_of(&e).is_some() => Err(format!(
            "could not work out which repository the PR is in: {e}"
        )),
        Err(_) => {
            remote_repo(root).ok_or_else(|| "this repository's remote isn't on GitHub".to_string())
        }
    }
}

/// A clone of `owner/name` among the repositories you work in: every open
/// pane's (when a Switchboard server answers), every one a Claude session
/// ran in lately (as wt-revive's cache has them), and the directories right
/// under your home. Its main clone, not a worktree of it.
fn find_clone(repo: &str) -> Option<PathBuf> {
    let home = paths::home();
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Ok(list) = socket::ask_ok(&["list"])
        && let Ok(list) = serde_json::from_str::<serde_json::Value>(&list)
    {
        for tab in list["sessions"]
            .as_array()
            .into_iter()
            .flatten()
            .flat_map(|s| s["tabs"].as_array().into_iter().flatten())
        {
            for pane in tab["panes"].as_array().into_iter().flatten() {
                if let Some(cwd) = pane["cwd"].as_str() {
                    dirs.push(PathBuf::from(cwd));
                }
            }
        }
    }
    if let Some(cache) = util::read_json(home.join(".cache/wt-revive/index.json"))
        && let Some(transcripts) = cache["transcripts"].as_object()
    {
        dirs.extend(
            transcripts
                .values()
                .filter_map(|t| t["cwd"].as_str())
                .map(PathBuf::from),
        );
    }
    if let Ok(entries) = std::fs::read_dir(&home) {
        let mut under: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_dir() && p.join(".git").exists())
            .collect();
        under.sort();
        dirs.extend(under);
    }
    // A worktree under .claude/worktrees is its main clone's; others are asked.
    let mut seen = BTreeSet::new();
    for dir in dirs {
        let text = dir.to_string_lossy().into_owned();
        let dir = match text.find("/.claude/worktrees/") {
            Some(at) => PathBuf::from(&text[..at]),
            None => dir,
        };
        if !dir.exists() || !seen.insert(dir.clone()) {
            continue;
        }
        if let Some(root) = worktree::repo_root_of(&dir)
            && remote_for(&root, repo).is_some()
        {
            return Some(root);
        }
    }
    None
}

/// The remote a fork's PR is fetched from: one already pointing at the fork,
/// else one added, named after its owner (or `<owner>-<name>` when that is
/// taken), in the protocol the base remote uses. `None` when the fork is
/// gone; an error when git won't add the remote, as the Node version ended
/// there rather than fetch from a remote that isn't there.
fn fork_remote(pr: &PrDetails, base: &str, root: &Path) -> Result<Option<String>, String> {
    let (Some(name), Some(owner)) = (pr.head_repository.as_deref(), pr.head_owner.as_deref())
    else {
        return Ok(None);
    };
    let repo = format!("{owner}/{name}");
    let url_of = |remote: &str| {
        worktree::git_at(root, &["config", "--get", &format!("remote.{remote}.url")])
            .unwrap_or_default()
    };
    let remotes = worktree::remotes(root);
    if let Some(existing) = remotes
        .iter()
        .find(|r| worktree::github_slug(&url_of(r)).is_some_and(|s| s.eq_ignore_ascii_case(&repo)))
    {
        return Ok(Some(existing.clone()));
    }
    let remote = if remotes.iter().any(|r| r == owner) {
        format!("{owner}-{name}")
    } else {
        owner.to_string()
    };
    let base_url = url_of(base);
    let url = if base_url.starts_with("git@") || base_url.starts_with("ssh:") {
        format!("git@github.com:{repo}.git")
    } else {
        format!("https://github.com/{repo}.git")
    };
    if !worktree::git_ok(root, &["remote", "add", &remote, &url]) {
        return Err(format!("could not add the fork's remote {remote} ({url})"));
    }
    eprintln!("Remote:   added {remote} ({url})");
    Ok(Some(remote))
}

/// A `--single-branch` clone (which `--depth` implies) fetches only the
/// branch it was cloned with, so `refs/remotes/<remote>/<branch>` stays
/// unmapped: push and pull still work off `branch.<name>.merge`, but
/// `@{upstream}` and ahead/behind don't resolve. `set-branches --add` widens
/// the refspec for just this branch; failing is not fatal.
fn ensure_refspec_covers(branch: &str, remote: &str, root: &Path) {
    let specs = worktree::git_at(
        root,
        &["config", "--get-all", &format!("remote.{remote}.fetch")],
    )
    .unwrap_or_default();
    let covered = specs.lines().any(|s| {
        s.ends_with(&format!(":refs/remotes/{remote}/*"))
            || s.ends_with(&format!(":refs/remotes/{remote}/{branch}"))
    });
    if !covered {
        worktree::git_ok(root, &["remote", "set-branches", "--add", remote, branch]);
    }
}

/// The local branch, the PR's branch on `remote` (the fork's, for a fork's
/// PR), and `base`, the clone's own remote, where the PR's last commit is
/// always kept.
struct Checkout<'a> {
    branch: &'a str,
    remote_branch: &'a str,
    remote: Option<&'a str>,
    base: &'a str,
}

fn create_worktree(path: &Path, c: &Checkout, root: &Path, number: u64) -> Result<(), String> {
    let on_remote = c
        .remote
        .is_some_and(|r| worktree::fetch_branch(root, r, c.remote_branch, FETCH));
    // Widened only for a branch the remote has: a refspec naming one it
    // doesn't would fail every later fetch.
    if on_remote && let Some(r) = c.remote {
        ensure_refspec_covers(c.remote_branch, r, root);
    }
    let start = match (on_remote, c.remote) {
        (true, Some(r)) => format!("{r}/{}", c.remote_branch),
        _ => {
            // The PR's last commit, which GitHub keeps under
            // refs/pull/<n>/head as long as the PR exists.
            let pull = format!("refs/remotes/{}/pr/{number}", c.base);
            worktree::count_fetch(root, c.base, FETCH);
            if !worktree::git_loud(
                root,
                &["fetch", c.base, &format!("+refs/pull/{number}/head:{pull}")],
                true,
            ) {
                return Err(format!(
                    "could not fetch PR #{number} from {}: neither its branch nor refs/pull/{number}/head (see output above)",
                    c.base
                ));
            }
            eprintln!(
                "{} is gone from {}, so the worktree starts at the PR's last commit, with no upstream.",
                c.remote_branch,
                c.remote.unwrap_or("the deleted fork")
            );
            pull
        }
    };
    // A branch that already exists locally keeps its own commits; -b would
    // refuse to reuse it. Anything it is missing is picked up below.
    let existing = worktree::branch_exists(root, c.branch);
    let at = path.to_string_lossy().into_owned();
    let mut args = vec!["worktree", "add"];
    if existing {
        args.extend([at.as_str(), c.branch]);
    } else {
        if !on_remote {
            args.push("--no-track");
        }
        args.extend(["-b", c.branch, at.as_str(), start.as_str()]);
    }
    if !worktree::git_loud(root, &args, true) {
        return Err("git worktree creation failed (see output above)".into());
    }
    let Some(remote) = c.remote.filter(|_| on_remote) else {
        return Ok(());
    };
    if !existing {
        worktree::set_upstream(root, c.branch, remote, c.remote_branch);
        return Ok(());
    }
    // Catches up a local branch left behind by an earlier run; --ff-only, so
    // a branch carrying your own unpushed commits is reported, never
    // rewritten.
    if !worktree::git_ok(
        path,
        &[
            "merge",
            "--ff-only",
            &format!("{remote}/{}", c.remote_branch),
        ],
    ) {
        eprintln!(
            "Note: {} has diverged from {remote}/{} and was left as-is.",
            c.branch, c.remote_branch
        );
    }
    Ok(())
}

pub fn main(args: Vec<String>) -> ExitCode {
    // A program you run asks the services whatever Settings says.
    crate::integrations::run_by_you();
    let given = match Given::parse(&args, &[]) {
        Ok(g) => g,
        Err(_) => return worktree::fail("--cd-file requires a path"),
    };
    match worktree::init_shell(&given.positional) {
        Some(true) => {
            println!("{}", shell_function());
            return ExitCode::SUCCESS;
        }
        Some(false) => {
            return worktree::fail("init requires a shell: worktree-from-pr init <zsh|bash>");
        }
        None => {}
    }
    if given.help {
        return usage();
    }
    let flag = |name: &str| given.positional.iter().any(|a| a == name);
    let asked = match (flag("--untrusted"), flag("--trusted")) {
        (true, true) => return worktree::fail("--untrusted and --trusted can't go together"),
        (true, false) => Asked::Untrusted,
        (false, true) => Asked::Trusted,
        (false, false) => Asked::Default,
    };
    let positional: Vec<&String> = given
        .positional
        .iter()
        .filter(|a| *a != "--untrusted" && *a != "--trusted")
        .collect();
    let (target, input) = match positional.as_slice() {
        [pr] => (worktree::DEFAULT_WORKTREE_DIR, pr.as_str()),
        [path, pr, ..] => (path.as_str(), pr.as_str()),
        [] => return usage(),
    };
    if !worktree::looks_like_pr(input) {
        return usage();
    }
    match run(&given, asked, target, input) {
        Ok(code) => code,
        Err(e) => worktree::fail(&e),
    }
}

/// Whether the PR was asked for as trusted or not: by default a fork's is
/// untrusted and any other trusted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Asked {
    Default,
    Untrusted,
    Trusted,
}

fn run(given: &Given, asked: Asked, target: &str, input: &str) -> Result<ExitCode, String> {
    let Some((number, named)) = worktree::pr_from(input) else {
        return Err(format!(
            "could not parse a pull request number or URL from \"{input}\""
        ));
    };
    // The worktree is made in a clone of the PR's repository: the one you
    // are in, or, for a URL naming another repository (or given from
    // outside any), a clone of that one found among those you work in, which
    // the `wtpr` function then cd's into. A clone with any remote pointing at
    // it counts (a fork's clone, whose upstream is the project). A bare
    // number is a PR of the repository you are in, as gh picked it.
    let here = std::env::current_dir()
        .ok()
        .and_then(|d| worktree::repo_root_of(&d));
    let mut root = here.clone();
    if let Some(named) = &named
        && !here
            .as_deref()
            .is_some_and(|here| remote_for(here, named).is_some())
    {
        root = find_clone(named);
        let Some(found) = &root else {
            return Err(format!(
                "That PR is in {named}, and no clone of it was found among your tabs, recent Claude sessions or the directories in your home.\nClone it, then run wtpr again."
            ));
        };
        eprintln!("Repo:     {}", found.display());
    }
    let Some(root) = root else {
        return Err(
            "not inside a git repository: give the PR's URL, or run this from its repository"
                .into(),
        );
    };
    let repo = match named.as_deref().and_then(|n| n.split_once('/')) {
        Some((owner, name)) => Repo {
            owner: owner.into(),
            name: name.into(),
        },
        None => {
            numbered_repo(&root).map_err(|e| format!("could not read pull request {input}: {e}"))?
        }
    };
    let pr = match util::block_on(worktree::pr_details(&repo, number, SOURCE)) {
        Ok(Some(pr)) => pr,
        Ok(None) => return Err(format!("{repo} has no pull request #{number}")),
        Err(e) => return Err(format!("could not read pull request {input}: {e}")),
    };
    // A fork's PR is someone else's code: untrusted unless asked otherwise.
    let fork = pr.is_cross_repository;
    if asked == Asked::Untrusted || (fork && asked != Asked::Trusted) {
        return untrusted(given, &pr, &repo, &root);
    }
    let wanted = settings::setting("wtpr_prompt") != Some(serde_json::Value::Bool(false));
    let me = if wanted {
        util::block_on(worktree::viewer_login(SOURCE))
    } else {
        None
    };

    // A fork's branch is on the fork, not this repository's remote, and is
    // often its main, so it is checked out as <owner>-<branch>.
    let owner = pr.head_owner.clone().unwrap_or_default();
    let branch = if fork {
        format!("{owner}-{}", pr.head_ref_name)
    } else {
        pr.head_ref_name.clone()
    };
    let path = worktree::normalize(&root.join(target).join(format!(
        "pr-{}-{}",
        pr.number,
        slug(&pr.head_ref_name)
    )));

    eprintln!("PR:       #{} — {}", pr.number, pr.title);
    eprintln!("Author:   {}", pr.author.as_deref().unwrap_or("unknown"));
    eprintln!("Branch:   {branch}");
    if pr.state != "OPEN" {
        eprintln!("State:    {}", pr.state);
    }
    if fork && pr.state == "OPEN" && !pr.maintainer_can_modify {
        eprintln!(
            "Note:     {owner} hasn't let maintainers push to the fork, so a push may be refused."
        );
    }

    // Reusing rather than erroring keeps a rerun on the same PR cheap. The
    // branch check comes second because a worktree at our own path is
    // already holding the branch.
    let existing = if path.exists() {
        eprintln!("Worktree already exists, reusing it.");
        Some(path.clone())
    } else {
        worktree::holding_branch(&root, &branch)
    };
    // The main clone is not somewhere to send anyone: git allows a branch in
    // one worktree at a time, and cd'ing to the main clone reads as the tool
    // silently doing nothing.
    if existing.as_deref() == Some(root.as_path()) {
        return Err(format!(
            "{branch} is already checked out in your main clone:\n  {}\n\
             Git allows a branch in only one worktree at a time. Either work there,\n\
             or switch that checkout away and rerun:\n  git -C {} switch <some-other-branch>",
            root.display(),
            root.display()
        ));
    }
    if existing.as_ref().is_some_and(|e| *e != path) {
        eprintln!("Branch is already checked out elsewhere, using that.");
    }
    let finished = existing.clone().unwrap_or_else(|| path.clone());
    eprintln!("Worktree: {}", finished.display());
    if existing.is_none() {
        // The PR's own branch, and refs/pull/<n>/head, are on the remote
        // that points at the PR's repository, which in a fork's clone is
        // upstream rather than origin.
        let Some(base) =
            remote_for(&root, &repo.to_string()).or_else(|| worktree::default_remote(&root))
        else {
            return Err("repository has no remotes".into());
        };
        let remote = if fork {
            fork_remote(&pr, &base, &root)?
        } else {
            Some(base.clone())
        };
        let checkout = Checkout {
            branch: &branch,
            remote_branch: &pr.head_ref_name,
            remote: remote.as_deref(),
            base: &base,
        };
        create_worktree(&path, &checkout, &root, pr.number)?;
    }
    worktree::link_local_settings(&root, &finished).map_err(|e| e.to_string())?;

    let prompt = if wanted {
        start_prompt(&pr, me.as_deref())
    } else {
        String::new()
    };
    if let Some(cd_file) = &given.cd_file {
        worktree::write_cd_file(cd_file, &format!("{}\n{prompt}\n", finished.display()))
            .map_err(|e| e.to_string())?;
    }
    if given.no_claude {
        return Ok(ExitCode::SUCCESS);
    }
    let args: Vec<String> = if prompt.is_empty() {
        Vec::new()
    } else {
        vec![prompt]
    };
    Ok(worktree::run_in("claude", &args, &finished))
}

// ── Someone else's PR ───────────────────────────────────────────────────────

/// A PR opened untrusted: its review worktree made or moved to the PR's
/// newest commit, and what to know about it printed. Exit code 3 for the
/// `wtpr` function, the only caller with `--cd-file`: nothing to cd into
/// or start.
fn untrusted(given: &Given, pr: &PrDetails, repo: &Repo, root: &Path) -> Result<ExitCode, String> {
    let from = match (pr.is_cross_repository, pr.head_owner.as_deref()) {
        (true, Some(owner)) => format!(" from {owner}'s fork"),
        (true, None) => " from a fork since deleted".to_string(),
        _ => String::new(),
    };
    eprintln!("PR:       #{}{from}", pr.number);
    let (dir, base) = untrusted_worktree(pr, repo, root)?;
    print_facts(pr, &dir, &base, root);
    if given.cd_file.is_some() {
        return Ok(ExitCode::from(3));
    }
    Ok(ExitCode::SUCCESS)
}

/// git's options that run nothing in the review worktree, for the commands
/// before its own config says so: no hook, no file-system monitor, no
/// submodule (which `submodule.recurse` in your config would fetch from the
/// URL the PR's `.gitmodules` names), and every filter your config names
/// emptied (`off`).
fn quiet(off: &[(String, String)]) -> Vec<String> {
    let mut args: Vec<String> = [
        "-c",
        "core.hooksPath=/dev/null",
        "-c",
        "core.fsmonitor=false",
        "-c",
        "submodule.recurse=false",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    for (key, value) in off {
        args.push("-c".into());
        args.push(format!("{key}={value}"));
    }
    args
}

/// Git's filters your config names (git-lfs's, say), which the PR's
/// `.gitattributes` choose among, each emptied and not required, so a
/// checkout or status there runs none of them: the keys and their values.
fn filters_off(root: &Path) -> Vec<(String, String)> {
    let listed = worktree::git_at(
        root,
        &[
            "config",
            "--get-regexp",
            r"^filter\..*\.(smudge|clean|process)$",
        ],
    )
    .unwrap_or_default();
    let names: BTreeSet<String> = listed
        .lines()
        .filter_map(|line| {
            let key = line.split_once(' ').map_or(line, |(k, _)| k);
            let (name, _) = key.strip_prefix("filter.")?.rsplit_once('.')?;
            Some(name.to_string())
        })
        .collect();
    names
        .iter()
        .flat_map(|name| {
            [
                ("smudge", ""),
                ("clean", ""),
                ("process", ""),
                ("required", "false"),
            ]
            .map(|(key, value)| (format!("filter.{name}.{key}"), value.to_string()))
        })
        .collect()
}

/// git's output in `dir` with `args` before the command, or its error.
fn git_with(dir: &Path, before: &[String], args: &[&str]) -> Result<String, String> {
    let mut all: Vec<&str> = before.iter().map(String::as_str).collect();
    all.extend(args);
    worktree::git_at(dir, &all).map_err(|e| e.to_string())
}

/// The repository's own git directory, the same for every worktree of it.
fn common_dir(dir: &Path) -> Option<PathBuf> {
    let common = worktree::git_at(dir, &["rev-parse", "--git-common-dir"]).ok()?;
    dir.join(common).canonicalize().ok()
}

/// The review worktree of a PR whose code isn't trusted: made, or moved to
/// the PR's newest commit, with no hook run, no remote added and nothing
/// linked. Gives it and the PR's base as `<remote>/<branch>`.
fn untrusted_worktree(
    pr: &PrDetails,
    repo: &Repo,
    root: &Path,
) -> Result<(PathBuf, String), String> {
    // A folder per repository, since two repositories' PRs share numbers.
    let mut slug = String::new();
    for c in format!("{}-{}", repo.owner, repo.name).chars() {
        let c = if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
            c
        } else {
            '-'
        };
        if !(c == '-' && slug.ends_with('-')) {
            slug.push(c);
        }
    }
    let dir = trust::review_dir().join(&slug).join(pr.number.to_string());
    let branch = format!("pr-review/{}", pr.number);
    let Some(base) = remote_for(root, &repo.to_string()).or_else(|| worktree::default_remote(root))
    else {
        return Err("repository has no remotes".into());
    };
    if pr.base_ref_name.is_empty() || !worktree::fetch_branch(root, &base, &pr.base_ref_name, FETCH)
    {
        return Err(format!(
            "could not fetch the PR's base, {}, from {base}",
            if pr.base_ref_name.is_empty() {
                "(none given)"
            } else {
                &pr.base_ref_name
            }
        ));
    }
    // The PR's last commit, which GitHub keeps under refs/pull/<n>/head
    // whoever's branch it came from: no remote of the fork's is added.
    let head = format!("refs/remotes/{base}/pr/{}", pr.number);
    worktree::count_fetch(root, &base, FETCH);
    if !worktree::git_loud(
        root,
        &[
            "fetch",
            &base,
            &format!("+refs/pull/{}/head:{head}", pr.number),
        ],
        true,
    ) {
        return Err(format!(
            "could not fetch PR #{} from {base}: refs/pull/{}/head (see output above)",
            pr.number, pr.number
        ));
    }
    let off = filters_off(root);
    let quiet = quiet(&off);
    let at = dir.to_string_lossy().into_owned();
    if dir.exists() {
        // Only one this clone's wtpr made and marked: anything else at that
        // path is left as it is.
        let ours = common_dir(&dir).is_some() && common_dir(&dir) == common_dir(root);
        if !ours || !trust::marked(&dir) {
            return Err(format!(
                "{} is there already but isn't a worktree wtpr --untrusted made from {}; move it away and run again",
                dir.display(),
                root.display()
            ));
        }
        if !git_with(&dir, &quiet, &["status", "--porcelain"])?.is_empty() {
            return Err(format!(
                "{} has changes of its own; clean it, then run again",
                dir.display()
            ));
        }
        git_with(&dir, &quiet, &["checkout", "-q", "-B", &branch, &head]).map_err(|e| {
            format!(
                "could not move {} to the PR's newest commit: {e}",
                dir.display()
            )
        })?;
        eprintln!("Worktree already there, moved to the PR's newest commit.");
    } else {
        if let Some(parent) = dir.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("could not make {}: {e}", parent.display()))?;
        }
        let mut add: Vec<&str> = quiet.iter().map(String::as_str).collect();
        add.extend([
            "worktree",
            "add",
            "-q",
            "--no-track",
            "-B",
            &branch,
            &at,
            &head,
        ]);
        if !worktree::git_loud(root, &add, true) {
            return Err("git worktree creation failed (see output above)".into());
        }
        // Its own config, read after the clone's, so every git command in
        // it runs no hook, whoever runs it. A worktree left without it is
        // taken away again rather than kept.
        let marked = worktree::git_ok(root, &["config", "extensions.worktreeConfig", "true"])
            && worktree::git_ok(
                &dir,
                &["config", "--worktree", "core.hooksPath", "/dev/null"],
            )
            && worktree::git_ok(&dir, &["config", "--worktree", trust::MARK, "true"]);
        if !marked {
            let mut remove: Vec<&str> = quiet.iter().map(String::as_str).collect();
            remove.extend(["worktree", "remove", "--force", &at]);
            worktree::git_ok(root, &remove);
            return Err(format!(
                "could not turn git's hooks off in {}, so it was taken away again",
                dir.display()
            ));
        }
    }
    // Set again on every run, for a filter, monitor or submodule recursion
    // your config has come to name since.
    let mut set = vec![
        ("core.fsmonitor".to_string(), "false".to_string()),
        ("submodule.recurse".to_string(), "false".to_string()),
    ];
    set.extend(off);
    for (key, value) in &set {
        if !worktree::git_ok(&dir, &["config", "--worktree", key, value]) {
            return Err(format!(
                "could not set {key} in {}'s own config",
                dir.display()
            ));
        }
    }
    Ok((dir, format!("{base}/{}", pr.base_ref_name)))
}

/// How long an account made at `created` (RFC 3339) has been on GitHub, in
/// words: whole years, or days under one.
fn on_github_for(created: &str, now_ms: i64) -> Option<String> {
    let made = created.parse::<jiff::Timestamp>().ok()?;
    let days = (now_ms - made.as_millisecond()).max(0) / 86_400_000;
    let (n, unit) = if days >= 365 {
        (days / 365, "year")
    } else {
        (days, "day")
    };
    Some(format!("{n} {unit}{}", if n == 1 { "" } else { "s" }))
}

/// What to know about a PR before reading it: who wrote it, how long they
/// have been on GitHub, its size, what in it runs without being asked, and
/// how to go on from your main clone.
fn print_facts(pr: &PrDetails, dir: &Path, base: &str, root: &Path) {
    let login = pr.author.as_deref().unwrap_or("unknown");
    let since = pr
        .author_since
        .as_deref()
        .and_then(|c| on_github_for(c, util::now_ms()))
        .map(|s| format!(" (on GitHub {s})"))
        .unwrap_or_default();
    eprintln!("Author:   {login}{since}");
    eprintln!(
        "Changes:  {} files, +{} −{}",
        pr.changed_files, pr.additions, pr.deletions
    );
    eprintln!(
        "Worktree: {} (untrusted: hooks off, nothing linked, no Claude)",
        dir.display()
    );
    match super::pr_risk::report(dir, base) {
        Ok(report) => {
            let lines = super::pr_risk::file_lines(&report);
            if lines.is_empty() {
                eprintln!("Runs without being asked: nothing it changes");
            } else {
                eprintln!("Runs without being asked:");
                for line in lines {
                    eprintln!("  {line}");
                }
            }
        }
        Err(e) => eprintln!("pr-risk: {e}"),
    }
    let at = shell_quote(&dir.to_string_lossy());
    let switchboard = super::try_::is_switchboard(root);
    eprintln!(
        "\nReview it from your main clone{}, or by hand:",
        if switchboard {
            format!(" with /review-pr {}", pr.number)
        } else {
            String::new()
        }
    );
    eprintln!("  switchboard pr-risk {at} {base}");
    eprintln!(
        "  switchboard codex-review -C {at} --pr {} --base {base}",
        pr.number
    );
    if switchboard {
        eprintln!("  switchboard try --sealed {at}");
    }
}

/// A head branch made flat and PR-numbered rather than mirrored: head
/// branches often contain slashes, which would nest worktrees inside shared
/// directories and confuse anything listing them. Unlike `slugify`, every
/// other character becomes a hyphen rather than going.
fn slug(text: &str) -> String {
    let mut out = String::new();
    for c in text.trim().to_lowercase().chars() {
        let c = if c.is_ascii_lowercase() || c.is_ascii_digit() {
            c
        } else {
            '-'
        };
        if c == '-' && out.ends_with('-') {
            continue;
        }
        out.push(c);
    }
    out.trim_matches('-').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn how_long_an_author_has_been_on_github() {
        let now = "2026-11-07T00:00:00Z"
            .parse::<jiff::Timestamp>()
            .unwrap()
            .as_millisecond();
        assert_eq!(
            on_github_for("2015-01-01T00:00:00Z", now).as_deref(),
            Some("11 years")
        );
        assert_eq!(
            on_github_for("2025-11-01T00:00:00Z", now).as_deref(),
            Some("1 year")
        );
        assert_eq!(
            on_github_for("2026-11-01T00:00:00Z", now).as_deref(),
            Some("6 days")
        );
        assert_eq!(on_github_for("yesterday", now), None);
    }

    #[test]
    fn head_branches_are_made_flat() {
        assert_eq!(slug("feature/Login_Fix"), "feature-login-fix");
        assert_eq!(slug("--x--"), "x");
    }

    #[test]
    fn someone_elses_pr_isnt_pushed_to_unasked() {
        let pr = PrDetails {
            number: 9,
            author: Some("alice".into()),
            state: "MERGED".into(),
            ..PrDetails::default()
        };
        let mine = start_prompt(&pr, Some("Alice"));
        assert!(mine.starts_with("Pick up PR #9 by alice. It is merged, so say that first. "));
        assert!(mine.ends_with("Don't change anything until I say."));
        assert!(
            start_prompt(&pr, None).ends_with("It's alice's PR, so don't push to it unless I ask.")
        );
    }
}
