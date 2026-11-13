//! The `lazygit` part: a lazygit config where you have none, so lazygit in
//! a pane's place (`M-p`'s "Open lazygit in place of this pane") quits on
//! Esc, as the diff and the views do, and the pane behind it comes back.
//! The tmux version linked its own `lazygit/config.yml`, removed with it
//! (commit e7e32268), whose `quitOnTopLevelReturn: true` is all of it that
//! Switchboard needs; its delta renderer and custom commands were that
//! setup's own, so a new install gets only `assets/lazygit.yml`.
//!
//! A config of yours, wherever lazygit reads one from, is never changed:
//! the guided setup only says the line to add when it lacks it. Offered
//! only where lazygit is installed, since Switchboard doesn't need it.

use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};

use super::{Report, home_tilde};
use crate::{paths, platform, util};

/// What a new config holds, compiled in.
pub const ASSET: &str = include_str!("../../assets/lazygit.yml");

/// Whether lazygit is installed.
pub fn installed() -> bool {
    util::which("lazygit").is_some()
}

/// A variable's value, unset when empty.
fn var(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

/// Every file lazygit may read its config from, by its own rules, the one
/// it would make first: `LG_CONFIG_FILE`'s files (comma-separated),
/// `$CONFIG_DIR/config.yml`, then `config.yml` under `lazygit` (and the
/// older `jesseduffield/lazygit`) in the XDG config home, which on a Mac
/// without `XDG_CONFIG_HOME` is `~/Library/Application Support`, and
/// `~/.config`, where the tmux version linked it.
fn places(
    lg_config_file: Option<&str>,
    config_dir: Option<&str>,
    xdg_config_home: Option<&str>,
    mac: bool,
) -> Vec<PathBuf> {
    let mut places: Vec<PathBuf> = lg_config_file
        .map(|files| {
            files
                .split(',')
                .filter(|f| !f.trim().is_empty())
                .map(|f| PathBuf::from(f.trim()))
                .collect()
        })
        .unwrap_or_default();
    if let Some(dir) = config_dir {
        places.push(PathBuf::from(dir).join("config.yml"));
    }
    let home = match xdg_config_home {
        Some(dir) => PathBuf::from(dir),
        None if mac => paths::home_join("Library/Application Support"),
        None => paths::home_join(".config"),
    };
    let dot_config = paths::home_join(".config");
    for dir in [&home, &dot_config] {
        for sub in ["lazygit", "jesseduffield/lazygit"] {
            let file = dir.join(sub).join("config.yml");
            if !places.contains(&file) {
                places.push(file);
            }
        }
    }
    places
}

/// Where lazygit reads its config here.
fn here() -> Vec<PathBuf> {
    places(
        var("LG_CONFIG_FILE").as_deref(),
        var("CONFIG_DIR").as_deref(),
        var("XDG_CONFIG_HOME").as_deref(),
        platform::NAME == "macOS",
    )
}

/// Your lazygit config, when there is one.
pub fn yours() -> Option<PathBuf> {
    here().into_iter().find(|f| f.is_file())
}

/// Whether a config already says what Esc does at the top level.
pub fn says_esc(text: &str) -> bool {
    text.lines()
        .any(|l| l.trim_start().starts_with("quitOnTopLevelReturn:"))
}

/// The part.
pub fn run(r: &mut Report) -> Result<()> {
    if !installed() {
        bail!("lazygit isn't installed, so there is nothing to set up");
    }
    if yours().is_some() {
        return Ok(());
    }
    let file = here().remove(0);
    if !r.check {
        if let Some(dir) = file.parent() {
            fs::create_dir_all(dir)?;
        }
        util::write_atomic(&file, crate::assets::text("lazygit.yml", ASSET).as_bytes())
            .with_context(|| format!("can't write {}", home_tilde(&file)))?;
    }
    r.change(format!(
        "write a lazygit config, {}, so Esc quits lazygit in a pane's place",
        home_tilde(&file)
    ));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lazygit_s_config_is_looked_for_where_lazygit_reads_it() {
        let home = paths::home();
        let mac = places(None, None, None, true);
        assert_eq!(
            mac[0],
            home.join("Library/Application Support/lazygit/config.yml")
        );
        assert!(mac.contains(&home.join(".config/lazygit/config.yml")));
        let linux = places(None, None, None, false);
        assert_eq!(linux[0], home.join(".config/lazygit/config.yml"));
        assert_eq!(linux.len(), 2, "{linux:?}");
        let xdg = places(None, None, Some("/x"), true);
        assert_eq!(xdg[0], PathBuf::from("/x/lazygit/config.yml"));
        let given = places(Some("/a.yml, /b.yml"), Some("/d"), None, false);
        assert_eq!(
            given[..3],
            [
                PathBuf::from("/a.yml"),
                PathBuf::from("/b.yml"),
                PathBuf::from("/d/config.yml")
            ]
        );
    }

    #[test]
    fn a_config_saying_what_esc_does_is_recognized() {
        assert!(says_esc("gui: {}\nquitOnTopLevelReturn: false\n"));
        assert!(says_esc(ASSET));
        assert!(!says_esc("# quitOnTopLevelReturn: true\n"));
    }
}
