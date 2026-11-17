//! How the PR view and the ticket view follow what they show. Each read
//! its PR or ticket once, as it opened (pr-view.sh and ticket-view.sh did
//! too), so a view left open went on showing the checks, reviewers and
//! state of that moment.
//!
//! A view reads again when it is told to, never on a timer of its own, so
//! it asks GitHub or Linear no more than the workspace's lookups say there
//! is something to ask about, and nothing while they are paused:
//!
//! - the server tells it what the lookups know of its PR or ticket, as a
//!   word that changes when that does (`Server::views_follow`,
//!   src/sources/serve.rs): `reread <word>` on the socket the view listens
//!   on, `view-<pane>.sock` in the state directory, a line a connection as
//!   Notes' socket takes them (src/notes/socket.rs), answered `ok`. It
//!   says so when the word changes and again each time the lookup
//!   answers, and the view reads only for a word other than the one its
//!   last such read succeeded for, or when a read has failed since: so a
//!   read that failed, whoever asked for it, is tried again when the
//!   lookup next answers, and an answer that says nothing new
//!   costs no request;
//! - `r` in the view (and `reread` alone on the socket);
//! - the view taking the focus after `STALE` or more without it, as the
//!   server tells a pane of the focus: between panes, tabs and apps, and
//!   not where a tool takes the view's place or a split closes beside it.
//!
//! One thread does the reading, a request at a time, those that came while
//! it read answered by one read. A copy that reads the same changes
//! nothing on the screen but when it was read; one that can't be read
//! leaves the copy shown, the pager saying how old it is once it is `STALE`
//! old (`age`). Each read after the first reads the settings again first,
//! so a view left open asks nothing of a service turned off since.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, SystemTime};

use serde_json::Value;

use super::pager::Content;
use crate::util;

/// How old a copy is before the view says so, and how long a view is
/// without the focus before taking it reads again.
pub const STALE: Duration = Duration::from_secs(5 * 60);

/// How long the view waits on whoever connected to its socket.
const WAIT: Duration = Duration::from_secs(2);

/// What the socket takes, alone or followed by the server's word for
/// what the lookups know.
pub const REREAD: &str = "reread";

/// A request for a read.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Ask {
    /// Read now: as the view opens, on r, on taking the focus.
    Now,
    /// The server's word for what the lookups know: read unless the last
    /// read told so succeeded for this word.
    Told(String),
}

/// The socket of the view in `pane` (`%12`, as `SWITCHBOARD_PANE` has it).
pub fn socket_for(pane: &str) -> PathBuf {
    let n: String = pane.chars().filter(char::is_ascii_digit).collect();
    crate::paths::state_dir().join(format!("view-{n}.sock"))
}

/// What a read came to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Update {
    /// A copy that differs from the one shown, laid out.
    Show(Content),
    /// Read, and the same as the copy shown.
    Same,
    /// It couldn't be read, and why; the copy shown stays.
    Kept(String),
}

/// A view's reads, for the pager: what each came to, and a way to ask for
/// one.
pub struct Follow {
    pub updates: Receiver<Update>,
    asks: Sender<Ask>,
    /// When the copy first shown was read: its file's modified time.
    pub read_at: Option<SystemTime>,
    _socket: Option<Listening>,
}

impl Follow {
    /// Asks for the PR or ticket to be read again.
    pub fn again(&self) {
        let _ = self.asks.send(Ask::Now);
    }
}

/// The view's socket, removed as the view ends.
struct Listening(PathBuf);

impl Drop for Listening {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Starts following: `cached` is the copy shown as the view opens, kept in
/// the file `cache` names; `read` reads it from its service, and `content`
/// lays a copy out. `cache` is asked before each read, since the PR view
/// reads its checkout's branch as it is then, and a branch checked out
/// since the view opened has a file of its own. It is read once at once,
/// and then as it is asked for, by the server on the pane's socket where
/// the view is in a pane.
pub fn start(
    cached: Option<Value>,
    cache: impl Fn() -> Option<PathBuf> + Send + 'static,
    read: impl Fn() -> Result<Value, String> + Send + 'static,
    content: fn(&Value) -> Content,
) -> Follow {
    let read_at = cached
        .as_ref()
        .and_then(|_| cache())
        .and_then(|file| std::fs::metadata(file).ok()?.modified().ok());
    let (asks, asked) = channel::<Ask>();
    let (tx, updates) = channel();
    std::thread::spawn(move || {
        let mut shown = cached;
        // The server's word the last read it told of succeeded for.
        let mut read_for: Option<String> = None;
        let mut first = true;
        while let Ok(ask) = asked.recv() {
            let mut asks = vec![ask];
            asks.extend(asked.try_iter());
            let told = asks.iter().rev().find_map(|ask| match ask {
                Ask::Told(word) => Some(word.clone()),
                Ask::Now => None,
            });
            let nothing_new = told.is_some() && told == read_for;
            if nothing_new && !asks.contains(&Ask::Now) {
                continue;
            }
            // The settings as they are now, which this process read once
            // as it started: a service turned off since the view opened is
            // asked nothing (`integrations` refuses), and the copy is laid
            // out as Settings now say.
            if !first {
                crate::settings::reload_settings();
            }
            first = false;
            let file = cache();
            let read = read();
            // A read that failed, whoever asked, is owed again: the next
            // answer reads whatever its word.
            read_for = match (&read, told) {
                (Ok(_), Some(word)) => Some(word),
                (Ok(_), None) => read_for,
                (Err(_), _) => None,
            };
            let update = match read {
                Ok(copy) if shown.as_ref() == Some(&copy) => Update::Same,
                Ok(copy) => {
                    if let Some(file) = &file {
                        if let Some(parent) = file.parent() {
                            let _ = std::fs::create_dir_all(parent);
                        }
                        let _ = util::write_atomic(file, util::json_compact(&copy));
                    }
                    let laid_out = content(&copy);
                    shown = Some(copy);
                    Update::Show(laid_out)
                }
                // With a copy shown, it stays; only with nothing to show is
                // the reason the view.
                Err(why) if shown.is_none() => Update::Show(Content::Failed(why)),
                Err(why) => Update::Kept(why),
            };
            if tx.send(update).is_err() {
                return;
            }
        }
    });
    let _ = asks.send(Ask::Now);
    let socket = std::env::var("SWITCHBOARD_PANE")
        .ok()
        .filter(|pane| !pane.is_empty())
        .and_then(|pane| listen(&socket_for(&pane), asks.clone()));
    Follow {
        updates,
        asks,
        read_at,
        _socket: socket,
    }
}

/// Listens on `at`, each `reread` asking for a read (`request`). A socket left there
/// by a view that has gone is taken over, since a pane has one view.
fn listen(at: &Path, asks: Sender<Ask>) -> Option<Listening> {
    let _ = std::fs::remove_file(at);
    if let Some(dir) = at.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let listener = UnixListener::bind(at).ok()?;
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            serve(stream, &asks);
        }
    });
    Some(Listening(at.to_path_buf()))
}

/// The ask a line on the socket is: `reread`, or `reread <word>`.
fn request(line: &str) -> Option<Ask> {
    let line = line.trim_end_matches(['\n', '\r']);
    match line.strip_prefix(REREAD)? {
        "" => Some(Ask::Now),
        rest => rest
            .strip_prefix(' ')
            .filter(|word| !word.is_empty())
            .map(|word| Ask::Told(word.to_string())),
    }
}

fn serve(stream: UnixStream, asks: &Sender<Ask>) {
    let _ = stream.set_read_timeout(Some(WAIT));
    let _ = stream.set_write_timeout(Some(WAIT));
    let mut line = String::new();
    let Ok(reader) = stream.try_clone() else {
        return;
    };
    if BufReader::new(reader).read_line(&mut line).is_err() {
        return;
    }
    let reply = match request(&line) {
        Some(ask) => {
            let _ = asks.send(ask);
            "ok"
        }
        None => "error not a request",
    };
    let mut stream = stream;
    let _ = stream.write_all(format!("{reply}\n").as_bytes());
}

/// How old a copy read at `read_at` is, in the view's words, once it is
/// `STALE` old: "read 12m ago".
pub fn age(read_at: SystemTime, now: SystemTime) -> Option<String> {
    let old = now.duration_since(read_at).ok()?;
    if old < STALE {
        return None;
    }
    let minutes = old.as_secs() / 60;
    Some(match minutes {
        0..60 => format!("read {minutes}m ago"),
        60..2880 => format!("read {}h ago", minutes / 60),
        _ => format!("read {}d ago", minutes / 1440),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::{Arc, Mutex};

    fn laid_out(copy: &Value) -> Content {
        Content::Log(copy.to_string())
    }

    #[test]
    fn a_copy_says_its_age_once_it_is_five_minutes_old() {
        let at = SystemTime::UNIX_EPOCH;
        let after = |secs: u64| at + Duration::from_secs(secs);
        assert_eq!(age(at, after(299)), None);
        assert_eq!(age(at, after(300)).as_deref(), Some("read 5m ago"));
        assert_eq!(
            age(at, after(59 * 60 + 59)).as_deref(),
            Some("read 59m ago")
        );
        assert_eq!(age(at, after(3 * 3600)).as_deref(), Some("read 3h ago"));
        assert_eq!(age(at, after(72 * 3600)).as_deref(), Some("read 3d ago"));
        assert_eq!(age(after(10), at), None, "a clock set back says nothing");
    }

    #[test]
    fn each_ask_reads_again_and_only_a_changed_copy_is_shown() {
        let dir = std::env::temp_dir().join(format!("follow-{}", std::process::id()));
        let cache = dir.join("copy.json");
        let answers = Arc::new(Mutex::new(vec![
            Err("GitHub is down".to_string()),
            Ok(json!({"checks": "passing"})),
            Ok(json!({"checks": "failing"})),
            Ok(json!({"checks": "failing"})),
        ]));
        let read = {
            let answers = answers.clone();
            move || {
                let next = answers.lock().unwrap().pop();
                next.unwrap_or_else(|| Err("GitHub is down".to_string()))
            }
        };
        // The file is the one named as each read starts: the second
        // copy is kept in the second file.
        let files = Arc::new(Mutex::new(vec![
            dir.join("unused.json"),
            dir.join("unused.json"),
            cache.clone(),
            dir.join("before.json"),
            dir.join("before.json"),
            dir.join("opening.json"),
        ]));
        let follow = start(
            Some(json!({"checks": "failing"})),
            move || files.lock().unwrap().pop(),
            read,
            laid_out,
        );
        let next = || {
            follow
                .updates
                .recv_timeout(Duration::from_secs(10))
                .unwrap()
        };
        assert_eq!(next(), Update::Same, "read as it opens, the same");
        follow.again();
        assert_eq!(next(), Update::Same);
        assert!(!dir.exists(), "nothing new to keep");
        follow.again();
        assert_eq!(
            next(),
            Update::Show(Content::Log(r#"{"checks":"passing"}"#.into()))
        );
        assert_eq!(
            util::read_json(&cache),
            Some(json!({"checks": "passing"})),
            "kept for the next time"
        );
        assert!(
            !dir.join("before.json").exists() && !dir.join("opening.json").exists(),
            "in the file named as that read started"
        );
        follow.again();
        assert_eq!(next(), Update::Kept("GitHub is down".into()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn told_what_the_lookups_know_it_reads_until_a_read_for_it_succeeds() {
        let answers = Arc::new(Mutex::new(vec![
            Ok(json!({"checks": "passing"})),
            Err("GitHub is down".to_string()),
            Ok(json!({"checks": "failing"})),
        ]));
        let reads = Arc::new(Mutex::new(0));
        let read = {
            let (answers, reads) = (answers.clone(), reads.clone());
            move || {
                *reads.lock().unwrap() += 1;
                let next = answers.lock().unwrap().pop();
                next.unwrap_or_else(|| Err("GitHub is down".to_string()))
            }
        };
        let follow = start(Some(json!({"checks": "failing"})), || None, read, laid_out);
        let told = |word: &str| follow.asks.send(Ask::Told(word.into())).unwrap();
        let next = || {
            follow
                .updates
                .recv_timeout(Duration::from_secs(10))
                .unwrap()
        };
        let quiet = || {
            follow
                .updates
                .recv_timeout(Duration::from_millis(200))
                .is_err()
        };
        assert_eq!(next(), Update::Same, "read as it opens");
        // The checks pass as GitHub fails: the copy stays, and the lookup's
        // next answer, the same word, has it read again.
        told("passing");
        assert_eq!(next(), Update::Kept("GitHub is down".into()));
        told("passing");
        assert_eq!(
            next(),
            Update::Show(Content::Log(r#"{"checks":"passing"}"#.into()))
        );
        // Read for that word, the answers that repeat it ask nothing.
        told("passing");
        told("passing");
        assert!(quiet());
        assert_eq!(*reads.lock().unwrap(), 3);
        // r reads whatever the word; failing, it is owed again, so the
        // next answer reads though its word is the one read for.
        follow.again();
        assert_eq!(next(), Update::Kept("GitHub is down".into()));
        told("passing");
        assert_eq!(next(), Update::Kept("GitHub is down".into()));
        // And a new word reads.
        told("merged");
        assert_eq!(next(), Update::Kept("GitHub is down".into()));
        assert_eq!(*reads.lock().unwrap(), 6);
    }

    #[test]
    fn with_nothing_shown_a_failed_read_is_the_view() {
        let follow = start(
            None,
            || None,
            || Err("No PR for this branch.".into()),
            laid_out,
        );
        assert_eq!(
            follow
                .updates
                .recv_timeout(Duration::from_secs(10))
                .unwrap(),
            Update::Show(Content::Failed("No PR for this branch.".into()))
        );
        assert_eq!(follow.read_at, None);
    }

    #[test]
    fn reread_on_the_socket_asks_for_a_read() {
        let at = std::env::temp_dir().join(format!("vsock-{}.sock", std::process::id()));
        let (asks, asked) = channel();
        let listening = listen(&at, asks).expect("listening");
        let send = |request: &str| crate::notes::socket::send(&at, request);
        let asked = || asked.recv_timeout(Duration::from_secs(10)).unwrap();
        assert!(send(REREAD).is_ok());
        assert_eq!(asked(), Ask::Now);
        assert!(send("reread 1f3a").is_ok());
        assert_eq!(asked(), Ask::Told("1f3a".into()));
        for not_one in ["rm -rf", "rereads", "reread "] {
            assert!(send(not_one).is_err(), "{not_one:?}");
        }
        drop(listening);
        assert!(!at.exists(), "the socket goes with the view");
    }
}
