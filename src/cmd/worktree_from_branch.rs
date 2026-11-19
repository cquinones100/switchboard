//! `worktree-from-branch [path] <branch>`: a git worktree on any branch,
//! with Claude opened in it: the branch as it is when it exists here or on
//! the remote, else a new one off the remote's default branch, fetched
//! first. Plain git: no Linear key, no GitHub. Ported from
//! `bin/worktree-from-branch`.
//!
//! `<path>` defaults to `.claude/worktrees`, resolved against the main
//! clone, and the worktree is named after the branch made flat
//! (`feature/login` → `feature-login`), so a branch with a slash doesn't
//! nest its worktree inside a shared directory. `init zsh` prints the `wtb`
//! function, which works as `wt` does (worktree_from_ticket.rs). A branch
//! already checked out in a worktree of someone else's PR, checked out
//! untrusted, is refused (src/trust.rs), and so is Claude on a branch with
//! commits beyond the default branch that someone else wrote
//! (`trust::branch_yours_or_listed`), unless `--trusted` opens it as your
//! own: the worktree is made, but nothing is linked into it, no shell moved
//! into it and no Claude started.
//!
//! `--prompt <text>` is what Claude starts with, as `wt`'s is added to its
//! ticket's: work with no ticket or note behind it, which M-p's "Start work
//! on a task" types in. With no branch given, the branch is named after it
//! (`task_branch`), a new one each time, so a task never lands on work that
//! is already there.

use std::path::Path;
use std::process::ExitCode;

use crate::worktree::{self, Given};

const SOURCE: &str = "wtb: fetch the branch";

fn usage() -> ExitCode {
    eprintln!(
        "Usage: worktree-from-branch [--trusted] [path] <branch> [--prompt <text>]\n       \
         worktree-from-branch [--trusted] --prompt <text>\n       \
         worktree-from-branch init <zsh|bash>\n\n\
         Examples:\n  \
         worktree-from-branch fix-login-redirect\n  \
         worktree-from-branch .worktrees fix-login-redirect\n  \
         worktree-from-branch --trusted their-branch   a branch with someone else's commits, as your own\n  \
         worktree-from-branch --prompt \"fix the login redirect\"   a new branch named after the task\n\n\
         Path defaults to {} under the repo root. --prompt is what Claude starts with;\n\
         with no branch, a new branch is named after it.",
        worktree::DEFAULT_WORKTREE_DIR
    );
    ExitCode::from(1)
}

/// The `wtb` function: runs the command with `--no-claude`, cd's into the
/// worktree and starts Claude there, as `wt` and `wtpr` do, with the
/// cd-file's second line, `--prompt`'s text, as its prompt when there is one.
pub fn shell_function() -> String {
    format!(
        r#"wtb() {{
  local cdfile rc dir prompt
  cdfile=$(mktemp) || return 1
  {} --no-claude --cd-file "$cdfile" "$@"
  rc=$?
  if [ $rc -ne 0 ]; then
    rm -f "$cdfile"
    return $rc
  fi
  dir=$(sed -n 1p "$cdfile")
  prompt=$(sed -n 2p "$cdfile")
  rm -f "$cdfile"
  if [ ! -d "$dir" ]; then
    echo "wtb: expected a worktree directory, got: $dir" >&2
    return 1
  fi
  cd "$dir" || return 1
  if [ -n "$prompt" ]; then
    claude -- "$prompt"
  else
    claude
  fi
}}"#,
        worktree::self_command("worktree-from-branch")
    )
}

/// The remote's default branch, from `refs/remotes/<remote>/HEAD`, else
/// main or master, whichever the remote has.
fn default_branch(root: &Path, remote: &str) -> Option<String> {
    if let Ok(head) = worktree::git_at(
        root,
        &[
            "symbolic-ref",
            "--short",
            &format!("refs/remotes/{remote}/HEAD"),
        ],
    ) {
        return Some(head[remote.len() + 1..].to_string());
    }
    ["main", "master"]
        .into_iter()
        .find(|b| worktree::fetch_branch(root, remote, b, SOURCE))
        .map(str::to_string)
}

fn add_worktree(root: &Path, args: &[&str]) -> Result<(), String> {
    let mut all = vec!["worktree", "add"];
    all.extend_from_slice(args);
    if worktree::git_loud(root, &all, true) {
        Ok(())
    } else {
        Err("git worktree creation failed (see output above)".into())
    }
}

fn create_worktree(path: &Path, branch: &str, root: &Path) -> Result<(), String> {
    let remote = worktree::default_remote(root);
    let on_remote = remote
        .as_deref()
        .is_some_and(|r| worktree::fetch_branch(root, r, branch, SOURCE));
    let at = path.to_string_lossy().into_owned();

    // A local branch keeps its own commits, caught up with the remote's when
    // it only falls behind; --ff-only so unpushed work is reported, never
    // rewritten.
    if worktree::branch_exists(root, branch) {
        add_worktree(root, &[&at, branch])?;
        if let (true, Some(remote)) = (on_remote, &remote)
            && !worktree::git_ok(path, &["merge", "--ff-only", &format!("{remote}/{branch}")])
        {
            eprintln!("Note: {branch} has diverged from {remote}/{branch} and was left as-is.");
        }
        return Ok(());
    }
    if let (true, Some(remote)) = (on_remote, &remote) {
        add_worktree(root, &["-b", branch, &at, &format!("{remote}/{branch}")])?;
        worktree::set_upstream(root, branch, remote, branch);
        return Ok(());
    }
    // A new branch, off the remote's default branch as it is now, with no
    // upstream until its first push.
    let base = remote
        .as_deref()
        .and_then(|r| default_branch(root, r).map(|b| (r.to_string(), b)));
    let start = match &base {
        Some((remote, base)) => {
            worktree::fetch_branch(root, remote, base, SOURCE);
            format!("{remote}/{base}")
        }
        None => "HEAD".to_string(),
    };
    eprintln!("New branch, off {start}.");
    add_worktree(root, &["--no-track", "-b", branch, &at, &start])
}

/// The branch a task with none given is worked on: its words made a slug,
/// as a note's item's is, with `-2`, `-3` and on after it when a branch or
/// worktree by that name is already here or on the remote (asked, not read
/// from tracking refs that may be stale, unless the remote can't be
/// reached), so it is always a new one.
fn task_branch(root: &Path, task: &str) -> Result<String, String> {
    let slug = crate::cmd::worktree_from_note::slug_of(task);
    if slug.is_empty() {
        return Err(format!("no branch name in the task: {task}"));
    }
    let remote = worktree::default_remote(root);
    let taken = |name: &str| {
        worktree::branch_exists(root, name)
            || remote.as_deref().is_some_and(|r| {
                match worktree::fetch_branch_answer(root, r, name, SOURCE) {
                    worktree::Fetched::Yes => true,
                    worktree::Fetched::NoSuchBranch => false,
                    worktree::Fetched::Failed => worktree::git_ok(
                        root,
                        &[
                            "show-ref",
                            "--verify",
                            "--quiet",
                            &format!("refs/remotes/{r}/{name}"),
                        ],
                    ),
                }
            })
            || root
                .join(worktree::DEFAULT_WORKTREE_DIR)
                .join(name)
                .exists()
    };
    Ok((1..)
        .map(|n| match n {
            1 => slug.clone(),
            n => format!("{slug}-{n}"),
        })
        .find(|name| !taken(name))
        .expect("an unbounded range finds a free name"))
}

pub fn main(args: Vec<String>) -> ExitCode {
    let given = match Given::parse(&args, &["--prompt"]) {
        Ok(g) => g,
        Err(flag) if flag == "--prompt" => return worktree::fail("--prompt requires text"),
        Err(_) => return worktree::fail("--cd-file requires a path"),
    };
    match worktree::init_shell(&given.positional) {
        Some(true) => {
            println!("{}", shell_function());
            return ExitCode::SUCCESS;
        }
        Some(false) => {
            return worktree::fail("init requires a shell: worktree-from-branch init <zsh|bash>");
        }
        None => {}
    }
    if given.help {
        return usage();
    }
    let trusted = given.positional.iter().any(|a| a == "--trusted");
    let positional: Vec<&String> = given
        .positional
        .iter()
        .filter(|a| *a != "--trusted")
        .collect();
    let prompt = worktree::one_line(&given.all("--prompt").join(" "));
    let (target, branch) = match positional.as_slice() {
        [branch] => (worktree::DEFAULT_WORKTREE_DIR, branch.to_string()),
        [path, branch, ..] => (path.as_str(), branch.to_string()),
        [] if !prompt.is_empty() => {
            let named = worktree::repo_root()
                .map_err(|e| e.to_string())
                .and_then(|root| task_branch(&root, &prompt));
            match named {
                Ok(branch) => (worktree::DEFAULT_WORKTREE_DIR, branch),
                Err(e) => return worktree::fail(&e),
            }
        }
        [] => return usage(),
    };
    match run(&given, trusted, target, &branch, &prompt) {
        Ok(code) => code,
        Err(e) => worktree::fail(&e),
    }
}

fn run(
    given: &Given,
    trusted: bool,
    target: &str,
    branch: &str,
    prompt: &str,
) -> Result<ExitCode, String> {
    let here = std::env::current_dir().map_err(|e| e.to_string())?;
    if !worktree::git_ok(&here, &["check-ref-format", "--branch", branch]) {
        return Err(format!("not a valid branch name: {branch}"));
    }
    let root = worktree::repo_root().map_err(|e| e.to_string())?;
    let path = worktree::normalize(&root.join(target).join(branch.replace('/', "-")));

    eprintln!("Branch:   {branch}");
    // Reused rather than refused, so a rerun reopens it. The main clone is
    // not somewhere to send anyone: git allows a branch in one worktree at a
    // time.
    let existing = if path.exists() {
        eprintln!("Worktree already exists, reusing it.");
        Some(path.clone())
    } else {
        worktree::holding_branch(&root, branch)
    };
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
    // Never a worktree of someone else's PR (src/trust.rs).
    if existing.as_deref().is_some_and(crate::trust::is_untrusted) {
        return Err(crate::trust::refusal(&finished));
    }
    eprintln!("Worktree: {}", finished.display());
    if existing.is_none() {
        create_worktree(&path, branch, &root)?;
    }
    if !trusted {
        // Against the default branch as it is now, so teammates' work merged
        // since the last fetch isn't taken for this branch's; offline, as it
        // was last fetched.
        if let Some(remote) = worktree::default_remote(&root)
            && let Some(base) = default_branch(&root, &remote)
        {
            worktree::fetch_branch(&root, &remote, &base, SOURCE);
        }
        crate::trust::branch_yours_or_listed(&crate::paths::home(), &finished)?;
    }
    crate::cmd::clone_build::start(&finished);
    worktree::link_local_settings(&root, &finished).map_err(|e| e.to_string())?;
    if let Some(cd_file) = &given.cd_file {
        worktree::write_cd_file(cd_file, &format!("{}\n{prompt}\n", finished.display()))
            .map_err(|e| e.to_string())?;
    }
    if given.no_claude {
        return Ok(ExitCode::SUCCESS);
    }
    // After `--`, so a task typed with a leading dash is the prompt, never
    // one of Claude's flags.
    let args: Vec<String> = if prompt.is_empty() {
        Vec::new()
    } else {
        vec!["--".to_string(), prompt.to_string()]
    };
    Ok(worktree::run_in("claude", &args, &finished))
}
