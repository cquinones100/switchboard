//! `aws-login`: logs in to AWS IAM Identity Center (`aws sso login`),
//! opening the login page in your Chrome profile rather than whichever
//! Chrome window is in front. Ported from tmux/aws-login.sh, which M-p's
//! "Log in to AWS" ran in a popup; the server runs this in a pane beside
//! yours, which stays until the login is done and a key is pressed, so the
//! verification code and any error can be read.
//!
//! No profile is named: it logs in to the one `[sso-session]` in
//! `~/.aws/config`, which covers every profile that uses it. Logging in
//! through a profile with its own `sso_start_url` is cached apart and
//! doesn't cover those. With several sessions, it says which there are
//! rather than guess.
//!
//! The page opens through `switchboard open-url --as awsapps.com`, given
//! to aws as its `BROWSER`, so it opens in the Chrome profile
//! `~/.config/switchboard/browser.json` gives AWS's access portal
//! (`{"hosts": {"awsapps.com": "Profile 2"}}`), else its `default`, else
//! the default browser, whatever host AWS's sign-in page is on; with
//! Settings' chrome_profiles off, in the default browser, as every link
//! then. `aws` is yours to install: Switchboard needs it for this alone.

use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, ExitCode};

use crate::{paths, requests, util, worktree};

const DIM: &str = "\x1b[2m";
const RED: &str = "\x1b[31m";
const GREEN: &str = "\x1b[32m";
const RESET: &str = "\x1b[0m";

/// The `[sso-session <name>]` sections of an AWS config file.
pub fn sso_sessions(config: &str) -> Vec<String> {
    config
        .lines()
        .filter_map(|l| {
            l.trim()
                .strip_prefix("[sso-session ")?
                .strip_suffix(']')
                .map(|n| n.trim().to_string())
        })
        .filter(|n| !n.is_empty())
        .collect()
}

/// The session to log in to, or what to say instead.
pub fn session_to_use(sessions: &[String]) -> Result<String, String> {
    match sessions {
        [] => Err("There is no [sso-session] in ~/.aws/config.".into()),
        [one] => Ok(one.clone()),
        several => Err(format!(
            "There are several SSO sessions in ~/.aws/config: {}",
            several.join(" ")
        )),
    }
}

/// What aws runs to open a page: this binary's `open-url`, choosing the
/// profile as for the access portal. Python's webbrowser, which aws uses,
/// splits a `BROWSER` holding `%s` as a shell would and puts the link there.
pub fn browser_command(exe: &Path) -> String {
    format!(
        "{} open-url --as awsapps.com %s",
        worktree::shell_quote(&exe.to_string_lossy())
    )
}

/// Says what came of it and waits for a key, so the pane can be read
/// before it closes.
fn done_with(message: &str) -> ExitCode {
    print!("\r\n  {message}\r\n\r\n  {DIM}Press any key to close.{RESET}");
    let _ = std::io::stdout().flush();
    let _ = crossterm::terminal::enable_raw_mode();
    let mut key = [0u8; 16];
    let _ = std::io::stdin().read(&mut key);
    let _ = crossterm::terminal::disable_raw_mode();
    ExitCode::SUCCESS
}

pub fn main(args: Vec<String>) -> ExitCode {
    if !args.is_empty() {
        eprintln!("Usage: switchboard aws-login");
        return ExitCode::from(2);
    }
    if util::which("aws").is_none() {
        return done_with(&format!(
            "{RED}aws isn't on the PATH.{RESET} Install the AWS CLI, then try again."
        ));
    }
    let config = util::read_text(paths::home_join(".aws/config")).unwrap_or_default();
    let session = match session_to_use(&sso_sessions(&config)) {
        Ok(s) => s,
        Err(why) => return done_with(&format!("{RED}{why}{RESET}")),
    };
    println!("\n  {DIM}Logging in to {session}...{RESET}\n");
    let mut aws = Command::new("aws");
    aws.args(["sso", "login", "--sso-session", &session]);
    if let Ok(exe) = util::exe() {
        aws.env("BROWSER", browser_command(&exe));
    }
    requests::count("AWS", "HTTPS", "AWS SSO login");
    let ok = aws.status().is_ok_and(|s| s.success());
    if ok {
        done_with(&format!("{GREEN}Logged in.{RESET}"))
    } else {
        done_with(&format!("{RED}The login failed.{RESET}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_one_sso_session_is_the_one_logged_in_to() {
        let config = "[default]\nregion = x\n[sso-session work]\nsso_start_url = https://example.awsapps.com/start\n[profile dev]\nsso_session = work\n";
        assert_eq!(sso_sessions(config), ["work"]);
        assert_eq!(session_to_use(&sso_sessions(config)), Ok("work".into()));
        assert!(
            session_to_use(&[])
                .unwrap_err()
                .contains("no [sso-session]")
        );
        let two = ["a".to_string(), "b".to_string()];
        assert!(session_to_use(&two).unwrap_err().ends_with("a b"));
    }

    #[test]
    fn aws_opens_pages_through_open_url_as_the_portal() {
        assert_eq!(
            browser_command(Path::new("/opt/my apps/switchboard")),
            "'/opt/my apps/switchboard' open-url --as awsapps.com %s"
        );
    }
}
