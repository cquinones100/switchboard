//! `switchboard setup [--check] [part...]`: what the binary sets up for
//! Claude Code, and nothing beside it. Ported from setup.sh's Claude steps;
//! the dotfiles it linked (tmux, Neovim, lazygit, Ghostty and the rest) are
//! gone with the programs they configured, so these parts are all that is
//! left of it:
//!
//! - `plugin` (`plugin`): Switchboard's skills, hooks and instructions as a
//!   Claude Code plugin in `~/.local/share/switchboard/plugin`, which the
//!   Claude sessions in its panes load and Claude elsewhere doesn't.
//! - `claude` (`claude_settings`): Switchboard's statusline and permissions
//!   merged into your own `~/.claude/settings.json`, with its hooks the old
//!   way, without them as a plugin.
//! - `claude-md` (`claude_md`): your `~/.claude/CLAUDE.md`, importing your
//!   rules, and Switchboard's part the old way.
//! - `skills` (`skills`): Switchboard's skills in `~/.claude/skills` the old
//!   way, taken out of it as a plugin, and the commands a repository's old
//!   skills run linked into `~/.local/bin`.
//! - `terminal` (`terminal`): Option acting as Alt in a Mac's terminal.
//! - `leftovers` (`leftovers`): what the tmux version's setup.sh left in
//!   your home that leads nowhere now, and a `switchboard` function or alias
//!   that starts tmux in the binary's place, tidied.
//!
//! Which way the middle three put Switchboard, as a plugin or in
//! `~/.claude` as earlier versions did, is Settings' "Load Switchboard as a
//! plugin", Claude Code's version and its managed settings (`way`);
//! `--move` runs the parts that move Switchboard over after the setting
//! changes, as Settings does. The guided setup also says which optional
//! parts are there (`optional`): the services and programs a feature needs,
//! each with how to set it up.
//!
//! As setup.sh did, running it again changes only what isn't in place yet:
//! each part says what it changed, or that it was up to date, and `--check`
//! says what would change and changes nothing. With parts named it does just
//! those, asking nothing; alone, it is the guided setup, which asks [Y/n]
//! before a part changes anything of yours the first time (Enter is yes, a
//! closed input no, so nothing piped or unattended says yes), brings a part
//! already set up up to date without asking, and ends with what is left to
//! do. The plugin is Switchboard's own, so it is never asked about. An
//! install an earlier version made is asked once whether to load
//! Switchboard as a plugin; after a yes, taking out what it put in your
//! files isn't asked about again.
//!
//! What the parts write is compiled into the binary, so a binary built from
//! a clone brings new skills, hooks and settings that reach the plugin and
//! `~/.claude` only through a run of setup (or, for the plugin, the
//! server's start). The setup.sh version had git's post-merge hook run it
//! after every pull; here the first run of a new build (`switchboard`
//! attaching, `refresh`) writes the plugin and brings the parts already set
//! up up to date quietly, as the guided setup would without asking, and
//! `switchboard setup` after a build does the same out loud.

pub mod claude_md;
pub mod claude_settings;
pub mod leftovers;
pub mod optional;
pub mod plugin;
pub mod skills;
pub mod terminal;
pub mod way;

use std::io::{BufRead, IsTerminal, Write};
use std::path::Path;
use std::process::ExitCode;

use anyhow::Result;

use crate::{paths, platform, util};
use way::Way;

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
        name: "plugin",
        about: "write Switchboard's plugin (its hooks, skills and instructions), which Claude in its panes loads, to ~/.local/share/switchboard/plugin",
        run: plugin::run,
    },
    Part {
        name: "claude",
        about: "add Switchboard's statusline and permissions to ~/.claude/settings.json, and its hooks unless it loads as a plugin (then take them out)",
        run: claude_settings::run,
    },
    Part {
        name: "claude-md",
        about: "write ~/.claude/CLAUDE.md to import your rules, and Switchboard's part unless it loads as a plugin (then take it out)",
        run: claude_md::run,
    },
    Part {
        name: "skills",
        about: "write Switchboard's skills into ~/.claude/skills unless it loads as a plugin (then take them out), and link skill-path and codex-review into ~/.local/bin",
        run: skills::run,
    },
    Part {
        name: "terminal",
        about: "make Option act as Alt in your terminal (a Mac's Ghostty, Terminal or iTerm2)",
        run: terminal::run,
    },
    Part {
        name: "leftovers",
        about: "remove the tmux version's links that lead nowhere and its worktree lines in ~/.zshrc, and take out a switchboard function or alias that starts tmux",
        run: leftovers::run,
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
            "--move" => return move_over(),
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
    if let (_, Some(why)) = way::settle(check) {
        println!("{why}");
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
    println!("as the first run of a new build does; --move moves Switchboard into its plugin");
    println!("or back into ~/.claude, as Settings' \"Load Switchboard as a plugin\" says; --parts");
    println!("lists the parts:\n");
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
    /// The optional parts not there, said again at the end.
    wanted: Vec<String>,
    /// The parts offered and not set up: said no to, or, with --check,
    /// what the setup would offer.
    skipped: Vec<&'static str>,
}

/// Whether the guided setup's list is drawn in colour: on a terminal, unless
/// `NO_COLOR` asks for none. Piped or redirected, it is plain text.
fn colour() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| {
        std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none_or(|v| v.is_empty())
    })
}

/// `text` in an SGR style when the list is in colour.
fn paint(style: &str, text: &str) -> String {
    if colour() {
        format!("\x1b[{style}m{text}\x1b[0m")
    } else {
        text.to_string()
    }
}

/// A heading over a group of the list's lines, in bold.
fn heading(text: &str) {
    println!("{}", paint("1", text));
}

/// The marks: in place, green; to do, red; optional or not now, dim.
fn ok_mark() -> String {
    paint("32", "✓")
}

fn todo_mark() -> String {
    paint("31", "✗")
}

fn dim_mark() -> String {
    paint("2", "–")
}

impl List {
    fn ok(&self, what: &str) {
        println!("  {} {what}", ok_mark());
    }

    fn todo(&mut self, what: impl Into<String>) {
        let what = what.into();
        println!("  {} {what}", todo_mark());
        self.left.push(what);
    }

    fn optional(&self, what: &str) {
        println!("  {} {what}", dim_mark());
    }

    /// A part offered and not set up, now.
    fn not_now(&mut self, part: &'static str, what: &str) {
        println!("  {} {what}", dim_mark());
        self.skipped.push(part);
    }
}

/// `switchboard setup` alone: each part offered in turn.
fn guided(check: bool) -> ExitCode {
    let mut list = List {
        left: Vec::new(),
        wanted: Vec::new(),
        skipped: Vec::new(),
    };
    leftovers_offered(check, &mut list);
    claude_code(check, &mut list);
    println!();
    heading("The terminal");
    terminal_offered(check, &mut list);
    println!();
    heading("Optional, each for what it names");
    for part in optional::all() {
        if part.there {
            list.ok(&part.what);
        } else {
            list.optional(&part.what);
            list.wanted.push(part.name);
        }
    }
    finish(check, list)
}

/// What the tmux version left, said only when there is any.
fn leftovers_offered(check: bool, list: &mut List) {
    let part = part_named("leftovers").expect("a part");
    match pending(part) {
        Err(e) => {
            heading("From the tmux version");
            list.todo(format!("what the tmux version left: {e:#}"));
        }
        Ok(0) => return,
        Ok(n) => {
            heading("From the tmux version");
            if check {
                list.todo(format!(
                    "{n} of the tmux version's links or ~/.zshrc lines are left: switchboard setup leftovers"
                ));
            } else if ask(
                check,
                "Remove the tmux version's links that lead nowhere now and its worktree lines in ~/.zshrc or ~/.bashrc, which nothing needs, and take out a switchboard function or alias there that starts tmux? A copy of each file changed is left beside it.",
            ) {
                if apply(part) {
                    list.ok("tidied what the tmux version left");
                } else {
                    list.todo("switchboard setup leftovers failed");
                }
            } else {
                list.not_now(
                    "leftovers",
                    "the tmux version's links that lead nowhere, and its ~/.zshrc lines: switchboard setup leftovers",
                );
            }
        }
    }
    println!();
}

/// Option as Alt: changed after asking where setup can, a line to add said
/// for any other terminal.
fn terminal_offered(check: bool, list: &mut List) {
    let part = part_named("terminal").expect("a part");
    match pending(part) {
        Err(e) => list.todo(format!("Option as Alt: {e:#}")),
        Ok(n) if n > 0 => {
            if ask(
                check,
                "Make Option act as Alt in your terminal, which every M- key needs?",
            ) {
                if apply(part) {
                    list.ok("Option acts as Alt; open a new window for it to take");
                } else {
                    list.todo("switchboard setup terminal failed");
                }
            } else {
                list.not_now(
                    "terminal",
                    "Option as Alt, which every M- key needs: switchboard setup terminal",
                );
            }
        }
        Ok(_) => match terminal::hint() {
            Some(hint) => list.optional(&format!("Option as Alt, which every M- key needs, in {hint}")),
            None if platform::NAME == "macOS" => list.ok("Option acts as Alt in your terminal"),
            None => list.optional(
                "the M- keys are Alt, which a Linux terminal sends as it is; one your desktop takes for itself can be moved in ~/.config/switchboard/keys.json",
            ),
        },
    }
}

/// Claude Code's parts: the plugin, written without asking, then, when
/// Claude Code is there, the way Switchboard reaches it and the parts that
/// way puts in place.
fn claude_code(check: bool, list: &mut List) {
    heading("Claude Code");
    if util::which("switchboard").is_some() {
        list.ok("switchboard on your PATH, which Claude Code's hooks and statusline run");
    } else {
        list.todo(
            "switchboard on your PATH: Claude Code's hooks and statusline run it by name, so put the binary (or a link to it) in a folder on your PATH",
        );
    }
    // The one program beside Claude Code the workspace expects, which
    // install.sh doesn't check, as the tmux version's did.
    if util::which("git").is_some() {
        list.ok("git, for the worktree commands, M-d's diff and the tabs' branches");
    } else {
        list.todo(if platform::NAME == "macOS" {
            "git, which the worktree commands, M-d's diff and the tabs' branches need: xcode-select --install"
        } else {
            "git, which the worktree commands, M-d's diff and the tabs' branches need: install it with your package manager"
        });
    }
    plugin_offered(check, list);
    if util::which("claude").is_none() {
        list.optional(
            "Claude Code, which every tab here runs, for tab states, the sidebar's sessions and M-a: curl -fsSL https://claude.ai/install.sh | bash",
        );
        return;
    }
    match way_offered(check, list) {
        Way::Plugin => plugin_way(check, list),
        Way::Old => old_way(check, list),
    }
}

/// The plugin: Switchboard's own, so written without asking.
fn plugin_offered(check: bool, list: &mut List) {
    const THERE: &str = "Switchboard's plugin for the Claude in its panes (its skills, the hooks behind tab states and M-a, its instructions), in ~/.local/share/switchboard/plugin";
    let part = part_named("plugin").expect("a part");
    match pending(part) {
        Err(e) => list.todo(format!("Switchboard's plugin: {e:#}")),
        Ok(0) => list.ok(THERE),
        Ok(_) if check => list.not_now(
            "plugin",
            "Switchboard's plugin, which setup writes without asking: switchboard setup plugin",
        ),
        Ok(_) => {
            if apply(part) {
                list.ok(THERE);
            } else {
                list.todo("switchboard setup plugin failed");
            }
        }
    }
}

/// The way Switchboard reaches Claude Code: a Claude Code too old for the
/// plugin, or managed settings that keep it from loading one, said, and an
/// install an earlier version made in `~/.claude`, whose owner hasn't
/// chosen, asked once whether to move into the plugin: one with the setting
/// at its default, or one a new build's first run kept the old way until
/// asked (`way::keep_unasked`), once the plugin can load.
fn way_offered(check: bool, list: &mut List) -> Way {
    let (way, why) = way::settle(check);
    if let Some(why) = why {
        list.optional(&why);
        return Way::Old;
    }
    let unasked = (way == Way::Plugin && way::old_install() && !way::chosen())
        || (way::unasked() && way::blocked().is_none());
    if unasked {
        if check {
            way::set(Way::Old);
            list.not_now(
                "loading Switchboard as a plugin",
                "Switchboard as a plugin in its panes, in place of its hooks, skills and instructions in ~/.claude: switchboard setup asks",
            );
            return Way::Old;
        }
        // A no changes what a new pane names only when the setting was on;
        // a yes always may (an earlier build's workspace names no plugin),
        // so it always says how the tabs open take it.
        let was_on = way::plugin_on();
        if ask(
            check,
            "Load Switchboard as a plugin in its panes? Its hooks, skills and instructions move out of ~/.claude, so Claude elsewhere has only your own; Settings' \"Load Switchboard as a plugin\" moves them back.",
        ) {
            // Kept the old way until asked, the setting is off; at its
            // default, it is on already.
            if let Err(e) = way::choose(Way::Plugin, false)
                && !way::plugin_on()
            {
                way::set(Way::Old);
                list.todo(format!(
                    "your answer, kept in Settings' \"Load Switchboard as a plugin\": {e:#}"
                ));
                return Way::Old;
            }
            list.ok(&format!(
                "Switchboard loads as a plugin in its panes; {}",
                way::OPEN_TABS
            ));
            return Way::Plugin;
        }
        return match way::choose(Way::Old, false) {
            Ok(()) => {
                let stays = "Switchboard stays in ~/.claude, for every Claude of yours: Settings' \"Load Switchboard as a plugin\" moves it into the plugin";
                list.optional(&if was_on {
                    format!("{stays}; {}", way::OPEN_TABS)
                } else {
                    stays.to_string()
                });
                Way::Old
            }
            Err(e) => {
                list.todo(format!(
                    "your answer, kept in Settings' \"Load Switchboard as a plugin\": {e:#}"
                ));
                Way::Old
            }
        };
    }
    if way == Way::Old && way::blocked().is_none() {
        list.optional(
            "Switchboard as a plugin, which keeps its hooks, skills and instructions to the Claude in its panes: Settings' \"Load Switchboard as a plugin\"",
        );
    }
    way
}

/// Switchboard as a plugin: your settings get its statusline and
/// permissions, asked the first time, and what an earlier version put in
/// `~/.claude` is taken out without asking.
fn plugin_way(check: bool, list: &mut List) {
    let settings = part_named("claude").expect("a part");
    match pending(settings) {
        Err(e) => list.todo(format!("Claude Code's settings: {e:#}")),
        Ok(0) => list.ok(
            "Claude Code's statusline and permissions (the Sessions and Usage views, the loops running unattended)",
        ),
        Ok(_) if claude_settings::set_up() => {
            if check {
                list.todo("your Claude Code settings have Switchboard's changes to take: switchboard setup claude");
            } else if apply(settings) {
                list.ok("Switchboard's Claude Code settings, brought up to date in yours");
            } else {
                list.todo("switchboard setup claude failed");
            }
        }
        Ok(_) => {
            if ask(
                check,
                "Add Switchboard's statusline and permissions to your ~/.claude/settings.json? Yours are kept, and a copy is left beside it.",
            ) {
                let had = paths::claude_dir().join("settings.json").exists();
                if apply(settings) {
                    list.ok(if had {
                        "added Switchboard's statusline and permissions to yours; your old ones are beside them as settings.json.bak.*"
                    } else {
                        "made ~/.claude/settings.json with Switchboard's statusline and permissions"
                    });
                } else {
                    list.todo("adding Switchboard's settings to ~/.claude/settings.json failed");
                }
            } else {
                list.not_now(
                    "claude",
                    "Claude Code's statusline and permissions, for the Sessions and Usage views and the loops: switchboard setup claude adds them",
                );
            }
        }
    }

    // Your CLAUDE.md: Switchboard's import taken out without asking, your
    // rules imported after asking the first time.
    let md = part_named("claude-md").expect("a part");
    match pending(md) {
        Err(e) => list.todo(format!("your CLAUDE.md: {e:#}")),
        Ok(0) => {
            if claude_md::rules_named() {
                list.ok("your CLAUDE.md imports your rules");
            }
        }
        Ok(_) if claude_md::set_up() || !claude_md::rules_named() => {
            if check {
                list.todo("your CLAUDE.md has changes to take: switchboard setup claude-md");
            } else if apply(md) {
                list.ok("your CLAUDE.md, brought up to date");
            } else {
                list.todo("switchboard setup claude-md failed");
            }
        }
        Ok(_) => {
            if ask(
                check,
                "Write your ~/.claude/CLAUDE.md to import your own rules, as ~/.config/switchboard/claude.json names them? A CLAUDE.md of yours there is moved aside first.",
            ) {
                if apply(md) {
                    list.ok("your CLAUDE.md imports your rules");
                } else {
                    list.todo("switchboard setup claude-md failed");
                }
            } else {
                list.not_now(
                    "claude-md",
                    "your rules in your CLAUDE.md: switchboard setup claude-md",
                );
            }
        }
    }

    // The skills an earlier version wrote, taken out.
    if skills::installed() {
        let part = part_named("skills").expect("a part");
        match pending(part) {
            Err(e) => list.todo(format!("Switchboard's skills: {e:#}")),
            Ok(_) if check => list.todo(
                "Switchboard's skills in ~/.claude/skills, which its plugin carries now: switchboard setup skills takes them out",
            ),
            Ok(_) => {
                if apply(part) {
                    list.ok("took Switchboard's skills out of ~/.claude/skills: its plugin carries them");
                } else {
                    list.todo("switchboard setup skills failed");
                }
            }
        }
    }
    let own = skills::own_skills();
    if !own.is_empty() {
        list.optional(&format!(
            "your own skills by the names of Switchboard's, which take the bare name (Switchboard's run as /switchboard:<name>): {}",
            own.join(" ")
        ));
    }
}

/// Switchboard in `~/.claude`, as earlier versions put it: its hooks,
/// statusline and permissions, its part of CLAUDE.md and its skills, each
/// asked the first time and then kept up to date.
fn old_way(check: bool, list: &mut List) {
    let settings = part_named("claude").expect("a part");
    let md = part_named("claude-md").expect("a part");
    let skills = part_named("skills").expect("a part");

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
                let had = paths::claude_dir().join("settings.json").exists();
                if apply(settings) {
                    list.ok(if had {
                        "added Switchboard's Claude Code settings to yours; your old ones are beside them as settings.json.bak.*"
                    } else {
                        "made ~/.claude/settings.json with Switchboard's Claude Code settings"
                    });
                } else {
                    list.todo("adding Switchboard's settings to ~/.claude/settings.json failed");
                }
            } else {
                list.not_now(
                    "claude",
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
                list.not_now(
                    "claude-md",
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
                list.not_now(
                    "skills",
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
}

/// The end of the guided setup: every ✗ again, so none has to be found
/// among the ticks.
fn finish(check: bool, list: List) -> ExitCode {
    println!();
    if !list.wanted.is_empty() {
        println!(
            "Left to do when you want what each is for (Optional, above): {}.",
            list.wanted.join("; ")
        );
    }
    if !list.left.is_empty() {
        heading("Left to do:");
        for what in &list.left {
            println!("  {} {what}", todo_mark());
        }
        println!("Run switchboard setup again once they're done.");
        return ExitCode::from(1);
    }
    if check && !list.skipped.is_empty() {
        println!(
            "switchboard setup would offer: {}.",
            list.skipped.join(", ")
        );
    } else {
        println!("{}", if check { "Nothing to do." } else { "Ready." });
    }
    ExitCode::SUCCESS
}

/// This binary's own `setup` with `args` and its input closed, as Settings
/// runs it: what it said last, in its own words, or why it failed.
fn run_setup(args: &[&str]) -> Result<String, String> {
    let exe = util::exe().map_err(|e| e.to_string())?;
    let out = std::process::Command::new(exe)
        .arg("setup")
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| e.to_string())?;
    let last = |bytes: &[u8]| {
        String::from_utf8_lossy(bytes)
            .lines()
            .map(str::trim)
            .rfind(|l| !l.is_empty())
            .unwrap_or("")
            .to_string()
    };
    let said = last(&out.stdout);
    if out.status.success() {
        // "claude-md: up to date" says nothing worth saying.
        Ok(if said.ends_with("up to date") {
            String::new()
        } else {
            said
        })
    } else {
        let why = last(&out.stderr);
        Err(if why.is_empty() { said } else { why })
    }
}

/// The `claude-md` part, as Settings runs it after claude.json changes.
pub fn run_claude_md() -> Result<String, String> {
    run_setup(&["claude-md"])
}

/// `setup --move`, as Settings runs it after "Load Switchboard as a plugin"
/// changes.
pub fn run_move() -> Result<String, String> {
    run_setup(&["--move"])
}

/// `switchboard setup --move`: Switchboard moved the way the setting now
/// says, by the parts that do it, out loud, asking nothing, ending with a
/// line saying where it is and how the tabs open now take it (Settings and
/// `switchboard setting` show that line). Into the plugin, each part runs
/// only when it has something of Switchboard's to take out, so nothing of
/// yours is added; back into `~/.claude`, all three put it there, as the
/// old way's whole setup did, but for moving a CLAUDE.md of yours aside,
/// which isn't asked here (`claude_md::run_moving`).
fn move_over() -> ExitCode {
    // The setting changed by you: a choice, which an install kept the old
    // way until asked needn't be asked again.
    way::asked();
    let (way, why) = way::settle(false);
    let mut report = Report::new(false);
    let mut failed = Vec::new();
    let plugin = part_named("plugin").expect("a part");
    if !run_part(&mut report, plugin, true) {
        failed.push(plugin.name);
    }
    let parts: Vec<&str> = match way {
        Way::Old => vec!["claude", "claude-md", "skills"],
        Way::Plugin => [
            ("claude", claude_settings::set_up()),
            (
                "claude-md",
                claude_md::set_up() || exists(&claude_md::part_file()),
            ),
            ("skills", skills::installed()),
        ]
        .into_iter()
        .filter(|(_, any)| *any)
        .map(|(name, _)| name)
        .collect(),
    };
    // Your CLAUDE.md, which a move asks nothing about, is never moved aside.
    const MOVING_CLAUDE_MD: Part = Part {
        name: "claude-md",
        about: "",
        run: claude_md::run_moving,
    };
    for name in parts {
        let part = if name == MOVING_CLAUDE_MD.name {
            &MOVING_CLAUDE_MD
        } else {
            part_named(name).expect("a part")
        };
        if !run_part(&mut report, part, true) {
            failed.push(part.name);
        }
    }
    if !failed.is_empty() {
        eprintln!("Failed: {}", failed.join(" "));
        return ExitCode::from(1);
    }
    println!(
        "{}",
        match (way, why) {
            (_, Some(why)) => why,
            (Way::Plugin, None) => format!(
                "Switchboard loads as a plugin in its panes, and none of it is in ~/.claude; {}",
                way::OPEN_TABS
            ),
            (Way::Old, None) => format!(
                "Switchboard's hooks, skills and instructions are in ~/.claude, for every Claude of yours; {}",
                way::OPEN_TABS
            ),
        }
    );
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
    for (path, bytes) in plugin::files() {
        add(path.as_bytes());
        add(&bytes);
    }
    format!("{} {hash:016x}", crate::update::version_line())
}

/// Whether this run has a state directory of its own
/// (`SWITCHBOARD_STATE_DIR`: a test's, or a worktree's build run by hand),
/// which shares your home, so what it wrote of Switchboard's parts there
/// would be its version in place of the one your workspace runs. Such a run
/// writes none of them by itself: neither `refresh` nor its server's start.
pub fn state_of_its_own() -> bool {
    std::env::var_os("SWITCHBOARD_STATE_DIR").is_some_and(|d| !d.is_empty())
}

/// The first run of a new build: the plugin written and the parts already
/// set up brought up to date, quietly, as `switchboard` attaches. Not in a
/// run with a state directory of its own (`state_of_its_own`).
pub fn refresh() {
    if state_of_its_own() {
        return;
    }
    refresh_now();
}

/// `refresh` whatever the state directory (`switchboard setup --refresh`):
/// once per build, recorded only once every part it ran worked, so one
/// that failed is tried again on the next run. A part you said no to, or
/// never ran, is left for the guided setup to ask about; your CLAUDE.md
/// gets only Switchboard's part written again (or, as a plugin, its import
/// taken out), with no pull of your rules' repository, which the update
/// look does. Nothing is moved quietly: an install an earlier version made
/// in `~/.claude` whose owner hasn't chosen is kept the old way, the setting
/// off so the panes don't load the plugin on top of its hooks, and noted as
/// unasked (`way::keep_unasked`), so the guided setup still asks.
pub fn refresh_now() {
    let file = build_file();
    let mark = build_mark();
    if util::read_text(&file).is_some_and(|t| t.trim() == mark) {
        return;
    }
    let mut report = Report::new(false);
    report.silent = true;
    let mut worked = plugin::run(&mut report).is_ok();
    if way::old_install() && !way::chosen() {
        worked &= way::keep_unasked().is_ok();
    }
    let (way, _) = way::settle(false);
    if claude_settings::set_up() {
        worked &= claude_settings::run(&mut report).is_ok();
    }
    match way {
        Way::Old if claude_md::set_up() => {
            worked &= claude_md::write_part(&mut report).is_ok();
        }
        Way::Plugin if claude_md::set_up() || exists(&claude_md::part_file()) => {
            worked &= claude_md::migrate(&mut report).is_ok();
        }
        _ => {}
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
