//! `worktree-resume <linear-ticket-pr-or-branch>`: finds the worktree `wt`,
//! `wtpr` or `wtb` already made for a Linear ticket, a GitHub pull request
//! or a branch, and hands off to `claude-here` there to resume the session
//! that was running. The companion to those three for the morning after a
//! restart. Ported from `bin/worktree-resume`.
//!
//! Nothing is created and nothing is fetched: the worktree is found by
//! matching the identifier against `git worktree list`, so it works offline
//! and needs neither a Linear key nor GitHub. A ticket and a PR both come
//! down to the token `wt` and `wtpr` bake into the names they make, `abc-123`
//! from the Linear branch name and `pr-123` from the worktree's directory;
//! anything else is a branch, as `wtb` takes, matched whole.
//!
//! `init zsh` prints the `wtr` function, which cd's into the worktree and
//! runs `claude-here` itself, as `wt` runs Claude (worktree_from_ticket.rs).
//!
//! A worktree of someone else's PR, checked out untrusted, is refused
//! (src/trust.rs): its CLAUDE.md, settings and hooks are its author's.

use std::path::Path;
use std::process::ExitCode;
use std::time::SystemTime;

use crate::worktree::{self, Given, Worktree};

fn usage() -> ExitCode {
    eprintln!(
        "Usage: worktree-resume <linear-ticket-pr-or-branch>\n       \
         worktree-resume init <zsh|bash>\n\n\
         Examples:\n  \
         worktree-resume ABC-123\n  \
         worktree-resume 123\n  \
         worktree-resume https://github.com/owner/repo/pull/123\n  \
         worktree-resume my-branch\n  \
         worktree-resume https://linear.app/workspace/issue/ABC-123/title"
    );
    ExitCode::from(1)
}

/// The `wtr` function: runs the command with `--no-claude`, cd's into the
/// worktree and resumes its session with `claude-here`, whose session
/// picker needs the terminal, which is why the path comes back through the
/// cd-file rather than stdout.
pub fn shell_function() -> String {
    format!(
        r#"wtr() {{
  local cdfile rc dir
  cdfile=$(mktemp) || return 1
  {} --no-claude --cd-file "$cdfile" "$@"
  rc=$?
  if [ $rc -ne 0 ]; then
    rm -f "$cdfile"
    return $rc
  fi
  dir=$(sed -n 1p "$cdfile")
  rm -f "$cdfile"
  if [ ! -d "$dir" ]; then
    echo "wtr: expected a worktree directory, got: $dir" >&2
    return 1
  fi
  cd "$dir" || return 1
  {}
}}"#,
        worktree::self_command("worktree-resume"),
        worktree::self_command("claude-here")
    )
}

/// What is looked for: a token in a worktree's directory or branch, or a
/// branch by its whole name, and how to say it and make it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub token: Option<String>,
    pub branch: Option<String>,
    pub label: String,
    pub create: String,
}

pub fn parse_target(input: &str) -> Target {
    if let Some(ticket) = worktree::ticket_from(input) {
        let token = ticket.to_lowercase();
        let upper = ticket.to_uppercase();
        return Target {
            token: Some(token),
            branch: None,
            label: upper.clone(),
            create: format!("wt {upper}"),
        };
    }
    if let Some((number, _)) = worktree::pr_from(input) {
        return Target {
            token: Some(format!("pr-{number}")),
            branch: None,
            label: format!("PR #{number}"),
            create: format!("wtpr {number}"),
        };
    }
    Target {
        token: None,
        branch: Some(input.to_string()),
        label: input.to_string(),
        create: format!("wtb {input}"),
    }
}

/// Whether a worktree is the one asked for: the branch whole (never the
/// main clone), or the token in its directory's name or its branch.
pub fn matches(target: &Target, w: &Worktree, root: &Path) -> bool {
    match (&target.branch, &target.token) {
        (Some(branch), _) => w.branch.as_deref() == Some(branch) && w.path != root,
        (None, Some(token)) => {
            let dir = w
                .path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            worktree::mentions(&dir, token)
                || w.branch
                    .as_deref()
                    .is_some_and(|b| worktree::mentions(b, token))
        }
        (None, None) => false,
    }
}

pub fn main(args: Vec<String>) -> ExitCode {
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
            return worktree::fail("init requires a shell: worktree-resume init <zsh|bash>");
        }
        None => {}
    }
    if given.help || given.positional.len() != 1 {
        return usage();
    }
    let target = parse_target(&given.positional[0]);
    let root = match worktree::repo_root() {
        Ok(root) => root,
        Err(e) => return worktree::fail(&e.to_string()),
    };
    let matched: Vec<Worktree> = match worktree::worktrees(&root) {
        Ok(all) => all
            .into_iter()
            .filter(|w| matches(&target, w, &root))
            .collect(),
        Err(e) => return worktree::fail(&e.to_string()),
    };
    // A worktree git still tracks but whose directory is gone is not
    // somewhere to send anyone, and is what rerunning `wt` or `wtpr`
    // recovers from, so that is said rather than nothing found. The most
    // recently touched first.
    let mut present: Vec<(SystemTime, Worktree)> = matched
        .iter()
        .filter_map(|w| Some((std::fs::metadata(&w.path).ok()?.modified().ok()?, w.clone())))
        .collect();
    present.sort_by_key(|p| std::cmp::Reverse(p.0));
    if present.is_empty() {
        return worktree::fail(&format!(
            "no worktree found for {}{} under:\n  {}\nCreate it with:\n  {}",
            target.label,
            if matched.is_empty() {
                ""
            } else {
                ", only a tracked one whose directory is gone"
            },
            root.display(),
            target.create
        ));
    }
    let chosen = if present.len() == 1 {
        present.remove(0).1
    } else {
        eprintln!("{} worktrees match — pick one:", present.len());
        for (i, (_, w)) in present.iter().enumerate() {
            let rel = w
                .path
                .strip_prefix(&root)
                .map(|r| r.display().to_string())
                .unwrap_or_else(|_| w.path.display().to_string());
            let branch = w
                .branch
                .as_deref()
                .map(|b| format!("  ({b})"))
                .unwrap_or_default();
            eprintln!("  {}. {rel}{branch}", i + 1);
        }
        let at = worktree::choose_numbered("Select worktree [1]: ", present.len());
        present.remove(at).1
    };
    // Never a worktree of someone else's PR (src/trust.rs).
    if crate::trust::is_untrusted(&chosen.path) {
        return worktree::fail(&crate::trust::refusal(&chosen.path));
    }
    eprintln!("Target:   {}", target.label);
    if let Some(branch) = &chosen.branch {
        eprintln!("Branch:   {branch}");
    }
    eprintln!("Worktree: {}", chosen.path.display());
    if let Some(cd_file) = &given.cd_file
        && let Err(e) = worktree::write_cd_file(cd_file, &format!("{}\n", chosen.path.display()))
    {
        return worktree::fail(&e.to_string());
    }
    if given.no_claude {
        return ExitCode::SUCCESS;
    }
    // claude-here is this program too, run in the worktree so it finds that
    // worktree's sessions.
    let exe = std::env::current_exe()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "switchboard".into());
    worktree::run_in(&exe, &["claude-here".to_string()], &chosen.path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn tree(path: &str, branch: &str) -> Worktree {
        Worktree {
            path: PathBuf::from(path),
            branch: Some(branch.into()),
            ..Worktree::default()
        }
    }

    #[test]
    fn tickets_prs_and_branches_find_their_worktrees() {
        let root = Path::new("/r");
        let ticket = parse_target("https://linear.app/w/issue/ABC-12/x");
        assert_eq!(ticket.label, "ABC-12");
        assert!(matches(
            &ticket,
            &tree("/r/.claude/worktrees/abc-12-fix", "abc-12-fix"),
            root
        ));
        assert!(!matches(
            &ticket,
            &tree("/r/.claude/worktrees/abc-123", "abc-123"),
            root
        ));
        let pr = parse_target("#7");
        assert!(matches(
            &pr,
            &tree("/r/.claude/worktrees/pr-7-feat", "feat"),
            root
        ));
        let branch = parse_target("main");
        assert!(
            !matches(&branch, &tree("/r", "main"), root),
            "never the main clone"
        );
        assert!(matches(&branch, &tree("/r/x", "main"), root));
    }
}
