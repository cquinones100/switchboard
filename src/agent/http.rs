//! A tool call's hooks over HTTP on the server's own socket, for Switchboard's
//! hooks module in Claude Code's process (`hooks/agent.js`, written by
//! `src/setup/plugin.rs`). New with the Rust port: a command hook starts a
//! process, and PreToolUse and PostToolUse run on every tool call, which on a
//! computer whose security agent inspects every process started made Claude
//! slow. A hooks module can reach a Unix socket only by HTTP
//! (`$.http.fetch` with `socketPath`), so the server tells such a request
//! from a frame by its first byte (a frame's is its tag, 1 or 2) and answers
//! it here.
//!
//! The request is `POST /hook/<event>`, the hook's JSON as its body and the
//! pane in `X-Switchboard-Pane` (none for a conversation in the background);
//! it is routed as `switchboard hook` routes it (`hook::route`, off the
//! server's task since it reads the sessions' files) and handed to the
//! server as the same command, and the answer is the JSON object Claude Code
//! would read from the hook, or an empty body. Only the tool call's events
//! come this way: the rest keep their command hook, which does things in its
//! own process (SessionStart's stand-ins on the PATH).

use std::path::PathBuf;

use serde_json::{Map, Value};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::{mpsc, oneshot};

use super::hook;
use crate::mux::proto::MAX_FRAME;
use crate::mux::server::{Answer, Event};

/// The first byte of an HTTP request this answers (`POST`).
pub const FIRST: u8 = b'P';

/// The events a hooks module sends here.
pub const EVENTS: [&str; 3] = ["PreToolUse", "PostToolUse", "PostToolUseFailure"];

/// The longest head read, so a broken peer can't make the server allocate
/// without end.
const MAX_HEAD: usize = 64 * 1024;

/// How long a request has to arrive whole.
const READ_WAIT: std::time::Duration = std::time::Duration::from_secs(10);

/// A request read: the event, the pane and the body.
#[derive(Debug, PartialEq)]
struct Request {
    event: String,
    pane: Option<String>,
    body: Vec<u8>,
}

/// Answers one request whose first byte, `FIRST`, is already read.
pub async fn serve(
    r: &mut (impl AsyncRead + Unpin),
    w: &mut (impl AsyncWrite + Unpin),
    tx: mpsc::UnboundedSender<Event>,
) {
    // A request not read whole in time is dropped, so a peer that stops
    // sending holds nothing.
    let read = tokio::time::timeout(READ_WAIT, read_request(r))
        .await
        .ok()
        .flatten();
    let answer = match read {
        Some(request) if EVENTS.contains(&request.event.as_str()) => {
            Some(answer(request, tx).await.unwrap_or_default())
        }
        _ => None,
    };
    let response = match answer {
        Some(body) => format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        ),
        None => {
            "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_string()
        }
    };
    let _ = w.write_all(response.as_bytes()).await;
    let _ = w.shutdown().await;
}

/// The server's answer to the event, as `switchboard hook` would print it.
async fn answer(request: Request, tx: mpsc::UnboundedSender<Event>) -> Option<String> {
    let payload = serde_json::from_slice::<Value>(&request.body)
        .ok()
        .filter(Value::is_object)
        .unwrap_or_else(|| Value::Object(Map::new()));
    let cwd = payload
        .get("cwd")
        .and_then(Value::as_str)
        .map_or_else(|| PathBuf::from("/"), PathBuf::from);
    let (event, pane) = (request.event, request.pane.clone());
    let routed =
        tokio::task::spawn_blocking(move || hook::route(&event, &payload, pane.as_deref()))
            .await
            .ok()??;
    let (rtx, rrx) = oneshot::channel();
    tx.send(Event::Command {
        args: routed.command,
        cwd,
        pane: request.pane,
        stdin: Some(request.body),
        by_hand: false,
        reply: rtx,
    })
    .ok()?;
    let Ok(Answer::Reply(reply)) = rrx.await else {
        return None;
    };
    (routed.answers && reply.code == 0)
        .then(|| hook::answer_json(reply.stdout.as_bytes()))
        .flatten()
}

/// Reads the rest of a request whose first byte was `FIRST`: `None` for
/// anything but a `POST /hook/<event>` read whole.
async fn read_request(r: &mut (impl AsyncRead + Unpin)) -> Option<Request> {
    let mut buf = vec![FIRST];
    let head_len = loop {
        let mut headers = [httparse::EMPTY_HEADER; 32];
        let mut req = httparse::Request::new(&mut headers);
        match req.parse(&buf) {
            Ok(httparse::Status::Complete(len)) => break len,
            Ok(httparse::Status::Partial) if buf.len() < MAX_HEAD => {}
            _ => return None,
        }
        let mut chunk = [0u8; 4096];
        let n = r.read(&mut chunk).await.ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
    };
    let mut headers = [httparse::EMPTY_HEADER; 32];
    let mut req = httparse::Request::new(&mut headers);
    req.parse(&buf).ok()?;
    if req.method != Some("POST") {
        return None;
    }
    let event = req.path?.strip_prefix("/hook/")?.to_string();
    let header = |name: &str| {
        req.headers
            .iter()
            .find(|h| h.name.eq_ignore_ascii_case(name))
            .and_then(|h| std::str::from_utf8(h.value).ok())
            .map(str::trim)
    };
    let length: usize = header("content-length")?.parse().ok()?;
    if length > MAX_FRAME {
        return None;
    }
    let pane = header("x-switchboard-pane")
        .filter(|p| !p.is_empty())
        .map(str::to_string);
    let mut body = buf.split_off(head_len);
    // Grown as the bytes come, as a frame is, never reserved from the
    // length the peer declares.
    if body.len() < length {
        let rest = (length - body.len()) as u64;
        r.take(rest).read_to_end(&mut body).await.ok()?;
    }
    if body.len() < length {
        return None;
    }
    body.truncate(length);
    Some(Request { event, pane, body })
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn read(text: &str) -> Option<Request> {
        // The first byte is the connection's to read.
        let mut rest = &text.as_bytes()[1..];
        read_request(&mut rest).await
    }

    #[tokio::test]
    async fn a_hook_request_is_read_whole() {
        let request = read(
            "POST /hook/PreToolUse HTTP/1.1\r\nHost: switchboard\r\nX-Switchboard-Pane: %3\r\nContent-Length: 2\r\n\r\n{}",
        )
        .await;
        assert_eq!(
            request,
            Some(Request {
                event: "PreToolUse".into(),
                pane: Some("%3".into()),
                body: b"{}".to_vec(),
            })
        );
    }

    #[tokio::test]
    async fn anything_else_is_refused() {
        assert_eq!(
            read("PUT /hook/Stop HTTP/1.1\r\nContent-Length: 0\r\n\r\n").await,
            None
        );
        assert_eq!(
            read("POST /other HTTP/1.1\r\nContent-Length: 0\r\n\r\n").await,
            None
        );
        assert_eq!(read("POST /hook/Stop HTTP/1.1\r\n\r\n").await, None);
        assert_eq!(
            read("POST /hook/Stop HTTP/1.1\r\nContent-Length: 9\r\n\r\n{}").await,
            None
        );
    }
}
