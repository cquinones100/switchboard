//! `switchboard setup [--check] [part...]`: what the binary sets up for
//! Claude Code, and nothing beside it. Ported from setup.sh's Claude steps;
//! the dotfiles it linked (tmux, Neovim, lazygit, Ghostty and the rest) are
//! gone with the programs they configured, so these parts are all that is
//! left of it, the only configs of other programs being what Switchboard
//! itself needs from Ghostty and lazygit, never an editor's:
//!
//! - `plugin` (`plugin`): Switchboard's skills, hooks and instructions as a
//!   Claude Code plugin in `~/.local/share/switchboard/claude/plugin`, which the
//!   Claude sessions in its panes load and Claude elsewhere doesn't.
//! - `claude` (`claude_settings`): the hooks, statusline and allow rules
//!   earlier versions added to your own `~/.claude/settings.json` taken out.
//! - `claude-md` (`claude_md`): your `~/.claude/CLAUDE.md`, importing your
//!   rules, with the import of Switchboard's part earlier versions added
//!   taken out.
//! - `skills` (`skills`): the skills earlier versions wrote into
//!   `~/.claude/skills` taken out.
//! - `terminal` (`terminal`): Option acting as Alt in a Mac's terminal,
//!   and Notes' easy mode's Cmd keys in Ghostty.
//! - `lazygit` (`lazygit`): Switchboard's lazygit settings, layered under
//!   your config at launch, so Esc quits lazygit in a pane's place.
//!
//! Switchboard reaches Claude Code as its plugin alone; where Claude Code's
//! managed settings or its version keep the plugin from loading
//! (`blocked`), setup says so. The guided setup also says which optional
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
//! do. Switchboard adds nothing to your Claude Code settings: its plugin
//! draws the statusline in its panes. The plugin is Switchboard's own, and
//! taking out what earlier versions put in `~/.claude` is never asked
//! about either.
//!
//! `switchboard setup slack-user` is apart from the parts (`slack_user`):
//! it asks for a Slack user token of your own and stores it as a secret,
//! so it runs only when named, alone.
//!
//! What the parts write is compiled into the binary, so a binary built from
//! a clone brings new skills, hooks and settings that reach the plugin only
//! through a run of setup (or the server's start). The setup.sh version had
//! git's post-merge hook run it after every pull; here the first run of a new build (`switchboard`
//! attaching, `refresh`) writes the plugin and takes out what earlier
//! versions put in `~/.claude` quietly, as the guided setup would without
//! asking, and `switchboard setup` after a build does the same out loud.

pub mod blocked;
pub mod claude_md;
pub mod claude_settings;
pub mod lazygit;
pub mod optional;
pub mod permissions;
pub mod plugin;
pub mod skills;
pub mod slack_user;
pub mod terminal;

use std::io::{BufRead, IsTerminal, Write};
use std::path::Path;
use std::process::ExitCode;

use anyhow::Result;

use crate::{paths, platform, util};

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
        about: "write Switchboard's plugin (its hooks, skills and instructions), which Claude in its panes loads, to ~/.local/share/switchboard/claude/plugin",
        run: plugin::run,
    },
    Part {
        name: "claude",
        about: "take the hooks, statusline and dropped allow rules earlier versions added out of ~/.claude/settings.json, since Switchboard's plugin carries them",
        run: claude_settings::run,
    },
    Part {
        name: "claude-md",
        about: "write ~/.claude/CLAUDE.md to import your rules, taking out the import of Switchboard's part earlier versions added",
        run: claude_md::run,
    },
    Part {
        name: "skills",
        about: "take the skills earlier versions wrote out of ~/.claude/skills, since Switchboard's plugin carries them",
        run: skills::run,
    },
    Part {
        name: "terminal",
        about: "make Option act as Alt in your terminal (a Mac's Ghostty, Terminal or iTerm2), and send Cmd-S, Cmd-Z, Cmd-Shift-Z and Cmd-X to Notes' easy mode in Ghostty",
        run: terminal::run,
    },
    Part {
        name: "lazygit",
        about: "write Switchboard's lazygit settings, which lazygit in a pane's place layers under your config so Esc quits it unless yours says otherwise, never writing your config",
        run: lazygit::run,
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
    let mut slack_sign_in = false;
    let mut named = Vec::new();
    for arg in &args {
        match arg.as_str() {
            "--check" => check = true,
            "--refresh" => {
                // Whether every part it ran worked, for the server's look
                // at the clone (src/setup/plugin.rs), which tries again.
                return if refresh_now() {
                    ExitCode::SUCCESS
                } else {
                    ExitCode::FAILURE
                };
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
            "slack-user" | "slack_user" => slack_sign_in = true,
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
    // It stores a token, so it has no dry run and runs with no part.
    if slack_sign_in {
        if check || !named.is_empty() {
            eprintln!(
                "slack-user runs alone: it asks for a token and stores it, so it takes no --check and no other part"
            );
            return ExitCode::from(1);
        }
        return slack_user::main();
    }
    if named.is_empty() {
        return guided(check);
    }
    if let Some(blocked) = blocked::blocked() {
        println!("{}", blocked.why());
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
    println!("changes nothing; --refresh brings the parts up to date, quietly, as the first");
    println!("run of a new build does; --parts lists the parts:\n");
    for part in PARTS {
        println!("  {:10} {}", part.name, part.about);
    }
    println!("\nAnd one apart from them, run only when named:\n");
    println!("  {:10} {}", "slack-user", slack_user::ABOUT);
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
    claude_code(check, &mut list);
    println!();
    heading("The terminal");
    terminal_offered(check, &mut list);
    lazygit_offered(check, &mut list);
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

/// Option as Alt: changed after asking where setup can, a line to add said
/// for any other terminal.
fn terminal_offered(check: bool, list: &mut List) {
    let part = part_named("terminal").expect("a part");
    match pending(part) {
        Err(e) => list.todo(format!("Option as Alt: {e:#}")),
        Ok(n) if n > 0 => {
            let cmd_keys = terminal::adds_cmd_keys();
            // Only the Cmd keys pending: Option already acts as Alt.
            let only_cmd_keys = cmd_keys && n == 1;
            let (question, done, later) = match (only_cmd_keys, cmd_keys) {
                (true, _) => (
                    "Send Cmd-S, Cmd-Z, Cmd-Shift-Z and Cmd-X to Notes' easy mode in Ghostty's config? Ghostty passes them on as keys rather than acting on them itself.",
                    "Ghostty sends Notes' easy mode its Cmd keys; reload its config for them to take",
                    "Notes' easy mode's Cmd keys in Ghostty: switchboard setup terminal",
                ),
                (false, true) => (
                    "Make Option act as Alt in your terminal, which every M- key needs, and send Cmd-S, Cmd-Z, Cmd-Shift-Z and Cmd-X to Notes' easy mode in Ghostty's config, which then passes them on as keys rather than acting on them itself?",
                    "Option acts as Alt, and Ghostty sends Notes' easy mode its Cmd keys; open a new window for them to take",
                    "Option as Alt, which every M- key needs, and Notes' easy mode's Cmd keys in Ghostty: switchboard setup terminal",
                ),
                (false, false) => (
                    "Make Option act as Alt in your terminal, which every M- key needs?",
                    "Option acts as Alt; open a new window for it to take",
                    "Option as Alt, which every M- key needs: switchboard setup terminal",
                ),
            };
            if ask(check, question) {
                if apply(part) {
                    list.ok(done);
                } else {
                    list.todo("switchboard setup terminal failed");
                }
            } else {
                list.not_now("terminal", later);
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

/// lazygit, where it is installed: Switchboard's settings reach it at
/// launch, layered under your config, so nothing is asked.
fn lazygit_offered(check: bool, list: &mut List) {
    if !lazygit::installed() {
        return;
    }
    let part = part_named("lazygit").expect("a part");
    if check || apply(part) {
        list.ok("lazygit in a pane's place gets Switchboard's settings under your config, so Esc quits it");
    } else {
        list.todo("switchboard setup lazygit failed");
    }
}

/// Claude Code's parts: the plugin, written without asking, then, when
/// Claude Code is there, what keeps it from loading the plugin, and what
/// earlier versions put in `~/.claude` taken out.
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
    match blocked::blocked() {
        Some(blocked @ blocked::Blocked::TooOld(_)) => list.todo(blocked.why()),
        Some(blocked) => list.optional(&blocked.why()),
        None => {}
    }
    in_claude(check, list);
}

/// The plugin: Switchboard's own, so written without asking.
fn plugin_offered(check: bool, list: &mut List) {
    const THERE: &str = "Switchboard's plugin for the Claude in its panes (its skills, the hooks behind tab states and M-a, its instructions), in ~/.local/share/switchboard/claude/plugin";
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

/// `~/.claude`: nothing added to your settings, the plugin drawing the
/// statusline in its panes; what an earlier version put there (the hooks
/// and statusline in your settings, its part of your CLAUDE.md, its skills)
/// taken out without asking, and your rules imported after asking the
/// first time.
fn in_claude(check: bool, list: &mut List) {
    let settings = part_named("claude").expect("a part");
    match pending(settings) {
            Err(e) => list.todo(format!("Claude Code's settings: {e:#}")),
            Ok(0) => {}
            Ok(_) if check => list.todo(
                "your ~/.claude/settings.json has what an earlier version added there and Switchboard's plugin carries now: switchboard setup claude takes it out",
            ),
            Ok(_) if apply(settings) => list.ok(
                "what an earlier version added to your ~/.claude/settings.json and Switchboard's plugin carries now, taken out (a statusline of yours kept)",
            ),
            Ok(_) => list.todo("switchboard setup claude failed"),
        }
    let statusline_skipped = || {
        crate::mux::socket::ask_quiet(
            vec!["show".into(), "@statusline_skipped".into()],
            None,
            None,
        )
        .is_some_and(|(code, out, _)| code == 0 && out.starts_with(b"1"))
    };
    if crate::settings::is_on("pane_statusline") && statusline_skipped() {
        list.optional(
            "Switchboard's statusline in its panes: a managed plugin answers Claude Code's settings before Switchboard's, so a pane's Claude draws none, and mission control's Usage view goes without it",
        );
    } else if crate::settings::is_on("pane_statusline") {
        list.ok(
            "Switchboard's statusline in its panes, drawn by its plugin (mission control's Usage view), yours everywhere else",
        );
    } else {
        list.optional(
            "Switchboard's statusline in its panes, for mission control's Usage view: off in Settings' \"Switchboard's statusline in its panes\", so they draw yours",
        );
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
    add(crate::assets::text("claude-settings.json", claude_settings::ASSET).as_bytes());
    add(crate::cmd::claude_context::part().as_bytes());
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
/// that failed is tried again on the next run. The plugin written, and what
/// earlier versions put in `~/.claude` taken out, since left in, their
/// hooks would run twice in a pane: your CLAUDE.md gets only Switchboard's
/// import taken out, with no pull of your rules' repository, which the
/// update look does, and your rules' import is left for the guided setup to
/// ask about.
pub fn refresh_now() -> bool {
    let file = build_file();
    let mark = build_mark();
    if util::read_text(&file).is_some_and(|t| t.trim() == mark) {
        return true;
    }
    let mut report = Report::new(false);
    report.silent = true;
    let mut worked = plugin::run(&mut report).is_ok();
    worked &= claude_settings::run(&mut report).is_ok();
    if claude_md::set_up() || exists(&claude_md::part_file()) {
        worked &= claude_md::migrate(&mut report).is_ok();
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
    worked
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
