//! What the commands that open a worktree for a piece of work share
//! (`worktree-from-ticket`, `worktree-from-note`): the branch's slug, the main
//! clone, the local settings linked in, and the worktree made or reused,
//! refused while Claude already works in it. Errors are returned, for each
//! command's own `fail` to print.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Result, anyhow, bail};
use serde_json::Value;

use crate::{paths, util};

/// A branch-name slug: lowercase letters, digits and single hyphens.
pub fn slugify(text: &str) -> String {
    let lower = text.trim().to_lowercase();
    let kept: String = lower
        .chars()
        .filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c.is_whitespace() || *c == '-')
        .collect();
    let mut out = String::new();
    let mut last_hyphen = false;
    for c in kept.chars() {
        let c = if c.is_whitespace() { '-' } else { c };
        if c == '-' {
            if !last_hyphen {
                out.push('-');
            }
            last_hyphen = true;
        } else {
            out.push(c);
            last_hyphen = false;
        }
    }
    out.trim_matches('-').to_string()
}

/// git's output, trimmed; an error when git fails.
pub fn git_output(args: &[&str]) -> Result<String> {
    Ok(util::run("git", args)?.trim().to_string())
}

/// Runs git with its output thrown away; an error when it fails.
pub fn git_run(args: &[&str]) -> Result<()> {
    let status = Command::new("git")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(anyhow!("git {} failed", args.first().unwrap_or(&"")))
    }
}

/// The main clone: `--git-common-dir` rather than `--show-toplevel`, since
/// inside an existing worktree the latter is that worktree, and new worktrees
/// belong beside the main repository.
pub fn repo_root() -> Result<PathBuf> {
    let common = git_output(&["rev-parse", "--git-common-dir"])
        .map_err(|_| anyhow!("not inside a git repository"))?;
    let common = std::env::current_dir()?.join(common);
    let resolved = common.parent().map(Path::to_path_buf).unwrap_or(common);
    Ok(normalize(&resolved))
}

/// A path with `.` and `..` resolved without touching the disk, as Node's
/// `path.resolve` does.
pub fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

/// Claude Code reads `.claude/settings.local.json` from the worktree root,
/// not the main clone, and git never copies it because it is gitignored.
/// Symlinking the main clone's file keeps one source of truth for personal
/// permissions and the auto mode environment profile. Skipped when the
/// worktree already has its own.
pub fn link_local_settings(root: &Path, worktree: &Path) -> Result<()> {
    let source = root.join(".claude/settings.local.json");
    let target = worktree.join(".claude/settings.local.json");
    if !source.exists() || target.exists() {
        return Ok(());
    }
    if let Some(dir) = target.parent() {
        fs::create_dir_all(dir)?;
    }
    std::os::unix::fs::symlink(&source, &target)?;
    Ok(())
}

/// The title the session picker shows: a name given with /rename, else the
/// title Claude Code generated from the first prompt. Both live in files
/// Claude Code does not document, so anything unreadable leaves it blank.
pub fn session_title(session: &Value) -> String {
    let name = session.get("name").and_then(Value::as_str);
    let source = session.get("nameSource").and_then(Value::as_str);
    if let (Some(name), Some(source)) = (name, source)
        && !name.is_empty()
        && !source.is_empty()
        && source != "derived"
    {
        return name.to_string();
    }
    let id = session
        .get("sessionId")
        .and_then(Value::as_str)
        .unwrap_or("");
    let projects = paths::claude_dir().join("projects");
    let transcript = fs::read_dir(&projects).ok().and_then(|dirs| {
        dirs.flatten()
            .map(|d| d.path().join(format!("{id}.jsonl")))
            .find(|f| f.exists())
    });
    let Some(transcript) = transcript else {
        return name.unwrap_or("").to_string();
    };
    let text = fs::read_to_string(transcript).unwrap_or_default();
    text.split('\n')
        .filter(|line| line.contains(r#""type":"ai-title""#))
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter_map(|v| {
            v.get("aiTitle")
                .and_then(Value::as_str)
                .filter(|t| !t.is_empty())
                .map(str::to_string)
        })
        .next_back()
        .unwrap_or_else(|| name.unwrap_or("").to_string())
}

/// The running Claude sessions working in a directory or below it. Claude
/// Code writes `~/.claude/sessions/<pid>.json` for every running session,
/// naming its working directory; a file whose process is gone is left over
/// from a session that ended without cleaning up, and is ignored.
pub fn live_sessions_in(worktree: &Path) -> Vec<Value> {
    let dir = paths::claude_dir().join("sessions");
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let root = worktree.to_string_lossy().into_owned();
    entries
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().ends_with(".json"))
        .filter_map(|e| util::read_json(e.path()))
        .filter(|s| {
            let has_id = s.get("sessionId").is_some_and(util::truthy);
            let cwd = s.get("cwd").and_then(Value::as_str).unwrap_or("");
            let inside = !cwd.is_empty() && (cwd == root || cwd.starts_with(&format!("{root}/")));
            let alive = s
                .get("pid")
                .and_then(Value::as_i64)
                .is_some_and(util::is_alive);
            has_id && inside && alive
        })
        .collect()
}

/// What a worktree is for, in the refusal's words: `work` ("ticket", "item")
/// and `command` ("wt", "wtn"); `start` is the commit a new branch is made
/// from, with no upstream, else the current HEAD.
pub struct Purpose<'a> {
    pub work: &'a str,
    pub command: &'a str,
    pub start: Option<&'a str>,
}

/// Makes the worktree, or reuses it. Idempotent so rerunning on the same work
/// reopens the worktree instead of erroring: the directory is reused if it is
/// there, and the branch if it is there.
pub fn ensure_worktree(worktree: &Path, branch: &str, purpose: &Purpose) -> Result<()> {
    if worktree.exists() {
        // Two sessions on one piece of work edit the same files and push the
        // same branch, so a worktree that already has Claude running in it is
        // left to that one.
        let sessions = live_sessions_in(worktree);
        if !sessions.is_empty() {
            let lines: Vec<String> = sessions
                .iter()
                .map(|s| {
                    let title = session_title(s);
                    let id = s.get("sessionId").and_then(Value::as_str).unwrap_or("");
                    format!(
                        "  Session: {id}\n  Title:   {}",
                        if title.is_empty() {
                            "(none yet)"
                        } else {
                            &title
                        }
                    )
                })
                .collect();
            bail!(
                "Claude is already working on this {}, so {} will not start another session.\n{}\nSwitch to that session, or end it and run {} again.",
                purpose.work,
                purpose.command,
                lines.join("\n\n"),
                purpose.command
            );
        }
        // A directory of the same name on another branch is another piece of
        // work, never taken for this one.
        let path = worktree.to_string_lossy();
        let checked_out =
            git_output(&["-C", &path, "branch", "--show-current"]).unwrap_or_default();
        if checked_out != branch {
            let on = if checked_out.is_empty() {
                "no branch of this repository".to_string()
            } else {
                format!("branch {checked_out}")
            };
            bail!(
                "{} is there already, on {on}, not {branch}.",
                worktree.display()
            );
        }
        eprintln!("Worktree already exists, reusing it.");
        return Ok(());
    }

    let branch_exists = git_run(&[
        "show-ref",
        "--verify",
        "--quiet",
        &format!("refs/heads/{branch}"),
    ])
    .is_ok();
    let path = worktree.to_string_lossy().into_owned();
    let args: Vec<&str> = if branch_exists {
        vec!["worktree", "add", &path, branch]
    } else if let Some(start) = purpose.start {
        vec!["worktree", "add", "--no-track", "-b", branch, &path, start]
    } else {
        vec!["worktree", "add", "-b", branch, &path]
    };
    // git's stdout to stderr, so it can't pollute anything a caller parses.
    let status = Command::new("git")
        .args(&args)
        .stdin(Stdio::inherit())
        .stdout(std::io::stderr())
        .stderr(Stdio::inherit())
        .status();
    if !status.is_ok_and(|s| s.success()) {
        bail!("git worktree creation failed (see output above)");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs_are_lowercase_words_joined_by_hyphens() {
        assert_eq!(slugify("  Fix the Thing: now!  "), "fix-the-thing-now");
        assert_eq!(slugify("a -- b"), "a-b");
        assert_eq!(slugify("-Café-"), "caf");
    }
}
