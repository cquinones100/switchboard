//! Who Notes asks for what isn't its own: the workspace, through the plugin
//! API (`switchboard_plugin_api::Host`), or, alone in a terminal, itself
//! (`Alone`). Every module asks through `host()`, which is whoever ran
//! Notes (`install`), so nothing here reaches the workspace another way;
//! the small modules at the end (`socket`, `settings`, `paths`, `platform`,
//! `links`, `graphics`, `requests`) give the answers the names Notes'
//! modules knew them by when they were part of `switchboard`.
//!
//! Alone, Notes has no server: no tab to go to or to link a note to, no
//! work started on an item, no list of the tabs working on one. Nothing
//! opens a link, shows a picture or reads the desktop's clipboard for it
//! either, since each is a rule the workspace keeps in one place (which
//! browser a site opens in, how a picture reaches a terminal, the
//! desktop's own tools): a link says it can't be opened, pictures stay
//! the lines that name them, and what is copied goes to the terminal's own
//! clipboard (OSC 52), which the app writes itself. Its settings are its
//! own defaults (the manifest's), over which it reads Switchboard's
//! `~/.config/switchboard/settings.json` when you have one, so the theme
//! and the options you set for Notes there are the ones it follows; its
//! state and cache are `~/.local/state/switchboard-notes` and
//! `~/.cache/switchboard-notes`.

use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, OnceLock, RwLock};

use serde_json::Value;
use switchboard_plugin_api::sdk::{self, AloneHost, home_join};
use switchboard_plugin_api::{Action, Answer, Folders, Host};

static HOST: RwLock<Option<Arc<dyn Host>>> = RwLock::new(None);

/// Says who answers for the workspace. The first one stays; until one is
/// said (a program that reads a note without running the app), Notes
/// answers for itself with its defaults, reading no file of yours and
/// keeping nothing in your home.
pub fn install(host: Arc<dyn Host>) {
    let mut said = HOST.write().unwrap_or_else(|e| e.into_inner());
    if said.is_none() {
        *said = Some(host);
    }
}

/// Who answers for the workspace. This crate's unit tests are answered by
/// a stand-in for it (`tests::Stand`).
pub fn host() -> Arc<dyn Host> {
    #[cfg(test)]
    {
        Arc::new(tests::Stand)
    }
    #[cfg(not(test))]
    {
        let said = HOST.read().unwrap_or_else(|e| e.into_inner());
        match said.as_ref() {
            Some(host) => host.clone(),
            None => Arc::new(Alone { yours: false }),
        }
    }
}

/// Notes answering for the workspace itself, with none running.
pub struct Alone {
    /// Whether what you set in Switchboard's settings file is read over
    /// the defaults.
    yours: bool,
}

impl Alone {
    /// As `switchboard-notes` runs: your settings over the defaults.
    pub fn new() -> Alone {
        Alone { yours: true }
    }
}

impl Default for Alone {
    fn default() -> Alone {
        Alone::new()
    }
}

/// Notes' own folders, alone: in your home as `switchboard-notes` runs
/// (`yours`), else, for a program that never said who answers it, a folder
/// of the system's temporary directory.
fn folders_alone(yours: bool) -> Folders {
    if !yours {
        let tmp = std::env::temp_dir().join("switchboard-notes-unsaid");
        return Folders {
            state: tmp.join("state"),
            cache: tmp.join("cache"),
        };
    }
    Folders {
        state: home_join(".local/state/switchboard-notes"),
        cache: home_join(".cache/switchboard-notes"),
    }
}

/// Switchboard's settings file, when you have one: what you changed there,
/// read again each time, since the app runs on while it changes.
fn saved() -> serde_json::Map<String, Value> {
    let file = match std::env::var_os("SWITCHBOARD_SETTINGS") {
        Some(f) if !f.is_empty() => PathBuf::from(f),
        _ => home_join(".config/switchboard/settings.json"),
    };
    std::fs::read_to_string(file)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default()
}

impl AloneHost for Alone {
    fn default_of(&self, key: &str) -> Option<Value> {
        crate::manifest()
            .default_of(key)
            .or_else(|| switchboard_ui::settings::default_of(key))
    }

    /// Read once, as the app starts; `Settings` reads the file again.
    fn saved(&self, key: &str) -> Option<Value> {
        static SAVED: OnceLock<serde_json::Map<String, Value>> = OnceLock::new();
        self.yours
            .then(|| SAVED.get_or_init(saved).get(key).cloned())
            .flatten()
    }

    fn own_folders(&self) -> Folders {
        folders_alone(self.yours)
    }

    // Several settings at once are read as they are now, since the app runs
    // on while they change.
    fn exception(&self, action: &Action) -> Option<Answer> {
        let Action::Settings { keys } = action else {
            return None;
        };
        let saved = self.yours.then(saved).unwrap_or_default();
        Some(Answer::Values {
            values: keys
                .iter()
                .map(|key| sdk::saved_over_default(saved.get(key).cloned(), self.default_of(key)))
                .collect(),
        })
    }
}

/// The pane the workspace runs the app in (`SWITCHBOARD_PANE`), `None`
/// where no workspace answers it: alone, the app is in no pane of its own
/// even when it was started in one, so its socket, the lock it holds on a
/// note and what it asks of a server never name that pane.
pub fn pane() -> Option<String> {
    if !host().in_pane() {
        return None;
    }
    std::env::var("SWITCHBOARD_PANE")
        .ok()
        .filter(|pane| !pane.is_empty())
}

/// Gives the drawing crate Notes' settings, for Notes alone: the workspace
/// says its own before it runs the app.
pub fn share_with_ui() {
    switchboard_ui::settings::install(switchboard_ui::settings::Source {
        cached: |key| host().setting(key),
        fresh: |keys| host().settings(keys),
    });
}

/// A Claude the app starts for itself (squashing notes): its environment
/// as the workspace says a headless one's is, so the workspace's own hooks
/// and instructions don't ride along.
pub fn headless(cmd: &mut Command) {
    let env = host().headless_claude();
    for (key, value) in env.set {
        cmd.env(key, value);
    }
    for key in env.remove {
        cmd.env_remove(key);
    }
}

/// The workspace's server, asked from the app's pane.
pub mod socket {
    use anyhow::{Result, anyhow, bail};

    /// A server command's exit code, what it printed and what it said went
    /// wrong; an error with no workspace to ask.
    pub fn ask(args: &[&str]) -> Result<(i32, String, String)> {
        super::host()
            .command(args)
            .ok_or_else(|| anyhow!("no Switchboard server is running"))
    }

    /// As `ask`, for a command that must succeed: what it printed, or an
    /// error carrying what it said went wrong.
    pub fn ask_ok(args: &[&str]) -> Result<String> {
        let (code, stdout, stderr) = ask(args)?;
        if code != 0 {
            bail!(
                "{}",
                stderr.trim().lines().next().unwrap_or("the server refused")
            );
        }
        Ok(stdout)
    }

    /// Whether the app runs in a pane of a running workspace.
    pub fn in_pane() -> bool {
        super::host().in_pane()
    }
}

/// The settings, as Notes' host has them now.
pub mod settings {
    use serde_json::Value;

    /// Whether an on/off option is on now. One that isn't listed counts
    /// as on.
    pub fn is_on(key: &str) -> bool {
        Read::of(&[key]).on(key)
    }

    /// Some options as they are now, read together.
    pub struct Read(Vec<(String, Option<Value>)>);

    impl Read {
        pub fn of(keys: &[&str]) -> Read {
            let values = super::host().settings(keys);
            Read(keys.iter().map(|k| k.to_string()).zip(values).collect())
        }

        pub fn value(&self, key: &str) -> Option<&Value> {
            self.0
                .iter()
                .find(|(k, _)| k == key)
                .and_then(|(_, v)| v.as_ref())
        }

        /// On unless set off: one that isn't listed counts as on.
        pub fn on(&self, key: &str) -> bool {
            self.value(key) != Some(&Value::Bool(false))
        }

        /// On only when set on, for an option that is off by default.
        pub fn set_on(&self, key: &str) -> bool {
            self.value(key) == Some(&Value::Bool(true))
        }
    }
}

/// Where Notes' files are, as its host says.
pub mod paths {
    use std::path::PathBuf;

    pub use switchboard_plugin_api::sdk::{home, home_join, tilde};

    fn folders() -> switchboard_plugin_api::Folders {
        super::host()
            .folders()
            .unwrap_or_else(|| super::folders_alone(false))
    }

    /// What must last: the palette's picks, the app's socket.
    pub fn state_dir() -> PathBuf {
        folders().state
    }

    /// What can be made again: a note's pictures made PNGs.
    pub fn cache_dir() -> PathBuf {
        folders().cache
    }
}

/// The desktop, which the workspace reaches by the platform's own tools.
pub mod platform {
    use std::path::Path;

    pub fn copy(text: &str) {
        super::host().copy(text);
    }

    /// The clipboard's text, `None` with none or nothing to read it.
    pub fn paste() -> Option<String> {
        super::host().paste()
    }

    /// The clipboard's picture written to `file` as a PNG; whether it was.
    pub fn paste_image(file: &Path) -> bool {
        super::host().paste_image(file)
    }

    /// A file shown in the desktop's viewer; whether it was.
    pub fn preview(file: &Path) -> bool {
        super::host().preview(file)
    }
}

/// Links, which the workspace opens in the browser it gives each site.
pub mod links {
    /// The words that say where a link is going, `None` while that isn't
    /// said (the link_opening_message setting off, or nothing to say it).
    pub fn opening_words(url: &str) -> Option<String> {
        super::host().link_words(url)
    }

    /// Opens a link, waiting until its browser is in front: whether it was
    /// seen to come there, or why it couldn't be opened.
    pub fn open_url_watched(url: &str) -> Result<bool, String> {
        super::host().open_link(url, true)
    }

    /// Opens a link without waiting on its browser.
    pub fn open_url(url: &str) -> Result<(), String> {
        super::host().open_link(url, false).map(|_| ())
    }
}

/// How a picture reaches the terminal, which the workspace decides in one
/// place (its src/mux/graphics.rs) and writes the commands for.
pub mod graphics {
    use std::path::Path;

    /// Whether the app draws its pictures.
    pub fn apps_draw() -> bool {
        super::host().draws("pictures")
    }

    /// Whether the rounded ends of a chip are drawn.
    pub fn apps_round_ends() -> bool {
        super::host().draws("round")
    }

    /// What gives the terminal a PNG by its path, placed where its
    /// placeholders of `id` are.
    pub fn by_path(id: u32, cols: u16, rows: u16, png: &Path) -> String {
        super::host().picture(id, cols, rows, png)
    }

    /// What takes back a picture given by `by_path`.
    pub fn take_back(id: u32) -> String {
        super::host().picture_gone(id)
    }
}

/// The request audit, which the workspace keeps.
pub mod requests {
    /// Counts a request of a service outside the computer.
    pub fn count(service: &str, protocol: &str, source: &str) {
        super::host().count_request(service, protocol, source);
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// The workspace's stand-in for this crate's unit tests: Notes alone,
    /// reading no file of yours, with the few answers the app's tests wait
    /// on given as a workspace would give them, each a stand-in for the
    /// rule `switchboard` keeps (and tests, in its src/notes/host.rs).
    pub struct Stand;

    impl Host for Stand {
        fn ask(&self, action: Action) -> Answer {
            match action {
                Action::LinkWords { url } => Answer::Text {
                    text: format!(
                        "Opening {} in your browser…",
                        url.split("://")
                            .nth(1)
                            .and_then(|rest| rest.split('/').next())
                            .unwrap_or("the link")
                    ),
                },
                Action::Picture {
                    id,
                    cols,
                    rows,
                    png,
                } => Answer::Text {
                    text: format!("<picture {id} {cols}x{rows} {}>", png.display()),
                },
                Action::PictureGone { id } => Answer::Text {
                    text: format!("<picture {id} gone>"),
                },
                Action::Item { text, line } => {
                    let at = line.checked_sub(1).and_then(|i| text.split('\n').nth(i));
                    match at {
                        Some(l) if l.starts_with("- [ ] ") => Answer::Done,
                        Some(l) if l.starts_with("- [x] ") => Answer::Text {
                            text: "is ticked".into(),
                        },
                        _ => Answer::Text {
                            text: "isn't a checklist item".into(),
                        },
                    }
                }
                other => Alone { yours: false }.ask(other),
            }
        }
    }

    #[test]
    fn alone_notes_answers_with_its_defaults_and_asks_nothing_of_a_workspace() {
        let alone = Alone { yours: false };
        assert_eq!(alone.setting("notes_images"), Some(Value::Bool(true)));
        assert_eq!(
            alone.setting("notes_relative_numbers"),
            Some(Value::Bool(false))
        );
        assert_eq!(alone.setting("theme"), Some(Value::String("dark".into())));
        assert_eq!(
            alone.settings(&["notes_toolbar", "no such setting"]),
            vec![Some(Value::Bool(true)), None]
        );
        assert!(alone.is_on("not listed"));
        assert!(!alone.in_pane());
        assert_eq!(alone.command(&["list"]), None);
        // Nothing here opens a link, shows a picture or reads the
        // desktop's clipboard.
        assert!(alone.open_link("https://rfc.example/1", true).is_err());
        assert!(!alone.draws("pictures") && !alone.draws("round"));
        assert_eq!(alone.picture(7, 10, 3, std::path::Path::new("/a.png")), "");
        assert_eq!(alone.paste(), None);
        assert!(!alone.preview(std::path::Path::new("/a.png")));
        // No work is started on an item, and no tab works on one.
        assert!(alone.item_refused("- [ ] one", 1).is_some());
        assert_eq!(alone.item_claim(std::path::Path::new("/tmp")), None);
        assert_eq!(alone.queued(), None);
        let folders = alone.folders().unwrap();
        assert_ne!(folders.state, folders.cache);
    }
}
