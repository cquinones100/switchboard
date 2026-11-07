//! The `terminal` part: Option acting as Alt, which every `M-` key needs on
//! a Mac, where Option otherwise types a character (`˙`, `∆`) into the
//! pane. Ported from setup.sh's `part_terminal`, `terminal_app_option`,
//! `iterm_option` and `terminal_hint`.
//!
//! What it can change, it changes: a Ghostty config of your own gets
//! `macos-option-as-alt = true` unless it already says how Option acts (and
//! one is made when you are in Ghostty with none), and the terminal this
//! runs in, when it is Terminal or iTerm2, has its default profile changed
//! through macOS's own preferences (`platform::set_option_as_alt`). For any
//! other terminal, `hint` gives the line to add to its config.
//!
//! On Linux a terminal sends Alt as it is, so there is nothing to change;
//! only xterm, which by default sets the eighth bit instead, gets a hint.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

use anyhow::{Context, Result};
use regex::Regex;

use super::{Report, home_tilde};
use crate::paths;
use crate::platform::{self, TerminalApp};

/// The terminal this runs in, as its environment says: setup.sh's
/// `terminal_name`. Inside Switchboard a pane has the environment the
/// server started with, which is the terminal's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Term {
    Ghostty,
    Terminal,
    ITerm,
    WezTerm,
    Kitty,
    Alacritty,
    Xterm,
    Other,
}

pub fn term() -> Term {
    let var = |name: &str| std::env::var(name).unwrap_or_default().to_lowercase();
    let set = |name: &str| std::env::var_os(name).is_some_and(|v| !v.is_empty());
    if set("KITTY_WINDOW_ID") {
        return Term::Kitty;
    }
    if set("ALACRITTY_WINDOW_ID") || set("ALACRITTY_SOCKET") {
        return Term::Alacritty;
    }
    let name = format!("{} {}", var("TERM_PROGRAM"), var("LC_TERMINAL"));
    if name.contains("ghostty") {
        Term::Ghostty
    } else if name.contains("apple_terminal") {
        Term::Terminal
    } else if name.contains("iterm") {
        Term::ITerm
    } else if name.contains("wezterm") {
        Term::WezTerm
    } else if set("XTERM_VERSION") {
        Term::Xterm
    } else {
        Term::Other
    }
}

/// What to add for a terminal this part can't change, or `None` when it
/// can, or there is nothing to add.
pub fn hint() -> Option<&'static str> {
    if platform::NAME != "macOS" {
        return (term() == Term::Xterm)
            .then_some("xterm: add XTerm*metaSendsEscape: true to ~/.Xresources");
    }
    match term() {
        Term::Ghostty | Term::Terminal | Term::ITerm => None,
        Term::WezTerm => Some(
            "WezTerm: add send_composed_key_when_left_alt_is_pressed = false and send_composed_key_when_right_alt_is_pressed = false to your wezterm.lua",
        ),
        Term::Kitty => Some("kitty: add macos_option_as_alt yes to kitty.conf"),
        Term::Alacritty => {
            Some("Alacritty: add option_as_alt = \"Both\" under [window] in alacritty.toml")
        }
        Term::Xterm | Term::Other => {
            Some("your terminal: make Option send Alt (Esc+, or \"Option as Meta\")")
        }
    }
}

/// Ghostty's config file: the first there of the places it reads one from,
/// else `None`.
fn ghostty_config() -> Option<PathBuf> {
    let xdg = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| paths::home_join(".config"))
        .join("ghostty");
    let mac = paths::home_join("Library/Application Support/com.mitchellh.ghostty");
    [&xdg, &mac]
        .iter()
        .flat_map(|dir| [dir.join("config"), dir.join("config.ghostty")])
        .find(|f| f.is_file())
}

/// Whether a Ghostty config already says how Option acts.
pub fn ghostty_says(text: &str) -> bool {
    Regex::new(r"(?m)^\s*macos-option-as-alt\s*=")
        .expect("a valid pattern")
        .is_match(text)
}

const GHOSTTY_LINES: &str =
    "\n# Switchboard: Option as Alt, which its M- keys need.\nmacos-option-as-alt = true\n";

/// The part.
pub fn run(r: &mut Report) -> Result<()> {
    if platform::NAME != "macOS" {
        return Ok(());
    }
    // Ghostty: a config of your own, or one made when you are in Ghostty.
    let config = ghostty_config()
        .or_else(|| (term() == Term::Ghostty).then(|| paths::home_join(".config/ghostty/config")));
    if let Some(config) = config {
        let text = fs::read_to_string(&config).unwrap_or_default();
        if !ghostty_says(&text) {
            if !r.check {
                if let Some(dir) = config.parent() {
                    fs::create_dir_all(dir)?;
                }
                OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&config)
                    .and_then(|mut f| f.write_all(GHOSTTY_LINES.as_bytes()))
                    .with_context(|| format!("can't add to {}", home_tilde(&config)))?;
            }
            r.change(format!(
                "make Option act as Alt in Ghostty's config, {} (reload its config, or open Ghostty again)",
                home_tilde(&config)
            ));
        }
    }
    let app = match term() {
        Term::Terminal => TerminalApp::Terminal,
        Term::ITerm => TerminalApp::ITerm,
        _ => return Ok(()),
    };
    if let Some((false, profile)) = platform::option_as_alt(app) {
        if !r.check {
            platform::set_option_as_alt(app)
                .with_context(|| format!("couldn't change the \"{profile}\" profile"))?;
        }
        r.change(match app {
            TerminalApp::Terminal => format!(
                "make Option act as Meta in Terminal's \"{profile}\" profile (open a new window, or quit Terminal and open it again)"
            ),
            TerminalApp::ITerm => format!(
                "make both Option keys send Esc+ in iTerm2's \"{profile}\" profile (quit iTerm2 and open it again)"
            ),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_ghostty_config_saying_how_option_acts_is_left_alone() {
        assert!(ghostty_says("font-size = 13\nmacos-option-as-alt = left\n"));
        assert!(ghostty_says("  macos-option-as-alt=false"));
        assert!(!ghostty_says(
            "# macos-option-as-alt = true\nfont-size = 13\n"
        ));
    }
}
