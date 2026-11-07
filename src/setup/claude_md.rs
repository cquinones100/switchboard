//! The `claude-md` part: your `~/.claude/CLAUDE.md`, your own rules and
//! Switchboard's part (`assets/switchboard.md`: the tabs and talking to
//! them, the notes app, repository skills). Ported from setup.sh's
//! `claude_md`, `claude_md_now`, `claude_md_pull` and
//! `claude_md_gh_helper`.
//!
//! Switchboard's part is written to `~/.local/share/switchboard/
//! switchboard.md` from the copy compiled in, where setup.sh imported it
//! from the clone. Your rules come from where `~/.config/switchboard/
//! claude.json` says: a git repository (`{"repo": "<url>", "file":
//! "CLAUDE.md"}`, cloned into `~/.local/share/switchboard/claude-md` and
//! pulled on every run, fast-forward only, with gh's sign-in among its
//! credential helpers on GitHub; without "file", its CLAUDE.md at the root
//! or the one nested least deep), or a file (`{"path": "~/somewhere/
//! CLAUDE.md"}`), else `~/.config/switchboard/CLAUDE.md` when there is one.
//! Then `~/.claude/CLAUDE.md` is a short file importing both, which Claude
//! Code follows (`@path`), so an edit to either shows at once; a real file
//! in its way that isn't one written here is moved aside first. With no
//! rules named, a CLAUDE.md of your own gets only the line importing
//! Switchboard's part (the clone's old import line pointed at it instead),
//! and a missing one is made with it. A link into a clone's old
//! `claude/CLAUDE.md` becomes `~/.config/switchboard/CLAUDE.md`, from what
//! it linked to, else from git once the file is gone.
//!
//! A pull (the update check's) and a change in Settings can overlap. The
//! pull, which can take as long as the network does, runs first and holds
//! nothing, and with `SWITCHBOARD_PULL_ONLY` (the update check's run)
//! nothing clones; then the rest runs one run at a time, under a lock the
//! system lets go of when the run ends however it ends (`flock`), reading
//! the setting afresh, so whichever writes last writes what the setting says
//! by then. A check takes no lock.

use std::fs::{self, OpenOptions};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use serde_json::Value;

use super::{Report, exists, home_tilde, is_link, stamp};
use crate::{paths, util, worktree};

/// Switchboard's part, as compiled in.
pub const PART: &str = include_str!("../../assets/switchboard.md");

/// The line importing Switchboard's part.
pub const IMPORT: &str = "@~/.local/share/switchboard/switchboard.md";

/// The line setup.sh wrote, importing it from the clone.
pub const OLD_IMPORT: &str = "@~/switchboard/claude/switchboard.md";

/// The first line of a CLAUDE.md written here. setup.sh's began the same
/// way (`Written by Switchboard's setup.sh`), so either is known as one of
/// ours, never moved aside.
pub const MARK: &str = "<!-- Written by Switchboard's setup: your rules, then Switchboard's. Edit them where they live, not here. -->";
const MARK_START: &str = "<!-- Written by Switchboard's setup";

/// Where Switchboard's part is written.
pub fn part_file() -> PathBuf {
    paths::home_join(".local/share/switchboard/switchboard.md")
}

fn claude_md() -> PathBuf {
    paths::claude_dir().join("CLAUDE.md")
}

fn config_file() -> PathBuf {
    paths::config_dir().join("claude.json")
}

fn own_rules() -> PathBuf {
    paths::config_dir().join("CLAUDE.md")
}

/// The clone of your rules' repository.
pub fn clone_dir() -> PathBuf {
    paths::home_join(".local/share/switchboard/claude-md")
}

/// A field of `claude.json`, empty when it isn't there.
fn config(key: &str) -> String {
    util::read_json(config_file())
        .and_then(|v| v.get(key).and_then(Value::as_str).map(str::to_string))
        .unwrap_or_default()
}

fn pull_only() -> bool {
    std::env::var("SWITCHBOARD_PULL_ONLY").is_ok_and(|v| v == "true" || v == "1")
}

/// Whether your CLAUDE.md already imports Switchboard's part, or was written
/// here: set up before, so the guided setup brings it up to date without
/// asking.
pub fn set_up() -> bool {
    util::read_text(claude_md()).is_some_and(|text| {
        text.contains(MARK_START) || text.lines().any(|l| l == IMPORT || l == OLD_IMPORT)
    })
}

/// The part.
pub fn run(r: &mut Report) -> Result<()> {
    if r.check {
        return now(r, false);
    }
    pull();
    let lock_file = paths::state_dir().join("claude-md.lock");
    let _held = lock(&lock_file, Duration::from_secs(120))?;
    now(r, true)
}

/// Holds `file` locked until the result is dropped (the run's end, however
/// it ends), waiting up to `wait` for another run to let go of it.
fn lock(file: &Path, wait: Duration) -> Result<fs::File> {
    if let Some(dir) = file.parent() {
        fs::create_dir_all(dir)?;
    }
    let handle = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(file)
        .with_context(|| format!("can't open {}", home_tilde(file)))?;
    let started = Instant::now();
    loop {
        // SAFETY: flock on a descriptor this function owns.
        let r = unsafe { libc::flock(handle.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if r == 0 {
            return Ok(handle);
        }
        if started.elapsed() >= wait {
            bail!(
                "another run of setup has held {} for {}s",
                home_tilde(file),
                wait.as_secs()
            );
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// gh's sign-in as a git credential helper, for a repository on GitHub (a
/// gist too), so cloning a private one, updating it and pushing from the
/// clone work without a password prompt; `None` when gh holds no sign-in.
/// Given to git after the helpers already set, which it asks first, so a
/// sign-in that works keeps working. gh's path is quoted for the shell git
/// runs the helper in.
fn gh_helper(repo: &str) -> Option<String> {
    // A user in the URL (https://me@github.com/...) is still GitHub.
    let github = regex::Regex::new(r"^https://([^/@]+@)?(gist\.)?github\.com/").ok()?;
    if !github.is_match(repo) {
        return None;
    }
    let gh = util::which("gh")?;
    let signed_in = Command::new(&gh)
        .args(["auth", "token", "--hostname", "github.com"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    signed_in.then(|| {
        format!(
            "!'{}' auth git-credential",
            gh.to_string_lossy().replace('\'', r"'\''")
        )
    })
}

fn git_with_helper(helper: Option<&str>) -> Command {
    let mut cmd = Command::new("git");
    if let Some(h) = helper {
        cmd.arg("-c").arg(format!("credential.helper={h}"));
    }
    cmd
}

fn origin_of(clone: &Path) -> String {
    worktree::git_at(clone, &["remote", "get-url", "origin"]).unwrap_or_default()
}

fn pull_clone(clone: &Path, helper: Option<&str>) {
    worktree::count_fetch(clone, "origin", "your CLAUDE.md's repository");
    let pulled = git_with_helper(helper)
        .arg("-C")
        .arg(clone)
        .args(["pull", "--quiet", "--ff-only"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    if !pulled {
        eprintln!(
            "  could not update your CLAUDE.md's repository: git -C {} pull --ff-only",
            clone.display()
        );
    }
}

/// Brings the clone of your rules' repository up to date, when claude.json
/// names the one it is a clone of. Outside the lock, since it waits on the
/// network.
pub fn pull() {
    let repo = config("repo");
    let clone = clone_dir();
    if !repo.is_empty() && clone.join(".git").is_dir() && origin_of(&clone) == repo {
        pull_clone(&clone, gh_helper(&repo).as_deref());
    }
}

/// The CLAUDE.md files in the clone nested least deep: one is yours; none
/// or several is said.
fn least_deep(clone: &Path, repo: &str) -> Result<PathBuf> {
    let listed = util::run_command(Command::new("git").arg("-C").arg(clone).args([
        "ls-files",
        "-z",
        "--",
        "CLAUDE.md",
        "*/CLAUDE.md",
    ]))?;
    let mut least: Option<usize> = None;
    let mut candidates: Vec<&str> = Vec::new();
    for found in listed.split('\0').filter(|f| !f.is_empty()) {
        // Only a file named CLAUDE.md: the pathspec also matches what is in
        // a folder of that name, and one can be a link to a folder.
        if !(found == "CLAUDE.md" || found.ends_with("/CLAUDE.md")) || !clone.join(found).is_file()
        {
            continue;
        }
        let depth = found.matches('/').count();
        match least {
            Some(l) if depth > l => {}
            Some(l) if depth == l => candidates.push(found),
            _ => {
                least = Some(depth);
                candidates = vec![found];
            }
        }
    }
    match candidates.as_slice() {
        [] => bail!(
            "no CLAUDE.md in your rules' repository ({repo}): add one, or name the file with \"file\" in ~/.config/switchboard/claude.json"
        ),
        [one] => Ok(clone.join(one)),
        several => bail!(
            "your rules' repository has more than one CLAUDE.md ({}): name yours with \"file\" in ~/.config/switchboard/claude.json",
            several.join(", ")
        ),
    }
}

/// Your rules as a clone's old `claude/CLAUDE.md` held them: what the link
/// leads to, else the file from git, the commit before the one that removed
/// it; the line importing Switchboard's part left out.
fn rules_from_link(target: &Path) -> Option<String> {
    let text = fs::read_to_string(target).ok().or_else(|| {
        let repo = target.parent()?.parent()?;
        let removed = worktree::git_at(
            repo,
            &[
                "log",
                "-1",
                "--format=%H",
                "--diff-filter=D",
                "--",
                "claude/CLAUDE.md",
            ],
        )
        .ok()
        .filter(|h| !h.is_empty())?;
        let mut cmd = Command::new("git");
        cmd.arg("-C")
            .arg(repo)
            .args(["show", &format!("{removed}^:claude/CLAUDE.md")]);
        util::run_command(&mut cmd).ok()
    })?;
    let kept: String = text
        .lines()
        .filter(|l| *l != OLD_IMPORT && *l != IMPORT)
        .map(|l| format!("{l}\n"))
        .collect();
    (!kept.trim().is_empty()).then_some(kept)
}

/// Switchboard's part, as this binary carries it, written where your
/// CLAUDE.md imports it from. Alone, it is what a new build brings to a
/// CLAUDE.md already set up (`super::refresh`), with no pull and no lock,
/// since it touches only that one file, written whole.
pub fn write_part(r: &mut Report) -> Result<()> {
    let part = part_file();
    if util::read_text(&part).as_deref() != Some(PART) {
        if !r.check {
            util::write_atomic(&part, PART)?;
        }
        r.change(format!("write Switchboard's part to {}", home_tilde(&part)));
    }
    Ok(())
}

/// The locked part of the run (all of it with `--check`).
fn now(r: &mut Report, pulled: bool) -> Result<()> {
    let file = claude_md();
    let clone = clone_dir();

    write_part(r)?;

    if is_link(&file)
        && let Ok(target) = fs::read_link(&file)
        && target.to_string_lossy().ends_with("/claude/CLAUDE.md")
        && !exists(&own_rules())
    {
        if !r.check {
            // A relative link leads from the folder it is in.
            let target = match file.parent() {
                Some(dir) => dir.join(&target),
                None => target,
            };
            let rules = rules_from_link(&target)
                .ok_or_else(|| anyhow!("could not copy your rules from {}", target.display()))?;
            util::write_atomic(own_rules(), rules)?;
        }
        r.change("copy your rules from the repo's claude/CLAUDE.md, which your ~/.claude/CLAUDE.md linked to, to ~/.config/switchboard/CLAUDE.md");
    }

    let repo = config("repo");
    let mut backup: Option<PathBuf> = None;
    // With --check, before the clone: it isn't there to look in yet.
    let mut not_cloned = false;
    let mine: Option<PathBuf> = if !repo.is_empty() {
        let helper = gh_helper(&repo);
        // A clone of another repository, from before the setting changed,
        // is moved aside rather than pulled.
        if clone.join(".git").is_dir() && origin_of(&clone) != repo {
            if pull_only() {
                return Ok(());
            }
            let aside = clone.with_file_name(format!("claude-md.bak.{}", stamp()));
            if !r.check {
                fs::rename(&clone, &aside)?;
            }
            r.change(format!(
                "move the clone of your CLAUDE.md's old repository aside to {}",
                home_tilde(&aside)
            ));
            backup = Some(aside);
        }
        not_cloned = !clone.join(".git").is_dir() || (r.check && backup.is_some());
        if not_cloned {
            // A clone, which waits on the network, is left to setup and
            // Settings.
            if pull_only() {
                return Ok(());
            }
            if !r.check {
                if let Some(dir) = clone.parent() {
                    fs::create_dir_all(dir)?;
                }
                let cloned = git_with_helper(helper.as_deref())
                    .args(["clone", "--quiet", &repo])
                    .arg(&clone)
                    .stdin(std::process::Stdio::null())
                    .status()
                    .is_ok_and(|s| s.success());
                if !cloned {
                    bail!("could not clone {repo} for your CLAUDE.md");
                }
                not_cloned = false;
            }
            r.change("clone your CLAUDE.md's repository");
        } else if !r.check && !pulled {
            pull_clone(&clone, helper.as_deref());
        }
        // The setting read again after the clone, which can take a while:
        // changed meanwhile (Settings' Your CLAUDE.md), this run does the
        // new one's work instead, so what it writes is what the setting
        // says.
        if config("repo") != repo {
            return now(r, pulled);
        }
        // gh's helper kept in the clone.
        if let Some(h) = &helper
            && clone.join(".git").is_dir()
            && !(r.check && backup.is_some())
        {
            let helpers = worktree::git_at(
                &clone,
                &["config", "--local", "--get-all", "credential.helper"],
            )
            .unwrap_or_default();
            if !helpers.lines().any(|l| l == h) {
                if !r.check
                    && !worktree::git_ok(
                        &clone,
                        &["config", "--local", "--add", "credential.helper", h],
                    )
                {
                    bail!(
                        "could not add gh as a credential helper in {}",
                        home_tilde(&clone)
                    );
                }
                r.change("add gh as a credential helper for your CLAUDE.md's repository");
            }
        }
        let named = config("file");
        Some(if !named.is_empty() {
            clone.join(named)
        } else if not_cloned {
            // What it will most likely hold.
            clone.join("CLAUDE.md")
        } else {
            least_deep(&clone, &repo)?
        })
    } else {
        let path = config("path");
        if !path.is_empty() {
            Some(match path.strip_prefix('~') {
                Some(rest) if rest.is_empty() || rest.starts_with('/') => {
                    paths::home().join(rest.trim_start_matches('/'))
                }
                _ => PathBuf::from(path),
            })
        } else if own_rules().is_file() {
            Some(own_rules())
        } else {
            None
        }
    };

    // A file named that isn't there would be imported as nothing, quietly.
    if let Some(m) = &mine
        && !m.is_file()
        && !(r.check && !repo.is_empty() && m.starts_with(&clone) && not_cloned)
    {
        bail!(
            "your rules aren't at {}: fix the path in ~/.config/switchboard/claude.json, or pick them again in mission control's Settings",
            home_tilde(m)
        );
    }

    let shown = home_tilde(&file);
    let current = if is_link(&file) {
        None
    } else {
        util::read_text(&file)
    };
    if let Some(m) = &mine {
        let want = format!("{MARK}\n\n@{}\n{IMPORT}\n", home_tilde(m));
        if current.as_deref() == Some(want.as_str()) {
            return Ok(());
        }
        if let Some(text) = &current
            && !text.contains(MARK_START)
        {
            let aside = file.with_file_name(format!("CLAUDE.md.bak.{}", stamp()));
            if !r.check {
                fs::rename(&file, &aside)?;
            }
            r.change(format!("move your {shown} aside to {}", home_tilde(&aside)));
        }
        if !r.check {
            if is_link(&file) {
                fs::remove_file(&file)?;
            }
            util::write_atomic(&file, &want)?;
        }
        r.change(format!(
            "write {shown} to import your rules ({}) and Switchboard's",
            home_tilde(m)
        ));
    } else if is_link(&file) {
        return Ok(());
    } else if let Some(text) = current {
        if text.lines().any(|l| l == IMPORT) {
            return Ok(());
        }
        if text.lines().any(|l| l == OLD_IMPORT) {
            if !r.check {
                let pointed: String = text
                    .split_inclusive('\n')
                    .map(|l| {
                        if l.trim_end_matches('\n') == OLD_IMPORT {
                            l.replacen(OLD_IMPORT, IMPORT, 1)
                        } else {
                            l.to_string()
                        }
                    })
                    .collect();
                util::write_atomic(&file, pointed)?;
            }
            r.change(format!(
                "point your {shown}'s import of Switchboard's part at {}",
                home_tilde(&part_file())
            ));
        } else {
            if !r.check {
                let mut f = OpenOptions::new().append(true).open(&file)?;
                std::io::Write::write_all(&mut f, format!("\n{IMPORT}\n").as_bytes())?;
            }
            r.change(format!(
                "add the line importing Switchboard's part to your {shown}"
            ));
        }
    } else {
        if !r.check {
            util::write_atomic(&file, format!("{IMPORT}\n"))?;
        }
        r.change(format!("make {shown}, importing Switchboard's part"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_part_says_what_every_user_s_claude_needs() {
        assert!(PART.contains("switchboard tabs"));
        assert!(!PART.contains("tmux"));
        assert!(MARK.starts_with(MARK_START));
    }
}
