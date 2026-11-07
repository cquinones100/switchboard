//! Telling the story of a diff, as nvim/lua/plugins/diffview.lua's
//! `ask_story` did: Haiku reads the change in the background, through
//! `claude -p` with no tools and its answer streamed (`stream-json`), each
//! piece added to the answer so far, which the cover shows; the whole answer
//! from the final `result` line, else from the streamed pieces once the
//! turn has ended, since a long answer's `result` line loses its end through
//! the pipe (a diff of 474 hunks, its last line cut at 63 KB of 69).
//!
//! A story is kept in the cache's `diff-story/` by its key (the diff, the
//! commits' messages and the prompt's version, `story::key`), as the answer
//! itself (`<key>.txt`), so the same diff opens as a story at once and an
//! edit since is a new one. While it is asked for, the answer so far is kept
//! beside it as `<key>.partial`, rewritten when it grows (at most every
//! 250ms) and every two seconds whatever, so another program asked for the
//! same story (M-d while it is told ahead, or the other way round) follows
//! this ask, its progress on its own cover, rather than paying for its own,
//! and asks only once the file is gone or 30 seconds old with no story kept.
//! A program quitting takes the partial files it writes away with it, so
//! another doesn't wait on an ask no one is finishing.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Result, anyhow};
use serde_json::Value;

use super::story::{self, Hunks, Progress, Story};
use crate::{paths, requests, util};

/// How long Haiku has to tell a story.
pub const STORY_TIMEOUT: Duration = Duration::from_secs(300);
/// A partial file updated this recently is an ask under way to follow.
const FOLLOW_FRESH: Duration = Duration::from_secs(10);
/// A followed ask whose partial hasn't changed for this long has stopped.
const FOLLOW_STALE: Duration = Duration::from_secs(30);

/// Claude Code's command: on the PATH, else where its installer puts it,
/// `~/.local/bin`, which the PATH of a program the server starts may leave out.
pub fn claude_command() -> Option<PathBuf> {
    util::which("claude").or_else(|| {
        let installed = paths::home_join(".local/bin/claude");
        is_executable(&installed).then_some(installed)
    })
}

fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// A headless Claude's environment: `~/.local/bin` on the PATH, and not
/// the pane's, so its hooks don't speak for the tab it runs under.
pub fn headless(cmd: &mut Command) {
    let path = format!(
        "{}:{}",
        std::env::var("PATH").unwrap_or_default(),
        paths::home_join(".local/bin").display()
    );
    cmd.env("PATH", path)
        .env_remove("SWITCHBOARD_PANE")
        .env_remove("SWITCHBOARD")
        .env_remove("TMUX")
        .env_remove("TMUX_PANE");
}

/// A command's stdout, fed `input`, killed after `seconds`: an error, with
/// the first line of what it said on stderr, when it fails.
pub fn run_with_timeout(mut cmd: Command, input: &[u8], seconds: u64) -> Result<String> {
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    if let Some(mut stdin) = child.stdin.take() {
        let input = input.to_vec();
        std::thread::spawn(move || {
            let _ = stdin.write_all(&input);
        });
    }
    let mut stdout = child.stdout.take().expect("piped");
    let reader = std::thread::spawn(move || {
        let mut out = String::new();
        let _ = stdout.read_to_string(&mut out);
        out
    });
    let deadline = Instant::now() + Duration::from_secs(seconds);
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(anyhow!("it took longer than {seconds} seconds"));
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let out = reader.join().unwrap_or_default();
    if status.success() {
        return Ok(out);
    }
    let mut err = String::new();
    if let Some(mut stderr) = child.stderr.take() {
        let _ = stderr.read_to_string(&mut err);
    }
    Err(anyhow!(
        "exited with {}: {}",
        status.code().unwrap_or(-1),
        err.lines().next().unwrap_or("").trim()
    ))
}

/// One line of `claude -p --output-format stream-json`, as the story and a
/// question about the diff read them.
#[derive(Clone, Debug, PartialEq)]
pub enum Streamed {
    /// A piece of the answer's text.
    Text(String),
    /// The turn has ended.
    EndTurn,
    /// A tool the answer calls, and its input.
    Tool(String, Value),
    /// The final result: the whole answer, or what went wrong.
    Result(std::result::Result<String, String>),
}

/// What a line of the stream says, or `None` for one that says nothing here.
pub fn streamed(line: &str) -> Vec<Streamed> {
    let Ok(event) = serde_json::from_str::<Value>(line) else {
        return Vec::new();
    };
    let kind = event.get("type").and_then(Value::as_str).unwrap_or("");
    let inner = event.get("event").filter(|e| e.is_object());
    let inner_kind = inner
        .and_then(|e| e.get("type"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let delta = inner.and_then(|e| e.get("delta")).filter(|d| d.is_object());
    match kind {
        "stream_event" if inner_kind == "content_block_delta" => {
            match (
                delta.and_then(|d| d.get("type")).and_then(Value::as_str),
                delta.and_then(|d| d.get("text")).and_then(Value::as_str),
            ) {
                (Some("text_delta"), Some(text)) => vec![Streamed::Text(text.to_string())],
                _ => Vec::new(),
            }
        }
        "stream_event" if inner_kind == "message_delta" => {
            let ended = delta
                .and_then(|d| d.get("stop_reason"))
                .and_then(Value::as_str)
                == Some("end_turn");
            if ended {
                vec![Streamed::EndTurn]
            } else {
                Vec::new()
            }
        }
        "assistant" => event
            .pointer("/message/content")
            .and_then(Value::as_array)
            .map(|blocks| {
                blocks
                    .iter()
                    .filter(|b| b.get("type").and_then(Value::as_str) == Some("tool_use"))
                    .map(|b| {
                        let name = b.get("name").and_then(Value::as_str).unwrap_or("?");
                        let input = b.get("input").filter(|i| i.is_object()).cloned();
                        Streamed::Tool(name.to_string(), input.unwrap_or(Value::Null))
                    })
                    .collect()
            })
            .unwrap_or_default(),
        "result" => {
            let error = event.get("is_error") == Some(&Value::Bool(true));
            let result = event.get("result").and_then(Value::as_str);
            vec![Streamed::Result(match (error, result) {
                (false, Some(text)) => Ok(text.to_string()),
                (_, Some(text)) => Err(text.to_string()),
                (_, None) => Err("an error".into()),
            })]
        }
        _ => Vec::new(),
    }
}

/// What telling a story comes to, told as it goes.
#[derive(Clone, Debug)]
pub enum Heard {
    /// The answer so far (an ask of this program's, or one followed).
    Progress(Progress),
    /// The story, with the answer it was read from.
    Told(Box<Story>),
    /// No story, and why.
    Untold(String),
}

/// A story to tell: its key, what Haiku reads, and what its answer is read
/// against (the diff's paths and hunks).
#[derive(Clone, Debug)]
pub struct Asked {
    pub key: String,
    pub input: String,
    pub paths: Vec<String>,
    pub hunks: Hunks,
}

/// Where stories are kept: the cache's `diff-story/`.
pub fn story_dir() -> PathBuf {
    paths::cache_dir().join("diff-story")
}

/// Whatever a teller has going that must end with the program: the
/// process asked, and the partial files written.
#[derive(Clone, Default)]
pub struct Teller {
    child: Arc<Mutex<Option<Child>>>,
    owned: Arc<Mutex<Vec<PathBuf>>>,
    stopped: Arc<AtomicBool>,
}

impl Teller {
    /// Ends the ask under way and takes its partial file away, as the
    /// program quits.
    pub fn stop(&self) {
        self.stopped.store(true, Ordering::Relaxed);
        if let Some(mut child) = self.child.lock().unwrap_or_else(|e| e.into_inner()).take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        for file in self
            .owned
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .drain(..)
        {
            let _ = std::fs::remove_file(file);
        }
    }

    fn stopped(&self) -> bool {
        self.stopped.load(Ordering::Relaxed)
    }

    /// Tells the story of `asked`, from the cache, from another program's
    /// ask under way, or by asking Haiku, saying what it hears through
    /// `heard`; blocks until it is told or can't be.
    pub fn tell(&self, asked: &Asked, heard: &dyn Fn(Heard)) {
        let dir = story_dir();
        let file = dir.join(format!("{}.txt", asked.key));
        let partial = dir.join(format!("{}.partial", asked.key));
        let parse =
            |text: &str| story::parse(text, &asked.paths, super::order::generated, &asked.hunks);
        let kept = || -> Option<Story> { parse(&util::read_text(&file)?).ok() };
        if let Some(story) = kept() {
            return heard(Heard::Told(Box::new(story)));
        }
        if age(&partial).is_some_and(|a| a < FOLLOW_FRESH) {
            // Another program telling this story now: its progress shown
            // here, its story taken from the cache once kept; asked here
            // only if it stops with no story kept.
            loop {
                if self.stopped() {
                    return;
                }
                let mut made = kept();
                let now = age(&partial);
                if made.is_none() && now.is_none() {
                    made = kept();
                }
                if let Some(story) = made {
                    return heard(Heard::Told(Box::new(story)));
                }
                match now {
                    Some(a) if a < FOLLOW_STALE => {}
                    _ => break,
                }
                if let Some(p) = util::read_text(&partial)
                    .and_then(|t| serde_json::from_str::<Progress>(&t).ok())
                {
                    heard(Heard::Progress(p));
                }
                std::thread::sleep(Duration::from_millis(250));
            }
        }
        self.ask(asked, &file, &partial, heard);
    }

    fn ask(&self, asked: &Asked, file: &Path, partial: &Path, heard: &dyn Fn(Heard)) {
        let Some(claude) = claude_command() else {
            return heard(Heard::Untold("claude isn't installed".into()));
        };
        let dir = file.parent().unwrap_or(Path::new("/")).to_path_buf();
        let _ = std::fs::create_dir_all(&dir);
        let mut cmd = Command::new(claude);
        cmd.args([
            "-p",
            "--model",
            "haiku",
            "--no-session-persistence",
            "--restricted",
            "--tools",
            "",
            "--strict-mcp-config",
            "--disable-slash-commands",
            "--system-prompt",
            story::PROMPT,
            "--output-format",
            "stream-json",
            "--include-partial-messages",
            "--verbose",
        ])
        .current_dir(&dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
        headless(&mut cmd);
        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => return heard(Heard::Untold(format!("claude couldn't start: {e}"))),
        };
        requests::count("Anthropic", "claude -p", "diff story");
        let progress = Arc::new(Mutex::new(Progress {
            text: String::new(),
            started: util::now_secs(),
        }));
        self.owned
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(partial.to_path_buf());
        let stdin = child.stdin.take();
        let stdout = child.stdout.take();
        let mut stderr = child.stderr.take();
        *self.child.lock().unwrap_or_else(|e| e.into_inner()) = Some(child);
        if let Some(mut stdin) = stdin {
            let input = asked.input.clone();
            std::thread::spawn(move || {
                let _ = stdin.write_all(input.as_bytes());
            });
        }
        let err_reader = std::thread::spawn(move || {
            let mut text = String::new();
            if let Some(e) = stderr.as_mut() {
                let _ = e.read_to_string(&mut text);
            }
            text
        });
        // The stream read on a thread of its own, so the partial file is
        // kept up to date while it comes.
        let (lines_tx, lines_rx) = std::sync::mpsc::channel::<String>();
        if let Some(stdout) = stdout {
            std::thread::spawn(move || {
                for line in BufReader::new(stdout).lines() {
                    let Ok(line) = line else { break };
                    if lines_tx.send(line).is_err() {
                        break;
                    }
                }
            });
        }
        let started = Instant::now();
        let mut written: Option<(String, Instant)> = None;
        let save = |written: &mut Option<(String, Instant)>| {
            let p = progress.lock().unwrap_or_else(|e| e.into_inner()).clone();
            if written
                .as_ref()
                .is_some_and(|(t, at)| *t == p.text && at.elapsed() < Duration::from_secs(2))
            {
                return;
            }
            let _ = util::write_atomic(partial, util::json_compact(&p));
            *written = Some((p.text, Instant::now()));
        };
        save(&mut written);
        let (mut answer, mut error_text, mut streamed_text, mut ended) =
            (None::<String>, None::<String>, String::new(), false);
        let mut last_save = Instant::now();
        let mut timed_out = false;
        loop {
            match lines_rx.recv_timeout(Duration::from_millis(250)) {
                Ok(line) => {
                    for event in streamed(&line) {
                        match event {
                            Streamed::Text(text) => {
                                progress
                                    .lock()
                                    .unwrap_or_else(|e| e.into_inner())
                                    .text
                                    .push_str(&text);
                                streamed_text.push_str(&text);
                                heard(Heard::Progress(
                                    progress.lock().unwrap_or_else(|e| e.into_inner()).clone(),
                                ));
                            }
                            Streamed::EndTurn => ended = true,
                            Streamed::Result(Ok(text)) => answer = Some(text),
                            Streamed::Result(Err(text)) => error_text = Some(text),
                            Streamed::Tool(..) => {}
                        }
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            }
            if last_save.elapsed() >= Duration::from_millis(250) {
                save(&mut written);
                last_save = Instant::now();
            }
            if self.stopped() {
                return;
            }
            if started.elapsed() >= STORY_TIMEOUT {
                timed_out = true;
                if let Some(mut child) = self.child.lock().unwrap_or_else(|e| e.into_inner()).take()
                {
                    let _ = child.kill();
                    let _ = child.wait();
                }
                break;
            }
        }
        let code = match self.child.lock().unwrap_or_else(|e| e.into_inner()).take() {
            Some(mut child) => child.wait().ok().and_then(|s| s.code()).unwrap_or(-1),
            None if timed_out => 124,
            None => return,
        };
        let said = err_reader.join().unwrap_or_default();
        let whole =
            (ended && error_text.is_none() && !streamed_text.is_empty()).then_some(streamed_text);
        let text = if code == 0 { answer.or(whole) } else { None };
        let first = |s: &str| s.lines().next().unwrap_or("").trim().to_string();
        let told = text
            .as_deref()
            .map(|t| story::parse(t, &asked.paths, super::order::generated, &asked.hunks));
        let why = if code == 124 {
            Some(format!(
                "Haiku took longer than {} seconds",
                STORY_TIMEOUT.as_secs()
            ))
        } else if code != 0 {
            let said = first(&said);
            Some(format!(
                "claude exited with code {code}{}",
                if said.is_empty() {
                    String::new()
                } else {
                    format!(": {said}")
                }
            ))
        } else if let Some(e) = &error_text {
            Some(format!("Claude answered with an error: {}", first(e)))
        } else {
            match &told {
                Some(Ok(_)) => None,
                Some(Err(Some(unfit))) => Some(unfit.clone()),
                _ => Some("Haiku's answer didn't parse as a story".into()),
            }
        };
        // The story kept before its progress goes, so a program following
        // finds one or the other.
        if let (Some(Ok(_)), Some(t)) = (&told, &text) {
            let _ = util::write_atomic(file, t);
        }
        let _ = std::fs::remove_file(partial);
        self.owned
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|p| p != partial);
        match (told, why) {
            (Some(Ok(story)), _) => heard(Heard::Told(Box::new(story))),
            (_, Some(why)) => heard(Heard::Untold(why)),
            _ => heard(Heard::Untold(
                "Haiku's answer didn't parse as a story".into(),
            )),
        }
    }
}

/// How long ago a file was last written, or `None` when it isn't there.
fn age(file: &Path) -> Option<Duration> {
    let modified = std::fs::metadata(file).and_then(|m| m.modified()).ok()?;
    Some(
        SystemTime::now()
            .duration_since(modified)
            .unwrap_or(Duration::ZERO),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stream_gives_text_the_turns_end_tools_and_the_result() {
        assert_eq!(
            streamed(
                r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"It "}}}"#
            ),
            vec![Streamed::Text("It ".into())]
        );
        assert_eq!(
            streamed(
                r#"{"type":"stream_event","event":{"type":"message_delta","delta":{"stop_reason":"end_turn"}}}"#
            ),
            vec![Streamed::EndTurn]
        );
        assert_eq!(
            streamed(
                r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Read","input":{"file_path":"/repo/lib/b.js"}}]}}"#
            ),
            vec![Streamed::Tool(
                "Read".into(),
                serde_json::json!({"file_path": "/repo/lib/b.js"})
            )]
        );
        assert_eq!(
            streamed(r#"{"type":"result","result":"It works."}"#),
            vec![Streamed::Result(Ok("It works.".into()))]
        );
        assert_eq!(
            streamed(r#"{"type":"result","is_error":true,"result":"Overloaded"}"#),
            vec![Streamed::Result(Err("Overloaded".into()))]
        );
        assert!(streamed("not json").is_empty());
    }
}
