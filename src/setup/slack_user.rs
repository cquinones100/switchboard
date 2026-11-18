//! `switchboard setup slack-user`: your own Slack sign-in, for the Slack
//! view's reminders and read marks (`day::slack_user`), which Claude's Slack
//! connector can't give. It says how to add the user scopes to an app of
//! yours (the Slack relay's works) and reinstall it, takes the user token
//! (`xoxp-…`) without showing it, asks Slack who it is for (`auth.test`),
//! and only then stores it as a secret (`platform::set_secret`: the
//! Keychain, or Linux's keyring, and where that can't take it, the
//! `secrets.json` only you can read), in no file of its own, and keeps who
//! it is for in the state directory, letting go of the reminders kept for
//! a sign-in before it, which is how the server knows there is a sign-in. New with the Rust version, with no
//! tmux-version file behind it.
//!
//! It is no part of the guided setup: it asks for a token, so it runs only
//! when named.

use std::io::{BufRead, IsTerminal, Write};
use std::process::ExitCode;

use crate::day::slack_user::{self, SCOPES, TOKEN_SERVICE};
use crate::{paths, platform, util};

/// What `switchboard setup --help` says of it.
pub const ABOUT: &str = "store a Slack user token of your own, for the Slack view's reminders, read marks and reads (asks for the token; never part of the guided setup)";

pub fn main() -> ExitCode {
    println!("Your own Slack sign-in, for the Slack view's reminders, read marks and reads.\n");
    println!("  1. Open https://api.slack.com/apps and choose an app of yours (the Slack");
    println!("     relay's works).");
    println!("  2. Under OAuth & Permissions, add these User Token Scopes:");
    for pair in SCOPES.chunks(2) {
        println!("       {}", pair.join(", "));
    }
    println!("  3. Reinstall the app to your workspace, and copy its User OAuth Token");
    println!("     (it starts with xoxp-).\n");
    print!("Paste the token (it isn't shown): ");
    let _ = std::io::stdout().flush();
    let token = match read_token() {
        Ok(token) => token,
        Err(e) => {
            eprintln!("Couldn't read the token: {e}");
            return ExitCode::from(1);
        }
    };
    let token = token.trim();
    if token.is_empty() {
        eprintln!("No token given; nothing changed.");
        return ExitCode::from(1);
    }
    if !token.starts_with("xoxp-") {
        eprintln!(
            "That isn't a user token, which starts with xoxp- (a bot token, xoxb-, can't read your reminders); nothing changed."
        );
        return ExitCode::from(1);
    }
    let who = match util::block_on(slack_user::auth_test(token)) {
        Ok(who) => who,
        Err(e) => {
            eprintln!("Slack didn't take it: {e}. Nothing changed.");
            return ExitCode::from(1);
        }
    };
    let user = std::env::var("USER").unwrap_or_default();
    let stored = match platform::set_secret(TOKEN_SERVICE, &user, token) {
        Ok(place) => place,
        Err(e) => {
            eprintln!("Couldn't store the token: {e:#}");
            return ExitCode::from(1);
        }
    };
    if let Err(e) = slack_user::save_sign_in(&paths::state_dir(), &who) {
        eprintln!("Stored the token, but couldn't keep who it is for: {e}");
        return ExitCode::from(1);
    }
    println!("Stored in {stored}.");
    let place = match who.url.as_str() {
        "" => "Slack".to_string(),
        url => url.trim_end_matches('/').to_string(),
    };
    println!("Signed in to {place}.\n");
    println!("Your reminders that are due show in mission control's Slack view within two");
    println!("minutes (Settings' \"Slack reminders in the Slack view\"). To mark what you open");
    println!("there read in Slack, turn on Settings' \"Mark what you open read in Slack\".");
    ExitCode::SUCCESS
}

/// The token typed or pasted at a terminal, unshown, up to Enter; else the
/// first line of the input, so it can be piped in. A closed input is none.
fn read_token() -> std::io::Result<String> {
    if !std::io::stdin().is_terminal() {
        let mut line = String::new();
        std::io::stdin().lock().read_line(&mut line)?;
        println!();
        return Ok(line);
    }
    use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers, read};
    crossterm::terminal::enable_raw_mode()?;
    let mut token = String::new();
    let read_all = loop {
        match read() {
            Ok(Event::Key(key)) if key.kind != KeyEventKind::Release => match key.code {
                KeyCode::Enter => break Ok(()),
                KeyCode::Char('c' | 'd') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    token.clear();
                    break Ok(());
                }
                KeyCode::Char(c) => token.push(c),
                KeyCode::Backspace => {
                    token.pop();
                }
                _ => {}
            },
            Ok(Event::Paste(text)) => token.push_str(&text),
            Ok(_) => {}
            Err(e) => break Err(e),
        }
    };
    let _ = crossterm::terminal::disable_raw_mode();
    println!();
    read_all.map(|()| token)
}
