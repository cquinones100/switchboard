//! The tests' stand-ins themselves (tests/common/stand_in.rs and the
//! services tests/common/http.rs takes from src/demo/services.rs), and the
//! demo's stand-in programs (src/demo/stand_in.rs): each takes the calls Switchboard makes of the real
//! program or service, word for word as the code makes them, and refuses a
//! wrong one as the real one does (a name it doesn't answer to, an option it
//! doesn't have or one missing its value, a missing argument, stdin it needs
//! and wasn't given; for a service the wrong method, a path it doesn't have,
//! a missing or wrong token), so a test that passes with them has called
//! them rightly. Every stand-in here is run directly, in a directory of its
//! own; nothing real is run or reached.

use crate::common;

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use common::http::{Fake, Reply, Service};
use common::stand_in::{self, Claude, Gh, Release};
use common::{TempDir, TestServer};
use serde_json::{Value, json};

/// Runs `program` with `args`, `input` on its stdin (none: stdin closed),
/// in `dir`.
fn run(program: &Path, args: &[&str], input: Option<&str>, dir: &Path) -> Output {
    let mut child = Command::new(program)
        .args(args)
        .current_dir(dir)
        .env("TMPDIR", dir)
        // gh's stand-in says a token in these as gh does, so one in the
        // environment of whoever runs the tests would answer for it.
        .env_remove("GH_TOKEN")
        .env_remove("GITHUB_TOKEN")
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap_or_else(|e| panic!("{} runs: {e}", program.display()));
    if let Some(text) = input {
        let mut stdin = child.stdin.take().unwrap();
        // A stand-in refusing its call exits without reading its input, as
        // the real program does, sometimes before this is written: the pipe
        // closing then is the refusal the test looks at, not a failure.
        if let Err(e) = stdin.write_all(text.as_bytes()) {
            assert_eq!(e.kind(), std::io::ErrorKind::BrokenPipe, "{e}");
        }
    }
    child.wait_with_output().unwrap()
}

fn out(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn err(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

/// Fails unless the call succeeded; gives its stdout.
fn took(o: &Output) -> String {
    assert!(o.status.success(), "refused: {}{}", out(o), err(o));
    out(o)
}

/// Fails unless the call was refused with `code`, saying `said` on stderr.
fn refused(o: &Output, code: i32, said: &str) {
    assert_eq!(
        o.status.code(),
        Some(code),
        "not refused with {code}: {}{}",
        out(o),
        err(o)
    );
    assert!(err(o).contains(said), "{said:?} not in {:?}", err(o));
}

// ── Claude Code ─────────────────────────────────────────────────────────────

/// A stand-in Claude that says how it was called, and what came on stdin.
fn claude(dir: &Path) -> PathBuf {
    Claude::new()
        .answer("printf 'call=%s\\n' \"$CLAUDE_CALL\"\n[ ! \"$CLAUDE_CALL\" = print ] || cat")
        .install(&dir.join("bin"))
}

#[test]
fn claude_takes_every_call_switchboard_makes_of_it() {
    let dir = TempDir::new("si-claude");
    let claude = claude(dir.path());
    let run = |args: &[&str], input: Option<&str>| took(&run(&claude, args, input, dir.path()));
    let quiet = [
        "-p",
        "--model",
        "haiku",
        "--no-session-persistence",
        "--restricted",
        "--tools",
        "",
        "--strict-mcp-config",
        "--disable-slash-commands",
    ];
    // The diff's order and a repository's CLAUDE.md read (src/diff/order.rs,
    // src/repos.rs), the todo note's (src/day/todo.rs).
    let mut order = quiet.to_vec();
    order.extend(["--system-prompt", "Order them.", "--output-format", "text"]);
    assert_eq!(run(&order, Some("a.rs\n")), "call=print\na.rs\n");
    // The story and the questions about it, streamed (src/diff/tell.rs,
    // src/diff/ask.rs).
    let mut story = quiet.to_vec();
    story.extend([
        "--system-prompt",
        "Tell it.",
        "--output-format",
        "stream-json",
        "--include-partial-messages",
        "--verbose",
    ]);
    assert_eq!(run(&story, Some("the diff")), "call=print\nthe diff");
    let ask = [
        "-p",
        "--model",
        "haiku",
        "--no-session-persistence",
        "--restricted",
        "--tools",
        "Read,Grep,Glob",
        "--allowedTools",
        "Read,Grep,Glob",
        "--strict-mcp-config",
        "--disable-slash-commands",
        "--system-prompt",
        "Answer.",
        "--output-format",
        "stream-json",
        "--include-partial-messages",
        "--verbose",
    ];
    assert_eq!(run(&ask, Some("why?")), "call=print\nwhy?");
    // The Slack relay's summaries (src/relay/summaries.rs).
    let mut summary = quiet.to_vec();
    summary.extend(["--system-prompt", "Sum up.", "--output-format", "json"]);
    assert_eq!(run(&summary, Some("thread")), "call=print\nthread");
    // A squash: the prompt as the argument, the notes on stdin
    // (src/notes/squash.rs).
    assert_eq!(
        run(
            &[
                "-p",
                "--model",
                "sonnet",
                "--tools",
                "",
                "--no-session-persistence",
                "Merge them."
            ],
            Some("notes")
        ),
        "call=print\nnotes"
    );
    // A connector's call, its prompt the argument and stdin closed
    // (src/connector.rs), and its check, which needs no model.
    assert_eq!(
        run(
            &[
                "-p",
                "--model",
                "haiku",
                "--output-format",
                "stream-json",
                "--verbose",
                "--no-session-persistence",
                "--tools",
                "ToolSearch",
                "--allowedTools",
                "ToolSearch,mcp__claude_ai_Slack__x",
                "--max-turns",
                "4",
                "Call the tool."
            ],
            None
        ),
        "call=print\n"
    );
    assert_eq!(
        run(&["mcp", "get", "claude.ai Slack"], None),
        "call=mcp get\n"
    );
    // codex-review's Claude (src/cmd/codex_review.rs's claude_args).
    assert_eq!(
        run(
            &[
                "-p",
                "--output-format",
                "json",
                "--json-schema",
                "{\"type\":\"object\"}",
                "--tools",
                "Read,Grep,Glob",
                "--permission-mode",
                "dontAsk",
                "--setting-sources",
                "",
                "--strict-mcp-config",
                "--disable-slash-commands",
                "--no-session-persistence",
                "--add-dir",
                "/tmp/diff",
                "--model",
                "opus"
            ],
            Some("review it")
        ),
        "call=print\nreview it"
    );
    // In a tab: alone, with a prompt, resuming a session.
    assert_eq!(run(&[], None), "call=interactive\n");
    assert_eq!(run(&["/shape ENG-1"], None), "call=interactive\n");
    assert_eq!(run(&["--resume", "0b8c-41aa"], None), "call=interactive\n");
    assert_eq!(
        run(&["--version"], None),
        "2.1.319 (Claude Code)\n",
        "the version, answered by Claude Code itself"
    );
    assert_eq!(
        stand_in::refusals(&dir.path().join("bin")),
        None,
        "nothing taken is written down as refused"
    );
}

#[test]
fn claude_refuses_what_claude_code_refuses() {
    let dir = TempDir::new("si-claude-no");
    let claude = claude(dir.path());
    let run = |args: &[&str], input: Option<&str>| run(&claude, args, input, dir.path());
    refused(
        &run(&["-p", "--bogus", "hi"], None),
        1,
        "error: unknown option '--bogus'",
    );
    refused(
        &run(&["-p", "--verbose=1", "hi"], None),
        1,
        "error: unknown option '--verbose=1'",
    );
    refused(
        &run(&["-p", "--model"], Some("hi")),
        1,
        "error: option '--model <model>' argument missing",
    );
    refused(
        &run(&["-p", "--output-format", "xml", "hi"], None),
        1,
        "error: option '--output-format <format>' argument 'xml' is invalid. Allowed choices are text, json, stream-json.",
    );
    refused(
        &run(&["--permission-mode", "default"], None),
        1,
        "argument 'default' is invalid",
    );
    refused(
        &run(&["-p", "--output-format", "stream-json", "hi"], None),
        1,
        "requires --verbose",
    );
    refused(
        &run(&["one", "two"], None),
        1,
        "error: too many arguments. Expected 1 argument but got 2.",
    );
    // -p with no prompt: none on stdin, closed or empty.
    for input in [None, Some("")] {
        refused(
            &run(&["-p", "--model", "haiku"], input),
            1,
            "Input must be provided either through stdin or as a prompt argument",
        );
    }
    // A list of tools takes every argument after it that isn't an option,
    // so a prompt there is a tool, and -p has none.
    refused(
        &run(&["-p", "--tools", "", "the prompt"], None),
        1,
        "Input must be provided",
    );
    refused(
        &run(&["mcp", "get"], None),
        1,
        "error: missing required argument 'name'",
    );
    refused(&run(&["mcp", "frob"], None), 1, "unknown command 'frob'");
    refused(&run(&["doctor"], None), 1, "no answer for `claude doctor`");
    // Each written down, the call and why, for the test to see.
    let written = stand_in::refusals(&dir.path().join("bin")).unwrap();
    assert!(
        written.starts_with("claude -p --bogus hi\n    error: unknown option '--bogus'\n"),
        "{written}"
    );
    assert!(written.contains("claude -p --tools  the prompt\n    Error: Input must be provided"));
}

/// A test whose stand-in service refused a request fails, however quietly
/// the code took the refusal.
#[test]
#[should_panic(expected = "refused a request the code made")]
fn a_service_that_refused_a_request_fails_its_test() {
    let fake = Fake::github("tok", |_| None);
    let (status, _) = ask(
        &fake.url,
        "GET",
        "/repos/o/r",
        &[("Authorization", "Bearer tok")],
        "",
    );
    assert_eq!(status, 403, "no User-Agent");
}

/// A test whose server's stand-in refused a call fails, however quietly the
/// server took the failure (here a tab whose program just ended).
#[test]
#[should_panic(expected = "refused a call the code made")]
fn a_server_whose_stand_in_refused_a_call_fails_its_test() {
    let server = TestServer::start();
    let claude = Claude::new().install(&server.home.join("bin"));
    let mut client = server.attach(80, 24);
    client.wait_text("$");
    server.ok(&["new-tab", "-d", "--", claude.to_str().unwrap(), "--bogus"]);
    let deadline = common::deadline(std::time::Duration::from_secs(10));
    while stand_in::refusals(&server.home.join("bin")).is_none() {
        assert!(
            std::time::Instant::now() < deadline,
            "the tab's claude never ran"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

// ── gh ──────────────────────────────────────────────────────────────────────

#[test]
fn gh_takes_its_sign_in_commands_and_refuses_the_rest() {
    let dir = TempDir::new("si-gh");
    let signed_in = dir.path().join("SIGNED_IN");
    let gh = Gh::signed_in_while(&signed_in).install(&dir.path().join("bin"));
    let run = |args: &[&str], input: Option<&str>| run(&gh, args, input, dir.path());
    refused(
        &run(&["auth", "token"], None),
        1,
        "no oauth token found for github.com",
    );
    std::fs::write(&signed_in, "").unwrap();
    assert_eq!(took(&run(&["auth", "token"], None)), "gho_stand_in\n");
    assert_eq!(
        took(&run(&["auth", "token", "--hostname", "github.com"], None)),
        "gho_stand_in\n"
    );
    assert_eq!(
        took(&run(
            &["auth", "git-credential", "get"],
            Some("protocol=https\nhost=github.com\n\n")
        )),
        "username=x-access-token\npassword=gho_stand_in\n"
    );
    refused(
        &run(&["auth", "token", "--hostname"], None),
        1,
        "flag needs an argument: --hostname",
    );
    refused(
        &run(&["auth", "token", "--bogus"], None),
        1,
        "unknown flag: --bogus",
    );
    refused(
        &run(&["auth", "token", "-x"], None),
        1,
        "unknown shorthand flag: 'x' in -x",
    );
    refused(
        &run(&["auth", "tokn"], None),
        1,
        "unknown command \"tokn\" for \"gh auth\"",
    );
    refused(
        &run(&["auth", "token", "extra"], None),
        1,
        "unknown command \"extra\" for \"gh auth token\"",
    );
    refused(
        &run(&["auth", "git-credential"], None),
        1,
        "accepts 1 arg(s), received 0",
    );
    refused(
        &run(&["auth", "git-credential", "fetch"], None),
        1,
        "\"fetch\" operation not supported",
    );
    refused(
        &run(&["frob"], None),
        1,
        "unknown command \"frob\" for \"gh\"",
    );
    refused(&run(&["pr", "view"], None), 1, "no answer for `gh pr`");
    let written = stand_in::refusals(&dir.path().join("bin")).unwrap();
    assert!(
        !written.contains("gh auth token\n"),
        "not signed in is no wrong call: {written}"
    );
    assert!(written.contains("gh auth tokn\n    unknown command \"tokn\" for \"gh auth\""));
}

// ── Switchboard's release ───────────────────────────────────────────────────

#[test]
fn a_release_answers_only_to_its_own_name_and_commands() {
    let dir = TempDir::new("si-release");
    let mark = dir.path().join("mark");
    let release = Release::new("0.3.0").on_setup(&format!(
        "echo \"setup $*\" >> {}",
        stand_in::sh_quote(&mark.to_string_lossy())
    ));
    let binary = release.install(&dir.path().join("bin/switchboard"));
    let run = |args: &[&str]| run(&binary, args, None, dir.path());
    assert_eq!(took(&run(&["--version"])), "switchboard 0.3.0\n");
    took(&run(&["setup"]));
    took(&run(&["setup", "--check", "claude-md"]));
    assert_eq!(
        std::fs::read_to_string(&mark).unwrap(),
        "setup setup\nsetup setup --check claude-md\n"
    );
    refused(&run(&["setup", "bogus"]), 1, "Unknown part: bogus");
    refused(
        &run(&["frob"]),
        2,
        "switchboard: no command named frob (switchboard help lists them)",
    );
    refused(
        &run(&["server", "--handover-version"]),
        2,
        "Usage: switchboard server",
    );
    let later = Release::new("0.4.0")
        .handover(7)
        .install(&dir.path().join("later/switchboard"));
    assert_eq!(
        took(&run_at(
            &later,
            &["server", "--handover-version"],
            dir.path()
        )),
        "7\n"
    );
    // Under another name, as an installer once ran it, it is no command.
    for name in [".switchboard.new.123", "switchboard-0.3.0"] {
        let other = release.install(&dir.path().join(name));
        refused(
            &run_at(&other, &["--version"], dir.path()),
            2,
            &format!("switchboard: no command named {name}"),
        );
    }
    let written = stand_in::refusals(dir.path()).unwrap();
    assert!(
        written.contains(".switchboard.new.123 --version\n    run as .switchboard.new.123, which no release answers to"),
        "{written}"
    );
}

fn run_at(program: &Path, args: &[&str], dir: &Path) -> Output {
    run(program, args, None, dir)
}

// ── The desktop's tools ─────────────────────────────────────────────────────

#[test]
fn the_clipboard_tools_copy_and_paste_as_each_desktop_s_do() {
    let dir = TempDir::new("si-clip");
    let bin = dir.path().join("bin");
    let log = dir.path().join("log");
    let clipboard = dir.path().join("clipboard");
    stand_in::desktop(&bin, stand_in::CLIPBOARD, &log, &clipboard);
    let tool = |name: &str, args: &[&str], input: Option<&str>| {
        run(&bin.join(name), args, input, dir.path())
    };
    // As src/platform's copy and paste call each.
    for (copy, copy_args, paste, paste_args) in [
        ("pbcopy", &[][..], "pbpaste", &[][..]),
        ("wl-copy", &[], "wl-paste", &["-n"]),
        (
            "xclip",
            &["-selection", "clipboard"],
            "xclip",
            &["-selection", "clipboard", "-o"],
        ),
        (
            "xsel",
            &["--clipboard", "--input"],
            "xsel",
            &["--clipboard", "--output"],
        ),
        (
            "clip.exe",
            &[],
            "powershell.exe",
            &["-NoProfile", "-Command", "Get-Clipboard"],
        ),
    ] {
        took(&tool(copy, copy_args, Some(&format!("from {copy}"))));
        assert_eq!(took(&tool(paste, paste_args, None)), format!("from {copy}"));
    }
    assert!(
        std::fs::read_to_string(&log)
            .unwrap()
            .starts_with("pbcopy \npbpaste \nwl-copy \nwl-paste -n\nxclip -selection clipboard\n")
    );
    refused(&tool("pbcopy", &["-x"], Some("t")), 1, "usage: pbcopy");
    refused(
        &tool("wl-copy", &["--bogus"], Some("t")),
        1,
        "wl-copy: unrecognized option '--bogus'",
    );
    refused(
        &tool("wl-paste", &["--type"], None),
        1,
        "option requires an argument",
    );
    refused(
        &tool("wl-paste", &["--type", "image/png"], None),
        1,
        "No suitable type of content copied",
    );
    refused(
        &tool("xclip", &["-selection"], Some("t")),
        1,
        "xclip: -selection requires an argument",
    );
    refused(
        &tool("xclip", &["-selction", "clipboard"], Some("t")),
        1,
        "xclip: -selction: No such file or directory",
    );
    refused(&tool("xsel", &["--bogus"], Some("t")), 1, "Usage: xsel");
    refused(
        &tool("clip.exe", &["/x"], Some("t")),
        1,
        "ERROR: Invalid argument/option - '/x'.",
    );
    let written = stand_in::refusals(&bin).unwrap();
    assert!(
        written.starts_with("pbcopy -x\n    usage: pbcopy"),
        "{written}"
    );
    assert!(
        !std::fs::read_to_string(&log).unwrap().contains("pbcopy -x"),
        "a refused call does nothing"
    );
}

#[test]
fn the_openers_open_what_is_there_and_refuse_the_rest() {
    let dir = TempDir::new("si-open");
    let bin = dir.path().join("bin");
    let log = dir.path().join("log");
    let tools: Vec<&str> = stand_in::OPENERS
        .iter()
        .chain(stand_in::CHROMES)
        .chain(&["osascript"])
        .copied()
        .collect();
    stand_in::desktop(&bin, &tools, &log, &dir.path().join("clipboard"));
    let file = dir.path().join("shot.png");
    std::fs::write(&file, b"png").unwrap();
    let f = file.to_str().unwrap();
    let missing = dir.path().join("missing.png");
    let m = missing.to_str().unwrap();
    let tool = |name: &str, args: &[&str]| run(&bin.join(name), args, None, dir.path());
    // As src/platform's open, reveal and notify, and src/links.rs, call them.
    let url = "https://example.com/a";
    took(&tool("open", &[url]));
    took(&tool("open", &["-R", f]));
    took(&tool(
        "open",
        &[
            "-na",
            "Google Chrome",
            "--args",
            "--profile-directory=Profile 2",
            url,
        ],
    ));
    took(&tool("xdg-open", &[f]));
    took(&tool("wslview", &[url]));
    took(&tool("gio", &["open", url]));
    took(&tool(
        "google-chrome",
        &["--profile-directory=Default", url],
    ));
    took(&tool(
        "osascript",
        &[
            "-e",
            "on run argv",
            "-e",
            "display notification (item 1 of argv) with title (item 2 of argv)",
            "-e",
            "end run",
            "the text",
            "the title",
        ],
    ));
    let logged = std::fs::read_to_string(&log).unwrap();
    assert!(
        logged.starts_with(&format!(
            "open {url}\nopen -R {f}\nopen -na Google Chrome --args --profile-directory=Profile 2 {url}\nxdg-open {f}\n"
        )),
        "{logged}"
    );
    refused(
        &tool("open", &[m]),
        1,
        &format!("The file {m} does not exist."),
    );
    refused(&tool("open", &["-x", url]), 1, "Usage: open");
    refused(&tool("open", &["-a"]), 1, "Usage: open");
    refused(&tool("open", &[]), 1, "Usage: open");
    refused(
        &tool("xdg-open", &[m]),
        2,
        &format!("xdg-open: file '{m}' does not exist"),
    );
    refused(
        &tool("xdg-open", &[url, url]),
        1,
        &format!("xdg-open: unexpected argument '{url}'"),
    );
    refused(
        &tool("xdg-open", &[]),
        1,
        "xdg-open: file or URL argument missing",
    );
    refused(
        &tool("xdg-open", &["--bogus"]),
        1,
        "xdg-open: unexpected option '--bogus'",
    );
    // Windows' own under WSL: a file named as Windows names it, and an
    // Explorer that says it failed whether or not it opened it.
    assert_eq!(
        took(&tool("wslpath", &["-w", "/tmp/a b"])),
        "\\\\wsl.localhost\\Stand-in\\tmp\\a b\n"
    );
    let explorer = tool("explorer.exe", &[url]);
    assert_eq!(explorer.status.code(), Some(1));
    assert!(
        std::fs::read_to_string(&log)
            .unwrap()
            .ends_with(&format!("explorer.exe {url}\n")),
        "written down all the same"
    );
    refused(&tool("wslpath", &["-z", "/tmp"]), 1, "Usage:");
    refused(&tool("wslpath", &[]), 1, "Usage:");
    refused(&tool("gio", &["opne", url]), 1, "gio: Unknown command opne");
    refused(&tool("gio", &["open"]), 1, "gio: No locations given");
    refused(
        &tool("osascript", &["-z"]),
        1,
        "osascript: illegal option -- z",
    );
    refused(&tool("osascript", &["-e"]), 1, "usage: osascript");
    refused(
        &tool("osascript", &["-l", "Python", "-e", "x"]),
        1,
        "invalid language 'Python'",
    );
}

#[test]
fn caffeinate_holds_while_the_process_it_watches_runs_and_refuses_the_rest() {
    let dir = TempDir::new("si-caffeinate");
    let bin = dir.path().join("bin");
    let log = dir.path().join("log");
    stand_in::desktop(&bin, &["caffeinate"], &log, &dir.path().join("clipboard"));
    let caffeinate = bin.join("caffeinate");
    // As src/platform/macos.rs's keep_awake calls it, watching a process
    // that ends: it holds while that runs, then ends too.
    let mut watched = Command::new("sleep").arg("1").spawn().unwrap();
    let pid = watched.id().to_string();
    // Reaped as it ends, since a process ended and not yet reaped is still
    // there to be watched.
    let reaped = std::thread::spawn(move || watched.wait());
    let start = std::time::Instant::now();
    took(&run(&caffeinate, &["-is", "-w", &pid], None, dir.path()));
    reaped.join().unwrap().unwrap();
    assert!(
        start.elapsed() >= std::time::Duration::from_millis(900),
        "it held while the process ran"
    );
    let said = std::fs::read_to_string(&log).unwrap();
    let held_as = said
        .strip_prefix(&format!("caffeinate -is -w {pid}\ncaffeinate is "))
        .and_then(|rest| rest.trim().parse::<u32>().ok());
    assert!(held_as.is_some(), "its call, then its pid: {said}");
    refused(
        &run(&caffeinate, &["-x"], None, dir.path()),
        1,
        "caffeinate: illegal option -- x",
    );
    refused(
        &run(&caffeinate, &["-i", "-w"], None, dir.path()),
        1,
        "caffeinate: option requires an argument -- w",
    );
    assert_eq!(
        stand_in::refusals(&bin)
            .map(|r| r.lines().filter(|l| l.starts_with("caffeinate -")).count()),
        Some(2),
        "each refusal written down"
    );
}

#[test]
fn security_keeps_a_keychain_and_refuses_what_security_refuses() {
    let dir = TempDir::new("si-security");
    let keychain = dir.path().join("keychain");
    let security = stand_in::security(&dir.path().join("bin"), &keychain);
    let run = |args: &[&str]| run(&security, args, None, dir.path());
    stand_in::keychain_item(&keychain, "github-token", "", "from-keychain");
    // As src/platform/macos.rs's secret and set_secret call it.
    assert_eq!(
        took(&run(&["find-generic-password", "-s", "github-token", "-w"])),
        "from-keychain\n"
    );
    refused(
        &run(&["find-generic-password", "-s", "linear-api-key", "-w"]),
        44,
        "The specified item could not be found in the keychain.",
    );
    took(&run(&[
        "add-generic-password",
        "-U",
        "-a",
        "me",
        "-s",
        "linear-api-key",
        "-w",
        "lin_api_1",
    ]));
    took(&run(&[
        "add-generic-password",
        "-U",
        "-a",
        "me",
        "-s",
        "linear-api-key",
        "-w",
        "lin_api_2",
    ]));
    assert_eq!(
        took(&run(&[
            "find-generic-password",
            "-a",
            "me",
            "-s",
            "linear-api-key",
            "-w"
        ])),
        "lin_api_2\n"
    );
    refused(
        &run(&[
            "add-generic-password",
            "-a",
            "me",
            "-s",
            "linear-api-key",
            "-w",
            "x",
        ]),
        45,
        "already exists",
    );
    refused(
        &run(&["find-generic-password", "-s"]),
        2,
        "Usage: security find-generic-password",
    );
    refused(
        &run(&["find-generic-password", "-z", "-s", "x"]),
        2,
        "security: illegal option -- z",
    );
    refused(
        &run(&["add-generic-password", "-s", "x"]),
        1,
        "no terminal to ask the password on",
    );
    refused(
        &run(&["find-password"]),
        1,
        "unknown command \"find-password\"",
    );
}

#[test]
fn uname_and_sysctl_say_the_computer_asked_for_and_refuse_what_they_refuse() {
    let dir = TempDir::new("si-uname");
    let linux = dir.path().join("linux");
    let mac = dir.path().join("mac");
    let intel = dir.path().join("intel");
    stand_in::platform(&linux, "Linux", "x86_64", "");
    stand_in::platform(&mac, "Darwin", "x86_64", "1");
    stand_in::platform(&intel, "Darwin", "x86_64", "");
    let at = |bin: &Path, name: &str, args: &[&str]| run(&bin.join(name), args, None, dir.path());
    // As install.sh asks.
    assert_eq!(took(&at(&linux, "uname", &["-s"])), "Linux\n");
    assert_eq!(took(&at(&linux, "uname", &["-m"])), "x86_64\n");
    assert_eq!(took(&at(&linux, "uname", &["-m", "-s"])), "Linux x86_64\n");
    assert_eq!(took(&at(&mac, "uname", &[])), "Darwin\n");
    assert_eq!(
        took(&at(&mac, "sysctl", &["-in", "sysctl.proc_translated"])),
        "1\n"
    );
    assert_eq!(
        took(&at(&intel, "sysctl", &["-in", "sysctl.proc_translated"])),
        "",
        "an Intel Mac has no such value, which -i passes over"
    );
    refused(
        &at(&intel, "sysctl", &["-n", "sysctl.proc_translated"]),
        1,
        "sysctl: unknown oid 'sysctl.proc_translated'",
    );
    refused(
        &at(&linux, "uname", &["-z"]),
        1,
        "uname: invalid option -- 'z'",
    );
    refused(&at(&mac, "uname", &["-z"]), 1, "uname: illegal option -- z");
    refused(&at(&linux, "uname", &["s"]), 1, "uname: extra operand 's'");
    refused(
        &at(&mac, "sysctl", &["-z", "x"]),
        1,
        "sysctl: illegal option -- z",
    );
    refused(&at(&mac, "sysctl", &["-n"]), 1, "usage: sysctl");
}

#[test]
fn cargo_builds_only_as_cargo_build_takes_it() {
    let dir = TempDir::new("si-cargo");
    let cargo = stand_in::cargo(&dir.path().join("bin"), "echo built");
    let crate_dir = dir.path().join("crate/src");
    std::fs::create_dir_all(&crate_dir).unwrap();
    std::fs::write(dir.path().join("crate/Cargo.toml"), "[package]\n").unwrap();
    // As src/cmd/try_.rs's build runs it, from below the manifest too.
    let build = ["build", "--message-format=json-render-diagnostics"];
    assert_eq!(took(&run(&cargo, &build, None, &crate_dir)), "built\n");
    refused(
        &run(&cargo, &build, None, dir.path()),
        101,
        "error: could not find `Cargo.toml`",
    );
    refused(
        &run(&cargo, &["build", "--message-format=jsn"], None, &crate_dir),
        101,
        "invalid message format specifier: `jsn`",
    );
    refused(
        &run(&cargo, &["build", "--message-format"], None, &crate_dir),
        1,
        "a value is required for '--message-format <FMT>'",
    );
    refused(
        &run(&cargo, &["build", "--bogus"], None, &crate_dir),
        1,
        "error: unexpected argument '--bogus' found",
    );
    refused(
        &run(&cargo, &["biuld"], None, &crate_dir),
        101,
        "error: no such command: `biuld`",
    );
}

#[test]
fn lazygit_refuses_an_argument_it_doesnt_have() {
    let dir = TempDir::new("si-lazygit");
    let lazygit = stand_in::lazygit(&dir.path().join("bin"));
    refused(
        &run(&lazygit, &["--bogus"], None, dir.path()),
        2,
        "Unknown arguments supplied:  --bogus",
    );
    refused(
        &run(&lazygit, &["-p"], None, dir.path()),
        2,
        "Argument missing for flag: -p",
    );
    // As M-g starts it, with nothing: it says where it runs and waits.
    let mut child = Command::new(&lazygit)
        .current_dir(dir.path())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    assert_eq!(
        line,
        format!("stand-in lazygit in {}\n", dir.path().display())
    );
    let _ = child.kill();
    let _ = child.wait();
}

// ── The services ────────────────────────────────────────────────────────────

/// One HTTP/1.1 request to `url` with `headers` and `body`: the status and
/// the body.
fn ask(url: &str, method: &str, path: &str, headers: &[(&str, &str)], body: &str) -> (u16, String) {
    let host = url.trim_start_matches("http://");
    let mut stream = TcpStream::connect(host).unwrap();
    let mut request = format!("{method} {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n");
    for (name, value) in headers {
        request.push_str(&format!("{name}: {value}\r\n"));
    }
    request.push_str(&format!("Content-Length: {}\r\n\r\n{body}", body.len()));
    stream.write_all(request.as_bytes()).unwrap();
    let mut reply = String::new();
    stream.read_to_string(&mut reply).unwrap();
    let status = reply
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let body = reply
        .split_once("\r\n\r\n")
        .map(|(_, b)| b.to_string())
        .unwrap_or_default();
    (status, body)
}

fn json_of(body: &str) -> Value {
    serde_json::from_str(body).unwrap_or(Value::Null)
}

#[test]
fn github_answers_only_a_request_made_as_github_takes_one() {
    let mut fake = Fake::github("tok", |r| match (r.method.as_str(), r.path()) {
        ("POST", "/graphql") => Some(Reply::json(
            200,
            json!({ "data": { "viewer": { "login": "me" } } }),
        )),
        ("GET", "/repos/o/r") => Some(Reply::json(200, json!({ "default_branch": "main" }))),
        _ => None,
    });
    fake.expect_refusals();
    let ua = ("User-Agent", "Switchboard/test");
    let auth = ("Authorization", "Bearer tok");
    let json = ("Content-Type", "application/json");
    let query = r#"{"query":"query { viewer { login } }","variables":{}}"#;
    // As src/integrations/github.rs asks.
    let (status, body) = ask(&fake.url, "POST", "/graphql", &[ua, auth, json], query);
    assert_eq!(
        (status, json_of(&body)["data"]["viewer"]["login"].clone()),
        (200, json!("me"))
    );
    let (status, body) = ask(
        &fake.url,
        "GET",
        "/repos/o/r",
        &[
            ua,
            auth,
            ("Accept", "application/vnd.github+json"),
            ("X-GitHub-Api-Version", "2022-11-28"),
        ],
        "",
    );
    assert_eq!(
        (status, json_of(&body)["default_branch"].clone()),
        (200, json!("main"))
    );

    let refused = |status: u16, message: &str, (got, body): (u16, String)| {
        assert_eq!(got, status, "{body}");
        let said = json_of(&body);
        let text = said["message"]
            .as_str()
            .or(said["errors"][0]["message"].as_str())
            .unwrap_or(&body)
            .to_string();
        assert!(text.contains(message), "{message:?} not in {body}");
    };
    refused(
        401,
        "This endpoint requires you to be authenticated.",
        ask(&fake.url, "POST", "/graphql", &[ua, json], query),
    );
    refused(
        401,
        "Bad credentials",
        ask(
            &fake.url,
            "POST",
            "/graphql",
            &[ua, ("Authorization", "Bearer other"), json],
            query,
        ),
    );
    refused(
        401,
        "Bad credentials",
        ask(
            &fake.url,
            "POST",
            "/graphql",
            &[ua, ("Authorization", "tok"), json],
            query,
        ),
    );
    refused(
        404,
        "Not Found",
        ask(&fake.url, "GET", "/graphql", &[ua, auth], ""),
    );
    refused(
        400,
        "Problems parsing JSON",
        ask(
            &fake.url,
            "POST",
            "/graphql",
            &[ua, auth, json],
            "query { x }",
        ),
    );
    refused(
        200,
        "A query attribute must be specified",
        ask(
            &fake.url,
            "POST",
            "/graphql",
            &[ua, auth, json],
            r#"{"q":"x"}"#,
        ),
    );
    refused(
        403,
        "User-Agent",
        ask(&fake.url, "POST", "/graphql", &[auth, json], query),
    );
    refused(
        404,
        "Not Found",
        ask(&fake.url, "GET", "/repos/o/r", &[ua], ""),
    );
    refused(
        404,
        "Not Found",
        ask(&fake.url, "GET", "/repos/o/nope", &[ua, auth], ""),
    );
    refused(
        404,
        "Not Found",
        ask(&fake.url, "DELETE", "/repos/o/r", &[ua, auth], ""),
    );
    refused(
        400,
        "API version 2099-01-01 is not supported",
        ask(
            &fake.url,
            "GET",
            "/repos/o/r",
            &[ua, auth, ("X-GitHub-Api-Version", "2099-01-01")],
            "",
        ),
    );
    assert_eq!(
        fake.seen().len(),
        13,
        "every request kept, refused ones too"
    );
    // What it has no such thing of is no wrong request.
    assert_eq!(fake.refused().len(), 9, "{:?}", fake.refused());
    assert!(fake.refused()[0].starts_with("POST /graphql -> 401 "));

    // A public repository's releases need no token, but a wrong one fails.
    let mut public = Fake::start(Service::GitHubPublic { token: None }, |_| {
        Some(Reply::json(200, json!([])))
    });
    public.expect_refusals();
    assert_eq!(
        ask(&public.url, "GET", "/repos/o/r/releases", &[ua], "").0,
        200
    );
    refused(
        401,
        "Bad credentials",
        ask(&public.url, "GET", "/repos/o/r/releases", &[ua, auth], ""),
    );
}

#[test]
fn linear_answers_only_its_own_key_at_its_endpoint() {
    let mut fake = Fake::linear("lin_api_x", |_| {
        Some(Reply::json(200, json!({ "data": { "issue": null } })))
    });
    fake.expect_refusals();
    let auth = ("Authorization", "lin_api_x");
    let json = ("Content-Type", "application/json");
    let query = r#"{"query":"query($id: String!) { issue(id: $id) { title } }","variables":{"id":"ENG-1"}}"#;
    // As src/integrations/linear.rs asks: the key alone, no Bearer.
    assert_eq!(
        ask(&fake.url, "POST", "/graphql", &[auth, json], query).0,
        200
    );
    let code = |(status, body): (u16, String)| {
        (
            status,
            json_of(&body)["errors"][0]["extensions"]["code"]
                .as_str()
                .unwrap_or(&body)
                .to_string(),
        )
    };
    assert_eq!(
        code(ask(&fake.url, "POST", "/graphql", &[json], query)),
        (400, "AUTHENTICATION_ERROR".into())
    );
    assert_eq!(
        code(ask(
            &fake.url,
            "POST",
            "/graphql",
            &[("Authorization", "Bearer lin_api_x"), json],
            query
        )),
        (400, "AUTHENTICATION_ERROR".into())
    );
    assert_eq!(
        code(ask(&fake.url, "GET", "/graphql", &[auth], "")),
        (400, "BAD_USER_INPUT".into())
    );
    assert_eq!(
        code(ask(&fake.url, "POST", "/graphql", &[auth], query)),
        (400, "BAD_USER_INPUT".into())
    );
    assert_eq!(
        code(ask(&fake.url, "POST", "/graphql", &[auth, json], "{}")),
        (400, "BAD_USER_INPUT".into())
    );
    assert_eq!(ask(&fake.url, "POST", "/", &[auth, json], query).0, 404);
}

#[test]
fn slack_answers_only_its_bot_token_at_its_methods() {
    let mut fake = Fake::start(
        Service::Slack {
            token: "xoxb-test".into(),
        },
        |r| {
            (r.path() == "/api/chat.postMessage")
                .then(|| Reply::json(200, json!({ "ok": true, "ts": "1.2" })))
        },
    );
    fake.expect_refusals();
    let form = ("Content-Type", "application/x-www-form-urlencoded");
    let error = |(status, body): (u16, String)| {
        (
            status,
            json_of(&body)["error"]
                .as_str()
                .unwrap_or(&body)
                .to_string(),
        )
    };
    // As src/relay/http.rs asks.
    assert_eq!(
        ask(
            &fake.url,
            "POST",
            "/api/chat.postMessage",
            &[("Authorization", "Bearer xoxb-test"), form],
            "channel=D1&text=hi"
        )
        .0,
        200
    );
    assert_eq!(
        error(ask(
            &fake.url,
            "POST",
            "/api/chat.postMessage",
            &[form],
            "channel=D1"
        )),
        (200, "not_authed".into())
    );
    assert_eq!(
        error(ask(
            &fake.url,
            "POST",
            "/api/chat.postMessage",
            &[("Authorization", "Bearer xoxb-other"), form],
            "channel=D1"
        )),
        (200, "invalid_auth".into())
    );
    assert_eq!(
        error(ask(
            &fake.url,
            "POST",
            "/api/chat.postMesage",
            &[("Authorization", "Bearer xoxb-test"), form],
            ""
        )),
        (404, "unknown_method".into())
    );
    assert_eq!(
        ask(
            &fake.url,
            "POST",
            "/chat.postMessage",
            &[("Authorization", "Bearer xoxb-test")],
            ""
        )
        .0,
        404
    );
}

#[test]
fn the_demos_claude_and_gh_refuse_what_they_dont_stand_in_for() {
    let dir = TempDir::new("sbsi");
    let demo = |args: &[&str]| {
        Command::new(common::BIN)
            .arg("demo-stand-in")
            .args(args)
            .env_clear()
            .env("HOME", dir.path())
            .stdin(Stdio::null())
            .output()
            .unwrap()
    };
    let token = demo(&["gh", "auth", "token"]);
    assert!(token.status.success());
    assert_eq!(
        String::from_utf8_lossy(&token.stdout).trim(),
        switchboard::demo::fixtures::GITHUB_TOKEN
    );
    let pr = demo(&["gh", "pr", "view", "12"]);
    assert!(!pr.status.success());
    assert!(String::from_utf8_lossy(&pr.stderr).contains("asked: pr view 12"));
    for asked in [&["claude", "-p", "hi"][..], &["claude", "mcp", "get", "x"]] {
        let out = demo(asked);
        assert!(!out.status.success(), "{asked:?}");
        assert!(String::from_utf8_lossy(&out.stderr).contains("reaches no model"));
    }
    let secret = demo(&[
        "security",
        "find-generic-password",
        "-s",
        "github-token",
        "-w",
    ]);
    assert_eq!(secret.status.code(), Some(44), "no secrets.json, no secret");
}
