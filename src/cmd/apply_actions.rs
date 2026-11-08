//! `switchboard apply-actions <repo> <from> <to>`: what a change needs to
//! take effect on this computer once it is checked out in `<repo>`, the
//! range `<from>..<to>` being what the clone just moved by, one action a
//! line, tab-separated:
//!
//! ```text
//! <key>  <label>  <why>  <command>
//! ```
//!
//! The command is a shell command to run as it is; an empty one is
//! something only you can do, said rather than offered. Each action is
//! listed once, however many files call for it, and nothing at all when
//! the change needs nothing. queue-and-clean runs it after a landing and
//! offers what it lists (the `apply_after_landing` setting).
//!
//! In Switchboard (`~/switchboard`, by its real path), its own list: a
//! change to the code (`src/`, `assets/`, the build's files) is built and
//! put in place of the running binary (`switchboard apply install`), then
//! the Slack relay restarted while it runs (`switchboard apply relay`),
//! and the workspace restarted, which is yours to do, since restarting the
//! server ends the Claude asking. A release build is left out: it is kept
//! current by releases, never by a clone. In any repository, the entry for
//! its origin's `owner/name` in `~/.config/switchboard/repos.json` may give
//! an `apply` list (src/repos.rs), run in the clone; without one, a changed
//! lockfile (an install with the package manager it names, where packages
//! are installed), new migration files (the package's migrate script, where
//! it has one) and a changed `.env.example`. An install or a script is the
//! repository's own code, so it is offered only in a clone listed in
//! `~/.config/switchboard/trusted-dirs`, and named for you to run elsewhere.
//!
//! `switchboard apply <install <repo> | relay>` does Switchboard's part, as
//! tmux/apply.sh did for the tmux version: `install` builds the clone with
//! cargo, puts the build in place of the running binary as `switchboard
//! update` puts a release (written beside it, run once, renamed over it)
//! and runs its `setup --refresh`, so Claude sessions started from then on
//! load its skills and hooks; `relay` stops the Slack relay, waits for it
//! to end, and starts it again in its tab. The tmux version's daemon
//! restart, tmux reload, sidebar panes, Neovim plugins, Ghostty config and
//! lib's packages are gone with what they kept running: the one binary and
//! its server hold all of it now. Ported from bin/apply-actions and
//! tmux/apply.sh.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::time::{Duration, Instant};

use regex::Regex;
use serde_json::Value;

use crate::repos::{self, Apply};
use crate::{paths, update, util, worktree};

/// One thing the change needs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Action {
    pub key: String,
    pub label: String,
    pub why: String,
    /// A shell command run from the clone; empty for one only you can do.
    pub command: String,
}

impl Action {
    pub fn line(&self) -> String {
        format!(
            "{}\t{}\t{}\t{}",
            self.key, self.label, self.why, self.command
        )
    }
}

/// What the list is worked out from, gathered by `main` so the rules are
/// tested with any of it.
pub struct Context {
    pub repo: PathBuf,
    /// The files the range changed, and those it added, as git names them.
    pub changed: Vec<String>,
    pub added: Vec<String>,
    /// Whether `repo` is your own clone of Switchboard, `~/switchboard`.
    pub switchboard: bool,
    /// Whether the running binary is a release's.
    pub release: bool,
    /// Whether the Slack relay runs.
    pub relay: bool,
    /// Whether `trusted-dirs` lists the clone.
    pub trusted: bool,
    /// The repository's own `apply` list in repos.json, if it has one.
    pub own: Option<Vec<Apply>>,
}

/// A word quoted for a shell, always, as the script wrote `'...'`.
fn quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

fn why(files: &[String]) -> String {
    match files {
        [] => String::new(),
        [one] => format!("{one} changed"),
        [first, rest @ ..] => format!("{first} and {} more changed", rest.len()),
    }
}

struct List(Vec<Action>);

impl List {
    fn add(&mut self, key: &str, label: &str, files: &[String], command: &str) {
        if files.is_empty() || self.0.iter().any(|a| a.key == key) {
            return;
        }
        self.0.push(Action {
            key: key.into(),
            label: label.into(),
            why: why(files),
            command: command.into(),
        });
    }
}

fn touching(changed: &[String], patterns: &[&str]) -> Vec<String> {
    let patterns: Vec<Regex> = patterns.iter().filter_map(|p| Regex::new(p).ok()).collect();
    changed
        .iter()
        .filter(|f| patterns.iter().any(|p| p.is_match(f)))
        .cloned()
        .collect()
}

// ── Switchboard ─────────────────────────────────────────────────────────────

/// What the binary is built from.
const BUILT_FROM: &[&str] = &[
    r"^src/",
    r"^assets/",
    r"^build\.rs$",
    r"^Cargo\.(toml|lock)$",
    r"^rust-toolchain\.toml$",
];
/// What the relay runs beyond its own part: the modules every part shares.
const RELAY_RUNS: &[&str] = &[
    r"^src/relay/",
    r"^src/platform/",
    r"^src/[^/]+\.rs$",
    r"^Cargo\.(toml|lock)$",
];

fn switchboard(list: &mut List, cx: &Context) {
    // A release is kept current by releases, never by a clone's build.
    if cx.release {
        return;
    }
    let built = touching(&cx.changed, BUILT_FROM);
    list.add(
        "install",
        "Build Switchboard and put it in place of the one running (cargo build --release)",
        &built,
        &format!(
            "switchboard apply install {}",
            quote(&cx.repo.to_string_lossy())
        ),
    );
    if cx.relay && !built.is_empty() {
        list.add(
            "relay",
            "Restart the Slack relay, which runs the build it started with",
            &touching(&cx.changed, RELAY_RUNS),
            "switchboard apply relay",
        );
    }
    list.add(
        "restart",
        "Restart the workspace yourself once the new build is in place, since the server, Notes, M-d's diff and the views run the build they started with: switchboard upgrade restarts the server on it keeping every tab and what runs in it, and Notes, the diff and the views take it when next opened",
        &built,
        "",
    );
}

// ── Any repository ──────────────────────────────────────────────────────────

/// A glob as a regex: `**/` any number of whole folders, a `**` left at the
/// end anything, `*` anything within a folder, `?` one character.
fn glob_regex(glob: &str) -> Option<Regex> {
    let mut out = String::from("^");
    let mut rest = glob;
    while let Some(c) = rest.chars().next() {
        if let Some(after) = rest.strip_prefix("**/") {
            out.push_str("(?:.*/)?");
            rest = after;
            continue;
        }
        if let Some(after) = rest.strip_prefix("**") {
            out.push_str(".*");
            rest = after;
            continue;
        }
        match c {
            '*' => out.push_str("[^/]*"),
            '?' => out.push_str("[^/]"),
            c => out.push_str(&regex::escape(&c.to_string())),
        }
        rest = &rest[c.len_utf8()..];
    }
    out.push('$');
    Regex::new(&out).ok()
}

/// Each lockfile, the package manager it is, and its install, in the order
/// a folder's manager is looked for.
const LOCKFILES: &[(&str, &str, &str)] = &[
    ("pnpm-lock.yaml", "pnpm", "pnpm install --frozen-lockfile"),
    ("package-lock.json", "npm", "npm ci"),
    ("yarn.lock", "yarn", "yarn install --frozen-lockfile"),
    ("bun.lock", "bun", "bun install --frozen-lockfile"),
    ("bun.lockb", "bun", "bun install --frozen-lockfile"),
    ("Gemfile.lock", "Bundler", "bundle install"),
    ("poetry.lock", "Poetry", "poetry install"),
    ("uv.lock", "uv", "uv sync --frozen"),
];
const JS: &[&str] = &["pnpm", "npm", "yarn", "bun"];
/// A package's migrate script, the first of these it has.
const MIGRATE_SCRIPTS: &[&str] = &[
    "db:migrate",
    "migrate",
    "migrate:latest",
    "migrate:up",
    "db:migrate:dev",
    "migrate:dev",
];

/// A path's folder as Node's `path.dirname` gives it: `.` for none.
fn dirname(file: &str) -> String {
    match file.rsplit_once('/') {
        Some(("", _)) => "/".into(),
        Some((dir, _)) => dir.into(),
        None => ".".into(),
    }
}

fn in_dir(dir: &str) -> String {
    if dir == "." {
        String::new()
    } else {
        format!(" in {dir}")
    }
}

fn under(repo: &Path, dir: &str) -> PathBuf {
    if dir == "." {
        repo.to_path_buf()
    } else {
        repo.join(dir)
    }
}

/// The JavaScript package manager whose lockfile is in `dir`.
fn manager_in(dir: &Path) -> Option<&'static str> {
    LOCKFILES
        .iter()
        .find(|(file, manager, _)| JS.contains(manager) && dir.join(file).exists())
        .map(|(_, manager, _)| *manager)
}

fn defaults(list: &mut List, cx: &Context) {
    let runs = cx.switchboard || cx.trusted;
    let untrusted = if runs {
        ""
    } else {
        " yourself (the clone isn't in trusted-dirs)"
    };
    for file in &cx.changed {
        let name = file.rsplit('/').next().unwrap_or(file);
        let Some((_, manager, command)) = LOCKFILES.iter().find(|(f, _, _)| *f == name) else {
            continue;
        };
        let dir = dirname(file);
        // Nothing installed there means nothing out of date (Yarn's
        // Plug'n'Play installs into .pnp.cjs rather than node_modules).
        if JS.contains(manager)
            && !["node_modules", ".pnp.cjs", ".pnp.js"]
                .iter()
                .any(|i| under(&cx.repo, &dir).join(i).exists())
        {
            continue;
        }
        let run = if runs {
            format!(
                "cd {} && {command}",
                quote(&under(&cx.repo, &dir).to_string_lossy())
            )
        } else {
            String::new()
        };
        list.add(
            &format!("install:{dir}"),
            &format!("Install packages with {manager}{}{untrusted}", in_dir(&dir)),
            std::slice::from_ref(file),
            &run,
        );
    }

    let migration = Regex::new(r"(^|/)(migrations?|migrate)/").expect("a regex");
    let migrations: Vec<String> = cx
        .added
        .iter()
        .filter(|f| migration.is_match(f))
        .cloned()
        .collect();
    for file in &migrations {
        // The nearest package.json above it with a migrate script.
        let mut dir = dirname(file);
        let script = loop {
            let scripts = util::read_json(under(&cx.repo, &dir).join("package.json"))
                .and_then(|p| p.get("scripts").cloned())
                .unwrap_or(Value::Null);
            let found = MIGRATE_SCRIPTS
                .iter()
                .find(|s| scripts.get(**s).is_some_and(util::truthy));
            if found.is_some() || dir == "." || dir == "/" {
                break found;
            }
            dir = dirname(&dir);
        };
        match script {
            Some(script) => {
                let manager = manager_in(&under(&cx.repo, &dir))
                    .or_else(|| manager_in(&cx.repo))
                    .unwrap_or("npm");
                let run = if runs {
                    format!(
                        "cd {} && {manager} run {script}",
                        quote(&under(&cx.repo, &dir).to_string_lossy())
                    )
                } else {
                    String::new()
                };
                list.add(
                    &format!("migrate:{dir}"),
                    &format!(
                        "Run the new migrations ({manager} run {script}{}){untrusted}",
                        in_dir(&dir)
                    ),
                    &migrations,
                    &run,
                );
            }
            None => list.add("migrate", "Run the new migrations", &migrations, ""),
        }
    }

    list.add(
        "env",
        "Copy what's new in .env.example into your .env",
        &touching(&cx.changed, &[r"(^|/)\.env\.(example|sample)$"]),
        "",
    );
}

/// What the change needs, in the order to do it.
pub fn actions(cx: &Context) -> Vec<Action> {
    let mut list = List(Vec::new());
    if cx.switchboard {
        switchboard(&mut list, cx);
    }
    match &cx.own {
        Some(own) => {
            for a in own {
                let patterns: Vec<Regex> = a.paths.iter().filter_map(|p| glob_regex(p)).collect();
                let files: Vec<String> = cx
                    .changed
                    .iter()
                    .filter(|f| patterns.iter().any(|p| p.is_match(f)))
                    .cloned()
                    .collect();
                // Your command, but it may run the clone's code (make, a
                // package script).
                let runs = !a.command.is_empty() && cx.trusted;
                let label = if runs || a.command.is_empty() {
                    a.label.clone()
                } else {
                    format!("{} yourself (the clone isn't in trusted-dirs)", a.label)
                };
                let command = if runs {
                    format!("cd {} && {}", quote(&cx.repo.to_string_lossy()), a.command)
                } else {
                    String::new()
                };
                list.add(&format!("own:{}", a.label), &label, &files, &command);
            }
        }
        None => defaults(&mut list, cx),
    }
    list.0
}

/// Whether `dir` is your own clone of Switchboard: `~/switchboard`, by its
/// real path, so another repository shaped like it never counts.
pub fn is_switchboard(dir: &Path) -> bool {
    let own = paths::home_join("switchboard").canonicalize();
    matches!((dir.canonicalize(), own), (Ok(a), Ok(b)) if a == b)
}

fn names(repo: &Path, from: &str, to: &str, added: bool) -> Result<Vec<String>, String> {
    let mut args = vec!["diff", "--name-only"];
    if added {
        args.push("--diff-filter=A");
    }
    args.extend([from, to, "--"]);
    worktree::git_at(repo, &args)
        .map(|out| {
            out.lines()
                .filter(|l| !l.is_empty())
                .map(String::from)
                .collect()
        })
        .map_err(|e| e.to_string())
}

const USAGE: &str = "usage: switchboard apply-actions <repo> <from> <to>";

/// `switchboard apply-actions`.
pub fn main(args: Vec<String>) -> ExitCode {
    let [repo, from, to] = args.as_slice() else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    let repo = std::path::absolute(repo).unwrap_or_else(|_| PathBuf::from(repo));
    let (changed, added) = match (names(&repo, from, to, false), names(&repo, from, to, true)) {
        (Ok(c), Ok(a)) => (c, a),
        (Err(e), _) | (_, Err(e)) => {
            eprintln!("apply-actions: {e}");
            return ExitCode::from(1);
        }
    };
    let cx = Context {
        switchboard: is_switchboard(&repo),
        release: update::is_release_build(),
        relay: crate::relay::cli::running(&crate::relay::Paths::current()),
        trusted: crate::trust::trusted(&paths::home(), &repo),
        own: repos::apply_of(&repo),
        repo,
        changed,
        added,
    };
    for action in actions(&cx) {
        println!("{}", action.line());
    }
    ExitCode::SUCCESS
}

// ── Doing Switchboard's part ────────────────────────────────────────────────

const APPLY_USAGE: &str = "usage: switchboard apply install <repo> | relay";

/// `switchboard apply`.
pub fn apply_main(args: Vec<String>) -> ExitCode {
    let result = match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["install", repo] => install(Path::new(repo)),
        ["relay"] => restart_relay(),
        _ => {
            eprintln!("{APPLY_USAGE}");
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(said) => {
            println!("{said}");
            ExitCode::SUCCESS
        }
        Err(why) => {
            eprintln!("switchboard apply: {why}");
            ExitCode::from(1)
        }
    }
}

/// Builds the clone, puts the build in place of the running binary and runs
/// its setup's refresh. Only your own clone, or one in trusted-dirs, since
/// a build runs the clone's code, and never someone else's PR checked out
/// untrusted (src/trust.rs).
fn install(repo: &Path) -> Result<String, String> {
    let repo = repo
        .canonicalize()
        .map_err(|e| format!("{}: {e}", repo.display()))?;
    // Someone else's PR, checked out untrusted, whatever trusted-dirs says.
    if crate::trust::is_untrusted(&repo) {
        return Err(format!(
            "{} holds a PR wtpr --untrusted set up, whose code runs only in switchboard try --sealed, so it isn't built",
            paths::tilde(&repo)
        ));
    }
    if !is_switchboard(&repo) && !crate::trust::trusted(&paths::home(), &repo) {
        return Err(format!(
            "{} is neither ~/switchboard nor in ~/.config/switchboard/trusted-dirs, so its code isn't built",
            paths::tilde(&repo)
        ));
    }
    let built = Command::new("cargo")
        .args(["build", "--release"])
        .current_dir(&repo)
        .stdin(Stdio::null())
        .stdout(std::io::stderr())
        .status()
        .map_err(|e| format!("cargo: {e}"))?;
    if !built.success() {
        return Err("cargo build --release failed".into());
    }
    let build = repo.join("target/release/switchboard");
    let target = update::install::installed_binary().map_err(|e| e.to_string())?;
    let same = build.canonicalize().ok() == target.canonicalize().ok();
    let version = if same {
        update::install::says_version(&target).map_err(|e| e.to_string())?
    } else {
        let bytes = std::fs::read(&build).map_err(|e| format!("{}: {e}", build.display()))?;
        update::install::replace(&target, &bytes).map_err(|e| format!("{e:#}"))?
    };
    // The new build's parts: its plugin and what is set up written again,
    // as its first attach would, so Claude started from now on loads them.
    let refreshed = Command::new(&target)
        .args(["setup", "--refresh"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    Ok(format!(
        "Put switchboard {version} in place of {}{}. The server, Notes, M-d's diff and the views run the build they started with: switchboard upgrade restarts the server on it keeping every tab and what runs in it, and Notes, the diff and the views take it when next opened.",
        paths::tilde(&target),
        if refreshed {
            ", and brought its setup up to date"
        } else {
            "; its setup couldn't be brought up to date, so run switchboard setup"
        }
    ))
}

/// How long the relay may take to stop: its shutdown may wait on Slack.
const RELAY_STOP: Duration = Duration::from_secs(20);

/// Stops the Slack relay, waits for it to end, and starts it again.
fn restart_relay() -> Result<String, String> {
    use crate::relay::{Paths, cli, lifecycle};
    let relay = Paths::current();
    let Some((pid, _)) = lifecycle::lock_holder(&relay) else {
        return Err("the Slack relay isn't running".into());
    };
    cli::stop(&relay)?;
    let started = Instant::now();
    while util::is_alive(pid) {
        if started.elapsed() > RELAY_STOP {
            return Err("the Slack relay didn't stop within 20 seconds".into());
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    cli::start(&crate::relay::mux::Server::current(), &relay)?;
    Ok("Restarted the Slack relay.".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cx(changed: &[&str]) -> Context {
        Context {
            repo: PathBuf::from("/nowhere/repo"),
            changed: changed.iter().map(|s| s.to_string()).collect(),
            added: Vec::new(),
            switchboard: true,
            release: false,
            relay: false,
            trusted: false,
            own: None,
        }
    }

    fn keys(list: &[Action]) -> Vec<&str> {
        list.iter().map(|a| a.key.as_str()).collect()
    }

    #[test]
    fn in_switchboard_a_change_to_the_code_is_built_and_the_workspace_restarted_and_docs_need_nothing()
     {
        let list = actions(&cx(&[
            "src/sidebar/model.rs",
            "src/sidebar/render.rs",
            "README.md",
        ]));
        assert_eq!(keys(&list), ["install", "restart"]);
        assert_eq!(list[0].why, "src/sidebar/model.rs and 1 more changed");
        assert_eq!(list[0].command, "switchboard apply install '/nowhere/repo'");
        assert_eq!(
            list[1].command, "",
            "restarting the workspace is yours to do"
        );

        let mut relay = cx(&["src/relay/run.rs"]);
        relay.relay = true;
        assert_eq!(keys(&actions(&relay)), ["install", "relay", "restart"]);
        let mut elsewhere = cx(&["src/diff/app.rs"]);
        elsewhere.relay = true;
        assert_eq!(
            keys(&actions(&elsewhere)),
            ["install", "restart"],
            "the relay's code didn't change"
        );

        assert!(actions(&cx(&["README.md", "docs/guide.md", "tests/mux.rs"])).is_empty());
        let mut release = cx(&["src/main.rs"]);
        release.release = true;
        assert!(
            actions(&release).is_empty(),
            "a release isn't replaced by a clone's build"
        );
    }

    #[test]
    fn globs_take_folders_as_the_script_did() {
        let m = |g: &str, f: &str| glob_regex(g).unwrap().is_match(f);
        assert!(m("server/**", "server/deep/b.js"));
        assert!(m("server/**/config.js", "server/config.js"));
        assert!(m("server/**/config.js", "server/a/b/config.js"));
        assert!(!m("server/**/config.js", "server/not-config.js"));
        assert!(m("docs/*.md", "docs/a.md"));
        assert!(!m("docs/*.md", "docs/sub/a.md"));
        assert!(m("a?.txt", "ab.txt"));
        assert!(!m("a.txt", "abtxt"), "a dot is a dot");
    }
}
