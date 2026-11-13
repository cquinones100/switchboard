//! The `claude-md` part: your `~/.claude/CLAUDE.md`, importing your own
//! rules. Ported from setup.sh's `claude_md`, `claude_md_now`,
//! `claude_md_pull` and `claude_md_gh_helper`.
//!
//! Your rules come from where `~/.config/switchboard/claude.json` says: a
//! git repository (`{"repo": "<url>", "file": "CLAUDE.md"}`, cloned into
//! `~/.local/share/switchboard/claude-md` and pulled on every run,
//! fast-forward only, with gh's sign-in among its credential helpers on
//! GitHub; without "file", its CLAUDE.md at the root or the one nested
//! least deep), or a file (`{"path": "~/somewhere/CLAUDE.md"}`), else
//! `~/.config/switchboard/CLAUDE.md` when there is one. Then
//! `~/.claude/CLAUDE.md` is a short file importing them, which Claude Code
//! follows (`@path`), so an edit shows at once; a real file in its way that
//! isn't one written here is moved aside first, but never by a move between
//! the plugin and `~/.claude` (`run_moving`), which asks nothing, so it
//! only adds or takes out Switchboard's import there. A link into a clone's
//! old `claude/CLAUDE.md` becomes `~/.config/switchboard/CLAUDE.md`, from
//! what it linked to, else from git once the file is gone.
//!
//! Switchboard's part of the instructions (`assets/switchboard.md`: the
//! tabs and talking to them, the notes app, repository skills) follows the
//! way setup puts Switchboard (src/setup/way.rs):
//!
//! - As a plugin, it reaches only the Claude sessions in Switchboard's
//!   panes, from the plugin's SessionStart hook (src/cmd/claude_context.rs).
//!   The line importing it that the old way added is taken out
//!   (`take_out`), with the blank line setup put before it when it added it
//!   to the end of a CLAUDE.md of yours, and nothing else of yours; a
//!   CLAUDE.md that held nothing but the line, as setup made one where there
//!   was none, would be left empty, so it is removed, which is how it was
//!   before setup made it. Then the copy of the part goes too. With no rules
//!   named, a CLAUDE.md of your own otherwise stays as it is.
//! - The old way, it is written to `~/.local/share/switchboard/
//!   switchboard.md` from the copy compiled in, where setup.sh imported it
//!   from the clone, and your CLAUDE.md imports it after your rules, for
//!   every Claude of yours. With no rules named, a CLAUDE.md of your own
//!   gets only the line importing it (the clone's old import line pointed
//!   at it instead), and a missing one is made with it.
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

use super::way::Way;
use super::{Report, exists, home_tilde, is_link, stamp};
use crate::{paths, util, worktree};

/// The line importing Switchboard's part, the old way.
pub const IMPORT: &str = "@~/.local/share/switchboard/switchboard.md";

/// The line setup.sh wrote, importing it from the clone.
pub const OLD_IMPORT: &str = "@~/switchboard/claude/switchboard.md";

/// The first line of a CLAUDE.md written here, as a plugin. Every version's
/// and setup.sh's began the same way (`Written by Switchboard's setup.sh`),
/// so any is known as one of ours, never moved aside.
pub const MARK: &str = "<!-- Written by Switchboard's setup to import your rules. Edit them where they live, not here. -->";
/// The first line of one written the old way, which imports Switchboard's
/// part too.
pub const MARK_WITH_PART: &str = "<!-- Written by Switchboard's setup: your rules, then Switchboard's. Edit them where they live, not here. -->";
const MARK_START: &str = "<!-- Written by Switchboard's setup";

/// Where Switchboard's part is written, the old way.
pub fn part_file() -> PathBuf {
    paths::home_join(".local/share/switchboard/switchboard.md")
}

/// Whether a line is one importing Switchboard's part.
fn imports_ours(line: &str) -> bool {
    line == IMPORT || line == OLD_IMPORT
}

/// Whether your CLAUDE.md imports Switchboard's part, as the old way has it.
pub fn imports_part() -> bool {
    !is_link(&claude_md())
        && util::read_text(claude_md()).is_some_and(|text| text.lines().any(imports_ours))
}

/// Whether claude.json (or your own `~/.config/switchboard/CLAUDE.md`)
/// names rules for your CLAUDE.md to import.
pub fn rules_named() -> bool {
    !config("repo").is_empty() || !config("path").is_empty() || own_rules().is_file()
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

/// Whether your CLAUDE.md was written here, or still imports Switchboard's
/// part as earlier versions made it: set up before, so the guided setup and
/// a new build bring it up to date without asking.
pub fn set_up() -> bool {
    util::read_text(claude_md())
        .is_some_and(|text| text.contains(MARK_START) || text.lines().any(imports_ours))
}

/// The part.
pub fn run(r: &mut Report) -> Result<()> {
    run_as(r, false)
}

/// The part as a move between the plugin and `~/.claude` runs it
/// (`super::move_over`), which no one was asked about: a CLAUDE.md of yours
/// in the way of your rules' import is never moved aside, and only gets
/// Switchboard's import line added (back into `~/.claude`) or taken out
/// (into the plugin); a link of yours is left as it is.
pub fn run_moving(r: &mut Report) -> Result<()> {
    run_as(r, true)
}

fn run_as(r: &mut Report, moving: bool) -> Result<()> {
    if r.check {
        return now(r, false, moving);
    }
    pull();
    let lock_file = paths::state_dir().join("claude-md.lock");
    let _held = lock(&lock_file, Duration::from_secs(120))?;
    now(r, true, moving)
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
        .filter(|l| !imports_ours(l))
        .map(|l| format!("{l}\n"))
        .collect();
    (!kept.trim().is_empty()).then_some(kept)
}

/// A CLAUDE.md without the lines importing Switchboard's part, or `None`
/// when it has none. Where setup added the line to the end of yours, after
/// a blank line of its own, that blank line goes too.
pub fn without_import(text: &str) -> Option<String> {
    if !text.lines().any(imports_ours) {
        return None;
    }
    let mut text = text.to_string();
    for line in [IMPORT, OLD_IMPORT] {
        let added = format!("\n\n{line}\n");
        if text.ends_with(&added) {
            text.truncate(text.len() - added.len() + 1);
        }
    }
    Some(
        text.split_inclusive('\n')
            .filter(|l| !imports_ours(l.trim_end_matches('\n')))
            .collect(),
    )
}

/// Switchboard's import taken out of your CLAUDE.md (`text`), and the file
/// removed when nothing else was in it.
fn take_out(r: &mut Report, file: &Path, text: &str) -> Result<()> {
    let Some(left) = without_import(text) else {
        return Ok(());
    };
    let shown = home_tilde(file);
    if left.is_empty() {
        if !r.check {
            fs::remove_file(file)?;
        }
        r.change(format!(
            "remove {shown}, which setup made only to import Switchboard's part: its plugin gives the Claude in its panes those instructions now"
        ));
    } else {
        if !r.check {
            util::write_atomic(file, left)?;
        }
        r.change(format!(
            "take the import of Switchboard's part out of {shown}: its plugin gives the Claude in its panes those instructions now"
        ));
    }
    Ok(())
}

/// The copy of Switchboard's part earlier versions wrote, removed once your
/// CLAUDE.md doesn't import it (a link of yours to a file that still does
/// keeps it).
fn remove_part(r: &mut Report, file: &Path) -> Result<()> {
    let part = part_file();
    let still = is_link(file)
        && util::read_text(file).is_some_and(|text| text.lines().any(|l| l == IMPORT));
    if still || !exists(&part) {
        return Ok(());
    }
    if !r.check {
        fs::remove_file(&part)?;
    }
    r.change(format!(
        "remove {}, which nothing imports now",
        home_tilde(&part)
    ));
    Ok(())
}

/// Switchboard's part, as this build reads it (src/assets.rs), written where your
/// CLAUDE.md imports it from the old way. Alone, it is what a new build
/// brings to a CLAUDE.md set up the old way (`super::refresh`), with no
/// pull and no lock, since it touches only that one file, written whole.
pub fn write_part(r: &mut Report) -> Result<()> {
    let part = part_file();
    let text = crate::cmd::claude_context::part();
    if util::read_text(&part).as_deref() != Some(&*text) {
        if !r.check {
            util::write_atomic(&part, &*text)?;
        }
        r.change(format!("write Switchboard's part to {}", home_tilde(&part)));
    }
    Ok(())
}

/// What a new build brings to a CLAUDE.md set up with Switchboard as a
/// plugin (`super::refresh`): Switchboard's import taken out, as the part
/// would, with no pull and no clone, which wait on the network, and the
/// lock waited on only a moment, so a run holding it is left to finish and
/// this is tried again by the next build's first run.
pub fn migrate(r: &mut Report) -> Result<()> {
    let _held = if r.check {
        None
    } else {
        Some(lock(
            &paths::state_dir().join("claude-md.lock"),
            Duration::from_secs(3),
        )?)
    };
    let file = claude_md();
    if !is_link(&file)
        && let Some(text) = util::read_text(&file)
    {
        take_out(r, &file, &text)?;
    }
    remove_part(r, &file)
}

/// The locked part of the run (all of it with `--check`), `moving` as
/// `run_moving` says.
fn now(r: &mut Report, pulled: bool, moving: bool) -> Result<()> {
    let file = claude_md();
    let clone = clone_dir();
    let way = super::way::current();

    if way == Way::Old {
        write_part(r)?;
    }

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
            return now(r, pulled, moving);
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
    // A CLAUDE.md of yours, which writing one to import your rules would
    // move aside (or a link of yours, which it would replace).
    let yours = is_link(&file) || current.as_ref().is_some_and(|t| !t.contains(MARK_START));
    if let Some(m) = &mine {
        let want = match way {
            Way::Plugin => format!("{MARK}\n\n@{}\n", home_tilde(m)),
            Way::Old => format!("{MARK_WITH_PART}\n\n@{}\n{IMPORT}\n", home_tilde(m)),
        };
        if moving && yours {
            // A move asks nothing, so yours stays: only Switchboard's line
            // goes in or out.
            match (way, &current) {
                (Way::Old, Some(text)) => add_import(r, &file, text)?,
                (Way::Plugin, Some(text)) => take_out(r, &file, text)?,
                (_, None) => {}
            }
        } else if current.as_deref() != Some(want.as_str()) {
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
            r.change(match way {
                Way::Plugin => format!("write {shown} to import your rules ({})", home_tilde(m)),
                Way::Old => format!(
                    "write {shown} to import your rules ({}) and Switchboard's",
                    home_tilde(m)
                ),
            });
        }
    } else if way == Way::Plugin {
        if let Some(text) = current {
            take_out(r, &file, &text)?;
        }
    } else if let Some(text) = current {
        add_import(r, &file, &text)?;
    } else if !is_link(&file) {
        if !r.check {
            util::write_atomic(&file, format!("{IMPORT}\n"))?;
        }
        r.change(format!("make {shown}, importing Switchboard's part"));
    }
    if way == Way::Plugin {
        remove_part(r, &file)?;
    }
    Ok(())
}

/// The old way, with no rules named: your own CLAUDE.md (`text`) given the
/// line importing Switchboard's part, or setup.sh's import pointed at it.
fn add_import(r: &mut Report, file: &Path, text: &str) -> Result<()> {
    let shown = home_tilde(file);
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
            util::write_atomic(file, pointed)?;
        }
        r.change(format!(
            "point your {shown}'s import of Switchboard's part at {}",
            home_tilde(&part_file())
        ));
    } else {
        if !r.check {
            let mut f = OpenOptions::new().append(true).open(file)?;
            std::io::Write::write_all(&mut f, format!("\n{IMPORT}\n").as_bytes())?;
        }
        r.change(format!(
            "add the line importing Switchboard's part to your {shown}"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_mark_is_known_by_how_it_starts() {
        assert!(MARK.starts_with(MARK_START));
        assert!(MARK_WITH_PART.starts_with(MARK_START));
    }

    #[test]
    fn switchboard_s_import_is_taken_out_and_nothing_else() {
        assert_eq!(without_import("# Mine\n- be brief\n"), None);
        // Added to the end by setup, after a blank line of its own.
        assert_eq!(
            without_import(&format!("# Mine\n- be brief\n\n{IMPORT}\n")).as_deref(),
            Some("# Mine\n- be brief\n")
        );
        // Moved by you, or setup.sh's, in the middle: only the line.
        assert_eq!(
            without_import(&format!("# Mine\n{OLD_IMPORT}\n\n- more\n")).as_deref(),
            Some("# Mine\n\n- more\n")
        );
        // A file setup made to hold only the line is left with nothing.
        assert_eq!(without_import(&format!("{IMPORT}\n")).as_deref(), Some(""));
        // A line that only mentions it is yours.
        assert_eq!(without_import(&format!("see {IMPORT}\n")), None);
        // What setup wrote with your rules: their import stays.
        assert_eq!(
            without_import(&format!("{MARK}\n\n@~/rules.md\n{IMPORT}\n")).as_deref(),
            Some(format!("{MARK}\n\n@~/rules.md\n").as_str())
        );
    }
}
