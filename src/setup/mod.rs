//! `switchboard setup [--check] [part...]`: what the binary sets up for
//! Claude Code, and nothing beside it. Ported from setup.sh's Claude steps;
//! the dotfiles it linked (tmux, Neovim, lazygit, Ghostty and the rest) are
//! gone with the programs they configured, so these three parts are all
//! that is left of it:
//!
//! - `claude` (`claude_settings`): Switchboard's hooks, statusline and
//!   permissions merged into your own `~/.claude/settings.json`.
//! - `claude-md` (`claude_md`): your `~/.claude/CLAUDE.md`, importing your
//!   rules and Switchboard's part of them.
//! - `skills` (`skills`): the skills that ship with Switchboard, written
//!   into `~/.claude/skills` from the copies compiled into the binary.
//!
//! As setup.sh did, running it again changes only what isn't in place yet:
//! each part says what it changed, or that it was up to date, and `--check`
//! says what would change and changes nothing. With parts named it does just
//! those, asking nothing; alone, it is the guided setup, which asks [Y/n]
//! before a part changes anything the first time (Enter is yes, a closed
//! input no, so nothing piped or unattended says yes), brings a part already
//! set up up to date without asking, and ends with what is left to do.
//!
//! What the parts write is compiled into the binary, so a binary built from
//! a clone brings new skills, hooks and a new part of CLAUDE.md that reach
//! `~/.claude` only through a run of setup. The setup.sh version had git's
//! post-merge hook run it after every pull; here the first run of a new
//! build (`switchboard` attaching, `refresh`) brings the parts already set
//! up up to date quietly, as the guided setup would without asking, and
//! `switchboard setup` after a build does the same out loud.

pub mod claude_md;
pub mod claude_settings;
pub mod skills;

use std::io::{BufRead, Write};
use std::path::Path;
use std::process::ExitCode;

use anyhow::Result;

use crate::{paths, util};

/// What a part changed, said as it goes: setup.sh's `change`. The part's
/// name is printed above its first change, so a part with nothing to do says
/// so on one line of its own.
pub struct Report {
    /// Say what would change, and change nothing.
    pub check: bool,
    /// Count the changes without printing them, for the guided setup's
    /// "would this part change anything".
    pub silent: bool,
    indent: &'static str,
    header: Option<String>,
    /// How many changes so far.
    pub changes: usize,
}

impl Report {
    pub fn new(check: bool) -> Report {
        Report {
            check,
            silent: false,
            indent: "",
            header: None,
            changes: 0,
        }
    }

    /// One change, done (or, with `--check`, to be done).
    pub fn change(&mut self, what: impl AsRef<str>) {
        self.changes += 1;
        if self.silent {
            return;
        }
        if let Some(header) = self.header.take() {
            println!("{}{header}", self.indent);
        }
        let would = if self.check { "would: " } else { "" };
        println!("{}  {would}{}", self.indent, what.as_ref());
    }
}

/// A part of the setup: its name, what it does, and the step itself.
pub struct Part {
    pub name: &'static str,
    pub about: &'static str,
    pub run: fn(&mut Report) -> Result<()>,
}

pub const PARTS: &[Part] = &[
    Part {
        name: "claude",
        about: "add Switchboard's hooks, statusline and permissions to ~/.claude/settings.json",
        run: claude_settings::run,
    },
    Part {
        name: "claude-md",
        about: "write ~/.claude/CLAUDE.md to import your rules and Switchboard's part",
        run: claude_md::run,
    },
    Part {
        name: "skills",
        about: "write the skills that ship with Switchboard into ~/.claude/skills",
        run: skills::run,
    },
];

/// A part by its name, setup.sh's own spelling (`claude_md`) taken too.
fn part_named(name: &str) -> Option<&'static Part> {
    let name = name.replace('_', "-");
    PARTS.iter().find(|p| p.name == name)
}

/// `switchboard setup`.
pub fn main(args: Vec<String>) -> ExitCode {
    let mut check = false;
    let mut named = Vec::new();
    for arg in &args {
        match arg.as_str() {
            "--check" => check = true,
            "--refresh" => {
                refresh_now();
                return ExitCode::SUCCESS;
            }
            "--parts" => {
                for part in PARTS {
                    println!("{}\t{}", part.name, part.about);
                }
                return ExitCode::SUCCESS;
            }
            "-h" | "--help" => {
                usage();
                return ExitCode::SUCCESS;
            }
            other => match part_named(other) {
                Some(part) => named.push(part),
                None => {
                    eprintln!("Unknown part: {other}");
                    eprintln!(
                        "  parts: {}",
                        PARTS.iter().map(|p| p.name).collect::<Vec<_>>().join(" ")
                    );
                    return ExitCode::from(1);
                }
            },
        }
    }
    if named.is_empty() {
        return guided(check);
    }
    let mut report = Report::new(check);
    let failed: Vec<&str> = named
        .iter()
        .filter(|part| !run_part(&mut report, part, false))
        .map(|part| part.name)
        .collect();
    if failed.is_empty() {
        ExitCode::SUCCESS
    } else {
        eprintln!("\nFailed: {}", failed.join(" "));
        ExitCode::from(1)
    }
}

fn usage() {
    println!("Usage: switchboard setup [--check] [part...]\n");
    println!("Sets Claude Code up for Switchboard. Alone, it asks before each part the first");
    println!("time; with parts named, it does just those. --check says what would change and");
    println!("changes nothing; --refresh brings the parts already set up up to date, quietly,");
    println!("as the first run of a new build does; --parts lists the parts:\n");
    for part in PARTS {
        println!("  {:10} {}", part.name, part.about);
    }
}

/// Runs one part, its changes under its name, or `<name>: up to date` when
/// it had none (left out when `quiet`, as the guided setup's list does);
/// whether it worked. What went wrong is said on stderr.
fn run_part(report: &mut Report, part: &Part, quiet: bool) -> bool {
    let before = report.changes;
    report.header = Some(format!("{}:", part.name));
    let result = (part.run)(report);
    if let Err(e) = &result {
        eprintln!("{}  {e:#}", report.indent);
    }
    if report.changes == before && !quiet && !report.silent {
        println!("{}{}: up to date", report.indent, part.name);
    }
    report.header = None;
    result.is_ok()
}

/// How many changes a part would make, changing nothing, or what keeps it
/// from running.
fn pending(part: &Part) -> Result<usize> {
    let mut report = Report::new(true);
    report.silent = true;
    (part.run)(&mut report)?;
    Ok(report.changes)
}

/// Runs a part for the guided setup, its changes indented under the list.
fn apply(part: &Part) -> bool {
    let mut report = Report::new(false);
    report.indent = "    ";
    run_part(&mut report, part, true)
}

/// A yes/no question, as every one in Switchboard asks it: Enter is yes, a
/// closed input (no terminal, end of file) no. Never asked with `--check`.
pub fn ask(check: bool, question: &str) -> bool {
    if check {
        return false;
    }
    print!("    {question} [Y/n] ");
    let _ = std::io::stdout().flush();
    let mut answer = String::new();
    match std::io::stdin().lock().read_line(&mut answer) {
        Ok(0) | Err(_) => {
            println!();
            false
        }
        Ok(_) => {
            let answer = answer.trim();
            answer.is_empty() || answer.starts_with(['y', 'Y'])
        }
    }
}

/// The guided setup's list: a line per part, and what is left at the end.
struct List {
    left: Vec<String>,
}

impl List {
    fn ok(&self, what: &str) {
        println!("  ✓ {what}");
    }

    fn todo(&mut self, what: impl Into<String>) {
        let what = what.into();
        println!("  ✗ {what}");
        self.left.push(what);
    }

    fn optional(&self, what: &str) {
        println!("  – {what}");
    }
}

/// `switchboard setup` alone: each part offered in turn.
fn guided(check: bool) -> ExitCode {
    let mut list = List { left: Vec::new() };
    println!("Claude Code");
    if util::which("switchboard").is_some() {
        list.ok("switchboard on your PATH, which Claude Code's hooks and statusline run");
    } else {
        list.todo(
            "switchboard on your PATH: Claude Code's hooks and statusline run it by name, so put the binary (or a link to it) in a folder on your PATH",
        );
    }
    if util::which("claude").is_none() {
        list.optional(
            "Claude Code, which every tab here runs, for tab states, the sidebar's sessions and M-a: curl -fsSL https://claude.ai/install.sh | bash",
        );
        return finish(check, list);
    }
    let [settings, md, skills] = [&PARTS[0], &PARTS[1], &PARTS[2]];

    // Without these hooks every tab shows idle, and M-a has nothing to walk.
    match pending(settings) {
        Err(e) => list.todo(format!("Claude Code's settings: {e:#}")),
        Ok(0) => list.ok(
            "Claude Code's hooks, statusline and permissions (tab states, the sidebar's Needs you, M-a, the loops)",
        ),
        Ok(_) if claude_settings::set_up() => {
            if check {
                list.todo("Switchboard's Claude Code settings have changed: switchboard setup claude adds them to yours");
            } else if apply(settings) {
                list.ok("Switchboard's Claude Code settings, brought up to date in yours");
            } else {
                list.todo("switchboard setup claude failed");
            }
        }
        Ok(_) => {
            if ask(
                check,
                "Add Switchboard's hooks, statusline and permissions to your ~/.claude/settings.json? Yours are kept, and a copy is left beside it.",
            ) {
                if apply(settings) {
                    list.ok("added Switchboard's Claude Code settings to yours; your old ones are beside them as settings.json.bak.*");
                } else {
                    list.todo("adding Switchboard's settings to ~/.claude/settings.json failed");
                }
            } else {
                list.optional(
                    "Claude Code's hooks, for tab states and M-a: switchboard setup claude adds them",
                );
            }
        }
    }

    // Your CLAUDE.md: why your rules can't be found is said whatever else.
    match pending(md) {
        Err(e) => list.todo(format!("your CLAUDE.md: {e:#}")),
        Ok(0) => list.ok("your CLAUDE.md imports Switchboard's part (the tabs, the notes app)"),
        Ok(_) if claude_md::set_up() => {
            if check {
                list.todo("your CLAUDE.md has changes to take: switchboard setup claude-md");
            } else if apply(md) {
                list.ok("your CLAUDE.md imports Switchboard's part, brought up to date");
            } else {
                list.todo("switchboard setup claude-md failed");
            }
        }
        Ok(_) => {
            if ask(
                check,
                "Add Switchboard's part (how to read the tabs, the notes app) to your ~/.claude/CLAUDE.md, with your own rules from ~/.config/switchboard/claude.json when it names them?",
            ) {
                if apply(md) {
                    list.ok("your CLAUDE.md imports Switchboard's part");
                } else {
                    list.todo("switchboard setup claude-md failed");
                }
            } else {
                list.optional(
                    "Switchboard's part of CLAUDE.md, for the tabs and the notes app: switchboard setup claude-md",
                );
            }
        }
    }

    // The skills, asked for the first time and then kept up to date, a new
    // one written without asking.
    let has = skills::installed();
    match pending(skills) {
        Err(e) => list.todo(format!("Switchboard's skills: {e:#}")),
        Ok(0) if has => {
            list.ok("Switchboard's skills (/wt, /shape, /review-loop, /ci-loop and the rest)")
        }
        Ok(_) if has => {
            if check {
                list.todo("Switchboard's skills have changes to write: switchboard setup skills");
            } else if apply(skills) {
                list.ok("Switchboard's skills, brought up to date");
            } else {
                list.todo("switchboard setup skills failed");
            }
        }
        Ok(_) => {
            if ask(
                check,
                "Install Switchboard's skills (/wt, /shape, /review-loop, /ci-loop and the rest) in ~/.claude/skills? A skill of your own by the same name is kept.",
            ) {
                if apply(skills) {
                    list.ok("installed Switchboard's skills");
                } else {
                    list.todo("switchboard setup skills failed");
                }
            } else {
                list.optional(
                    "Switchboard's skills (/wt, /shape, /review-loop, /ci-loop and the rest): switchboard setup skills",
                );
            }
        }
    }
    let own = skills::own_skills();
    if skills::installed() && !own.is_empty() {
        list.optional(&format!(
            "your own skills kept in place of Switchboard's of the same name: {}",
            own.join(" ")
        ));
    }
    finish(check, list)
}

/// The end of the guided setup: every ✗ again, so none has to be found
/// among the ticks.
fn finish(check: bool, list: List) -> ExitCode {
    println!();
    if !list.left.is_empty() {
        println!("Left to do:");
        for what in &list.left {
            println!("  ✗ {what}");
        }
        println!("Run switchboard setup again once they're done.");
        return ExitCode::from(1);
    }
    println!("{}", if check { "Nothing to do." } else { "Ready." });
    ExitCode::SUCCESS
}

// ── A new build ─────────────────────────────────────────────────────────────

/// Where the build whose parts were last put in place is recorded.
fn build_file() -> std::path::PathBuf {
    paths::home_join(".local/share/switchboard/setup-build")
}

/// What this build would put in place, in one line: its version and a hash
/// of everything the parts write, so a build from a clone with changed
/// skills counts as new though its commit is the same. FNV-1a, which stays
/// the same from one Rust to the next, as the std hasher needn't.
fn build_mark() -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let mut add = |bytes: &[u8]| {
        for b in bytes.iter().chain([0u8].iter()) {
            hash ^= u64::from(*b);
            hash = hash.wrapping_mul(0x100_0000_01b3);
        }
    };
    add(claude_settings::ASSET.as_bytes());
    add(claude_md::PART.as_bytes());
    for (path, bytes) in skills::FILES {
        add(path.as_bytes());
        add(bytes);
    }
    format!("{} {hash:016x}", crate::update::version_line())
}

/// The first run of a new build: the parts already set up brought up to
/// date, quietly, as `switchboard` attaches. Not in a run with a state
/// directory of its own (a try of a worktree's build, or a test's), which
/// shares your home and would put its version of the skills in it.
pub fn refresh() {
    if std::env::var_os("SWITCHBOARD_STATE_DIR").is_some() {
        return;
    }
    refresh_now();
}

/// `refresh` whatever the state directory (`switchboard setup --refresh`):
/// once per build, recorded only once every part it ran worked, so one
/// that failed is tried again on the next run. A part you said no to, or
/// never ran, is left for the guided setup to ask about; your CLAUDE.md
/// gets only Switchboard's part written again, with no pull of your rules'
/// repository, which the update look does.
pub fn refresh_now() {
    let file = build_file();
    let mark = build_mark();
    if util::read_text(&file).is_some_and(|t| t.trim() == mark) {
        return;
    }
    let mut report = Report::new(false);
    report.silent = true;
    let mut worked = true;
    if claude_settings::set_up() {
        worked &= claude_settings::run(&mut report).is_ok();
    }
    if claude_md::set_up() {
        worked &= claude_md::write_part(&mut report).is_ok();
    }
    if skills::installed() {
        worked &= skills::run(&mut report).is_ok();
    }
    if worked {
        if let Some(dir) = file.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = util::write_atomic(&file, format!("{mark}\n"));
    }
}

// ── What the parts share ────────────────────────────────────────────────────

/// A path under the home directory written from `~`, as setup.sh wrote
/// `${path/#$HOME/~}`: only a leading home is shortened.
pub fn home_tilde(path: &Path) -> String {
    let home = paths::home();
    match path.strip_prefix(&home) {
        Ok(rest) if home != Path::new("/") => {
            if rest.as_os_str().is_empty() {
                "~".into()
            } else {
                format!("~/{}", rest.display())
            }
        }
        _ => path.display().to_string(),
    }
}

/// `date +%Y%m%d%H%M%S`, for the names of what is moved aside.
pub fn stamp() -> String {
    util::local_now().strftime("%Y%m%d%H%M%S").to_string()
}

/// Whether a path is a symbolic link (one leading nowhere included).
pub fn is_link(path: &Path) -> bool {
    path.symlink_metadata()
        .is_ok_and(|m| m.file_type().is_symlink())
}

/// Whether anything is at a path, a link leading nowhere included.
pub fn exists(path: &Path) -> bool {
    path.symlink_metadata().is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parts_are_found_by_either_spelling() {
        assert_eq!(part_named("claude_md").map(|p| p.name), Some("claude-md"));
        assert_eq!(part_named("skills").map(|p| p.name), Some("skills"));
        assert!(part_named("tmux").is_none());
    }

    #[test]
    fn a_question_is_never_asked_with_check() {
        assert!(!ask(true, "anything?"));
    }
}
