//! `switchboard platform <what>`: what differs between macOS and Linux, for
//! the shell scripts, which call this rather than `security`, `pbcopy`,
//! `osascript` or `open`, so they run on both.
//!
//!   platform name                      macOS or Linux
//!   platform secret <service> [-a <account>]
//!                                      prints the secret; exits 1 with none
//!   platform secret-set <service> [-a <account>]
//!                                      stores the secret read from stdin
//!                                      (never an argument, which ps shows)
//!   platform copy                      puts stdin on the clipboard
//!   platform paste                     prints the clipboard
//!   platform notify <title> <text>     a desktop notification
//!   platform presence                  "<idle ms>\t<locked: yes/no>", either
//!                                      blank when unknown
//!   platform open <file or link>       opens it with the desktop's app
//!   platform front-app                 the app in front; blank when unknown
//!   platform chrome-local-state        where Chrome lists its profiles

use std::io::Read;
use std::process::ExitCode;

use crate::platform;

fn usage() -> ExitCode {
    eprintln!(
        "Usage: switchboard platform name|secret|secret-set|copy|paste|notify|presence|open|front-app|chrome-local-state"
    );
    ExitCode::from(2)
}

/// The value after `-a`/`--account` in `args`, if any.
fn account(args: &[String]) -> Option<&str> {
    args.windows(2)
        .find(|w| w[0] == "-a" || w[0] == "--account")
        .map(|w| w[1].as_str())
}

pub fn main(args: Vec<String>) -> ExitCode {
    let Some((what, rest)) = args.split_first() else {
        return usage();
    };
    match what.as_str() {
        "name" => println!("{}", platform::NAME),
        "secret" => {
            let Some(service) = rest.first() else {
                return usage();
            };
            let found = match account(rest) {
                Some(a) => platform::secret_for(service, a),
                None => platform::secret(service),
            };
            match found {
                Some(s) => println!("{s}"),
                None => return ExitCode::FAILURE,
            }
        }
        "secret-set" => {
            let Some(service) = rest.first() else {
                return usage();
            };
            let user = std::env::var("USER").unwrap_or_default();
            let mut value = String::new();
            if std::io::stdin().read_to_string(&mut value).is_err() {
                return ExitCode::FAILURE;
            }
            match platform::set_secret(service, account(rest).unwrap_or(&user), value.trim()) {
                Ok(where_) => eprintln!("Stored in {where_}."),
                Err(e) => {
                    eprintln!("Couldn't store it: {e}");
                    return ExitCode::FAILURE;
                }
            }
        }
        "copy" => {
            let mut text = String::new();
            if std::io::stdin().read_to_string(&mut text).is_err() {
                return ExitCode::FAILURE;
            }
            if let Err(e) = platform::copy(&text) {
                eprintln!("{e}");
                return ExitCode::FAILURE;
            }
        }
        "paste" => match platform::paste() {
            Some(text) => print!("{text}"),
            None => return ExitCode::FAILURE,
        },
        "notify" => {
            let [title, text, ..] = rest else {
                return usage();
            };
            platform::notify(title, text);
        }
        "presence" => {
            let p = platform::presence();
            let idle = p
                .idle_ms
                .map(|ms| format!("{}", ms.round() as i64))
                .unwrap_or_default();
            let locked = p
                .locked
                .map(|l| if l { "yes" } else { "no" })
                .unwrap_or_default();
            println!("{idle}\t{locked}");
        }
        "open" => {
            let Some(target) = rest.first() else {
                return usage();
            };
            if let Err(e) = platform::open(target) {
                eprintln!("{e}");
                return ExitCode::FAILURE;
            }
        }
        "front-app" => println!("{}", platform::front_app().unwrap_or_default()),
        "chrome-local-state" => match platform::chrome_local_state() {
            Some(p) => println!("{}", p.display()),
            None => return ExitCode::FAILURE,
        },
        _ => return usage(),
    }
    ExitCode::SUCCESS
}
