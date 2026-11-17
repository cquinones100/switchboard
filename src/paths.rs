//! Where things are. Everything finds itself through `~` (`$HOME`), as the
//! Node scripts did with `os.homedir()`, so the tests (and a try of a work in
//! progress) can give a run a home of its own. The binary needs nothing
//! beside it: what the repository holds for it (the settings schema, the
//! skills) is compiled in from `assets/`.

use std::env;
use std::path::{Path, PathBuf};

/// `$HOME`, read every time, so a test or a try that changes it is followed.
pub fn home() -> PathBuf {
    match env::var_os("HOME") {
        Some(h) if !h.is_empty() => PathBuf::from(h),
        _ => env::home_dir().unwrap_or_else(|| PathBuf::from("/")),
    }
}

/// A path under the home directory: `home_join(".config/switchboard")`.
pub fn home_join(rel: impl AsRef<Path>) -> PathBuf {
    home().join(rel)
}

/// Someone's own files: `~/.config/switchboard`, or, while they are kept
/// in a git repository (src/files), the folder in its clone that holds them.
pub fn config_dir() -> PathBuf {
    crate::files::dir().unwrap_or_else(local_config_dir)
}

/// `~/.config/switchboard` whatever `config_dir` is: where the files that
/// never leave this computer are (`files::LOCAL`: files.json, trusted-dirs,
/// repos.json, sidebar-plugins, slack-relay.json, linear.json, oncall.json,
/// oncall-rules.md, slack-updates.json, secrets.json,
/// leak-check.txt, the `.bak` copies).
pub fn local_config_dir() -> PathBuf {
    home_join(".config/switchboard")
}

/// `config_dir` as it is shown: `~/.config/switchboard`, or the clone's
/// folder written from `~`.
pub fn config_shown() -> String {
    tilde(config_dir())
}

/// Switchboard's own state: `SWITCHBOARD_STATE_DIR`, else
/// `~/.local/state/switchboard`. The server's socket is in it, so it is kept
/// short: a Unix socket's path can't be over 104 bytes on macOS.
pub fn state_dir() -> PathBuf {
    match env::var_os("SWITCHBOARD_STATE_DIR") {
        Some(d) if !d.is_empty() => PathBuf::from(d),
        _ => home_join(".local/state/switchboard"),
    }
}

/// `~/.claude`.
pub fn claude_dir() -> PathBuf {
    home_join(".claude")
}

/// `~/.cache/switchboard`.
pub fn cache_dir() -> PathBuf {
    home_join(".cache/switchboard")
}

/// Where Switchboard's plugin for Claude Code is written (its skills, hooks
/// and instructions, src/setup/plugin.rs), relative to the home: under
/// `claude/`, since it is Claude Code's.
pub const PLUGIN: &str = ".local/share/switchboard/claude/plugin";

/// `~/.local/share/switchboard/claude/plugin`.
pub fn plugin_dir() -> PathBuf {
    home_join(PLUGIN)
}

/// Where earlier builds wrote the plugin, which setup leaves as a link to
/// `PLUGIN` for the tabs opened before, whose environment still names it.
pub const FORMER_PLUGIN: &str = ".local/share/switchboard/plugin";

/// `~/.local/share/switchboard/plugin`.
pub fn former_plugin_dir() -> PathBuf {
    home_join(FORMER_PLUGIN)
}

/// A path under the home directory written from `~` (`~/code/web`, or `~`
/// for the home itself), any other path as it is. The home's real path
/// counts too, since a worktree's root is read canonical: under a home in
/// `/tmp` on macOS it starts `/private/tmp`. The Node scripts wrote
/// `file.replace(os.homedir(), "~")`, which also matched the home inside
/// that real path (`/private~/code`).
pub fn tilde(path: impl AsRef<Path>) -> String {
    let path = path.as_ref();
    let home = home();
    if home.as_os_str().is_empty() || home == Path::new("/") {
        return path.display().to_string();
    }
    let rest = match path.strip_prefix(&home) {
        Ok(rest) => Some(rest),
        Err(_) => real_home(&home).and_then(|r| path.strip_prefix(r).ok()),
    };
    match rest {
        Some(rest) if rest.as_os_str().is_empty() => "~".into(),
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    }
}

/// The home's real path, read once per home and kept, since `tilde` runs
/// on the drawing path, which never waits on the file system again.
fn real_home(home: &Path) -> Option<&'static Path> {
    static KNOWN: std::sync::Mutex<Vec<(PathBuf, Option<&'static Path>)>> =
        std::sync::Mutex::new(Vec::new());
    let mut known = KNOWN.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((_, real)) = known.iter().find(|(h, _)| h == home) {
        return *real;
    }
    let real = home
        .canonicalize()
        .ok()
        .filter(|r| r != home)
        .map(|r| &*Box::leak(r.into_boxed_path()));
    known.push((home.to_path_buf(), real));
    real
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tilde_writes_paths_under_the_home_from_tilde() {
        let home = home();
        assert_eq!(tilde(home.join("code/web")), "~/code/web");
        assert_eq!(tilde(&home), "~");
        assert_eq!(tilde("/opt/elsewhere"), "/opt/elsewhere");
        // A sibling whose name starts with the home's is not under it.
        let sibling = format!("{}-other/x", home.display());
        assert_eq!(tilde(&sibling), sibling);
        if let Ok(real) = home.canonicalize() {
            assert_eq!(tilde(real.join("code")), "~/code");
        }
    }
}
