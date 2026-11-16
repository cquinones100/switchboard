//! The `lazygit` part: Switchboard's own lazygit settings, so lazygit in
//! a pane's place (`M-p`'s "Open lazygit in place of this pane") quits on
//! Esc, as the diff and the views do, and the pane behind it comes back.
//! The tmux version linked its own `lazygit/config.yml`, removed with it
//! (commit e7e32268), whose `quitOnTopLevelReturn: true` is all of it that
//! Switchboard needs; its delta renderer and custom commands were that
//! setup's own, so Switchboard's settings are only `assets/lazygit.yml`.
//!
//! They reach lazygit at launch, never through its config file: lazygit
//! opened in a pane's place is given `LG_CONFIG_FILE` naming Switchboard's
//! copy in `~/.local/share/switchboard/lazygit.yml` and then your config
//! (where lazygit reads one, when there is one), which lazygit layers in
//! that order, so what yours says wins: Switchboard's are defaults, and a
//! `quitOnTopLevelReturn: false` of yours keeps Esc as it was. A config of yours, wherever lazygit reads one from, is never
//! written, a config an earlier setup wrote there included, which is yours.

use std::fs;
use std::path::{Path, PathBuf};

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

/// Switchboard's copy of its lazygit settings, under `~`.
pub const OWN: &str = ".local/share/switchboard/lazygit.yml";

/// Switchboard's copy, `~/.local/share/switchboard/lazygit.yml`.
pub fn own() -> PathBuf {
    paths::home_join(OWN)
}

/// Whether Switchboard's copy is missing or says something other than
/// `text`.
fn own_is_stale(file: &Path, text: &str) -> bool {
    fs::read_to_string(file).map_or(true, |t| t != text)
}

/// Switchboard's copy written, when it isn't already what the build says.
fn write_own(file: &Path, text: &str) -> Result<bool> {
    if !own_is_stale(file, text) {
        return Ok(false);
    }
    if let Some(dir) = file.parent() {
        fs::create_dir_all(dir)?;
    }
    util::write_atomic(file, text.as_bytes())
        .with_context(|| format!("can't write {}", home_tilde(file)))?;
    Ok(true)
}

/// `LG_CONFIG_FILE` for lazygit in a pane's place: Switchboard's copy,
/// then the files the variable already names, else your config when there
/// is one, last so what yours says wins over Switchboard's defaults.
fn config_files(lg_config_file: Option<&str>, yours: Option<&Path>, own: &Path) -> String {
    let mut files = vec![own.to_string_lossy().into_owned()];
    files.extend::<Vec<String>>(match lg_config_file {
        Some(given) => given
            .split(',')
            .map(str::trim)
            .filter(|f| !f.is_empty())
            .map(String::from)
            .collect(),
        None => yours
            .map(|f| f.to_string_lossy().into_owned())
            .into_iter()
            .collect(),
    });
    files.join(",")
}

/// The command that opens lazygit in a pane's place with Switchboard's
/// settings layered under yours (`env LG_CONFIG_FILE=... lazygit`, so the
/// pane's program is lazygit once `env` has run it), Switchboard's copy
/// written first when it is missing or out of date; plain `lazygit` when
/// it can't be written.
pub fn command() -> Vec<String> {
    let own = own();
    if write_own(&own, &crate::assets::text("lazygit.yml", ASSET)).is_err() {
        return vec!["lazygit".into()];
    }
    let files = config_files(var("LG_CONFIG_FILE").as_deref(), yours().as_deref(), &own);
    vec![
        "env".into(),
        format!("LG_CONFIG_FILE={files}"),
        "lazygit".into(),
    ]
}

/// The part: Switchboard's copy written, never a config of yours.
pub fn run(r: &mut Report) -> Result<()> {
    if !installed() {
        bail!("lazygit isn't installed, so there is nothing to set up");
    }
    let file = own();
    let text = crate::assets::text("lazygit.yml", ASSET);
    if !own_is_stale(&file, &text) {
        return Ok(());
    }
    if !r.check {
        write_own(&file, &text)?;
    }
    r.change(format!(
        "write Switchboard's lazygit settings, {}, which lazygit in a pane's place layers under your config so Esc quits it unless yours says otherwise",
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
    fn lazygit_is_given_switchboard_s_settings_then_your_config() {
        let own = Path::new("/s/lazygit.yml");
        assert_eq!(
            config_files(None, Some(Path::new("/h/config.yml")), own),
            "/s/lazygit.yml,/h/config.yml"
        );
        assert_eq!(config_files(None, None, own), "/s/lazygit.yml");
        assert_eq!(
            config_files(Some("/a.yml, /b.yml,"), Some(Path::new("/a.yml")), own),
            "/s/lazygit.yml,/a.yml,/b.yml"
        );
    }

    #[test]
    fn only_switchboard_s_copy_is_written() {
        let dir = std::env::temp_dir().join(format!("sb-lazygit-{}", std::process::id()));
        let file = dir.join("share/lazygit.yml");
        assert!(write_own(&file, ASSET).unwrap());
        assert_eq!(fs::read_to_string(&file).unwrap(), ASSET);
        assert!(
            !write_own(&file, ASSET).unwrap(),
            "unchanged, so not written"
        );
        fs::write(&file, "old").unwrap();
        assert!(write_own(&file, ASSET).unwrap(), "out of date, so written");
        assert!(ASSET.contains("quitOnTopLevelReturn: true"));
        let _ = fs::remove_dir_all(&dir);
        let own = own();
        assert!(own.ends_with(".local/share/switchboard/lazygit.yml"));
        assert!(
            !places(None, None, None, true).contains(&own)
                && !places(None, None, None, false).contains(&own),
            "never a place lazygit reads its own config from"
        );
    }
}
