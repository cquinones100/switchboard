//! Switchboard's plugin API: everything a plugin and the workspace say to
//! each other, as plain serde types with a version.
//!
//! A plugin is a program built without the workspace: it depends on this
//! crate (and on `switchboard-ui` to draw), never on `switchboard`, so the
//! compiler holds the boundary. The diff (`switchboard-diff`) is the first
//! and the notes app (`switchboard-notes`) the second: each runs in a pane,
//! where `switchboard` loads it in its own process through `Plugin`, and
//! alone in any terminal with no server, where it answers for the
//! workspace itself.
//!
//! - `Manifest`: what a plugin says of itself: its name, the protocol it
//!   was built for, and each setting it reads with its default.
//! - `Event`: what the workspace tells a plugin has happened.
//! - `Action` and `Answer`: what a plugin asks of the workspace, and what
//!   comes back. `Host` is whoever answers: the workspace, or the plugin's
//!   own stand-in when it runs alone.
//! - `Plugin`: a plugin loaded in the workspace's own process.
//! - `Folders`: where a plugin keeps its state and its cache.
//! - `release`: a release of Switchboard or of an app that ships apart
//!   from it, checked and put in place (the `release` feature).
//! - `sdk`: the few helpers a plugin and the workspace both need (the home,
//!   a file written whole, a command's output).
//!
//! Every type reads and writes as JSON, so a plugin in a process of its own
//! says the same things over a socket that one in process says by a call. A
//! message between two programs is wrapped with the protocol's version
//! (`Message`): each side refuses one from a newer protocol than its own
//! rather than misreading it (`Refused::Newer`), and a workspace refuses to
//! load a plugin built for a newer protocol (`Manifest::check`).
//!
//! The rule a plugin keeps: a plugin that runs an existing app (git, an
//! editor, a pager) gives it its settings as it starts it, by flags or
//! environment, and never edits that app's own config files.

#[cfg(feature = "release")]
pub mod release;
pub mod sdk;

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The protocol these types are. Bumped when a type changes in a way an
/// older reader would misread; a field added with a default is not one.
pub const PROTOCOL_VERSION: u32 = 1;

/// Why something from the other side wasn't taken.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refused {
    /// It is of a newer protocol than this build's.
    Newer { theirs: u32, ours: u32 },
    /// It doesn't read as what was expected.
    Unreadable(String),
}

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refused::Newer { theirs, ours } => write!(
                f,
                "it speaks version {theirs} of the plugin protocol and this build version {ours}: update Switchboard"
            ),
            Refused::Unreadable(why) => write!(f, "it doesn't read as the plugin protocol: {why}"),
        }
    }
}

impl std::error::Error for Refused {}

/// Something said between a plugin and the workspace in two processes: the
/// protocol's version, then what is said.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Message<T> {
    pub protocol: u32,
    pub body: T,
}

impl<T: Serialize + DeserializeOwned> Message<T> {
    pub fn new(body: T) -> Message<T> {
        Message {
            protocol: PROTOCOL_VERSION,
            body,
        }
    }

    /// As one line of JSON.
    pub fn write(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    /// What a line of JSON says, refused when it is of a newer protocol:
    /// the version is read before the rest, which a newer protocol may
    /// have changed the shape of.
    pub fn read(text: &str) -> Result<T, Refused> {
        let value: Value =
            serde_json::from_str(text).map_err(|e| Refused::Unreadable(e.to_string()))?;
        let theirs = value
            .get("protocol")
            .and_then(Value::as_u64)
            .and_then(|v| u32::try_from(v).ok())
            .ok_or_else(|| Refused::Unreadable("it names no protocol version".into()))?;
        if theirs > PROTOCOL_VERSION {
            return Err(Refused::Newer {
                theirs,
                ours: PROTOCOL_VERSION,
            });
        }
        let message: Message<T> =
            serde_json::from_value(value).map_err(|e| Refused::Unreadable(e.to_string()))?;
        Ok(message.body)
    }
}

/// Where a plugin's setting is one to change.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    /// In the workspace and in the plugin alone.
    #[default]
    Both,
    /// Only in the workspace: it is about a tab, a pane or the server.
    Workspace,
    /// Only in the plugin alone, where the workspace has a setting of its
    /// own for the same thing.
    Alone,
}

/// A setting a plugin reads: what it is while you haven't set it, and how
/// whoever shows the plugin's settings names it (the workspace's Settings,
/// or the plugin's own view of them when it runs alone), so both say the
/// same of it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Setting {
    pub key: String,
    pub default: Value,
    /// Its name and what it does, on and off.
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub scope: Scope,
}

impl Setting {
    pub fn new(key: &str, default: impl Into<Value>) -> Setting {
        Setting {
            key: key.to_string(),
            default: default.into(),
            label: String::new(),
            description: String::new(),
            scope: Scope::Both,
        }
    }

    /// With its name and what it does.
    pub fn said(mut self, label: &str, description: &str) -> Setting {
        self.label = label.to_string();
        self.description = description.to_string();
        self
    }

    /// Shown only there.
    pub fn only(mut self, scope: Scope) -> Setting {
        self.scope = scope;
        self
    }
}

/// What a plugin says of itself.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    /// The protocol it was built for.
    pub protocol: u32,
    /// Its name, which its folders are named after.
    pub name: String,
    pub version: String,
    /// Every setting of its own it reads.
    #[serde(default)]
    pub settings: Vec<Setting>,
}

impl Manifest {
    /// Whether this build can load the plugin: not one built for a newer
    /// protocol.
    pub fn check(&self) -> Result<(), Refused> {
        if self.protocol > PROTOCOL_VERSION {
            return Err(Refused::Newer {
                theirs: self.protocol,
                ours: PROTOCOL_VERSION,
            });
        }
        Ok(())
    }

    /// What one of its settings is while nobody has set it.
    pub fn default_of(&self, key: &str) -> Option<Value> {
        self.settings
            .iter()
            .find(|s| s.key == key)
            .map(|s| s.default.clone())
    }
}

/// What the workspace tells a plugin has happened.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    /// A tab's branch has a commit the plugin hasn't been told of: the
    /// repository's top, and the tab's plan note ("" for none), a path in
    /// the notes directory.
    Commit { dir: PathBuf, plan: String },
}

/// What a plugin asks of the workspace.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "ask", rename_all = "snake_case")]
pub enum Action {
    /// One setting's value now. `Answer::Value`.
    Setting { key: String },
    /// Where the plugin keeps its state and its cache. `Answer::Folders`.
    Folders,
    /// Whether the plugin runs in a pane of a running workspace.
    /// `Answer::Flag`.
    InPane,
    /// A command of the workspace's server (`say`, `split`, `select-pane`,
    /// `show`), from the plugin's pane. `Answer::Ran`, or `Answer::None`
    /// with no workspace to ask.
    Command { args: Vec<String> },
    /// Text put on the desktop's clipboard. `Answer::Done`.
    Copy { text: String },
    /// The pull request the branch in `dir` has on GitHub. `Answer::Pr`,
    /// or `Answer::None` with no PR, or none known.
    Pr { dir: PathBuf },
    /// The number of the pull request the worktree at `dir` was opened to
    /// review, which its folder carries. `Answer::Number`, or
    /// `Answer::None` for any other directory.
    ReviewedPr { dir: PathBuf },
    /// Whether `dir` holds code someone else wrote that you haven't
    /// trusted, where nothing of its own may run or be loaded.
    /// `Answer::Flag`.
    Untrusted { dir: PathBuf },
    /// A request the plugin makes of a service outside the computer,
    /// counted in the workspace's request audit. `Answer::Done`.
    Request {
        service: String,
        protocol: String,
        source: String,
    },
    /// A fetch from a git remote of the repository at `dir`, counted in the
    /// audit under the remote's host. `Answer::Done`.
    Fetch {
        dir: PathBuf,
        remote: String,
        source: String,
    },
    /// The environment of a Claude the plugin starts for itself
    /// (`claude -p`), so the workspace's own hooks and instructions don't
    /// ride along. `Answer::Env`.
    HeadlessClaude,
    /// The tab's plan note as context for what the plugin sends Claude,
    /// while you have turned that on: `plan` a path in the notes
    /// directory, or `None` for the note of the tab the plugin runs in.
    /// `Answer::Text`, or `Answer::None`.
    Plan { plan: Option<String> },
    /// Each of these settings' values as the settings are now, in their
    /// order, for a plugin that runs long enough to see one change.
    /// `Answer::Values`.
    Settings { keys: Vec<String> },
    /// The text on the desktop's clipboard. `Answer::Text`, or
    /// `Answer::None` with none, or no way to read it.
    Paste,
    /// The picture on the desktop's clipboard written to `file` as a PNG.
    /// `Answer::Flag`, on when it was.
    PasteImage { file: PathBuf },
    /// A file shown in the desktop's viewer (Quick Look). `Answer::Done`,
    /// or `Answer::None` when it couldn't be.
    Preview { file: PathBuf },
    /// The words that say where a link is going while its browser comes
    /// to the front, for the plugin to show. `Answer::Text`, or
    /// `Answer::None` while you have turned them off.
    LinkWords { url: String },
    /// A link opened in the browser the workspace gives its site, waited
    /// on until the browser is in front with `watched`. `Answer::Flag`, on
    /// when the browser was seen to come to the front; `Answer::Text`, why
    /// it couldn't be opened; `Answer::None` with nothing to open links.
    OpenLink { url: String, watched: bool },
    /// What the terminal the plugin draws in is given, as words:
    /// `pictures` when a picture the plugin places is drawn, `round` when
    /// the rounded ends of a chip are. `Answer::Text`, or `Answer::None`
    /// with nobody to say, when neither is drawn.
    Draws,
    /// What the plugin writes to its terminal to place the PNG at `png`,
    /// `cols` by `rows` cells, where its placeholder cells of `id` are.
    /// `Answer::Text`.
    Picture {
        id: u32,
        cols: u16,
        rows: u16,
        png: PathBuf,
    },
    /// What takes a picture placed by `Picture` back. `Answer::Text`.
    PictureGone { id: u32 },
    /// The checklist item the branch in `dir` was started for.
    /// `Answer::Value`, `{"note": <the note's ID, or null>, "item": <its
    /// first line>}`, or `Answer::None`.
    ItemClaim { dir: PathBuf },
    /// The items the workspace works next by itself. `Answer::Value`,
    /// `{"note": <the note's ID>, "items": [<first line>...]}`, or
    /// `Answer::None` while it works none.
    Queued,
    /// Whether work can be started on the item at `line` (from 1) of a
    /// note's `text`. `Answer::Done`, or `Answer::Text`, why not, to say
    /// after "that line ".
    Item { text: String, line: usize },
    /// The mouse pointer's shape over the plugin's pane, by its CSS cursor
    /// name (`pointer` the hand, `default` the arrow, `text` the beam,
    /// `ew-resize`, `ns-resize`), which the workspace shows there in the
    /// beam's place while the mouse is over the pane; `None` gives the pane
    /// its beam back. `Answer::Done`, or `Answer::None` with no workspace
    /// to show it, when the plugin sets its terminal's pointer itself.
    Pointer { shape: Option<String> },
    /// What the plugin's pane was loading is ready, `what` naming it
    /// ("Diff"): the workspace marks the pane's tab as needing you, and
    /// says so in a notification, while that tab isn't in front.
    /// `Answer::Done`, or `Answer::None` with no workspace to tell.
    Ready { what: String },
}

/// Where a plugin keeps what it writes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Folders {
    /// What must last: what it has done, and for which commit.
    pub state: PathBuf,
    /// What it can make again.
    pub cache: PathBuf,
}

impl Folders {
    /// A plugin's own folders by its name, for one the workspace keeps no
    /// older place for: `plugins/<name>` under the workspace's state and
    /// cache folders.
    pub fn of(name: &str, state: &Path, cache: &Path) -> Folders {
        Folders {
            state: state.join("plugins").join(name),
            cache: cache.join("plugins").join(name),
        }
    }
}

/// A pull request, as far as a plugin is told: the branch it goes into, its
/// number, and the branch it is of.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pr {
    pub base: String,
    pub number: u64,
    pub head: String,
}

/// What a headless Claude's environment needs changed.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Env {
    pub set: Vec<(String, String)>,
    pub remove: Vec<String>,
}

/// What the workspace answers.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "answer", rename_all = "snake_case")]
pub enum Answer {
    /// Nothing to say: no such setting, no PR, no workspace to ask.
    None,
    Done,
    Value {
        value: Value,
    },
    /// Several values in the order asked, `null` for one not known.
    Values {
        values: Vec<Option<Value>>,
    },
    Text {
        text: String,
    },
    Flag {
        on: bool,
    },
    Number {
        number: u64,
    },
    Pr {
        pr: Pr,
    },
    Folders {
        folders: Folders,
    },
    Env {
        env: Env,
    },
    /// A server command's exit code, what it printed and what it said
    /// went wrong.
    Ran {
        code: i32,
        out: String,
        err: String,
    },
}

/// Whoever answers a plugin: the workspace, or the plugin's own stand-in
/// when it runs alone. The one thing to implement is `ask`; the rest read
/// its answers.
pub trait Host: Send + Sync {
    fn ask(&self, action: Action) -> Answer;

    /// One setting's value now.
    fn setting(&self, key: &str) -> Option<Value> {
        match self.ask(Action::Setting { key: key.into() }) {
            Answer::Value { value } => Some(value),
            _ => None,
        }
    }

    /// Whether an on/off setting is on. One that isn't listed counts as on.
    fn is_on(&self, key: &str) -> bool {
        self.setting(key) != Some(Value::Bool(false))
    }

    fn folders(&self) -> Option<Folders> {
        match self.ask(Action::Folders) {
            Answer::Folders { folders } => Some(folders),
            _ => None,
        }
    }

    fn in_pane(&self) -> bool {
        self.ask(Action::InPane) == Answer::Flag { on: true }
    }

    /// Runs a server command: its code, what it printed, what it said went
    /// wrong; `None` with no workspace.
    fn command(&self, args: &[&str]) -> Option<(i32, String, String)> {
        let args = args.iter().map(|a| a.to_string()).collect();
        match self.ask(Action::Command { args }) {
            Answer::Ran { code, out, err } => Some((code, out, err)),
            _ => None,
        }
    }

    fn copy(&self, text: &str) {
        self.ask(Action::Copy { text: text.into() });
    }

    fn pr(&self, dir: &Path) -> Option<Pr> {
        match self.ask(Action::Pr { dir: dir.into() }) {
            Answer::Pr { pr } => Some(pr),
            _ => None,
        }
    }

    fn reviewed_pr(&self, dir: &Path) -> Option<u64> {
        match self.ask(Action::ReviewedPr { dir: dir.into() }) {
            Answer::Number { number } => Some(number),
            _ => None,
        }
    }

    /// Whether `dir` holds someone else's code. A host that doesn't say
    /// is taken to say it does.
    fn untrusted(&self, dir: &Path) -> bool {
        self.ask(Action::Untrusted { dir: dir.into() }) != Answer::Flag { on: false }
    }

    fn count_request(&self, service: &str, protocol: &str, source: &str) {
        self.ask(Action::Request {
            service: service.into(),
            protocol: protocol.into(),
            source: source.into(),
        });
    }

    fn count_fetch(&self, dir: &Path, remote: &str, source: &str) {
        self.ask(Action::Fetch {
            dir: dir.into(),
            remote: remote.into(),
            source: source.into(),
        });
    }

    fn headless_claude(&self) -> Env {
        match self.ask(Action::HeadlessClaude) {
            Answer::Env { env } => env,
            _ => Env::default(),
        }
    }

    /// The plan note's text as context, "" for none.
    fn plan(&self, plan: Option<&str>) -> String {
        let plan = plan.map(str::to_string);
        match self.ask(Action::Plan { plan }) {
            Answer::Text { text } => text,
            _ => String::new(),
        }
    }

    /// Each of these settings as they are now, in their order.
    fn settings(&self, keys: &[&str]) -> Vec<Option<Value>> {
        let asked = keys.iter().map(|k| k.to_string()).collect();
        match self.ask(Action::Settings { keys: asked }) {
            Answer::Values { values } if values.len() == keys.len() => values,
            _ => keys.iter().map(|key| self.setting(key)).collect(),
        }
    }

    /// Names the pointer's shape over the plugin's pane (none: the beam
    /// again); whether a workspace took it.
    fn pointer(&self, shape: Option<&str>) -> bool {
        let shape = shape.map(str::to_string);
        self.ask(Action::Pointer { shape }) == Answer::Done
    }

    /// Says what the plugin's pane was loading (`what`) is ready.
    fn ready(&self, what: &str) {
        self.ask(Action::Ready { what: what.into() });
    }

    fn paste(&self) -> Option<String> {
        match self.ask(Action::Paste) {
            Answer::Text { text } => Some(text),
            _ => None,
        }
    }

    fn paste_image(&self, file: &Path) -> bool {
        self.ask(Action::PasteImage { file: file.into() }) == Answer::Flag { on: true }
    }

    fn preview(&self, file: &Path) -> bool {
        self.ask(Action::Preview { file: file.into() }) == Answer::Done
    }

    fn link_words(&self, url: &str) -> Option<String> {
        match self.ask(Action::LinkWords { url: url.into() }) {
            Answer::Text { text } => Some(text),
            _ => None,
        }
    }

    /// Opens a link: whether its browser was seen to come to the front,
    /// or why it couldn't be opened.
    fn open_link(&self, url: &str, watched: bool) -> Result<bool, String> {
        let url = url.to_string();
        match self.ask(Action::OpenLink { url, watched }) {
            Answer::Flag { on } => Ok(on),
            Answer::Text { text } => Err(text),
            _ => Err("nothing here opens a link".into()),
        }
    }

    /// Whether the terminal is given `what` (`pictures`, `round`).
    fn draws(&self, what: &str) -> bool {
        match self.ask(Action::Draws) {
            Answer::Text { text } => text.split_whitespace().any(|w| w == what),
            _ => false,
        }
    }

    /// What places a PNG by its path, "" with nobody to say.
    fn picture(&self, id: u32, cols: u16, rows: u16, png: &Path) -> String {
        let png = png.to_path_buf();
        match self.ask(Action::Picture {
            id,
            cols,
            rows,
            png,
        }) {
            Answer::Text { text } => text,
            _ => String::new(),
        }
    }

    fn picture_gone(&self, id: u32) -> String {
        match self.ask(Action::PictureGone { id }) {
            Answer::Text { text } => text,
            _ => String::new(),
        }
    }

    /// The note's ID (when the claim has one) and the item the branch in
    /// `dir` was started for.
    fn item_claim(&self, dir: &Path) -> Option<(Option<String>, String)> {
        match self.ask(Action::ItemClaim { dir: dir.into() }) {
            Answer::Value { value } => Some((
                value["note"].as_str().map(str::to_string),
                value["item"].as_str()?.to_string(),
            )),
            _ => None,
        }
    }

    /// The note's ID and the items worked next, none while none are.
    fn queued(&self) -> Option<(String, Vec<String>)> {
        match self.ask(Action::Queued) {
            Answer::Value { value } => Some((
                value["note"].as_str()?.to_string(),
                value["items"]
                    .as_array()?
                    .iter()
                    .filter_map(|i| i.as_str().map(str::to_string))
                    .collect(),
            )),
            _ => None,
        }
    }

    /// Why work can't be started on a note's line, `None` when it can. A
    /// host that doesn't say starts none.
    fn item_refused(&self, text: &str, line: usize) -> Option<String> {
        let text = text.to_string();
        match self.ask(Action::Item { text, line }) {
            Answer::Done => None,
            Answer::Text { text } => Some(text),
            _ => Some("can't be started here".into()),
        }
    }
}

/// A plugin loaded in the workspace's own process.
pub trait Plugin: Send + Sync {
    fn manifest(&self) -> Manifest;

    /// Runs its program on this terminal, as `program` with `args`, until
    /// it ends.
    fn run(&self, program: &str, args: Vec<String>, host: Arc<dyn Host>) -> ExitCode;

    /// Takes an event. `Err` says why what it does about the event
    /// couldn't be done, for the workspace to try again later.
    fn on(&self, event: Event, host: Arc<dyn Host>) -> Result<(), String>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn round<T: Serialize + DeserializeOwned + PartialEq + std::fmt::Debug>(value: T) {
        let text = Message::new(value).write();
        let back: Message<T> = serde_json::from_str(&text).unwrap();
        assert_eq!(Message::<T>::read(&text).as_ref(), Ok(&back.body), "{text}");
        assert_eq!(serde_json::to_string(&back).unwrap(), text);
    }

    #[test]
    fn every_type_goes_round_as_json() {
        round(Manifest {
            protocol: PROTOCOL_VERSION,
            name: "diff".into(),
            version: "0.1.0".into(),
            settings: vec![
                Setting::new("diff_story", true),
                Setting::new("share", "75"),
            ],
        });
        round(Event::Commit {
            dir: "/repo".into(),
            plan: "Plans/Plan - x.md".into(),
        });
        for action in [
            Action::Setting {
                key: "diff_order".into(),
            },
            Action::Folders,
            Action::InPane,
            Action::Command {
                args: vec!["say".into(), "--".into(), "hello".into()],
            },
            Action::Copy {
                text: "a\nb".into(),
            },
            Action::Pr {
                dir: "/repo".into(),
            },
            Action::ReviewedPr {
                dir: "/repo".into(),
            },
            Action::Untrusted {
                dir: "/repo".into(),
            },
            Action::Request {
                service: "Anthropic".into(),
                protocol: "claude -p".into(),
                source: "diff story".into(),
            },
            Action::Fetch {
                dir: "/repo".into(),
                remote: "origin".into(),
                source: "fetch base".into(),
            },
            Action::HeadlessClaude,
            Action::Plan { plan: None },
            Action::Plan {
                plan: Some("Plans/Plan - x.md".into()),
            },
            Action::Settings {
                keys: vec!["notes_images".into()],
            },
            Action::Paste,
            Action::PasteImage {
                file: "/n/a.png".into(),
            },
            Action::Preview {
                file: "/n/a.png".into(),
            },
            Action::LinkWords {
                url: "https://rfc.example/1".into(),
            },
            Action::OpenLink {
                url: "https://rfc.example/1".into(),
                watched: true,
            },
            Action::Draws,
            Action::Picture {
                id: 7,
                cols: 10,
                rows: 3,
                png: "/n/a.png".into(),
            },
            Action::PictureGone { id: 7 },
            Action::ItemClaim {
                dir: "/repo".into(),
            },
            Action::Queued,
            Action::Item {
                text: "- [ ] one".into(),
                line: 1,
            },
        ] {
            round(action);
        }
        for answer in [
            Answer::None,
            Answer::Done,
            Answer::Value { value: json!("75") },
            Answer::Values {
                values: vec![Some(json!(true)), None],
            },
            Answer::Text {
                text: "main".into(),
            },
            Answer::Flag { on: true },
            Answer::Number { number: 12345 },
            Answer::Pr {
                pr: Pr {
                    base: "main".into(),
                    number: 12345,
                    head: "feature".into(),
                },
            },
            Answer::Folders {
                folders: Folders::of("diff", Path::new("/s"), Path::new("/c")),
            },
            Answer::Env {
                env: Env {
                    set: vec![("A".into(), "b".into())],
                    remove: vec!["C".into()],
                },
            },
            Answer::Ran {
                code: 1,
                out: "o".into(),
                err: "e".into(),
            },
        ] {
            round(answer);
        }
    }

    #[test]
    fn what_is_said_reads_as_the_json_it_is_documented_as() {
        assert_eq!(
            serde_json::to_value(Message::new(Action::Setting {
                key: "theme".into()
            }))
            .unwrap(),
            json!({"protocol": 1, "body": {"ask": "setting", "key": "theme"}})
        );
        assert_eq!(
            serde_json::to_value(Answer::Flag { on: false }).unwrap(),
            json!({"answer": "flag", "on": false})
        );
        assert_eq!(
            serde_json::to_value(Event::Commit {
                dir: "/r".into(),
                plan: String::new()
            })
            .unwrap(),
            json!({"event": "commit", "dir": "/r", "plan": ""})
        );
    }

    #[test]
    fn a_newer_protocol_is_refused_before_it_is_read() {
        let newer = PROTOCOL_VERSION + 1;
        // Its body is of a shape this build doesn't know: still refused as
        // newer, not as unreadable.
        let text = format!(r#"{{"protocol":{newer},"body":{{"ask":"something new","n":1}}}}"#);
        assert_eq!(
            Message::<Action>::read(&text),
            Err(Refused::Newer {
                theirs: newer,
                ours: PROTOCOL_VERSION
            })
        );
        // This build's own, and an older one's, are read.
        assert_eq!(
            Message::<Action>::read(r#"{"protocol":1,"body":{"ask":"folders"}}"#),
            Ok(Action::Folders)
        );
        assert_eq!(
            Message::<Action>::read(r#"{"protocol":0,"body":{"ask":"in_pane"}}"#),
            Ok(Action::InPane)
        );
        for unreadable in [
            "not json",
            r#"{"body":{"ask":"folders"}}"#,
            r#"{"protocol":1,"body":{"ask":"nope"}}"#,
        ] {
            assert!(
                matches!(
                    Message::<Action>::read(unreadable),
                    Err(Refused::Unreadable(_))
                ),
                "{unreadable}"
            );
        }

        let mut manifest = Manifest {
            protocol: PROTOCOL_VERSION,
            name: "diff".into(),
            version: "0.1.0".into(),
            settings: Vec::new(),
        };
        assert_eq!(manifest.check(), Ok(()));
        manifest.protocol = newer;
        assert_eq!(
            manifest.check(),
            Err(Refused::Newer {
                theirs: newer,
                ours: PROTOCOL_VERSION
            })
        );
        // A manifest written before `settings` was listed still reads.
        let old: Manifest =
            serde_json::from_str(r#"{"protocol":1,"name":"x","version":"1"}"#).unwrap();
        assert!(old.settings.is_empty());
    }

    #[test]
    fn a_host_s_answers_are_read_and_one_that_says_nothing_is_safe() {
        struct Quiet;
        impl Host for Quiet {
            fn ask(&self, _: Action) -> Answer {
                Answer::None
            }
        }
        let quiet = Quiet;
        assert_eq!(quiet.setting("x"), None);
        assert!(quiet.is_on("x"), "one not listed counts as on");
        assert!(!quiet.in_pane());
        assert_eq!(quiet.pr(Path::new("/r")), None);
        assert_eq!(quiet.reviewed_pr(Path::new("/r")), None);
        assert!(
            quiet.untrusted(Path::new("/r")),
            "not said: taken as untrusted"
        );
        assert_eq!(quiet.plan(None), "");
        assert_eq!(quiet.headless_claude(), Env::default());
        assert_eq!(quiet.command(&["say", "x"]), None);
        assert_eq!(quiet.settings(&["x", "y"]), vec![None, None]);
        assert_eq!(quiet.paste(), None);
        assert!(!quiet.paste_image(Path::new("/n/a.png")));
        assert!(!quiet.preview(Path::new("/n/a.png")));
        assert_eq!(quiet.link_words("https://rfc.example/1"), None);
        assert!(quiet.open_link("https://rfc.example/1", false).is_err());
        assert!(!quiet.draws("pictures"));
        assert_eq!(quiet.picture(7, 10, 3, Path::new("/n/a.png")), "");
        assert_eq!(quiet.item_claim(Path::new("/r")), None);
        assert_eq!(quiet.queued(), None);
        assert!(
            quiet.item_refused("- [ ] one", 1).is_some(),
            "not said: no work is started"
        );
    }
}
