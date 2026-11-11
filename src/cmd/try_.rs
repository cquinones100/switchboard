//! `switchboard try [--print | --detached] [dir]` and `switchboard try
//! --clean [dir]` / `--clean-all`: a worktree's Switchboard tried without
//! touching yours. Ported from bin/switchboard-try, which tried a worktree's
//! setup in a tmux of its own; the binary is what is tried now.
//!
//! It builds the worktree's binary with cargo (which does nothing when it is
//! up to date), then runs it under a home of its own, `/tmp/sb-<hash of the
//! worktree's path>`, kept short since a Unix socket's path can't be over
//! 104 bytes on macOS. Everything Switchboard reads is found through `~`
//! (src/paths.rs), so that home gives the try its own settings, state,
//! server and socket, and the binary's own `switchboard setup` parts run
//! there by name, asking nothing (`plugin`, `claude`, `claude-md`,
//! `skills`), so setup is tried too: its plugin, the hooks, skills and
//! instructions its panes' Claude loads, and its Claude Code statusline and
//! permissions, or, with Settings' "Load Switchboard as a plugin" off in
//! the settings it copies, its hooks, CLAUDE.md and skills in its own
//! `~/.claude` as before (src/setup/way.rs). Its `~/.local/bin/switchboard`
//! is the worktree's build, first on the try's PATH, so Claude Code's hooks
//! and statusline in its panes, which run `switchboard` by name, reach the
//! try's server.
//!
//! What isn't Switchboard's comes from your real home, as the script had it:
//!
//! - linked: `~/Library/Keychains`, without which Claude Code isn't signed in
//!   on a Mac and the Keychain's secrets aren't found, and Chrome's list of
//!   its profiles; every dotfile in `~` but those below (`.ssh`,
//!   `.gitconfig`, shell history, `.cargo`); every folder in `~/.config` but
//!   Switchboard's (gh's sign-in among them); `~/.local/share` but
//!   Switchboard's; the programs in `~/.local/bin` that aren't Switchboard's;
//!   Claude's plugins, your own skills and hooks, and its sign-in on Linux
//!   (`~/.claude/.credentials.json`).
//! - copied: your shell's startup files (each ended by moving any XDG,
//!   notes or Switchboard path variable naming your home under the try's),
//!   `~/.claude.json`, `~/.claude/settings.json` and `settings.local.json`,
//!   `~/.config/switchboard` and the clone your CLAUDE.md's rules come from,
//!   through any link, so nothing the try does writes to yours.
//! - fresh: `~/.local/state` (the server's, the relay's), `~/.cache`,
//!   Claude's sessions and transcripts, `~/Notes`.
//!
//! Its settings turn off what would duplicate your real workspace or spend
//! on its own (`TURNED_OFF`: the Slack look and view, meeting, standup and
//! todo notes, banners, the update check, restoring saved tabs); any can be
//! turned back on in its Settings. Linked things are yours: an edit made
//! through them (a git config, gh signing in again) is made to the real one.
//!
//! Since its build and its setup run with your credentials within reach, a
//! worktree is tried only when its code is yours (the script's checks, now
//! src/trust.rs's `yours_or_listed`): its `.git` leads back to it, its
//! commits beyond its origin's branches are found on no other remote and
//! were written and committed under your git email, and it isn't partway
//! through bringing in other commits; else only when
//! `~/.config/switchboard/trusted-dirs` lists it by its exact real path. A
//! PR `wtpr --untrusted` checked out is never tried this way, whatever
//! trusted-dirs says: only with `--sealed` (src/cmd/try_sealed.rs), in a
//! home holding nothing of yours under macOS's sandbox. A home made one way
//! is never laid out the other way until `--clean` has removed it.
//!
//! Alone it attaches to the try in this terminal, which must not be one of
//! Switchboard's panes, since the workspace around it would take every key;
//! `--print` prints the environment to export in another terminal, after
//! which `switchboard` there is the try's; `--detached` starts its server
//! with no client and prints the command prefix that reaches it, for a
//! Claude in the worktree to drive with `send-keys` and read with
//! `capture-pane`. Running it again sets the home up again, keeping what the
//! try saved. `--clean` ends its server and removes its home, for a worktree
//! already removed too by the path it had, and `--clean-all` does that for
//! every try of yours.

use std::collections::HashSet;
use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Map, Value, json};

use crate::mux::proto::{self, ClientMsg, ServerMsg};
use crate::worktree::shell_quote;
use crate::{paths, util};

/// Where the tries' homes are made.
pub(crate) const ROOT: &str = "/tmp";

/// The settings a try starts with turned off, over yours: what would do
/// twice what your real workspace does, or spend on its own.
pub const TURNED_OFF: &[(&str, &str)] = &[
    ("slack_background", "false"),
    ("view_slack", "false"),
    ("meeting_notes", "false"),
    ("standup_note", "false"),
    ("standup_summary", "false"),
    ("todo_note", "false"),
    ("notifications", "false"),
    ("update_check", "\"off\""),
    ("auto_restore", "false"),
];

/// The variables neither the try's setup nor its server gets: your XDG
/// folders and the paths Switchboard and Claude Code read ahead of `~`,
/// which would send the try to your real ones, and those naming the
/// workspace or tmux the command was run in.
const LEFT_OUT: &[&str] = &[
    "SWITCHBOARD",
    "SWITCHBOARD_PANE",
    "SWITCHBOARD_STATE_DIR",
    "SWITCHBOARD_SETTINGS",
    "TMUX",
    "TMUX_PANE",
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
    "XDG_STATE_HOME",
    "XDG_CACHE_HOME",
    "NOTES_DIR",
    "CLAUDE_CONFIG_DIR",
    "ZDOTDIR",
];

/// The path variables a startup file of yours may set to a place in your
/// home, moved under the try's at the end of its copy.
const MOVED: &[&str] = &[
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
    "XDG_STATE_HOME",
    "XDG_CACHE_HOME",
    "NOTES_DIR",
    "SWITCHBOARD_STATE_DIR",
    "SWITCHBOARD_SETTINGS",
    "CLAUDE_CONFIG_DIR",
];

/// Dotfiles in your home that aren't linked: what the try copies or keeps
/// fresh, and what would only get in the way.
const NOT_LINKED: &[&str] = &[
    ".claude",
    ".claude.json",
    ".config",
    ".local",
    ".cache",
    ".zshenv",
    ".zprofile",
    ".zshrc",
    ".zlogin",
    ".bashrc",
    ".bash_profile",
    ".bash_login",
    ".profile",
    ".Trash",
    ".DS_Store",
    ".pgclirc",
    ".tmux.conf",
    ".tmux",
];

/// The shell startup files copied and ended with the try's own lines.
const STARTUP: &[&str] = &[
    ".zshenv",
    ".zprofile",
    ".zshrc",
    ".zlogin",
    ".bashrc",
    ".bash_profile",
    ".bash_login",
    ".profile",
];

/// The first line of a startup file the try wrote.
const STARTUP_MARK: &str = "# switchboard try:";

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Mode {
    Attach,
    Print,
    Detached,
    Clean,
    CleanAll,
}

pub fn main(args: Vec<String>) -> ExitCode {
    // What a sealed try runs of Switchboard's own, in a sandbox that
    // reaches its server alone (try_sealed.rs).
    if args.first().map(String::as_str) == Some("--sealed-call") {
        return super::try_sealed::call_main(&args[1..]);
    }
    let mut mode = Mode::Attach;
    let mut target: Option<PathBuf> = None;
    let mut sealed = false;
    let mut fetch = false;
    let mut run: Vec<String> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--print" => mode = Mode::Print,
            "--detached" => mode = Mode::Detached,
            "--clean" => mode = Mode::Clean,
            "--clean-all" => mode = Mode::CleanAll,
            "--sealed" => sealed = true,
            "--fetch" => fetch = true,
            // Everything after it is the command, run sealed.
            "--run" => {
                run = args[i + 1..].to_vec();
                if run.is_empty() {
                    eprintln!("switchboard try: --run needs a command");
                    return ExitCode::from(2);
                }
                break;
            }
            "-h" | "--help" => {
                usage();
                return ExitCode::SUCCESS;
            }
            flag if flag.starts_with('-') => {
                eprintln!("switchboard try: unknown option {flag}");
                return ExitCode::from(2);
            }
            dir => target = Some(PathBuf::from(dir)),
        }
        i += 1;
    }
    for (given, flag) in [(!run.is_empty(), "--run"), (fetch, "--fetch")] {
        if given && !sealed {
            eprintln!("switchboard try: {flag} works only with --sealed");
            return ExitCode::from(2);
        }
    }
    let result = match mode {
        Mode::CleanAll => clean_all(Path::new(ROOT)),
        Mode::Clean => clean_named(target.as_deref()),
        _ if sealed => super::try_sealed::start(mode, target.as_deref(), fetch, &run),
        _ => start(mode, target.as_deref()),
    };
    match result {
        Ok(code) => code,
        Err(e) => {
            eprintln!("switchboard try: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn usage() {
    println!("Usage: switchboard try [--print | --detached] [dir]");
    println!("       switchboard try --sealed [--fetch] [--print | --detached] [dir]");
    println!("       switchboard try --sealed [--fetch] [dir] --run <command>...");
    println!("       switchboard try --clean [dir] | --clean-all\n");
    println!("Builds a Switchboard worktree (the one you are in, or dir) and runs it under");
    println!("a home of its own, /tmp/sb-<hash>, leaving your workspace alone. Alone it");
    println!("attaches here; --print prints the environment to export in another terminal;");
    println!("--detached starts its server with no client and prints how to drive it.");
    println!("--sealed tries someone else's code (a PR wtpr --untrusted checked out) in a");
    println!("home holding nothing of yours, its build, server and clients in macOS's");
    println!("sandbox; --fetch fetches a Cargo.lock other than your clone's from crates.io;");
    println!("--run runs a command there (its tests). --clean ends its server and removes");
    println!("its home; --clean-all does every try's.");
}

/// The try's home for a worktree: `/tmp/sb-` and the first eight hex digits
/// of the SHA-1 of its path, as the script named it.
pub fn home_of(root: &Path, wt: &Path) -> PathBuf {
    use sha1::{Digest, Sha1};
    use std::os::unix::ffi::OsStrExt;
    let hash = Sha1::digest(wt.as_os_str().as_bytes());
    let hex: String = hash.iter().take(4).map(|b| format!("{b:02x}")).collect();
    root.join(format!("sb-{hex}"))
}

/// The try's socket: the default under its home, since its server runs with
/// that home and no `SWITCHBOARD_STATE_DIR`.
pub fn socket_of(home: &Path) -> PathBuf {
    home.join(".local/state/switchboard/sock")
}

/// Whether a directory is a clone of Switchboard's Rust version: its
/// Cargo.toml names the package, and the skills and server are beside it.
pub fn is_switchboard(dir: &Path) -> bool {
    let named = util::read_text(dir.join("Cargo.toml")).is_some_and(|toml| {
        toml.lines()
            .any(|l| l.replace(' ', "") == "name=\"switchboard\"")
    });
    named && dir.join("assets/skills").is_dir() && dir.join("src/mux").is_dir()
}

// ── Starting a try ──────────────────────────────────────────────────────────

fn start(mode: Mode, target: Option<&Path>) -> Result<ExitCode> {
    let from = match target {
        Some(dir) => dir.to_path_buf(),
        None => std::env::current_dir().context("can't read the directory you are in")?,
    };
    let top = git(&from, &["rev-parse", "--show-toplevel"]).ok_or_else(|| {
        anyhow!("not in a git repository: run it in a Switchboard worktree, or name one")
    })?;
    let wt = PathBuf::from(top)
        .canonicalize()
        .context("the worktree can't be read")?;
    if !is_switchboard(&wt) {
        bail!("{} isn't a Switchboard clone", wt.display());
    }
    // Refused before anything is built: attached inside one of
    // Switchboard's panes, the try could never be used.
    if mode == Mode::Attach && std::env::var_os("SWITCHBOARD").is_some_and(|s| !s.is_empty()) {
        bail!(
            "you're in one of Switchboard's panes, whose workspace would take every key of the try's; run it in a terminal of its own, or there eval \"$(switchboard try --print)\" and then switchboard, or use --detached to drive it with commands"
        );
    }
    // Someone else's PR checked out untrusted runs only sealed, whatever
    // trusted-dirs says.
    if crate::trust::is_untrusted(&wt) {
        bail!(
            "{} holds a PR wtpr --untrusted set up, whose code runs only sealed: switchboard try --sealed{}",
            wt.display(),
            if crate::platform::sandbox_ready().is_ok() {
                ""
            } else {
                ", which needs macOS"
            }
        );
    }
    let real = paths::home();
    crate::trust::yours_or_listed(&real, &wt).map_err(|why| anyhow!("{why}"))?;
    let home = home_of(Path::new(ROOT), &wt);
    // A sealed home is the try's code's to change: never laid out again
    // with your files linked in.
    if super::try_sealed::is_sealed(&home) {
        bail!(
            "{} was made with --sealed; run switchboard try --clean {} first",
            home.display(),
            shell_quote(&wt.to_string_lossy())
        );
    }
    let bin = build(&wt)?;
    let layout = Try {
        real: real.clone(),
        home: home.clone(),
        wt: wt.clone(),
    };
    eprintln!("Setting up {} from {}…", home.display(), wt.display());
    for note in layout.lay_out(&bin)? {
        eprintln!("switchboard try: {note}");
    }
    let env = Env::new(
        &real,
        &home,
        &wt,
        &std::env::var("PATH").unwrap_or_default(),
        std::env::vars_os().filter_map(|(k, _)| k.into_string().ok()),
    );
    run_setup(&env, &home);
    let try_bin = home.join(".local/bin/switchboard");
    match mode {
        Mode::Print => {
            print!("{}", env.exports());
            Ok(ExitCode::SUCCESS)
        }
        Mode::Detached => {
            start_detached(&env, &try_bin, &home, &wt)?;
            let prefix = env.prefix(&try_bin);
            println!("Its server is running with no client. Drive and read it with:");
            println!("  {prefix} send-keys -l 'some text' && {prefix} send-keys Enter");
            println!("  {prefix} capture-pane");
            println!("  {prefix} list");
            println!(
                "switchboard try --clean {} ends it.",
                shell_quote(&wt.to_string_lossy())
            );
            Ok(ExitCode::SUCCESS)
        }
        _ => {
            let mut command = Command::new(&try_bin);
            env.apply(&mut command);
            command.current_dir(&wt);
            let e = command.exec();
            bail!("couldn't run {}: {e}", try_bin.display())
        }
    }
}

/// Builds the worktree's binary with cargo, which does nothing when it is up
/// to date, and gives its path, from cargo's own account of what it built.
/// Cargo's progress goes to the terminal as it comes.
fn build(wt: &Path) -> Result<PathBuf> {
    eprintln!("Building {}…", wt.display());
    let output = Command::new("cargo")
        .args(["build", "--message-format=json-render-diagnostics"])
        .current_dir(wt)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .output()
        .map_err(|e| anyhow!("couldn't run cargo: {e}"))?;
    if !output.status.success() {
        bail!("cargo build failed in {}", wt.display());
    }
    built_binary(&String::from_utf8_lossy(&output.stdout))
        .ok_or_else(|| anyhow!("cargo built no switchboard binary in {}", wt.display()))
}

/// The `switchboard` executable cargo's JSON messages say it built.
fn built_binary(messages: &str) -> Option<PathBuf> {
    messages
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|m| m["reason"] == "compiler-artifact" && m["target"]["name"] == "switchboard")
        .filter_map(|m| m["executable"].as_str().map(PathBuf::from))
        .next_back()
}

/// The binary's own setup parts, run by name in the try's home, so its
/// plugin (or, the old way, its hooks, CLAUDE.md and skills), statusline
/// and permissions are the worktree's. What they say is shown indented; a
/// failure is said, and the try goes on.
fn run_setup(env: &Env, home: &Path) {
    let bin = home.join(".local/bin/switchboard");
    let mut command = Command::new(&bin);
    command
        .args(["setup", "plugin", "claude", "claude-md", "skills"])
        .current_dir(home)
        .stdin(Stdio::null());
    env.apply(&mut command);
    match command.output() {
        Ok(out) => {
            for line in String::from_utf8_lossy(&out.stdout)
                .lines()
                .chain(String::from_utf8_lossy(&out.stderr).lines())
            {
                eprintln!("  {line}");
            }
            if !out.status.success() {
                eprintln!("switchboard try: its setup reported a failure above; starting anyway.");
            }
        }
        Err(e) => eprintln!("switchboard try: couldn't run its setup ({e}); starting anyway."),
    }
}

/// Starts the try's server with no client, unless one answers already, and
/// gives it a session by attaching and leaving at once, as `tmux new -d`
/// does; the server lives on with no client, as it does after a detach.
fn start_detached(env: &Env, bin: &Path, home: &Path, wt: &Path) -> Result<()> {
    let socket = socket_of(home);
    if UnixStream::connect(&socket).is_ok() {
        return Ok(());
    }
    let state = home.join(".local/state/switchboard");
    fs::create_dir_all(&state)?;
    fs::set_permissions(&state, fs::Permissions::from_mode(0o700))?;
    let log = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(state.join("server.log"))?;
    let mut command = Command::new(bin);
    command
        .arg("server")
        .current_dir(wt)
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log);
    env.apply(&mut command);
    // SAFETY: setsid is async-signal-safe, so it may run between fork and
    // exec; it fails only for a process group leader, which a child just
    // forked is not. A session of its own keeps the server from being hung
    // up with the terminal that started it, as socket.rs's start_server does.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    command.spawn().context("couldn't start its server")?;
    let deadline = Instant::now() + Duration::from_secs(10);
    while UnixStream::connect(&socket).is_err() {
        if Instant::now() >= deadline {
            bail!(
                "its server didn't start; {} says why",
                state.join("server.log").display()
            );
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    make_session(&socket)
}

/// Gives the server on `socket` a session by attaching and leaving at once,
/// as `tmux new -d` does. A sealed try's runs in a sandbox of its own
/// (try_sealed.rs's `call`), since its server is the try's code.
pub(crate) fn make_session(socket: &Path) -> Result<()> {
    let mut stream = UnixStream::connect(socket).context("its server doesn't answer")?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.write_all(&proto::encode_client(&ClientMsg::Attach {
        session: None,
        cols: 200,
        rows: 50,
        term: "xterm-256color".into(),
        // No terminal: its cell's size in pixels is unknown.
        xpixel: 0,
        ypixel: 0,
        cwd: None,
    }))?;
    // The first thing drawn says the session is there.
    match proto::read_server(&mut stream) {
        Ok(Some(ServerMsg::Output(_))) => {}
        Ok(Some(ServerMsg::Exit { message })) => {
            bail!(
                "its server ended the session: {}",
                message.unwrap_or_default()
            )
        }
        _ => bail!("its server didn't open a session"),
    }
    let _ = stream.write_all(&proto::encode_client(&ClientMsg::Detach));
    Ok(())
}

// ── The environment ─────────────────────────────────────────────────────────

/// What the try's setup, server and client run with: the variables left
/// out, and its home and PATH.
#[derive(Debug, PartialEq, Eq)]
pub struct Env {
    pub unset: Vec<String>,
    pub set: Vec<(String, String)>,
}

impl Env {
    /// The try's environment, from your PATH and the names of the variables
    /// set now: those in `LEFT_OUT`, and the ones a Claude session or other
    /// agent passes on (`CLAUDE*`, `AI_AGENT`), which would make the try's
    /// Claude take itself for that session's child, and any other
    /// `SWITCHBOARD_*`, left out, but `SWITCHBOARD_CLAUDE_MANAGED_SETTINGS`,
    /// which names the computer's policy for Claude Code rather than a place
    /// of yours (the tests set it, so a try's setup never reads the real
    /// one). `SWITCHBOARD_CHECKOUT` names the worktree `wt`, so M-p's
    /// restart in the try builds the worktree under test, not its main
    /// clone (src/mux/latest.rs).
    pub fn new(
        real: &Path,
        home: &Path,
        wt: &Path,
        path: &str,
        vars: impl Iterator<Item = String>,
    ) -> Env {
        let mut unset: Vec<String> = LEFT_OUT.iter().map(|v| v.to_string()).collect();
        let mut present: Vec<String> = vars
            .filter(|v| v.starts_with("CLAUDE") || v == "AI_AGENT" || v.starts_with("SWITCHBOARD"))
            .filter(|v| v != "SWITCHBOARD_CLAUDE_MANAGED_SETTINGS")
            .filter(|v| v.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'))
            .filter(|v| !unset.contains(v))
            .collect();
        present.sort();
        present.dedup();
        unset.extend(present);
        Env {
            unset,
            set: vec![
                ("HOME".into(), home.to_string_lossy().into_owned()),
                ("PATH".into(), try_path(real, home, path)),
                (
                    "SWITCHBOARD_CHECKOUT".into(),
                    wt.to_string_lossy().into_owned(),
                ),
            ],
        }
    }

    pub fn apply(&self, command: &mut Command) {
        for name in &self.unset {
            command.env_remove(name);
        }
        command.envs(self.set.iter().map(|(k, v)| (k, v)));
    }

    /// The lines to `eval` in a shell, after which `switchboard` is the try's.
    pub fn exports(&self) -> String {
        let mut out = format!("unset {}\n", self.unset.join(" "));
        for (name, value) in &self.set {
            out.push_str(&format!("export {name}={}\n", shell_quote(value)));
        }
        out.push_str("# switchboard now attaches to the try; switchboard try --clean ends it\n");
        out
    }

    /// The command prefix that runs `bin` in the try's environment.
    pub fn prefix(&self, bin: &Path) -> String {
        let mut words = vec!["env".to_string()];
        for name in &self.unset {
            words.push(format!("-u {name}"));
        }
        for (name, value) in &self.set {
            words.push(format!("{name}={}", shell_quote(value)));
        }
        words.push(shell_quote(&bin.to_string_lossy()));
        words.join(" ")
    }
}

/// Your PATH for the try: its `~/.local/bin` first, where its `switchboard`
/// is, and each folder under your home moved under the try's where the try
/// has it, so a program run by name is the try's; one it neither links nor
/// copies (`~/go/bin`) stays yours rather than naming nothing.
pub fn try_path(real: &Path, home: &Path, path: &str) -> String {
    let own = home.join(".local/bin");
    let mut parts = vec![own.to_string_lossy().into_owned()];
    let real_text = real.to_string_lossy();
    for part in path.split(':').filter(|p| !p.is_empty()) {
        // Written with a ~ (as /etc/paths.d can give it), which is yours.
        let mut part = match part.strip_prefix("~/") {
            Some(rest) => format!("{real_text}/{rest}"),
            None => part.to_string(),
        };
        if let Some(rest) = part.strip_prefix(real_text.as_ref())
            && (rest.is_empty() || rest.starts_with('/'))
        {
            let moved = format!("{}{rest}", home.display());
            if Path::new(&moved).exists() {
                part = moved;
            }
        }
        if !parts.contains(&part) {
            parts.push(part);
        }
    }
    parts.join(":")
}

// ── The home ────────────────────────────────────────────────────────────────

/// A try's home, your real one, and the worktree it tries.
pub struct Try {
    pub real: PathBuf,
    pub home: PathBuf,
    pub wt: PathBuf,
}

impl Try {
    /// Sets the home up from the worktree and your real home, as described
    /// at the top, `bin` being the worktree's build; gives what it couldn't
    /// do, to say. Running it again keeps what the try saved.
    pub fn lay_out(&self, bin: &Path) -> Result<Vec<String>> {
        let (real, home) = (&self.real, &self.home);
        let mut notes = Vec::new();
        for dir in [
            "Library",
            ".config",
            ".local/share",
            ".local/bin",
            ".local/state",
            ".claude/skills",
            ".claude/hooks",
        ] {
            fs::create_dir_all(home.join(dir))
                .with_context(|| format!("could not make {}", home.display()))?;
        }
        fs::set_permissions(home, fs::Permissions::from_mode(0o700))?;
        self.drop_old_links();
        replace_link(&self.wt, &home.join("switchboard"))?;
        replace_link(bin, &home.join(".local/bin/switchboard"))?;

        link(
            &real.join("Library/Keychains"),
            &home.join("Library/Keychains"),
        );
        // Chrome's list of its profiles, which Settings' Default Chrome
        // profile reads; the profiles themselves stay out.
        let chrome = "Library/Application Support/Google/Chrome/Local State";
        if real.join(chrome).exists() {
            let _ = fs::create_dir_all(home.join(chrome).parent().unwrap_or(home));
            link(&real.join(chrome), &home.join(chrome));
        }
        for entry in entries(real) {
            let name = file_name(&entry);
            if name.starts_with('.') && !NOT_LINKED.contains(&name.as_str()) {
                link(&entry, &home.join(&name));
            }
        }
        for entry in entries(&real.join(".config")) {
            let name = file_name(&entry);
            if !["switchboard", "editor-configs"].contains(&name.as_str()) {
                link(&entry, &home.join(".config").join(&name));
            }
        }
        for entry in entries(&real.join(".local/share")) {
            let name = file_name(&entry);
            if !["switchboard", "editor-configs", "tmux"].contains(&name.as_str()) {
                link(&entry, &home.join(".local/share").join(&name));
            }
        }
        for entry in entries(&real.join(".local/bin")) {
            if !self.switchboards_program(&entry) {
                link(&entry, &home.join(".local/bin").join(file_name(&entry)));
            }
        }
        for kind in ["skills", "hooks"] {
            for entry in entries(&real.join(".claude").join(kind)) {
                if !switchboards_skill(&entry, kind) {
                    link(
                        &entry,
                        &home.join(".claude").join(kind).join(file_name(&entry)),
                    );
                }
            }
        }
        link(&real.join(".claude/plugins"), &home.join(".claude/plugins"));
        // Claude Code's sign-in on Linux, which a Mac keeps in the Keychain.
        link(
            &real.join(".claude/.credentials.json"),
            &home.join(".claude/.credentials.json"),
        );

        self.startup_files(&mut notes);
        copy_missing(&real.join(".claude.json"), &home.join(".claude.json"));
        for file in ["settings.json", "settings.local.json"] {
            copy_missing(
                &real.join(".claude").join(file),
                &home.join(".claude").join(file),
            );
        }
        self.own_files()?;
        self.claude_json()?;
        // Your clone of your CLAUDE.md's repository, copied so the try's
        // setup only pulls it, which may fail, rather than clones it.
        let rules = ".local/share/switchboard/claude-md";
        if !crate::setup::exists(&home.join(rules)) && real.join(rules).join(".git").is_dir() {
            let _ = fs::create_dir_all(home.join(".local/share/switchboard"));
            if let Err(e) = copy_in(&real.join(rules), &home.join(rules)) {
                notes.push(format!(
                    "could not copy your CLAUDE.md's repository ({e:#}), so the try's setup clones it"
                ));
            }
        }
        Ok(notes)
    }

    /// Takes away the links an earlier run made into your home, so each is
    /// made again by today's rules: one kept from before would stay a link
    /// where they now copy or leave a thing out. A link into the worktree is
    /// the try's own and kept.
    fn drop_old_links(&self) {
        let wt_real = self.wt.canonicalize().unwrap_or_else(|_| self.wt.clone());
        for dir in [
            "",
            ".config",
            ".local/share",
            ".local/bin",
            ".claude/skills",
            ".claude/hooks",
            ".claude",
        ] {
            for entry in entries(&self.home.join(dir)) {
                if dir.is_empty() && !file_name(&entry).starts_with('.') {
                    continue;
                }
                let Ok(target) = fs::read_link(&entry) else {
                    continue;
                };
                if target.starts_with(&self.wt) || target.starts_with(&wt_real) {
                    continue;
                }
                if target.starts_with(&self.real) {
                    let _ = fs::remove_file(&entry);
                }
            }
        }
    }

    /// Whether a program in your `~/.local/bin` is Switchboard's: the binary
    /// or a link to it (one named after a command), or a link into a clone,
    /// as setup.sh made. The try has its own.
    fn switchboards_program(&self, entry: &Path) -> bool {
        if file_name(entry) == "switchboard" {
            return true;
        }
        let Ok(target) = fs::read_link(entry) else {
            return false;
        };
        let resolved = entry.canonicalize().ok();
        if resolved
            .as_deref()
            .and_then(Path::file_name)
            .is_some_and(|n| n == "switchboard")
        {
            return true;
        }
        let clones = [
            self.real.join("switchboard"),
            self.real.join("editor-configs"),
        ];
        clones.iter().any(|c| target.starts_with(c))
            || crate::cmd::work::main_clone(&self.wt)
                .zip(resolved)
                .is_some_and(|(clone, r)| r.starts_with(clone))
    }

    /// Your zsh and bash startup files, each copied beside the try's own
    /// (`<file>.yours`, the first time) and the try's file sourcing it and
    /// then moving each path variable it set into your home to the same
    /// place under the try's, kept only when it really is in the try's home
    /// or the worktree, its links followed, since a dotfolder there may be a
    /// link to yours; anything else is unset, so a shell in the try doesn't
    /// take back an export of your notes, settings or XDG folders. A ZDOTDIR
    /// of your own is read as it is.
    fn startup_files(&self, notes: &mut Vec<String>) {
        let (real, home) = (&self.real, &self.home);
        let home_real = home.canonicalize().unwrap_or_else(|_| home.clone());
        let wt_real = self.wt.canonicalize().unwrap_or_else(|_| self.wt.clone());
        let ending = startup_ending(real, &home_real, &wt_real);
        for file in STARTUP {
            let path = home.join(file);
            let yours = home.join(format!("{file}.yours"));
            if crate::setup::is_link(&path) {
                continue;
            }
            if !yours.exists() {
                if !real.join(file).is_file() {
                    continue;
                }
                if let Err(e) = fs::copy(real.join(file), &yours) {
                    notes.push(format!("could not copy your {file} ({e})"));
                    continue;
                }
            }
            let text = format!(
                "{STARTUP_MARK} your {file}, then the try's own ending\n. {}\n{ending}\n",
                shell_quote(&yours.to_string_lossy())
            );
            if let Err(e) = util::write_atomic(&path, text) {
                notes.push(format!("could not write its {file} ({e})"));
            }
        }
    }

    /// `~/.config/switchboard`: yours copied in the first time, made aside
    /// and moved into place only once all of it has worked, with
    /// `TURNED_OFF` over your settings; on a later run only a link that
    /// writes through into your home (other than the plugins') is made a
    /// copy of what it names, the try's own files kept as they are.
    fn own_files(&self) -> Result<()> {
        let dest = self.home.join(".config/switchboard");
        let wt_real = self.wt.canonicalize().unwrap_or_else(|_| self.wt.clone());
        if crate::setup::exists(&dest) {
            for entry in walk_links(&dest, &dest.join("sidebar-plugins")) {
                let Ok(target) = entry.canonicalize() else {
                    continue;
                };
                if !target.starts_with(&self.real) || target.starts_with(&wt_real) {
                    continue;
                }
                fs::remove_file(&entry)?;
                copy_follow(&target, &entry, &mut HashSet::new()).with_context(|| {
                    format!(
                        "could not copy {} into {}; run switchboard try --clean, then try again",
                        target.display(),
                        self.home.display()
                    )
                })?;
            }
            return Ok(());
        }
        let staged = self.home.join(".config/switchboard.partial");
        let _ = fs::remove_dir_all(&staged);
        if let Err(e) = self.stage_own(&staged) {
            let _ = fs::remove_dir_all(&staged);
            return Err(e.context("could not copy ~/.config/switchboard into the try"));
        }
        fs::rename(&staged, &dest)?;
        Ok(())
    }

    /// Your `~/.config/switchboard` copied to `staged`, with `TURNED_OFF`
    /// over your settings.
    fn stage_own(&self, staged: &Path) -> Result<()> {
        match self.real.join(".config/switchboard").canonicalize() {
            Ok(source) => copy_own(&source, staged, true, false)?,
            Err(_) => fs::create_dir_all(staged)?,
        }
        let file = staged.join("settings.json");
        let mut settings = match fs::read_to_string(&file) {
            Ok(text) if !text.trim().is_empty() => match serde_json::from_str(&text) {
                Ok(Value::Object(map)) => map,
                _ => bail!(
                    "could not turn off the try's own Slack look, notes and update check: is ~/.config/switchboard/settings.json valid JSON?"
                ),
            },
            _ => Map::new(),
        };
        turn_off(&mut settings);
        util::write_atomic(
            &file,
            format!("{}\n", util::json_pretty(&Value::Object(settings))),
        )?;
        Ok(())
    }

    /// Your CLAUDE.md's rules named by a `~/` path are in your real home,
    /// which the try's `~` isn't: written out in full, so its setup finds
    /// them.
    fn claude_json(&self) -> Result<()> {
        let file = self.home.join(".config/switchboard/claude.json");
        let Some(Value::Object(mut config)) = util::read_json(&file) else {
            return Ok(());
        };
        let Some(rest) = config
            .get("path")
            .and_then(Value::as_str)
            .and_then(|p| p.strip_prefix("~/"))
        else {
            return Ok(());
        };
        let full = self.real.join(rest).to_string_lossy().into_owned();
        config.insert("path".into(), Value::String(full));
        util::write_atomic(
            &file,
            format!("{}\n", util::json_pretty(&Value::Object(config))),
        )?;
        Ok(())
    }
}

/// Puts `TURNED_OFF` over a settings object.
pub fn turn_off(settings: &mut Map<String, Value>) {
    for (key, value) in TURNED_OFF {
        let value: Value = serde_json::from_str(value).unwrap_or(json!(false));
        settings.insert(key.to_string(), value);
    }
}

/// The lines a startup file of the try's ends with: POSIX, so bash and zsh
/// both read them.
fn startup_ending(real: &Path, home_real: &Path, wt_real: &Path) -> String {
    let real = shell_quote(&real.to_string_lossy());
    let home = shell_quote(&home_real.to_string_lossy());
    let wt = shell_quote(&wt_real.to_string_lossy());
    format!(
        "for __sb_v in {vars}; do eval \"__sb_n=\\${{$__sb_v:-}}\"; [ -n \"$__sb_n\" ] || continue; \
case $__sb_n in {real}|{real}/*) __sb_n=$HOME${{__sb_n#{real}}} ;; esac; \
__sb_r=$(cd -P -- \"$__sb_n\" >/dev/null 2>&1 && pwd -P) || __sb_r=$__sb_n; \
case $__sb_r in {home}|{home}/*|{wt}|{wt}/*) export \"$__sb_v=$__sb_n\" ;; *) unset \"$__sb_v\" ;; esac; \
done; unset __sb_v __sb_n __sb_r",
        vars = MOVED.join(" ")
    )
}

/// Whether a skill or hook of your `~/.claude` is Switchboard's: a folder
/// its setup wrote (with its list of files), or a link into a clone's
/// skills or hooks, as setup.sh made. The try's setup writes its own.
fn switchboards_skill(entry: &Path, kind: &str) -> bool {
    if entry
        .join(crate::setup::skills::MANIFEST)
        .symlink_metadata()
        .is_ok()
        && !crate::setup::is_link(entry)
    {
        return true;
    }
    let Ok(target) = fs::read_link(entry) else {
        return false;
    };
    let text = target.to_string_lossy();
    let text = text.trim_end_matches('/');
    let name = file_name(entry);
    [
        format!("/claude/{kind}/{name}"),
        format!("/assets/{kind}/{name}"),
    ]
    .iter()
    .any(|tail| text.ends_with(tail.as_str()))
}

/// A folder's entries, none when it can't be read.
fn entries(dir: &Path) -> Vec<PathBuf> {
    let mut list: Vec<PathBuf> = fs::read_dir(dir)
        .map(|d| d.filter_map(|e| e.ok()).map(|e| e.path()).collect())
        .unwrap_or_default();
    list.sort();
    list
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Links `dest` to `source`, unless something of the try's own is there
/// already or `source` isn't.
fn link(source: &Path, dest: &Path) {
    if crate::setup::exists(dest) || !crate::setup::exists(source) {
        return;
    }
    let _ = symlink(source, dest);
}

/// Makes `dest` a link to `source`, in place of a link there.
fn replace_link(source: &Path, dest: &Path) -> Result<()> {
    if crate::setup::is_link(dest) {
        fs::remove_file(dest)?;
    } else if crate::setup::exists(dest) {
        bail!("{} is in the way of the try's own link", dest.display());
    }
    symlink(source, dest).with_context(|| format!("could not link {}", dest.display()))
}

/// Copies a file of yours in, following links, unless the try has one.
fn copy_missing(source: &Path, dest: &Path) {
    if !crate::setup::exists(dest) && source.is_file() {
        let _ = fs::copy(source, dest);
    }
}

/// Copies `source` to `dest`, following links, whole or not at all: made as
/// `<dest>.partial` and renamed, since one that fails partway would
/// otherwise be kept and never made again.
fn copy_in(source: &Path, dest: &Path) -> Result<()> {
    if crate::setup::exists(dest) {
        return Ok(());
    }
    let mut partial = dest.as_os_str().to_owned();
    partial.push(".partial");
    let partial = PathBuf::from(partial);
    let _ = fs::remove_dir_all(&partial);
    let _ = fs::remove_file(&partial);
    match copy_follow(source, &partial, &mut HashSet::new()) {
        Ok(()) => Ok(fs::rename(&partial, dest)?),
        Err(e) => {
            let _ = fs::remove_dir_all(&partial);
            let _ = fs::remove_file(&partial);
            Err(e)
        }
    }
}

/// A folder of Switchboard's own copied whole, for a sealed try's crates
/// (on a Mac each file a clone of the other, which costs nothing).
pub(crate) fn copy_tree(source: &Path, dest: &Path) -> Result<()> {
    copy_follow(source, dest, &mut HashSet::new())
}

/// `cp -RL`: a file or folder copied with every link followed. A folder a
/// link leads back into (one already being copied) is left out, as is
/// anything neither a file nor a folder.
fn copy_follow(source: &Path, dest: &Path, within: &mut HashSet<PathBuf>) -> Result<()> {
    let meta = fs::metadata(source).with_context(|| format!("{}", source.display()))?;
    if meta.is_file() {
        fs::copy(source, dest).with_context(|| format!("{}", source.display()))?;
    } else if meta.is_dir() {
        let real = source.canonicalize()?;
        if !within.insert(real.clone()) {
            return Ok(());
        }
        fs::create_dir_all(dest)?;
        for entry in entries(source) {
            copy_follow(&entry, &dest.join(file_name(&entry)), within)?;
        }
        // Its mode last, so a folder of yours that can't be written to
        // could still be filled.
        fs::set_permissions(dest, meta.permissions())?;
        within.remove(&real);
    }
    Ok(())
}

/// `~/.config/switchboard` copied: each link in it followed from where it is
/// in your folder, since a relative one (as stow makes) names something else
/// from the copy. In `sidebar-plugins` it stays a link, written out in full,
/// so a plugin loads what is beside where it really is; anywhere else (a
/// file of yours kept in a dotfiles repository) it is made a copy of what it
/// names, since Settings and setup write through a link to the real file.
/// One naming nothing is dropped.
fn copy_own(source: &Path, dest: &Path, top: bool, plugins: bool) -> Result<()> {
    fs::create_dir_all(dest)?;
    for entry in entries(source) {
        let name = file_name(&entry);
        let to = dest.join(&name);
        let in_plugins = plugins || (top && name == "sidebar-plugins");
        let meta = entry.symlink_metadata()?;
        if meta.file_type().is_symlink() {
            let Ok(target) = entry.canonicalize() else {
                continue;
            };
            if in_plugins {
                symlink(&target, &to)?;
            } else {
                copy_follow(&target, &to, &mut HashSet::new())?;
            }
        } else if meta.is_dir() {
            copy_own(&entry, &to, false, in_plugins)?;
        } else if meta.is_file() {
            fs::copy(&entry, &to)?;
        }
    }
    Ok(())
}

/// Every link under `dir`, but under `skip`.
fn walk_links(dir: &Path, skip: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for entry in entries(dir) {
        if entry == skip {
            continue;
        }
        match entry.symlink_metadata() {
            Ok(m) if m.file_type().is_symlink() => found.push(entry),
            Ok(m) if m.is_dir() => found.extend(walk_links(&entry, skip)),
            _ => {}
        }
    }
    found
}

// ── git ──────────────────────────────────────────────────────────────────────

/// A git command's output in `dir`, trimmed; `None` when it fails.
pub(crate) fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let mut command = Command::new("git");
    command.arg("-C").arg(dir).args(args);
    util::run_command(&mut command)
        .ok()
        .map(|s| s.trim_end().to_string())
}

// ── Cleaning up ─────────────────────────────────────────────────────────────

/// Ends the try's server, if one answers on its socket, and removes its
/// home. Nothing is followed out of it: its links into your home are
/// removed as links. A sealed try's server is stopped from a sandbox
/// (try_sealed.rs), since a link to your own server's socket could stand
/// in for its own.
pub fn clean(home: &Path) -> Result<()> {
    if super::try_sealed::is_sealed(home) {
        return super::try_sealed::clean(home);
    }
    stop_server(&socket_of(home));
    fs::remove_dir_all(home).with_context(|| format!("could not remove {}", home.display()))
}

/// Has the server on `socket` end, if one answers, and waits a little for
/// it to go.
pub(crate) fn stop_server(socket: &Path) {
    if let Ok(mut stream) = UnixStream::connect(socket) {
        let _ = stream.set_read_timeout(Some(Duration::from_secs(3)));
        let _ = stream.write_all(&proto::encode_client(&ClientMsg::Command {
            args: vec!["kill-server".into()],
            cwd: PathBuf::from("/"),
            pane: None,
            stdin: None,
        }));
        let _ = proto::read_server(&mut stream);
        let deadline = Instant::now() + Duration::from_secs(3);
        while UnixStream::connect(socket).is_ok() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

/// `--clean [dir]`: the try of the worktree named, found by its path written
/// as git writes a top level, absolute with its links resolved, whether or
/// not it is still there; else of the one you are in.
fn clean_named(target: Option<&Path>) -> Result<ExitCode> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
    let mut wt = match target {
        Some(dir) => resolve_gone(&cwd.join(dir)),
        None => resolve_gone(Path::new(
            &git(&cwd, &["rev-parse", "--show-toplevel"]).ok_or_else(|| {
                anyhow!("not in a git repository: name the worktree's path, or use --clean-all")
            })?,
        )),
    };
    let root = Path::new(ROOT);
    let mut home = home_of(root, &wt);
    // A path inside a worktree still there names the try of its top level,
    // as trying it did.
    if !home.is_dir()
        && let Some(top) =
            git(&wt, &["rev-parse", "--show-toplevel"]).map(|t| resolve_gone(Path::new(&t)))
        && home_of(root, &top).is_dir()
    {
        home = home_of(root, &top);
        wt = top;
    }
    if !home.is_dir() && !super::try_sealed::is_sealed(&home) {
        println!("Nothing to clean for {}.", wt.display());
        return Ok(ExitCode::SUCCESS);
    }
    clean(&home)?;
    println!("Removed {} and stopped its server.", home.display());
    Ok(ExitCode::SUCCESS)
}

/// A path made absolute with its links resolved as far as it is still
/// there, the rest added back as written.
fn resolve_gone(path: &Path) -> PathBuf {
    let mut at = path.to_path_buf();
    let mut rest = Vec::new();
    while !at.is_dir() {
        match (at.file_name().map(|n| n.to_os_string()), at.parent()) {
            (Some(name), Some(parent)) => {
                rest.push(name);
                at = parent.to_path_buf();
            }
            _ => break,
        }
    }
    let mut out = at.canonicalize().unwrap_or(at);
    for name in rest.into_iter().rev() {
        out.push(name);
    }
    out
}

/// Whether a folder in the root is one of your tries' homes: `sb-` and eight
/// hex digits, a folder of yours (not a link), with the link to its
/// worktree.
pub fn is_try_home(dir: &Path) -> bool {
    let name = file_name(dir);
    let hex = name
        .strip_prefix("sb-")
        .is_some_and(|h| h.len() == 8 && h.bytes().all(|b| b.is_ascii_hexdigit()));
    // SAFETY: geteuid only reads the process's user id.
    let me = unsafe { libc::geteuid() };
    hex && dir
        .symlink_metadata()
        .is_ok_and(|m| m.is_dir() && m.uid() == me)
        && crate::setup::is_link(&dir.join("switchboard"))
}

/// `--clean-all`: every try of yours in the root.
fn clean_all(root: &Path) -> Result<ExitCode> {
    let mut cleaned = false;
    let mut homes: Vec<PathBuf> = entries(root)
        .into_iter()
        .filter(|h| is_try_home(h))
        .collect();
    // A sealed try's, by its rules in your cache, whatever its code did to
    // its home.
    for home in super::try_sealed::homes(root) {
        if !homes.contains(&home) {
            homes.push(home);
        }
    }
    for home in homes {
        clean(&home)?;
        println!("Removed {} and stopped its server.", home.display());
        cleaned = true;
    }
    if !cleaned {
        println!("No tries to clean.");
    }
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A temporary folder under /tmp, removed when dropped.
    struct Temp(PathBuf);

    impl Temp {
        fn new(name: &str) -> Temp {
            let dir = std::env::temp_dir().join(format!(
                "sbtry-{name}-{}-{}",
                std::process::id(),
                util::now_ms()
            ));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Temp(dir.canonicalize().unwrap())
        }
    }

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    #[test]
    fn a_home_is_short_and_named_by_the_path() {
        let a = home_of(Path::new("/tmp"), Path::new("/home/me/switchboard"));
        let b = home_of(Path::new("/tmp"), Path::new("/home/me/other"));
        assert_ne!(a, b);
        assert_eq!(a.to_string_lossy().len(), "/tmp/sb-12345678".len());
        assert!(socket_of(&a).to_string_lossy().len() < 104);
    }

    #[test]
    fn its_settings_turn_off_what_your_workspace_does() {
        let schema = crate::settings::schema();
        let mut settings = Map::new();
        settings.insert("slack_background".into(), json!(true));
        settings.insert("github".into(), json!(false));
        turn_off(&mut settings);
        for (key, _) in TURNED_OFF {
            let option = schema.iter().find(|o| o.key == *key).expect("a setting");
            assert!(option.valid(&settings[*key]), "{key} takes its value");
        }
        assert_eq!(settings["slack_background"], json!(false));
        assert_eq!(settings["update_check"], json!("off"));
        assert_eq!(settings["auto_restore"], json!(false));
        assert_eq!(settings["github"], json!(false), "yours are kept");
    }

    #[test]
    fn its_environment_leaves_out_your_paths_and_the_session_it_came_from() {
        let env = Env::new(
            Path::new("/home/me"),
            Path::new("/tmp/sb-1"),
            Path::new("/code/wt"),
            "/home/me/.cargo/bin:/usr/bin:~/go/bin",
            [
                "CLAUDECODE",
                "AI_AGENT",
                "SWITCHBOARD_GITHUB_API",
                "SWITCHBOARD_CLAUDE_MANAGED_SETTINGS",
                "EDITOR",
            ]
            .into_iter()
            .map(String::from),
        );
        for name in [
            "SWITCHBOARD",
            "XDG_CONFIG_HOME",
            "NOTES_DIR",
            "CLAUDECODE",
            "AI_AGENT",
        ] {
            assert!(env.unset.iter().any(|v| v == name), "{name} left out");
        }
        assert!(env.unset.iter().any(|v| v == "SWITCHBOARD_GITHUB_API"));
        assert!(!env.unset.iter().any(|v| v == "EDITOR"));
        assert!(
            !env.unset
                .iter()
                .any(|v| v == "SWITCHBOARD_CLAUDE_MANAGED_SETTINGS"),
            "the computer's policy, kept"
        );
        // .cargo isn't in the try, so it stays yours; nor ~/go/bin.
        assert_eq!(
            env.set[1].1,
            "/tmp/sb-1/.local/bin:/home/me/.cargo/bin:/usr/bin:/home/me/go/bin"
        );
        let exports = env.exports();
        assert!(exports.starts_with("unset SWITCHBOARD SWITCHBOARD_PANE "));
        assert!(exports.contains("export HOME=/tmp/sb-1\n"));
        // A restart in the try builds the worktree under test.
        assert!(exports.contains("export SWITCHBOARD_CHECKOUT=/code/wt\n"));
        let prefix = env.prefix(Path::new("/tmp/sb-1/.local/bin/switchboard"));
        assert!(prefix.starts_with("env -u SWITCHBOARD -u SWITCHBOARD_PANE"));
        assert!(prefix.ends_with("HOME=/tmp/sb-1 PATH=/tmp/sb-1/.local/bin:/home/me/.cargo/bin:/usr/bin:/home/me/go/bin SWITCHBOARD_CHECKOUT=/code/wt /tmp/sb-1/.local/bin/switchboard"));
    }

    #[test]
    fn a_folder_the_try_has_is_its_own_on_the_path() {
        let t = Temp::new("path");
        let (real, home) = (t.0.join("real"), t.0.join("home"));
        fs::create_dir_all(home.join("bin")).unwrap();
        let path = try_path(
            &real,
            &home,
            &format!("{0}/bin:{0}/.local/bin", real.display()),
        );
        assert_eq!(
            path,
            format!(
                "{0}/.local/bin:{0}/bin:{1}/.local/bin",
                home.display(),
                real.display()
            )
        );
    }

    #[test]
    fn cargo_names_the_binary_it_built() {
        let messages = concat!(
            "{\"reason\":\"compiler-artifact\",\"target\":{\"name\":\"build-script-build\"},\"executable\":null}\n",
            "{\"reason\":\"compiler-artifact\",\"target\":{\"name\":\"switchboard\"},\"executable\":null}\n",
            "{\"reason\":\"compiler-artifact\",\"target\":{\"name\":\"switchboard\"},\"executable\":\"/w/target/debug/switchboard\"}\n",
            "{\"reason\":\"build-finished\",\"success\":true}\n",
        );
        assert_eq!(
            built_binary(messages),
            Some(PathBuf::from("/w/target/debug/switchboard"))
        );
        assert_eq!(built_binary(""), None);
    }

    /// A real home with something of each kind, and a worktree.
    fn a_real_home(t: &Temp) -> (PathBuf, PathBuf, PathBuf) {
        let real = t.0.join("real");
        let wt = t.0.join("wt");
        write(&real.join(".gitconfig"), "[user]\n");
        write(&real.join(".ssh/id"), "key");
        write(&real.join(".zsh_history"), "ls\n");
        write(&real.join(".zshrc"), "export NOTES_DIR=$HOME/Notes\n");
        write(&real.join(".config/gh/hosts.yml"), "github.com:\n");
        write(
            &real.join(".config/switchboard/settings.json"),
            "{\"github\": false, \"slack_background\": true}\n",
        );
        write(
            &real.join(".config/switchboard/claude.json"),
            "{\"path\": \"~/rules/CLAUDE.md\"}\n",
        );
        write(&real.join("dotfiles/keys.json"), "{}\n");
        symlink(
            real.join("dotfiles/keys.json"),
            real.join(".config/switchboard/keys.json"),
        )
        .unwrap();
        write(&real.join("plugins/mine/index.js"), "\n");
        fs::create_dir_all(real.join(".config/switchboard/sidebar-plugins")).unwrap();
        symlink(
            "../../../plugins/mine",
            real.join(".config/switchboard/sidebar-plugins/mine"),
        )
        .unwrap();
        write(&real.join(".local/share/nvim/x"), "\n");
        write(&real.join(".local/share/switchboard/switchboard.md"), "\n");
        write(&real.join(".local/state/switchboard/layout.json"), "{}\n");
        write(&real.join(".cache/switchboard/x"), "\n");
        write(&real.join(".local/bin/rg"), "#!/bin/sh\n");
        write(&real.join("bin/switchboard"), "#!/bin/sh\n");
        symlink(
            real.join("bin/switchboard"),
            real.join(".local/bin/switchboard"),
        )
        .unwrap();
        symlink(
            real.join("bin/switchboard"),
            real.join(".local/bin/wt-jump"),
        )
        .unwrap();
        write(&real.join(".claude.json"), "{}\n");
        write(
            &real.join(".claude/settings.json"),
            "{\"model\": \"opus\"}\n",
        );
        write(&real.join(".claude/skills/mine/SKILL.md"), "mine\n");
        write(&real.join(".claude/skills/wt/SKILL.md"), "Switchboard's\n");
        write(
            &real.join(".claude/skills/wt/.switchboard-skill"),
            "SKILL.md\n",
        );
        write(&real.join(".claude/hooks/check.sh"), "\n");
        write(&real.join(".claude/plugins/x"), "\n");
        write(&real.join(".claude/projects/p/s.jsonl"), "\n");
        fs::create_dir_all(&wt).unwrap();
        write(&t.0.join("target/switchboard"), "#!/bin/sh\n");
        (real, wt, t.0.join("target/switchboard"))
    }

    fn link_to(path: &Path) -> Option<PathBuf> {
        fs::read_link(path).ok()
    }

    #[test]
    fn the_home_links_what_is_yours_and_copies_what_switchboard_writes() {
        let t = Temp::new("layout");
        let (real, wt, bin) = a_real_home(&t);
        let home = t.0.join("home");
        let layout = Try {
            real: real.clone(),
            home: home.clone(),
            wt: wt.clone(),
        };
        let notes = layout.lay_out(&bin).unwrap();
        assert!(notes.is_empty(), "{notes:?}");

        assert_eq!(link_to(&home.join("switchboard")), Some(wt.clone()));
        assert_eq!(
            link_to(&home.join(".local/bin/switchboard")),
            Some(bin.clone())
        );
        // Linked: your dotfiles, other tools' folders, programs and skills.
        for (at, from) in [
            (".gitconfig", ".gitconfig"),
            (".ssh", ".ssh"),
            (".zsh_history", ".zsh_history"),
            (".config/gh", ".config/gh"),
            (".local/share/nvim", ".local/share/nvim"),
            (".local/bin/rg", ".local/bin/rg"),
            (".claude/skills/mine", ".claude/skills/mine"),
            (".claude/hooks/check.sh", ".claude/hooks/check.sh"),
            (".claude/plugins", ".claude/plugins"),
        ] {
            assert_eq!(link_to(&home.join(at)), Some(real.join(from)), "{at}");
        }
        // Switchboard's own are left for the try's setup to make.
        for at in [
            ".local/bin/wt-jump",
            ".claude/skills/wt",
            ".local/share/switchboard",
            ".local/state/switchboard",
            ".cache",
            ".claude/projects",
        ] {
            assert!(!home.join(at).exists(), "{at} isn't there");
        }
        // Copied: Claude's files and yours, through their links.
        for at in [
            ".claude.json",
            ".claude/settings.json",
            ".config/switchboard/keys.json",
        ] {
            let m = home.join(at).symlink_metadata().unwrap();
            assert!(m.is_file(), "{at} is a file of the try's");
        }
        assert_eq!(
            link_to(&home.join(".config/switchboard/sidebar-plugins/mine")),
            Some(real.join("plugins/mine")),
            "a plugin stays a link, written out in full"
        );
        let settings = util::read_json(home.join(".config/switchboard/settings.json")).unwrap();
        assert_eq!(settings["github"], json!(false));
        assert_eq!(settings["slack_background"], json!(false));
        assert_eq!(settings["update_check"], json!("off"));
        let claude = util::read_json(home.join(".config/switchboard/claude.json")).unwrap();
        assert_eq!(
            claude["path"],
            json!(real.join("rules/CLAUDE.md").to_string_lossy())
        );
        // The shell's startup file sources yours, then moves its paths.
        let zshrc = fs::read_to_string(home.join(".zshrc")).unwrap();
        assert!(zshrc.starts_with(STARTUP_MARK));
        assert!(zshrc.contains(".zshrc.yours"));
        assert!(zshrc.contains("NOTES_DIR"));
        assert!(home.join(".zshrc.yours").is_file());

        // Again: what the try saved is kept, and its links made again.
        fs::write(
            home.join(".config/switchboard/settings.json"),
            "{\"github\": true}\n",
        )
        .unwrap();
        layout.lay_out(&bin).unwrap();
        let settings = util::read_json(home.join(".config/switchboard/settings.json")).unwrap();
        assert_eq!(settings, json!({"github": true}));
        assert_eq!(
            link_to(&home.join(".gitconfig")),
            Some(real.join(".gitconfig"))
        );
    }

    #[test]
    fn the_startup_ending_moves_a_path_into_the_try_and_drops_one_into_yours() {
        if util::which("sh").is_none() {
            return;
        }
        let t = Temp::new("ending");
        let (real, home, wt) = (t.0.join("real"), t.0.join("home"), t.0.join("wt"));
        for d in [&real, &home, &wt] {
            fs::create_dir_all(d).unwrap();
        }
        fs::create_dir_all(real.join("notes")).unwrap();
        symlink(real.join("notes"), home.join("linked")).unwrap();
        let ending = startup_ending(&real, &home, &wt);
        let script = format!(
            "export XDG_CONFIG_HOME={r}/.config NOTES_DIR={h}/linked CLAUDE_CONFIG_DIR=/elsewhere\n{ending}\necho \"$XDG_CONFIG_HOME|${{NOTES_DIR-unset}}|${{CLAUDE_CONFIG_DIR-unset}}\"",
            r = real.display(),
            h = home.display()
        );
        let out = Command::new("sh")
            .args(["-c", &script])
            .env("HOME", &home)
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&out.stdout).trim(),
            format!("{}/.config|unset|unset", home.display())
        );
    }

    #[test]
    fn clean_removes_the_home_and_none_of_yours() {
        let t = Temp::new("clean");
        let real = t.0.join("real");
        write(&real.join(".gitconfig"), "[user]\n");
        let home = t.0.join("sb-0123abcd");
        fs::create_dir_all(&home).unwrap();
        symlink(real.join(".gitconfig"), home.join(".gitconfig")).unwrap();
        symlink(&real, home.join("switchboard")).unwrap();
        assert!(is_try_home(&home));
        assert!(!is_try_home(&real));
        clean_all(&t.0).unwrap();
        assert!(!home.exists());
        assert!(real.join(".gitconfig").is_file(), "yours is left alone");
    }

    #[test]
    fn a_removed_worktree_is_named_by_the_path_it_had() {
        let t = Temp::new("gone");
        assert_eq!(
            resolve_gone(&t.0.join("gone/deeper")),
            t.0.join("gone/deeper")
        );
    }
}
