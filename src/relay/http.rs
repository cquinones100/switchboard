//! Slack over the network (lib/slack-relay/slack.js's `apiCall`, `putFile`,
//! `getFile` and `socketMode`): the Web API over HTTPS with reqwest, and
//! Socket Mode's WebSocket with tokio-tungstenite, both on rustls. Every
//! request is counted in the request audit as it is made.
//!
//! A Web API call goes out form-encoded, which every method accepts
//! (objects and arrays as JSON), with the bot's token. Slack's own refusal
//! (`ok: false`) is an error naming the method and why; a rate limit is
//! waited out as Slack asks, up to three times. None of this decides what
//! may be called: `Slack::api` has already refused anything that isn't for
//! the DM before a call gets here.
//!
//! Built without the `net` feature, each of these says Switchboard can't
//! reach Slack, and Socket Mode never connects.

use anyhow::Result;
use futures_util::future::LocalBoxFuture;
use serde_json::Value;

use super::slack::Transport;

/// A call that hasn't answered in this long has failed.
pub const API_TIMEOUT_SECS: u64 = 30;
/// A file sent or fetched can be a video of up to 200 MB.
pub const FILE_TIMEOUT_SECS: u64 = 5 * 60;

/// A call's arguments as a form: each present one, an object or array as
/// JSON, a string as it is (`URLSearchParams`).
pub fn form_body(params: &Value) -> String {
    let Some(map) = params.as_object() else {
        return String::new();
    };
    map.iter()
        .filter(|(_, v)| !v.is_null())
        .map(|(k, v)| {
            let text = match v {
                Value::String(s) => s.clone(),
                Value::Object(_) | Value::Array(_) => crate::util::json_compact(v),
                other => other.to_string(),
            };
            format!("{}={}", form_escape(k), form_escape(&text))
        })
        .collect::<Vec<_>>()
        .join("&")
}

/// `application/x-www-form-urlencoded`'s escaping: letters, digits and
/// `*-._` as they are, a space as `+`, everything else as UTF-8 bytes in
/// `%XX`.
fn form_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for b in text.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'*' | b'-' | b'.' | b'_' => {
                out.push(b as char)
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// The relay's way to Slack: the Web API at `base`, as the bot.
pub struct Https {
    pub base: String,
    pub bot_token: String,
    /// Whether requests are counted in the request audit (always, but in
    /// the tests, which must not write to your state directory).
    pub audit: bool,
    #[cfg(feature = "net")]
    client: reqwest::Client,
}

impl Https {
    pub fn new(base: String, bot_token: String) -> Result<Https> {
        #[cfg(feature = "net")]
        let client = reqwest::Client::builder()
            .user_agent(crate::net::USER_AGENT)
            .connect_timeout(std::time::Duration::from_secs(10))
            .build()
            .map_err(|e| anyhow::anyhow!("couldn't set up HTTPS: {e}"))?;
        Ok(Https {
            base,
            bot_token,
            audit: true,
            #[cfg(feature = "net")]
            client,
        })
    }

    #[cfg(feature = "net")]
    fn count(&self, protocol: &str, source: &str) {
        if self.audit {
            crate::requests::count("Slack", protocol, source);
        }
    }
}

#[cfg(feature = "net")]
impl Transport for Https {
    fn call<'a>(&'a self, method: &'a str, params: &'a Value) -> LocalBoxFuture<'a, Result<Value>> {
        Box::pin(async move {
            let body = form_body(params);
            let mut attempt = 0;
            loop {
                self.count("REST", &format!("relay: {method}"));
                let response = self
                    .client
                    .post(format!("{}{method}", self.base))
                    .bearer_auth(&self.bot_token)
                    .header("Content-Type", "application/x-www-form-urlencoded")
                    .body(body.clone())
                    .timeout(std::time::Duration::from_secs(API_TIMEOUT_SECS))
                    .send()
                    .await
                    .map_err(|e| anyhow::anyhow!("{method}: {}", reason(&e)))?;
                if response.status().as_u16() == 429 && attempt < 3 {
                    let wait = response
                        .headers()
                        .get("retry-after")
                        .and_then(|v| v.to_str().ok())
                        .and_then(|v| v.trim().parse::<f64>().ok())
                        .filter(|s| *s > 0.0)
                        .unwrap_or(1.0);
                    tokio::time::sleep(std::time::Duration::from_secs_f64(wait)).await;
                    attempt += 1;
                    continue;
                }
                let bytes = response
                    .bytes()
                    .await
                    .map_err(|e| anyhow::anyhow!("{method}: {}", reason(&e)))?;
                let reply: Value = serde_json::from_slice(&bytes)
                    .map_err(|_| anyhow::anyhow!("{method}: Slack's answer wasn't JSON"))?;
                if reply.get("ok") != Some(&Value::Bool(true)) {
                    let error = reply
                        .get("error")
                        .and_then(Value::as_str)
                        .unwrap_or("undefined");
                    anyhow::bail!("{method}: {error}");
                }
                return Ok(reply);
            }
        })
    }

    fn put<'a>(&'a self, url: &'a str, bytes: Vec<u8>) -> LocalBoxFuture<'a, Result<()>> {
        Box::pin(async move {
            self.count("HTTPS", "relay: file upload");
            let response = self
                .client
                .post(url)
                .body(bytes)
                .timeout(std::time::Duration::from_secs(FILE_TIMEOUT_SECS))
                .send()
                .await
                .map_err(|e| anyhow::anyhow!("file upload: {}", reason(&e)))?;
            if !response.status().is_success() {
                anyhow::bail!("file upload: HTTP {}", response.status().as_u16());
            }
            Ok(())
        })
    }

    fn get<'a>(&'a self, url: &'a str) -> LocalBoxFuture<'a, Result<Vec<u8>>> {
        Box::pin(async move {
            self.count("HTTPS", "relay: file download");
            let response = self
                .client
                .get(url)
                .bearer_auth(&self.bot_token)
                .timeout(std::time::Duration::from_secs(FILE_TIMEOUT_SECS))
                .send()
                .await
                .map_err(|e| anyhow::anyhow!("file download: {}", reason(&e)))?;
            if !response.status().is_success() {
                anyhow::bail!("file download: HTTP {}", response.status().as_u16());
            }
            Ok(response
                .bytes()
                .await
                .map_err(|e| anyhow::anyhow!("file download: {}", reason(&e)))?
                .to_vec())
        })
    }
}

/// What reqwest says went wrong, its cause rather than its wrapper.
#[cfg(feature = "net")]
fn reason(e: &reqwest::Error) -> String {
    if e.is_timeout() {
        return "it took too long to answer".into();
    }
    let mut why = e.to_string();
    let mut source = std::error::Error::source(e);
    while let Some(cause) = source {
        why = cause.to_string();
        source = cause.source();
    }
    why
}

#[cfg(not(feature = "net"))]
const NO_NET: &str = "Switchboard was built without network support, so it can't reach Slack.";

#[cfg(not(feature = "net"))]
impl Transport for Https {
    fn call<'a>(&'a self, _: &'a str, _: &'a Value) -> LocalBoxFuture<'a, Result<Value>> {
        Box::pin(async { Err(anyhow::anyhow!(NO_NET)) })
    }

    fn put<'a>(&'a self, _: &'a str, _: Vec<u8>) -> LocalBoxFuture<'a, Result<()>> {
        Box::pin(async { Err(anyhow::anyhow!(NO_NET)) })
    }

    fn get<'a>(&'a self, _: &'a str) -> LocalBoxFuture<'a, Result<Vec<u8>>> {
        Box::pin(async { Err(anyhow::anyhow!(NO_NET)) })
    }
}

// ── Socket Mode ─────────────────────────────────────────────────────────────

/// Socket Mode: Slack sends each event down the socket in an envelope,
/// acknowledged before anything else (see `slack::envelope`). Slack also
/// asks the app to reconnect now and then (a "disconnect"), and a dropped
/// connection is opened again, waiting longer after each failure in a row.
/// Every (re)connection catches up on what was sent while the socket was
/// down. Runs until the relay stops.
#[cfg(feature = "net")]
pub async fn socket_mode(cx: super::Cx, app_token: String) {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message as Ws;

    use super::slack::{Envelope, RECONNECT_DELAYS_MS, envelope};

    let mut failures = 0usize;
    loop {
        if cx.life.borrow().stopping {
            return;
        }
        let why = match open_socket(&app_token).await {
            Err(e) => e.to_string(),
            Ok(url) => match tokio_tungstenite::connect_async(url.as_str()).await {
                Err(e) => format!("couldn't connect: {e}"),
                Ok((mut socket, _)) => {
                    crate::requests::count("Slack", "WebSocket", "relay: Socket Mode");
                    while let Some(message) = socket.next().await {
                        if cx.life.borrow().stopping {
                            let _ = socket.close(None).await;
                            return;
                        }
                        let text = match message {
                            Ok(Ws::Text(t)) => t.to_string(),
                            Ok(Ws::Close(_)) | Err(_) => break,
                            Ok(_) => continue,
                        };
                        let Some((ack, what)) = envelope(&text) else {
                            continue;
                        };
                        if let Some(ack) = ack
                            && socket.send(Ws::text(ack)).await.is_err()
                        {
                            break;
                        }
                        match what {
                            Envelope::Hello => {
                                failures = 0;
                                cx.log("Slack: connected");
                                super::run::on_connect(&cx);
                            }
                            Envelope::Disconnect => break,
                            Envelope::Event { event, team, app } => {
                                super::run::on_event(&cx, event, team, app)
                            }
                            Envelope::Interactive(payload) => {
                                super::run::on_interactive(&cx, payload)
                            }
                            Envelope::Other => {}
                        }
                    }
                    "connection closed".to_string()
                }
            },
        };
        if cx.life.borrow().stopping {
            return;
        }
        let delay = RECONNECT_DELAYS_MS[failures.min(RECONNECT_DELAYS_MS.len() - 1)];
        failures += 1;
        cx.log(&format!("Slack: {why}; reconnecting in {}s", delay / 1000));
        tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
    }
}

/// The socket's address, from `apps.connections.open` with the app-level
/// token.
#[cfg(feature = "net")]
async fn open_socket(app_token: &str) -> Result<String> {
    crate::requests::count("Slack", "REST", "relay: apps.connections.open");
    let client = reqwest::Client::builder()
        .user_agent(crate::net::USER_AGENT)
        .timeout(std::time::Duration::from_secs(API_TIMEOUT_SECS))
        .build()?;
    let reply: Value = client
        .post(format!("{}apps.connections.open", super::slack::api_base()))
        .bearer_auth(app_token)
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("apps.connections.open: {}", reason(&e)))?
        .json()
        .await
        .map_err(|e| anyhow::anyhow!("apps.connections.open: {}", reason(&e)))?;
    if reply.get("ok") != Some(&Value::Bool(true)) {
        anyhow::bail!(
            "apps.connections.open: {}",
            reply
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("undefined")
        );
    }
    reply
        .get("url")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| anyhow::anyhow!("apps.connections.open: no address"))
}

#[cfg(not(feature = "net"))]
pub async fn socket_mode(cx: super::Cx, _app_token: String) {
    cx.log(&format!("Slack: {NO_NET}"));
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn arguments_go_form_encoded_with_objects_as_json() {
        let body = form_body(&json!({
            "channel": "D1",
            "text": "a b&c=é",
            "thread_ts": null,
            "unfurl_links": false,
            "blocks": [{ "type": "markdown" }],
        }));
        assert_eq!(
            body,
            "channel=D1&text=a+b%26c%3D%C3%A9&unfurl_links=false&blocks=%5B%7B%22type%22%3A%22markdown%22%7D%5D"
        );
    }

    /// A server of the test's own on 127.0.0.1, answering each request with
    /// the next of `answers` (status, headers, body), and handing back each
    /// request it read.
    #[cfg(feature = "net")]
    fn stand_in(
        answers: Vec<(u16, &'static str, String)>,
    ) -> (String, std::sync::mpsc::Receiver<String>) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}/api/", listener.local_addr().unwrap());
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for (status, headers, body) in answers {
                let Ok((mut stream, _)) = listener.accept() else {
                    return;
                };
                let mut request = Vec::new();
                let mut buf = [0u8; 4096];
                loop {
                    let n = stream.read(&mut buf).unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    request.extend_from_slice(&buf[..n]);
                    let text = String::from_utf8_lossy(&request);
                    if let Some(end) = text.find("\r\n\r\n") {
                        let length = text[..end]
                            .lines()
                            .find_map(|l| {
                                l.to_ascii_lowercase()
                                    .strip_prefix("content-length:")
                                    .map(|v| v.trim().parse::<usize>().unwrap_or(0))
                            })
                            .unwrap_or(0);
                        if request.len() >= end + 4 + length {
                            break;
                        }
                    }
                }
                let _ = tx.send(String::from_utf8_lossy(&request).into_owned());
                let reply = format!(
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n{headers}\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(reply.as_bytes());
            }
        });
        (base, rx)
    }

    #[cfg(feature = "net")]
    #[test]
    fn the_api_client_form_encodes_waits_out_rate_limits_and_reports_refusals() {
        let (base, requests) = stand_in(vec![
            (
                429,
                "Retry-After: 1\r\n",
                r#"{"ok":false,"error":"ratelimited"}"#.into(),
            ),
            (200, "", r#"{"ok":true,"ts":"1.2"}"#.into()),
            (
                200,
                "",
                r#"{"ok":false,"error":"channel_not_found"}"#.into(),
            ),
        ]);
        let mut https = Https::new(base, "xoxb-test".into()).unwrap();
        https.audit = false;
        let slack = crate::relay::slack::Slack::new(Box::new(https));
        *slack.channel.borrow_mut() = Some("D1".into());
        *slack.owner.borrow_mut() = Some("U1".into());
        crate::util::block_on(async {
            let started = std::time::Instant::now();
            let sent = slack
                .api(
                    "chat.postMessage",
                    json!({ "channel": "D1", "text": "hi there" }),
                )
                .await
                .unwrap();
            assert_eq!(sent["ts"], "1.2");
            assert!(
                started.elapsed() >= std::time::Duration::from_millis(900),
                "the limit was waited out"
            );
            let refused = slack
                .api("chat.postMessage", json!({ "channel": "D1", "text": "x" }))
                .await
                .unwrap_err();
            assert_eq!(refused.to_string(), "chat.postMessage: channel_not_found");
            // Another conversation never reaches Slack at all.
            let elsewhere = slack
                .api("chat.postMessage", json!({ "channel": "C2", "text": "x" }))
                .await
                .unwrap_err();
            assert!(elsewhere.to_string().contains("only uses its DM, not C2"));
        });
        let first = requests.recv().unwrap();
        assert!(first.starts_with("POST /api/chat.postMessage"));
        assert!(
            first
                .to_ascii_lowercase()
                .contains("authorization: bearer xoxb-test")
        );
        assert!(first.ends_with("channel=D1&text=hi+there"));
        assert_eq!(
            requests.try_iter().count(),
            2,
            "three requests in all, none for C2"
        );
    }
}
