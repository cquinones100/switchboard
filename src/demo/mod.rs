//! `switchboard demo`: the workspace on made-up data, for seeing what it
//! does before setting it up, for showing it to someone, and for testing a
//! view without anyone's screen (`switchboard headless`). New here; the tmux
//! version had nothing like it, and `switchboard try` (src/cmd/try_.rs),
//! the nearest thing, links your own credentials in, so its views show your
//! real data.
//!
//! It lays out a home of its own, `/tmp/sb-demo-<uid>` (made 0700, refused
//! when it is a link or someone else's), that links nothing of yours: no
//! Keychain, gh sign-in, `~/.config`, Claude sign-in, dotfiles or
//! `~/.local/share`. Its server runs with an environment made from nothing
//! but the terminal's own variables (`env`), its home and PATH, and the
//! addresses of its services, so no token or agent socket of yours comes
//! along. It runs the binary it is, building nothing, so it works from a
//! release as well as a clone.
//!
//! - `services`: strict stand-ins for GitHub, Linear and Slack on
//!   127.0.0.1, shared with the tests; `fixtures`: the made-up data they
//!   answer with. `switchboard demo-services` serves them in a process of
//!   its own, beside the server, which reaches them through
//!   `SWITCHBOARD_GITHUB_API`, `SWITCHBOARD_GITHUB_GRAPHQL`,
//!   `SWITCHBOARD_LINEAR_API` and `SWITCHBOARD_SLACK_API`, with made-up
//!   tokens: GitHub's from its `gh auth token`, the others in
//!   `LINEAR_API_KEY` and in its own `secrets.json`.
//! - `stand_in`: the programs on its PATH in place of `claude`, `gh`, the
//!   openers, the clipboard, Chrome, `osascript` and `security`, each a
//!   script running `switchboard demo-stand-in <name>`: a link, a file or
//!   the clipboard goes to the demo's own `desktop-log` and `clipboard`
//!   rather than your desktop, `security` reads the demo's `secrets.json`
//!   rather than your Keychain, and `claude` is `agent`.
//! - `agent`: a stand-in Claude that plays a script a tab, sending
//!   Switchboard's hooks as Claude Code does, so the sidebar shows tabs
//!   working, waiting on you and done. It reaches no model.
//! - `tour`: the steps `switchboard demo --tour` walks with a headless
//!   client attached as Ghostty, saving a picture of each with the
//!   server's images in it.
//! - `usage`: a made-up week of spend by tab, the rate limits and the
//!   request audit, for the Usage view.
//!
//! The connectors' kept answers (`calendar.json`, `slack.json`,
//! `slack-threads.json`) are written
//! as `/today` and `/slack-updates` save them, and the Slack look that would
//! ask a headless Claude again is off in its settings. Every name, person,
//! repository, ticket and message is made up (`acme/`, `ABC-…`, people by
//! first name).

pub mod agent;
pub mod fixtures;
pub mod services;
pub mod stand_in;
pub mod tour;
pub mod usage;

use std::fs;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Map, Value, json};

use crate::mux::proto::ClientMsg;
use crate::util;

/// The settings the demo starts with: every view on, and what would reach
/// a service or a model it hasn't got off.
pub const SETTINGS: &[(&str, &str)] = &[
    ("ux2", "true"),
    ("update_check", "\"off\""),
    ("upgrade_after_update", "false"),
    ("auto_restore", "false"),
    ("keep_awake", "false"),
    ("slack_background", "false"),
    ("search_slack", "false"),
    ("standup_note", "false"),
    ("standup_summary", "false"),
    ("haiku_summaries", "false"),
    ("turn_summaries", "false"),
    ("diff_story", "false"),
    ("diff_ask", "false"),
    ("photos", "false"),
    ("image_cache", "false"),
    ("relay_draft_check", "false"),
    ("build_before_restart", "false"),
    ("chrome_profiles", "false"),
    ("tips", "false"),
];

/// How often the services' process writes the day's data again: inside
/// the minute after which opening the Slack view asks for a fresh look
/// (`day::serve`'s `SLACK_NOW_MS`), let alone the fifteen after which it
/// counts as stale.
const DAY_EVERY: Duration = Duration::from_secs(45);

/// The programs the demo stands in for, on its PATH.
pub const STAND_INS: &[&str] = &[
    "claude",
    "gh",
    "open",
    "xdg-open",
    "gio",
    "wslview",
    "explorer.exe",
    "pbcopy",
    "pbpaste",
    "wl-copy",
    "wl-paste",
    "xclip",
    "osascript",
    "notify-send",
    "google-chrome",
    "google-chrome-stable",
    "chromium",
    "chromium-browser",
    "caffeinate",
    "security",
    "qlmanage",
];

/// Where the demos' homes are made: `/tmp`, or `SWITCHBOARD_DEMO_ROOT`
/// (the tests' own folder).
pub fn root() -> PathBuf {
    match std::env::var_os("SWITCHBOARD_DEMO_ROOT") {
        Some(r) if !r.is_empty() => PathBuf::from(r),
        _ => PathBuf::from("/tmp"),
    }
}

/// Your demo's home in `root`: `sb-demo-<uid>`.
pub fn home_in(root: &Path) -> PathBuf {
    // SAFETY: geteuid only reads the process's user id.
    let uid = unsafe { libc::geteuid() };
    root.join(format!("sb-demo-{uid}"))
}

/// The demo's server's socket.
pub fn socket_of(home: &Path) -> PathBuf {
    home.join(".local/state/switchboard/sock")
}

/// The demo's own state: the services' addresses, the desktop log, the
/// clipboard.
pub fn own_state(home: &Path) -> PathBuf {
    home.join(".local/state/demo")
}

/// Where the stand-ins write what would have reached your desktop.
pub fn desktop_log(home: &Path) -> PathBuf {
    own_state(home).join("desktop-log")
}

/// Whether `home` is fit to be the demo's: absent, or a folder of yours
/// that isn't a link.
pub fn check_home(home: &Path) -> Result<()> {
    let Ok(meta) = home.symlink_metadata() else {
        return Ok(());
    };
    // SAFETY: geteuid only reads the process's user id.
    let me = unsafe { libc::geteuid() };
    if meta.file_type().is_symlink() {
        bail!("{} is a link; remove it to run the demo", home.display());
    }
    if !meta.is_dir() || meta.uid() != me {
        bail!(
            "{} isn't a folder of yours; remove it to run the demo",
            home.display()
        );
    }
    Ok(())
}

/// Whether the demo's home has been laid out.
pub fn laid_out(home: &Path) -> bool {
    home.join(".local/state/demo/laid-out").is_file()
}

/// Lays the demo's home out from nothing, for the binary at `bin`.
pub fn lay_out(home: &Path, bin: &Path) -> Result<()> {
    check_home(home)?;
    if home.exists() {
        fs::remove_dir_all(home).with_context(|| format!("couldn't remove {}", home.display()))?;
    }
    fs::DirBuilder::new()
        .mode(0o700)
        .create(home)
        .with_context(|| format!("couldn't make {}", home.display()))?;
    for dir in [
        ".config/switchboard",
        ".local/bin",
        ".local/share",
        ".local/state/switchboard",
        ".local/state/demo",
        ".claude/sessions",
        ".cache",
        "code",
        "Notes/Demo Plans",
    ] {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(home.join(dir))?;
    }
    let mut settings = Map::new();
    for (key, value) in SETTINGS {
        settings.insert(key.to_string(), serde_json::from_str(value)?);
    }
    util::write_atomic(
        home.join(".config/switchboard/settings.json"),
        format!("{}\n", util::json_pretty(&Value::Object(settings))),
    )?;
    // No starter routines: the demo's day is its services' alone, and a
    // Slack routine would open a tab of its own every 15 minutes.
    util::write_atomic(
        home.join(".local/state/switchboard/routines")
            .join(crate::routines::starters::MARKER),
        "",
    )?;
    let secrets = json!({
        "linear-api-key": fixtures::LINEAR_KEY,
        "slack-relay-bot-token": fixtures::SLACK_TOKEN,
    });
    let secrets_file = home.join(".config/switchboard/secrets.json");
    util::write_atomic(&secrets_file, util::json_pretty(&secrets))?;
    fs::set_permissions(&secrets_file, fs::Permissions::from_mode(0o600))?;
    let mut gitconfig = String::from(
        "[user]\n\tname = Alex\n\temail = alex@example.com\n[init]\n\tdefaultBranch = main\n[commit]\n\tgpgsign = false\n[core]\n\thooksPath = /dev/null\n",
    );
    for repo in fixtures::REPOS {
        gitconfig.push_str(&format!(
            "[url \"{}\"]\n\tinsteadOf = https://github.com/acme/{}.git\n",
            home.join("code/.origin")
                .join(format!("{}.git", repo.name))
                .display(),
            repo.name
        ));
    }
    util::write_atomic(home.join(".gitconfig"), gitconfig)?;
    // A pane's shell is a login shell, whose system startup files
    // (macOS's path_helper in /etc/zprofile) put the system's folders ahead
    // of the demo's; these run after them and put its stand-ins first again.
    let front = "PATH=\"$HOME/.local/bin:$PATH\"\nexport PATH\n";
    util::write_atomic(
        home.join(".zshrc"),
        format!(
            "# The demo's own: nothing of yours.\n{front}PS1='%F{{cyan}}%1~%f $ '\nexport PS1\n"
        ),
    )?;
    util::write_atomic(
        home.join(".bashrc"),
        format!("# The demo's own: nothing of yours.\n{front}PS1='\\W $ '\n"),
    )?;
    util::write_atomic(
        home.join(".profile"),
        format!("# The demo's own: nothing of yours.\n{front}PS1='$ '\n"),
    )?;
    util::write_atomic(
        home.join(".bash_profile"),
        "# The demo's own: nothing of yours.\n. \"$HOME/.bashrc\"\n",
    )?;
    stand_in::install(home, bin)?;
    for repo in fixtures::REPOS {
        make_repo(home, repo).with_context(|| format!("making the repository {}", repo.name))?;
    }
    notes(home)?;
    let now = util::now_ms();
    day(home, jiff::Zoned::now(), now)?;
    usage::lay_out(&home.join(".local/state/switchboard"), home, now)?;
    util::write_atomic(own_state(home).join("laid-out"), "")?;
    Ok(())
}

/// A git command in the demo's home, with its own config and none of yours.
fn git(home: &Path, dir: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env_clear()
        .env("HOME", home)
        .env("PATH", base_path())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .stdin(Stdio::null())
        .output()
        .context("couldn't run git")?;
    if !out.status.success() {
        bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// A made-up file's contents for a repository.
fn file_of(repo: &str, name: &str, line: &str) -> String {
    format!("// {repo}: {name}\n\nexport const {line};\n")
}

/// A made-up repository in `~/code`, with a short history on main, its
/// origin a bare repository in the home that git reaches by the GitHub
/// address `.gitconfig` sends there, and a worktree for each of its PRs'
/// branches that a tab works in.
fn make_repo(home: &Path, repo: &fixtures::Repo) -> Result<()> {
    let dir = home.join("code").join(repo.name);
    let origin = home.join("code/.origin").join(format!("{}.git", repo.name));
    fs::create_dir_all(&dir)?;
    fs::create_dir_all(&origin)?;
    git(home, &origin, &["init", "-q", "--bare", "-b", "main"])?;
    git(home, &dir, &["init", "-q", "-b", "main"])?;
    git(
        home,
        &dir,
        &[
            "remote",
            "add",
            "origin",
            &format!("https://github.com/acme/{}.git", repo.name),
        ],
    )?;
    // What gh's `repo set-default` writes, which the workspace's lookups
    // read before origin's address.
    git(
        home,
        &dir,
        &[
            "config",
            "remote.origin.gh-resolved",
            &format!("acme/{}", repo.name),
        ],
    )?;
    util::write_atomic(
        dir.join("README.md"),
        format!("# acme/{}\n\n{}.\n", repo.name, repo.about),
    )?;
    util::write_atomic(
        dir.join(".gitignore"),
        ".claude/worktrees/\nnode_modules/\n",
    )?;
    let history = [
        ("src/index.ts", "version = 1", "Start the project"),
        (
            "src/config.ts",
            "port = 8080",
            "Read the port from the config",
        ),
        ("src/log.ts", "level = \"info\"", "Log at info by default"),
    ];
    for (file, line, message) in history {
        let path = dir.join(file);
        fs::create_dir_all(path.parent().unwrap())?;
        util::write_atomic(&path, file_of(repo.name, file, line))?;
        git(home, &dir, &["add", "-A"])?;
        git(home, &dir, &["commit", "-q", "-m", message])?;
    }
    git(home, &dir, &["push", "-q", "-u", "origin", "main"])?;
    for pr in fixtures::PRS
        .iter()
        .filter(|p| p.repo == repo.name && p.state == "OPEN")
    {
        let wt = dir.join(".claude/worktrees").join(pr.branch);
        git(
            home,
            &dir,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                pr.branch,
                &wt.to_string_lossy(),
                "main",
            ],
        )?;
        let file = wt.join("src/feature.ts");
        util::write_atomic(&file, file_of(repo.name, pr.title, "enabled = true"))?;
        git(home, &wt, &["add", "-A"])?;
        git(home, &wt, &["commit", "-q", "-m", pr.title])?;
        // A change not committed yet, for the diff to show.
        util::write_atomic(
            &file,
            format!(
                "{}export const limit = 10;\n",
                file_of(repo.name, pr.title, "enabled = true")
            ),
        )?;
        git(home, &wt, &["push", "-q", "-u", "origin", pr.branch])?;
    }
    Ok(())
}

/// The notes: a checklist note with an item a tab works on, and its plan.
fn notes(home: &Path) -> Result<()> {
    let notes = home.join("Notes");
    util::write_atomic(
        notes.join("Demo Notes.md"),
        "# Demo Notes\n\n\
         - [ ] FEAT: Rate-limit the login endpoint\n  Plan: [[Plan - Rate-limit the login endpoint]]\n\
         - [ ] BUG: Password reset emails arrive late\n\
         - [ ] FEAT: Export the billing history as CSV\n\n\
         - [x] BUG: Avatar upload fails on Safari\n",
    )?;
    util::write_atomic(
        notes.join("Demo Plans/Plan - Rate-limit the login endpoint.md"),
        "# Plan - Rate-limit the login endpoint\n\n\
         Item: \"FEAT: Rate-limit the login endpoint\"\n\n\
         ## Steps\n\n\
         1. A limiter of ten attempts a minute for each address, in src/auth/limit.ts.\n\
         2. A 429 with `Retry-After` once it is reached.\n\
         3. Tests for both.\n",
    )?;
    util::write_atomic(
        notes.join("Groceries.md"),
        "# Groceries\n\n- [ ] Coffee\n- [ ] Lemons\n- [ ] Bread\n",
    )?;
    Ok(())
}

/// What the connectors last answered, as `/today` and `/slack-updates`
/// save it, and each connector marked as connected. Written again every
/// `DAY_EVERY` by the services' process, so the views never find it stale
/// and open a tab to refresh it, which the stand-in Claude couldn't: the
/// meetings timed from `meetings`, today at the time of day the demo was
/// laid out, so they are today's and keep their times all day (a meeting
/// moved would show as a calendar change), and the Slack messages
/// from `since`, so each writing is the same messages, aging, never new
/// ones.
fn day(home: &Path, meetings: jiff::Zoned, since: i64) -> Result<()> {
    let state = home.join(".local/state/switchboard");
    let _ = fs::remove_file(state.join("slack.json"));
    let _ = fs::remove_file(crate::day::slack_thread::file(&state));
    // Written whole rather than through /today's save, which keeps only
    // what /today gives (the attendees, agenda and tickets are the feed's),
    // so the Calendar view has the feed's detail to show.
    let calendar = fixtures::calendar(meetings);
    crate::day::calendar::save_today(&state, &calendar.to_string()).map_err(|e| anyhow!(e))?;
    let mut whole = calendar;
    whole["updated"] = json!(util::now_ms());
    util::write_atomic(
        crate::day::calendar::saved_file(&state),
        util::json_compact(&whole),
    )?;
    crate::day::slack::save(
        &state,
        &home.join(".local/state/slack-relay"),
        &fixtures::slack_updates(since),
        util::now_ms(),
    )?;
    if let Some(threads) = fixtures::slack_threads(since).as_array() {
        crate::day::slack_thread::save_found(&state, threads, util::now_ms())?;
    }
    let now = util::now_ms();
    util::write_atomic(
        state.join("connectors.json"),
        util::json_compact(&json!({
            "Google Calendar": { "ok": true, "at": now },
            "Slack": { "ok": true, "at": now },
        })),
    )?;
    Ok(())
}

/// The system's own folders, and the one git is in when it is elsewhere
/// (Homebrew's), after the demo's stand-ins.
pub fn base_path() -> String {
    let mut dirs: Vec<String> = ["/usr/bin", "/bin", "/usr/sbin", "/sbin"]
        .iter()
        .map(|d| d.to_string())
        .collect();
    if let Some(git) = util::which("git")
        && let Some(dir) = git.parent()
    {
        let dir = dir.to_string_lossy().into_owned();
        if !dirs.contains(&dir) {
            dirs.push(dir);
        }
    }
    dirs.join(":")
}

/// The services' addresses, as `demo-services` writes them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Services {
    pub github: String,
    pub linear: String,
    pub slack: String,
}

impl Services {
    fn file(home: &Path) -> PathBuf {
        own_state(home).join("services.json")
    }

    pub fn read(home: &Path) -> Option<Services> {
        let v = util::read_json(Self::file(home))?;
        let s = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
        Some(Services {
            github: s("github")?,
            linear: s("linear")?,
            slack: s("slack")?,
        })
    }

    pub fn write(&self, home: &Path) -> Result<()> {
        util::write_atomic(
            Self::file(home),
            util::json_pretty(&json!({
                "github": self.github, "linear": self.linear, "slack": self.slack,
                "pid": std::process::id(),
            })),
        )?;
        Ok(())
    }

    /// Whether they all answer.
    pub fn answer(&self) -> bool {
        [&self.github, &self.linear, &self.slack].iter().all(|url| {
            url.strip_prefix("http://")
                .is_some_and(|addr| std::net::TcpStream::connect(addr).is_ok())
        })
    }
}

/// The variables the terminal sets that the demo keeps, all of them about
/// the terminal itself and none naming a place or a token of yours.
const KEPT: &[&str] = &[
    "TERM",
    "COLORTERM",
    "TERM_PROGRAM",
    "TERM_PROGRAM_VERSION",
    "LANG",
    "LC_ALL",
    "LC_CTYPE",
    "USER",
    "LOGNAME",
    "SHELL",
];

/// The demo's environment, made from nothing but `KEPT`, its home, PATH and
/// services.
pub fn env(home: &Path, services: &Services) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = KEPT
        .iter()
        .filter_map(|k| std::env::var(k).ok().map(|v| (k.to_string(), v)))
        .collect();
    let set = [
        ("HOME", home.to_string_lossy().into_owned()),
        (
            "PATH",
            format!("{}:{}", home.join(".local/bin").display(), base_path()),
        ),
        ("TMPDIR", "/tmp/".to_string()),
        (
            "NOTES_DIR",
            home.join("Notes").to_string_lossy().into_owned(),
        ),
        ("LINEAR_API_KEY", fixtures::LINEAR_KEY.into()),
        ("SWITCHBOARD_GITHUB_API", services.github.clone()),
        (
            "SWITCHBOARD_GITHUB_GRAPHQL",
            format!("{}/graphql", services.github),
        ),
        (
            "SWITCHBOARD_LINEAR_API",
            format!("{}/graphql", services.linear),
        ),
        ("SWITCHBOARD_SLACK_API", format!("{}/api", services.slack)),
        ("SWITCHBOARD_DEMO", home.to_string_lossy().into_owned()),
        // A display named, so Linux's clipboard code picks a tool (the
        // demo's `wl-copy` stand-in); it names no real one, and macOS
        // reads no such variable.
        ("WAYLAND_DISPLAY", "switchboard-demo".into()),
    ];
    for (k, v) in set {
        out.retain(|(name, _)| name != k);
        out.push((k.to_string(), v));
    }
    out
}

/// A command run in the demo's environment alone.
pub fn command(program: &Path, env: &[(String, String)]) -> Command {
    let mut c = Command::new(program);
    c.env_clear();
    c.envs(env.iter().map(|(k, v)| (k, v)));
    c
}

/// Starts `demo-services` unless they answer, on the ports they had before
/// when there were some, and gives their addresses.
pub fn start_services(home: &Path, bin: &Path) -> Result<Services> {
    if let Some(known) = Services::read(home)
        && known.answer()
    {
        return Ok(known);
    }
    let log = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(own_state(home).join("services.log"))?;
    let mut c = Command::new(bin);
    c.arg("demo-services")
        .arg(home)
        .env_clear()
        .env("HOME", home)
        .env("PATH", base_path())
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log);
    detach(&mut c);
    let pid_now =
        || util::read_json(Services::file(home)).and_then(|v| v.get("pid").and_then(Value::as_u64));
    let child = c.spawn().context("couldn't start the demo's services")?;
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if pid_now() == Some(u64::from(child.id()))
            && let Some(now) = Services::read(home)
            && now.answer()
        {
            return Ok(now);
        }
        if Instant::now() > deadline {
            bail!(
                "the demo's services didn't start; {} says why",
                own_state(home).join("services.log").display()
            );
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// A process in a session of its own, so it outlives the terminal that
/// started it, as the try's server does.
pub fn detach(c: &mut Command) {
    use std::os::unix::process::CommandExt;
    // SAFETY: setsid is async-signal-safe, so it may run between fork and
    // exec; a child just forked is no process group leader, so it succeeds.
    unsafe {
        c.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

/// Runs the services in this process until it is ended: each answers with
/// the fixtures, keeping what it refused in `refused.log`.
pub fn serve_services(home: &Path) -> Result<()> {
    use services::{Fake, Service};
    let known = Services::read(home);
    let port = |url: Option<&String>| -> u16 {
        url.and_then(|u| u.rsplit(':').next())
            .and_then(|p| p.parse().ok())
            .unwrap_or(0)
    };
    let github = Fake::start_on(
        port(known.as_ref().map(|k| &k.github)),
        Service::GitHub {
            token: fixtures::GITHUB_TOKEN.into(),
        },
        fixtures::github,
    );
    let linear = Fake::start_on(
        port(known.as_ref().map(|k| &k.linear)),
        Service::Linear {
            key: fixtures::LINEAR_KEY.into(),
        },
        fixtures::linear,
    );
    let slack = Fake::start_on(
        port(known.as_ref().map(|k| &k.slack)),
        Service::Slack {
            token: fixtures::SLACK_TOKEN.into(),
        },
        fixtures::slack,
    );
    // Fresh before the services say they are ready, so a demo reopened
    // after a while never shows its server a stale day.
    refresh_day(home);
    Services {
        github: github.url.clone(),
        linear: linear.url.clone(),
        slack: slack.url.clone(),
    }
    .write(home)?;
    let log = own_state(home).join("refused.log");
    let mut day_at = Instant::now();
    loop {
        std::thread::sleep(Duration::from_secs(1));
        // Taken as they come, so the services keep nothing they read.
        let refused: Vec<String> = [&github, &linear, &slack]
            .iter()
            .flat_map(|f| f.take_refused())
            .collect();
        if !refused.is_empty() {
            let mut file = fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&log)?;
            for line in &refused {
                use std::io::Write;
                writeln!(file, "{line}")?;
            }
        }
        // Gone with the demo's home.
        if !home.is_dir() {
            std::process::exit(0);
        }
        if day_at.elapsed() >= DAY_EVERY {
            refresh_day(home);
            day_at = Instant::now();
        }
    }
}

/// How long the Slack messages keep the time they were given: inside the
/// day after which the Slack view forgets a message.
const SLACK_ANCHOR_MS: i64 = 12 * 60 * 60 * 1000;

/// The day's data written again (`day`), the Slack messages timed from
/// when the demo was laid out, moved on every `SLACK_ANCHOR_MS`, so they
/// stay the same messages for half a day and never age out of the view.
fn refresh_day(home: &Path) {
    let now = util::now_ms();
    let laid_out = fs::metadata(own_state(home).join("laid-out"))
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(now, |d| d.as_millis() as i64);
    if let Err(e) = day(home, meetings_at(laid_out), slack_anchor(laid_out, now)) {
        eprintln!("switchboard demo-services: the day's data: {e:#}");
    }
    if let Err(e) = usage::limits(&home.join(".local/state/switchboard"), laid_out, now) {
        eprintln!("switchboard demo-services: the rate limits: {e:#}");
    }
}

/// Today, at the time of day `laid_out` was, here.
fn meetings_at(laid_out: i64) -> jiff::Zoned {
    let now = jiff::Zoned::now();
    jiff::Timestamp::from_millisecond(laid_out)
        .ok()
        .map(|t| t.to_zoned(now.time_zone().clone()))
        .and_then(|laid| {
            now.date()
                .to_datetime(laid.time())
                .to_zoned(now.time_zone().clone())
                .ok()
        })
        .unwrap_or(now)
}

/// When the Slack messages are timed from: `laid_out`, moved on by whole
/// `SLACK_ANCHOR_MS` steps to the last one before `now`.
fn slack_anchor(laid_out: i64, now: i64) -> i64 {
    laid_out + (now - laid_out).max(0) / SLACK_ANCHOR_MS * SLACK_ANCHOR_MS
}

/// Stops the demo's services, when they are running.
pub fn stop_services(home: &Path) {
    let pid = util::read_json(own_state(home).join("services.json"))
        .and_then(|v| v.get("pid").and_then(Value::as_i64));
    // Only while that pid is still the demo's services: one that has ended
    // may have had its pid handed to another program of yours.
    let ours = |pid: i64| {
        crate::mux::procinfo::args_of(pid as u32)
            .is_some_and(|args| args.iter().any(|a| a == "demo-services"))
    };
    if let Some(pid) = pid.filter(|p| *p > 1 && ours(*p)) {
        // SAFETY: kill only sends a signal; a pid that is gone is an error.
        unsafe {
            libc::kill(pid as libc::pid_t, libc::SIGTERM);
        }
    }
}

/// Runs a server command on the demo's server, giving what it printed.
pub fn ask(home: &Path, args: &[&str]) -> Result<String> {
    let msg = ClientMsg::Command {
        args: args.iter().map(|a| a.to_string()).collect(),
        cwd: home.to_path_buf(),
        pane: None,
        stdin: None,
    };
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let code = crate::mux::socket::run_command_at(&socket_of(home), &msg, &mut out, &mut err);
    if code != 0 {
        bail!(
            "{} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&err).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out).into_owned())
}

/// The tabs the demo opens: (name, directory under the home, the program
/// and its arguments).
pub fn tabs(home: &Path, bin: &Path) -> Vec<(String, PathBuf, Vec<String>)> {
    let wt = |repo: &str, branch: &str| {
        home.join("code")
            .join(repo)
            .join(".claude/worktrees")
            .join(branch)
    };
    let claude = |script: &str| vec!["claude".to_string(), "--demo-script".into(), script.into()];
    vec![
        (
            "Rate-limit the login endpoint".into(),
            wt("api", "abc-12-rate-limit-login"),
            claude("rate-limit"),
        ),
        (
            "Dark mode toggle".into(),
            wt("web", "abc-15-dark-mode-toggle"),
            claude("dark-mode"),
        ),
        (
            "API quickstart guide".into(),
            wt("docs", "abc-18-api-guide"),
            claude("api-guide"),
        ),
        (
            "Notes".into(),
            home.join("Notes"),
            vec![bin.to_string_lossy().into_owned(), "notes".into()],
        ),
    ]
}

/// Starts the demo's server with no client, unless one of this build
/// answers, gives it a session and opens the demo's tabs. One another build
/// started (a demo left running while you rebuilt) is ended first, since
/// what it does is that build's: a tour of this one would press keys it
/// doesn't know. A server from before the `server-version` command answers none.
pub fn start_server(home: &Path, bin: &Path, env: &[(String, String)]) -> Result<()> {
    let socket = socket_of(home);
    if UnixStream::connect(&socket).is_ok() {
        let running = ask(home, &["server-version"]).map(|v| v.trim().to_string());
        let ours = crate::update::version_line();
        if running.as_deref().is_ok_and(|v| v == ours) {
            return Ok(());
        }
        let theirs = running.unwrap_or_else(|_| "an older one".into());
        eprintln!(
            "The demo was started by another build ({theirs}); starting it again on this one ({ours})."
        );
        crate::cmd::try_::stop_server(&socket);
        if UnixStream::connect(&socket).is_ok() {
            bail!(
                "the demo's server from another build didn't end; switchboard demo --clean ends it"
            );
        }
    }
    let state = home.join(".local/state/switchboard");
    let log = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(state.join("server.log"))?;
    let mut c = command(bin, env);
    c.arg("server")
        .current_dir(home.join("code"))
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log);
    detach(&mut c);
    c.spawn().context("couldn't start the demo's server")?;
    let deadline = Instant::now() + Duration::from_secs(10);
    while UnixStream::connect(&socket).is_err() {
        if Instant::now() >= deadline {
            bail!(
                "the demo's server didn't start; {} says why",
                state.join("server.log").display()
            );
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    crate::cmd::try_::make_session(&socket)?;
    for (name, dir, program) in tabs(home, bin) {
        let mut args = vec![
            "new-tab".to_string(),
            "-d".into(),
            "-E".into(),
            "-c".into(),
            dir.to_string_lossy().into_owned(),
            "-n".into(),
            name,
        ];
        args.extend(program);
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        ask(home, &args)?;
    }
    Ok(())
}

/// Ends the demo's server and services and removes its home.
pub fn clean(home: &Path) -> Result<()> {
    check_home(home)?;
    crate::cmd::try_::stop_server(&socket_of(home));
    stop_services(home);
    if home.exists() {
        fs::remove_dir_all(home).with_context(|| format!("couldn't remove {}", home.display()))?;
    }
    Ok(())
}

/// Every path under `dir` that is a link leading outside `within`, for the
/// test that the demo links nothing of yours.
pub fn links_out(dir: &Path, within: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(meta) = path.symlink_metadata() else {
            continue;
        };
        if meta.file_type().is_symlink() {
            let target = fs::read_link(&path).unwrap_or_default();
            let target = if target.is_absolute() {
                target
            } else {
                path.parent().unwrap_or(dir).join(target)
            };
            if !target.starts_with(within) {
                out.push(path);
            }
        } else if meta.is_dir() {
            out.extend(links_out(&path, within));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_slack_messages_move_on_by_half_days() {
        let h = 60 * 60 * 1000;
        assert_eq!(slack_anchor(1000, 1000 + 3 * h), 1000);
        assert_eq!(slack_anchor(1000, 1000 + 30 * h), 1000 + 24 * h);
        assert_eq!(slack_anchor(1000, 500), 1000);
    }

    #[test]
    fn the_meetings_are_today_at_the_time_of_day_the_demo_was_laid_out() {
        let laid = jiff::Zoned::now() - jiff::SignedDuration::from_hours(30);
        let at = meetings_at(laid.timestamp().as_millisecond());
        assert_eq!(at.date(), jiff::Zoned::now().date());
        assert_eq!((at.hour(), at.minute()), (laid.hour(), laid.minute()));
    }
}
