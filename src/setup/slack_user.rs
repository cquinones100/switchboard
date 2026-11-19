//! `switchboard setup slack-user`: your own Slack sign-in, for the Slack
//! view's reminders and read marks (`day::slack_user`), which Claude's Slack
//! connector can't give. It says how to add the user scopes to an app of
//! yours (the Slack relay's works) and reinstall it, takes the user token
//! (`xoxp-…`) without showing it, asks Slack who it is for (`auth.test`),
//! and only then stores it as a secret (`platform::set_secret`: the
//! Keychain, or Linux's keyring, and where that can't take it, the
//! `secrets.json` only you can read), in no file of its own, and keeps who
//! it is for in the state directory, letting go of the reminders kept for
//! a sign-in before it, which is how the server knows there is a sign-in.
//! It then asks for the same app's app-level token (`xapp-…`), for the
//! Slack view to take messages as they're posted (`day::slack_live`), which
//! may be skipped: checked with `apps.connections.open` and stored as a
//! secret of its own, the sign-in marked as having one. New with the Rust
//! version, with no tmux-version file behind it.
//!
//! It is no part of the guided setup: it asks for a token, so it runs only
//! when named.

use std::io::{BufRead, IsTerminal, Write};
use std::process::ExitCode;

use crate::day::slack_live::APP_TOKEN_SERVICE;
use crate::day::slack_user::{self, SCOPES, TOKEN_SERVICE};
use crate::{paths, platform, util};

/// What `switchboard setup --help` says of it.
pub const ABOUT: &str = "store a Slack user token of your own, for the Slack view's reminders, read marks and reads, and its app-level token for messages as they're posted (asks for the tokens; never part of the guided setup)";

pub fn main() -> ExitCode {
    println!("Your own Slack sign-in, for the Slack view's reminders, read marks and reads.\n");
    println!("  1. Open https://api.slack.com/apps and choose an app of yours (the Slack");
    println!("     relay's works, but not for messages as they're posted, below).");
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
    // Both tokens are asked for and checked before either is stored, so
    // the sign-in and its tokens change together; a canceled or refused
    // app-level token changes nothing.
    let app = match app_token() {
        Ok(app) => app,
        Err(why) => {
            eprintln!("{why} Nothing changed.");
            return ExitCode::from(1);
        }
    };
    let user = std::env::var("USER").unwrap_or_default();
    let before = slack_user::signed_in(&paths::state_dir());
    let stored = match platform::set_secret(TOKEN_SERVICE, &user, token) {
        Ok(place) => place,
        Err(e) => {
            eprintln!("Couldn't store the token: {e:#}");
            return ExitCode::from(1);
        }
    };
    let app_stored =
        app.is_some_and(
            |app| match platform::set_secret(APP_TOKEN_SERVICE, &user, &app) {
                Ok(place) => {
                    println!("Stored the app-level token in {place}.");
                    true
                }
                Err(e) => {
                    eprintln!("Couldn't store the app-level token: {e:#}");
                    false
                }
            },
        );
    let mut who = who;
    // Skipped or not taken, one stored before for the same member still
    // counts.
    who.live =
        app_stored || before.is_some_and(|b| b.live && b.team == who.team && b.user == who.user);
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
    if who.live {
        println!("New messages reach the view as they're posted (Settings' \"Take Slack");
        println!("messages as they're posted\").");
    }
    ExitCode::SUCCESS
}

/// Asks for the app's app-level token, for messages as they're posted,
/// and checks it with Slack: the token, `None` when skipped (Enter alone),
/// or why it wasn't taken.
fn app_token() -> Result<Option<String>, String> {
    println!("\nTo have the Slack view take messages as they're posted, in an app of yours");
    println!("that isn't the Slack relay's:");
    println!("  1. Under Socket Mode, turn it on and generate an app-level token with the");
    println!("     connections:write scope (it starts with xapp-).");
    println!("  2. Under Event Subscriptions, turn them on and, under events on behalf of");
    println!("     users, add message.channels, message.groups, message.im and message.mpim.");
    println!("  3. Reinstall the app to your workspace.\n");
    print!("Paste the app-level token, or press Enter to skip (it isn't shown): ");
    let _ = std::io::stdout().flush();
    let token = read_token().map_err(|e| format!("Couldn't read the app-level token: {e}."))?;
    let token = token.trim();
    if token.is_empty() {
        return Ok(None);
    }
    if !token.starts_with("xapp-") {
        return Err("That isn't an app-level token, which starts with xapp-.".to_string());
    }
    if crate::relay::slack::token(crate::relay::slack::APP_TOKEN_SERVICE).as_deref() == Some(token)
    {
        return Err(
            "That is the Slack relay's app-level token, whose events it would take; use an app of your own.".to_string(),
        );
    }
    #[cfg(feature = "net")]
    if let Err(e) = util::block_on(crate::relay::http::open_socket(token, "setup: slack-user")) {
        return Err(format!("Slack didn't take the app-level token: {e}."));
    }
    Ok(Some(token.to_string()))
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
                    break Err(std::io::Error::new(
                        std::io::ErrorKind::Interrupted,
                        "canceled",
                    ));
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
