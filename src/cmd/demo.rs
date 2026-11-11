//! `switchboard demo [--detached | --print | --reset | --clean | --tour
//! <dir>]`: the workspace on made-up data (src/demo), in a home of its own
//! that links nothing of yours and reaches no service. Alone it attaches in
//! this terminal; `--detached` starts its server with no client and prints
//! the prefix that reaches it, for `send-keys`, `list` and `switchboard
//! headless`; `--print` prints the environment to `eval` in another
//! terminal; `--reset` lays its data out again; `--clean` ends it and
//! removes its home; `--tour <dir>` walks its boards with a headless client
//! and saves a picture of each.
//!
//! Also here, for the demo alone: `demo-services <home>`, its services'
//! process, and `demo-stand-in <name>`, its stand-in programs.

use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Result, anyhow, bail};

use crate::demo;
use crate::worktree::shell_quote;

#[derive(PartialEq, Eq)]
enum Mode {
    Attach,
    Detached,
    Print,
    Clean,
    Tour(PathBuf),
}

fn usage() {
    println!("Usage: switchboard demo [--detached | --print | --reset | --clean]");
    println!("       switchboard demo --tour <dir> [--size <cols>x<rows>]\n");
    println!("Runs the workspace on made-up data, in a home of its own (/tmp/sb-demo-<uid>)");
    println!("that links nothing of yours and reaches no service: GitHub, Linear and Slack are");
    println!("stand-ins on this computer, and its Claude plays a script. Alone it attaches");
    println!("here; --detached starts it with no client and prints how to drive it; --print");
    println!("prints the environment to eval in another terminal; --reset lays the data out");
    println!("again; --clean ends it and removes its home; --tour walks its boards with a");
    println!("headless client and saves a picture of each in <dir>.");
}

pub fn main(args: Vec<String>) -> ExitCode {
    let mut mode = Mode::Attach;
    let mut reset = false;
    let mut size = (160u16, 45u16);
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--detached" => mode = Mode::Detached,
            "--print" => mode = Mode::Print,
            "--clean" => mode = Mode::Clean,
            "--reset" => reset = true,
            "--tour" => match args.get(i + 1) {
                Some(dir) => {
                    mode = Mode::Tour(PathBuf::from(dir));
                    i += 1;
                }
                None => {
                    eprintln!("switchboard demo: --tour needs a folder");
                    return ExitCode::from(2);
                }
            },
            "--size" => match args.get(i + 1).and_then(|s| super::headless::parse_size(s)) {
                Some(s) => {
                    size = s;
                    i += 1;
                }
                None => {
                    eprintln!("switchboard demo: --size takes <cols>x<rows>");
                    return ExitCode::from(2);
                }
            },
            "-h" | "--help" => {
                usage();
                return ExitCode::SUCCESS;
            }
            other => {
                eprintln!("switchboard demo: unknown option {other}");
                return ExitCode::from(2);
            }
        }
        i += 1;
    }
    match run(mode, reset, size) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("switchboard demo: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn run(mode: Mode, reset: bool, size: (u16, u16)) -> Result<ExitCode> {
    let home = demo::home_in(&demo::root());
    if mode == Mode::Clean {
        if !home.exists() {
            println!("No demo to clean.");
            return Ok(ExitCode::SUCCESS);
        }
        demo::clean(&home)?;
        println!("Removed {} and stopped its server.", home.display());
        return Ok(ExitCode::SUCCESS);
    }
    if mode == Mode::Attach && std::env::var_os("SWITCHBOARD").is_some_and(|s| !s.is_empty()) {
        bail!(
            "you're in one of Switchboard's panes, whose workspace would take every key of the demo's; run it in a terminal of its own, or use --detached and switchboard headless to drive it"
        );
    }
    let bin = std::env::current_exe()
        .and_then(|b| b.canonicalize())
        .map_err(|e| anyhow!("can't find the binary running: {e}"))?;
    demo::check_home(&home)?;
    if reset || !demo::laid_out(&home) {
        if home.exists() {
            crate::cmd::try_::stop_server(&demo::socket_of(&home));
            demo::stop_services(&home);
        }
        eprintln!("Laying out the demo in {}…", home.display());
        demo::lay_out(&home, &bin)?;
    }
    // The scripts name the binary they run: written again from this one, so
    // a demo first run from a build that has since gone runs on this.
    demo::stand_in::install(&home, &bin)?;
    let services = demo::start_services(&home, &bin)?;
    let env = demo::env(&home, &services);
    let demo_bin = home.join(".local/bin/switchboard");
    match mode {
        Mode::Print => {
            demo::start_server(&home, &bin, &env)?;
            print!("{}", exports(&env));
            Ok(ExitCode::SUCCESS)
        }
        Mode::Detached => {
            demo::start_server(&home, &bin, &env)?;
            let prefix = prefix(&env, &demo_bin);
            println!(
                "The demo's server is running with no client. Its commands run with this prefix:\n"
            );
            println!("  {prefix}\n");
            println!(
                "followed by list, send-keys, capture-pane, or headless (which prints a client's"
            );
            println!(
                "id), then headless-keys <id> M-Space and headless-screen <id> [--ansi | --png"
            );
            println!("<file>]. switchboard demo --clean ends it.");
            Ok(ExitCode::SUCCESS)
        }
        Mode::Tour(dir) => {
            demo::start_server(&home, &bin, &env)?;
            let dir = absolute(&dir)?;
            let saved = demo::tour::run(&demo::socket_of(&home), &dir, size.0, size.1)?;
            println!("Saved {} boards in {}.", saved.len(), dir.display());
            Ok(ExitCode::SUCCESS)
        }
        _ => {
            demo::start_server(&home, &bin, &env)?;
            let mut command = demo::command(&demo_bin, &env);
            command.current_dir(home.join("code"));
            let e = command.exec();
            bail!("couldn't run {}: {e}", demo_bin.display())
        }
    }
}

fn absolute(path: &Path) -> Result<PathBuf> {
    Ok(if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    })
}

/// The lines to `eval` in a shell, after which `switchboard` is the demo's.
fn exports(env: &[(String, String)]) -> String {
    let mut unset: Vec<&str> = crate::cmd::try_::LEFT_OUT.to_vec();
    // And what reaches your desktop session, a token or the network: the
    // session bus (Linux's keyring and notifications), its runtime folder,
    // the display, a proxy.
    unset.extend([
        "GITHUB_TOKEN",
        "SSH_AUTH_SOCK",
        "SSH_AGENT_PID",
        "DBUS_SESSION_BUS_ADDRESS",
        "XDG_RUNTIME_DIR",
        "DISPLAY",
        "GNOME_KEYRING_CONTROL",
        // A proxy would carry the stand-ins' requests off the computer.
        "HTTP_PROXY",
        "http_proxy",
        "HTTPS_PROXY",
        "https_proxy",
        "ALL_PROXY",
        "all_proxy",
    ]);
    let mut out = format!("unset {}\n", unset.join(" "));
    for (k, v) in env {
        out.push_str(&format!("export {k}={}\n", shell_quote(v)));
    }
    out.push_str("# switchboard now attaches to the demo; switchboard demo --clean ends it\n");
    out
}

/// The command prefix that runs `bin` in the demo's environment alone.
fn prefix(env: &[(String, String)], bin: &Path) -> String {
    let mut words = vec!["env".to_string(), "-i".into()];
    for (k, v) in env {
        words.push(format!("{k}={}", shell_quote(v)));
    }
    words.push(shell_quote(&bin.to_string_lossy()));
    words.join(" ")
}

/// `switchboard demo-services <home>`: the demo's services, until ended.
pub fn services_main(args: Vec<String>) -> ExitCode {
    let Some(home) = args.first().map(PathBuf::from) else {
        eprintln!("Usage: switchboard demo-services <home>");
        return ExitCode::from(2);
    };
    if !demo::laid_out(&home) {
        eprintln!(
            "switchboard demo-services: {} isn't a demo's home",
            home.display()
        );
        return ExitCode::FAILURE;
    }
    match demo::serve_services(&home) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("switchboard demo-services: {e:#}");
            ExitCode::FAILURE
        }
    }
}
