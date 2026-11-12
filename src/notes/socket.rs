//! The socket the notes app listens on, so the workspace can show a tab's
//! note in it (M-e bringing Notes beside a tab, a new note in the
//! notifications), as bin/notes had Neovim listen on one and
//! tmux/notes-open.sh reached it with `nvim --server`.
//!
//! The socket is `$NOTES_SOCKET`, else `notes.sock` in Switchboard's state
//! directory. A socket left by an app that has gone is removed and taken
//! over; one still answering belongs to the app already running, which a
//! second gives way to (it holds the notes' lock too).
//!
//! The protocol is a line a connection: one request, one reply.
//!
//! ```text
//! open <path in the notes directory>   the note shown          → ok | error <why>
//! sidebar on | sidebar off             the list shown or put away → ok
//! shown                                 the note shown           → ok <path> | ok
//! palette                               the note shown and the commands
//!                                       the app's palette lists there,
//!                                       for the workspace's M-p  → ok <JSON>
//! run <command's words>                 that palette command run → ok | error <why>
//! ping                                                           → ok
//! ```
//!
//! `palette`'s JSON is one line, `{"note": <path> | null, "commands":
//! [{"name", "key", "tag"}]}`; `run` names a command by its words as
//! `palette` gave them, looked up again where the app is when it comes.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Sender, channel};
use std::time::Duration;

use anyhow::{Context, Result, bail};

use super::app::Msg;

/// How long either side waits on the other.
const WAIT: Duration = Duration::from_secs(2);

/// `$NOTES_SOCKET`, else `notes.sock` in the state directory.
pub fn path() -> PathBuf {
    match std::env::var_os("NOTES_SOCKET") {
        Some(p) if !p.is_empty() => PathBuf::from(p),
        _ => crate::paths::state_dir().join("notes.sock"),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    Open(String),
    Sidebar(bool),
    Shown,
    Palette,
    Run(String),
    Ping,
}

pub fn parse(line: &str) -> Option<Request> {
    let line = line.trim_end_matches(['\n', '\r']);
    let (verb, rest) = line.split_once(' ').unwrap_or((line, ""));
    match (verb, rest) {
        ("open", rel) if !rel.is_empty() => Some(Request::Open(rel.to_string())),
        ("sidebar", "on") => Some(Request::Sidebar(true)),
        ("sidebar", "off") => Some(Request::Sidebar(false)),
        ("shown", "") => Some(Request::Shown),
        ("palette", "") => Some(Request::Palette),
        ("run", name) if !name.is_empty() => Some(Request::Run(name.to_string())),
        ("ping", "") => Some(Request::Ping),
        _ => None,
    }
}

/// What `palette` answers: the note shown, and the commands the app's
/// palette lists where the app is.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InFront {
    pub note: Option<String>,
    pub commands: Vec<Listed>,
}

/// One of the app's palette commands, as `palette` gives it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Listed {
    pub name: String,
    pub key: String,
    pub tag: String,
}

impl InFront {
    /// The reply to `palette`, after its `ok `: one line of JSON.
    pub fn reply(&self) -> String {
        let commands: Vec<serde_json::Value> = self
            .commands
            .iter()
            .map(|c| serde_json::json!({"name": c.name, "key": c.key, "tag": c.tag}))
            .collect();
        serde_json::json!({"note": self.note, "commands": commands}).to_string()
    }

    /// `palette`'s reply read back; `None` when it isn't one.
    pub fn read(reply: &str) -> Option<InFront> {
        let value: serde_json::Value = serde_json::from_str(reply).ok()?;
        let text = |v: &serde_json::Value, key: &str| {
            v.get(key)
                .and_then(|s| s.as_str())
                .unwrap_or_default()
                .to_string()
        };
        let commands = value
            .get("commands")?
            .as_array()?
            .iter()
            .map(|c| Listed {
                name: text(c, "name"),
                key: text(c, "key"),
                tag: text(c, "tag"),
            })
            .filter(|c| !c.name.is_empty() && !c.name.contains('\n'))
            .collect();
        Some(InFront {
            note: value
                .get("note")
                .and_then(|n| n.as_str())
                .map(str::to_string),
            commands,
        })
    }
}

/// Listens on `at`, sending each request to the app with where its reply
/// goes. `None` when another app answers there already.
pub fn listen(at: &Path, tx: Sender<Msg>) -> Option<UnixListener> {
    if at.exists() {
        if UnixStream::connect(at).is_ok() {
            return None;
        }
        let _ = std::fs::remove_file(at);
    }
    if let Some(dir) = at.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let listener = UnixListener::bind(at).ok()?;
    let accepting = listener.try_clone().ok()?;
    std::thread::spawn(move || {
        for stream in accepting.incoming() {
            let Ok(stream) = stream else { continue };
            let tx = tx.clone();
            std::thread::spawn(move || serve(stream, tx));
        }
    });
    Some(listener)
}

fn serve(stream: UnixStream, tx: Sender<Msg>) {
    let _ = stream.set_read_timeout(Some(WAIT));
    let _ = stream.set_write_timeout(Some(WAIT));
    let mut line = String::new();
    let Ok(reader) = stream.try_clone() else {
        return;
    };
    if BufReader::new(reader).read_line(&mut line).is_err() {
        return;
    }
    let reply = match parse(&line) {
        None => "error not a request".to_string(),
        Some(request) => {
            let (back, answer) = channel();
            if tx.send(Msg::Socket(request, back)).is_err() {
                return;
            }
            answer
                .recv_timeout(WAIT)
                .unwrap_or_else(|_| "error the app didn't answer".into())
        }
    };
    let mut stream = stream;
    let _ = stream.write_all(format!("{reply}\n").as_bytes());
}

/// Sends a request to the running app and gives its reply; an error when
/// none answers.
pub fn send(at: &Path, request: &str) -> Result<String> {
    let mut stream = UnixStream::connect(at).context("Notes isn't running")?;
    stream.set_read_timeout(Some(WAIT))?;
    stream.set_write_timeout(Some(WAIT))?;
    stream.write_all(format!("{request}\n").as_bytes())?;
    let mut reply = String::new();
    BufReader::new(stream).read_line(&mut reply)?;
    let reply = reply.trim_end().to_string();
    match reply.strip_prefix("error ") {
        Some(why) => bail!("{why}"),
        None => Ok(reply
            .strip_prefix("ok")
            .unwrap_or(&reply)
            .trim()
            .to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_are_read_a_line_each() {
        assert_eq!(
            parse("open Work/Plan.md\n"),
            Some(Request::Open("Work/Plan.md".into()))
        );
        assert_eq!(parse("sidebar off"), Some(Request::Sidebar(false)));
        assert_eq!(parse("shown"), Some(Request::Shown));
        assert_eq!(parse("palette"), Some(Request::Palette));
        assert_eq!(
            parse("run Find a note\n"),
            Some(Request::Run("Find a note".into()))
        );
        assert_eq!(parse("run "), None);
        assert_eq!(parse("open "), None);
        assert_eq!(parse("rm -rf"), None);
    }

    #[test]
    fn the_palette_reply_reads_back_as_it_was_written() {
        let front = InFront {
            note: Some("Work/Plan.md".into()),
            commands: vec![Listed {
                name: "Add an item".into(),
                key: "ga".into(),
                tag: "note".into(),
            }],
        };
        assert_eq!(InFront::read(&front.reply()), Some(front));
        assert_eq!(
            InFront::read(r#"{"note":null,"commands":[]}"#),
            Some(InFront::default())
        );
        assert_eq!(InFront::read("Plan.md"), None);
    }

    #[test]
    fn a_request_reaches_the_app_and_its_reply_comes_back() {
        let at = std::env::temp_dir().join(format!("nsock-{}.sock", std::process::id()));
        let (tx, rx) = channel();
        let _listener = listen(&at, tx).expect("listening");
        assert!(
            listen(&at, channel().0).is_none(),
            "a second app gives way to the one answering"
        );
        let asking = std::thread::spawn({
            let at = at.clone();
            move || send(&at, "shown")
        });
        match rx.recv_timeout(Duration::from_secs(5)).unwrap() {
            Msg::Socket(Request::Shown, back) => back.send("ok Plan.md".into()).unwrap(),
            _ => panic!("not the request"),
        }
        assert_eq!(asking.join().unwrap().unwrap(), "Plan.md");
        let _ = std::fs::remove_file(&at);
    }
}
