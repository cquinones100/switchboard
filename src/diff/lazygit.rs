//! lazygit's config in Switchboard's panes: lazygit run there, whether
//! Switchboard opens it in a pane's place (M-p's "Open lazygit in place of
//! this pane", the sidebar's `L`, mission control's lazygit) or you type
//! `lazygit` in a pane's shell, quits on Esc with nothing left to back out
//! of (`quitOnTopLevelReturn`), as the diff and the views close, so the pane
//! a tool stands in for comes back.
//!
//! The setting reaches lazygit through the panes' environment, as
//! Switchboard's plugin reaches the Claude in them, never through lazygit's
//! own config: it is in a file of Switchboard's own,
//! `~/.local/share/switchboard/lazygit/config.yml` (`assets/lazygit.yml`,
//! written when it differs), and every pane's environment names it first in
//! `LG_CONFIG_FILE`, your own config after it (`pane_value`, which the
//! server's `start_program` adds as each pane starts). lazygit reads the
//! files that variable lists in order, a later one's settings over an
//! earlier one's, so whatever yours says wins, and yours is never written; a
//! lazygit outside Switchboard reads your config alone. Earlier builds
//! offered to write the setting into lazygit's own config instead, which the
//! `lazygit` setup part now takes out (src/setup/lazygit.rs). Settings' "Esc
//! quits lazygit" (`lazygit_esc_quits`) off leaves the variable as the
//! server was given it.
//!
//! `LG_CONFIG_FILE` takes the place of the config lazygit would read
//! itself, so yours is found here by lazygit's own rules (`yours`), and only
//! files that are there are named, since lazygit refuses to start on one
//! that isn't. A pane keeps the value it started with: a config of yours
//! made later is read in panes opened after it, and a shell whose startup
//! files set `LG_CONFIG_FILE` themselves has theirs.

use std::path::{Path, PathBuf};

use crate::{paths, platform, settings, util};

/// Settings' "Esc quits lazygit".
pub const SETTING: &str = "lazygit_esc_quits";

/// The variable naming the config files lazygit reads, comma-separated.
pub const ENV: &str = "LG_CONFIG_FILE";

/// Switchboard's lazygit config, compiled in.
pub const ASSET: &str = include_str!("../../assets/lazygit.yml");

/// Where it is written, under the home: beside Switchboard's plugin for
/// Claude Code, in a folder of Switchboard's own.
pub const FILE: &str = ".local/share/switchboard/lazygit/config.yml";

/// `~/.local/share/switchboard/lazygit/config.yml`.
pub fn file() -> PathBuf {
    paths::home_join(FILE)
}

/// A variable of the server's environment, unset when empty.
pub fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

/// Whether lazygit runs here on a Mac, whose config home differs.
pub fn mac() -> bool {
    platform::NAME == "macOS"
}

/// `LG_CONFIG_FILE` for a pane starting now: with the setting on,
/// Switchboard's config and then yours; none with it off, or when
/// Switchboard's can't be written, leaving the variable as the server was
/// given it. Run on the server's task as a pane starts: the file it writes
/// is a few lines, written only when it differs, and yours are only looked
/// for.
pub fn pane_value() -> Option<String> {
    let ours = settings::is_on(SETTING).then(written).flatten()?;
    value_with(&ours, &yours(&env, &paths::home(), mac()))
}

/// Switchboard's config, written when it differs from what this build
/// carries; none when it can't be.
fn written() -> Option<PathBuf> {
    let file = file();
    let text = crate::assets::text("lazygit.yml", ASSET);
    if util::read_text(&file).as_deref() != Some(&*text) {
        util::write_atomic(&file, text.as_bytes()).ok()?;
    }
    Some(file)
}

/// `LG_CONFIG_FILE` naming `ours` and then `yours`, comma-separated as
/// lazygit reads it. A config of Switchboard's among yours (the server
/// started from one of its panes, whose variable already names one) is left
/// out, so it isn't named twice. None when a path holds a comma, which the
/// list can't, so that lazygit reads your config as it would.
fn value_with(ours: &Path, yours: &[PathBuf]) -> Option<String> {
    let files: Vec<String> = std::iter::once(ours)
        .chain(
            yours
                .iter()
                .map(PathBuf::as_path)
                .filter(|f| !f.ends_with(FILE)),
        )
        .map(|f| f.to_string_lossy().into_owned())
        .collect();
    (!files.iter().any(|f| f.contains(','))).then(|| files.join(","))
}

/// The files `LG_CONFIG_FILE` names, comma-separated.
fn listed(files: &str) -> Vec<PathBuf> {
    files
        .split(',')
        .map(str::trim)
        .filter(|f| !f.is_empty())
        .map(PathBuf::from)
        .collect()
}

/// Each `config.yml` lazygit looks for when no variable names one, in the
/// order it looks: under `jesseduffield/lazygit` (its older place), then
/// `lazygit`, each in the XDG config home (`~/.config`, or on a Mac
/// `~/Library/Application Support`, unless `XDG_CONFIG_HOME` says) and, on
/// a Mac, then in `~/.config`, the one of its other config folders a config
/// is ever put in.
fn looked_for(home: &Path, xdg_config_home: Option<&str>, mac: bool) -> Vec<PathBuf> {
    let config_home = match xdg_config_home {
        Some(dir) => PathBuf::from(dir),
        None if mac => home.join("Library/Application Support"),
        None => home.join(".config"),
    };
    let mut dirs = vec![config_home];
    if mac && !dirs.contains(&home.join(".config")) {
        dirs.push(home.join(".config"));
    }
    ["jesseduffield/lazygit", "lazygit"]
        .iter()
        .flat_map(|sub| dirs.iter().map(move |dir| dir.join(sub).join("config.yml")))
        .collect()
}

/// Every file lazygit may read its config from: `LG_CONFIG_FILE`'s,
/// `$CONFIG_DIR/config.yml`, and those it looks for (`looked_for`).
pub fn places(var: &dyn Fn(&str) -> Option<String>, home: &Path, mac: bool) -> Vec<PathBuf> {
    let mut places = var(ENV).map(|files| listed(&files)).unwrap_or_default();
    if let Some(dir) = var("CONFIG_DIR") {
        places.push(PathBuf::from(dir).join("config.yml"));
    }
    for file in looked_for(home, var("XDG_CONFIG_HOME").as_deref(), mac) {
        if !places.contains(&file) {
            places.push(file);
        }
    }
    places
}

/// Your lazygit config, by lazygit's own rules, only files that are there:
/// the files `LG_CONFIG_FILE` names; else `$CONFIG_DIR/config.yml`; else
/// the first of those it looks for. None when you have none.
pub fn yours(var: &dyn Fn(&str) -> Option<String>, home: &Path, mac: bool) -> Vec<PathBuf> {
    let candidates = match (var(ENV), var("CONFIG_DIR")) {
        (Some(files), _) => listed(&files),
        (None, Some(dir)) => vec![PathBuf::from(dir).join("config.yml")],
        (None, None) => looked_for(home, var("XDG_CONFIG_HOME").as_deref(), mac)
            .into_iter()
            .filter(|f| f.is_file())
            .take(1)
            .collect(),
    };
    candidates.into_iter().filter(|f| f.is_file()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mux::socket::testing::TempDir;
    use std::fs;

    /// Variables from a list, the rest unset.
    fn vars(set: &[(&str, String)]) -> impl Fn(&str) -> Option<String> + use<> {
        let set: Vec<(String, String)> = set
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect();
        move |name| set.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone())
    }

    fn touch(file: &Path) {
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(file, "gui: {}\n").unwrap();
    }

    #[test]
    fn lazygit_s_config_is_looked_for_where_lazygit_reads_it() {
        let home = Path::new("/h");
        let none = vars(&[]);
        assert_eq!(
            places(&none, home, true),
            [
                "/h/Library/Application Support/jesseduffield/lazygit/config.yml",
                "/h/.config/jesseduffield/lazygit/config.yml",
                "/h/Library/Application Support/lazygit/config.yml",
                "/h/.config/lazygit/config.yml",
            ]
            .map(PathBuf::from)
        );
        assert_eq!(
            places(&none, home, false),
            [
                "/h/.config/jesseduffield/lazygit/config.yml",
                "/h/.config/lazygit/config.yml",
            ]
            .map(PathBuf::from)
        );
        // XDG_CONFIG_HOME in place of the config home; on Linux, the only
        // place lazygit looks.
        let xdg = vars(&[("XDG_CONFIG_HOME", "/x".into())]);
        assert_eq!(
            places(&xdg, home, false),
            [
                "/x/jesseduffield/lazygit/config.yml",
                "/x/lazygit/config.yml"
            ]
            .map(PathBuf::from)
        );
        assert_eq!(places(&xdg, home, true).len(), 4);
        let given = places(
            &vars(&[
                ("LG_CONFIG_FILE", "/a.yml, /b.yml".into()),
                ("CONFIG_DIR", "/d".into()),
            ]),
            home,
            false,
        );
        assert_eq!(
            given[..3],
            ["/a.yml", "/b.yml", "/d/config.yml"].map(PathBuf::from)
        );
    }

    #[test]
    fn your_config_is_the_one_lazygit_would_read() {
        let dir = TempDir::new();
        let home = dir.path().join("home");
        let at = |rel: &str| home.join(rel).join("config.yml");
        let none = vars(&[]);
        assert!(yours(&none, &home, false).is_empty());

        // The first lazygit looks for, its older place before its newer.
        touch(&at(".config/lazygit"));
        assert_eq!(yours(&none, &home, false), [at(".config/lazygit")]);
        assert_eq!(yours(&none, &home, true), [at(".config/lazygit")]);
        touch(&at("Library/Application Support/lazygit"));
        assert_eq!(
            yours(&none, &home, true),
            [at("Library/Application Support/lazygit")]
        );
        touch(&at(".config/jesseduffield/lazygit"));
        assert_eq!(
            yours(&none, &home, false),
            [at(".config/jesseduffield/lazygit")]
        );

        // CONFIG_DIR is the only place lazygit reads, there or not.
        let config_dir = dir.path().join("config-dir");
        let with_dir = vars(&[("CONFIG_DIR", config_dir.to_string_lossy().into())]);
        assert!(yours(&with_dir, &home, false).is_empty());
        touch(&config_dir.join("config.yml"));
        assert_eq!(
            yours(&with_dir, &home, false),
            [config_dir.join("config.yml")]
        );

        // LG_CONFIG_FILE's files, in order, only those there.
        let (a, b) = (dir.path().join("a.yml"), dir.path().join("b.yml"));
        touch(&a);
        touch(&b);
        let gone = dir.path().join("gone.yml");
        let listed = vars(&[
            (
                "LG_CONFIG_FILE",
                format!("{},{},{}", b.display(), gone.display(), a.display()),
            ),
            ("CONFIG_DIR", config_dir.to_string_lossy().into()),
        ]);
        assert_eq!(yours(&listed, &home, false), [b, a]);
    }

    #[test]
    fn switchboard_s_config_comes_first_and_yours_after_it() {
        let ours = Path::new("/h/.local/share/switchboard/lazygit/config.yml");
        let yours = PathBuf::from("/h/.config/lazygit/config.yml");
        assert_eq!(
            value_with(ours, std::slice::from_ref(&yours)).as_deref(),
            Some("/h/.local/share/switchboard/lazygit/config.yml,/h/.config/lazygit/config.yml")
        );
        assert_eq!(
            value_with(ours, &[]).as_deref(),
            Some("/h/.local/share/switchboard/lazygit/config.yml")
        );
        // A server started from a pane: Switchboard's config, its own or
        // another home's, isn't named again.
        let given = [
            PathBuf::from("/other/.local/share/switchboard/lazygit/config.yml"),
            yours.clone(),
        ];
        assert_eq!(
            value_with(ours, &given).as_deref(),
            Some("/h/.local/share/switchboard/lazygit/config.yml,/h/.config/lazygit/config.yml")
        );
        // A path the list can't hold: lazygit reads your config itself.
        let comma = [PathBuf::from("/h/a,b/config.yml")];
        assert_eq!(value_with(ours, &comma), None);
    }

    #[test]
    fn the_asset_says_what_esc_does() {
        assert!(
            ASSET
                .lines()
                .any(|l| l.trim() == "quitOnTopLevelReturn: true")
        );
    }
}
