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

// ── Discord ─────────────────────────────────────────────────────────────────

/// The relay's way to Discord: the REST API at `base`, as the bot. A route
/// Discord says has no requests left waits until its bucket resets (its
/// `X-RateLimit-Remaining` and `X-RateLimit-Reset-After`), and a 429 is
/// waited out as its `retry_after` says, up to three times. None of this
/// decides what may be called: `Discord::api` has already refused anything
/// that isn't for the DM.
pub struct DiscordHttps {
    pub base: String,
    pub bot_token: String,
    pub audit: bool,
    /// When each route may next be called, by its method and path.
    #[cfg_attr(not(feature = "net"), allow(dead_code))]
    waits: std::cell::RefCell<std::collections::HashMap<String, std::time::Instant>>,
    #[cfg(feature = "net")]
    client: reqwest::Client,
}

impl DiscordHttps {
    pub fn new(base: String, bot_token: String) -> Result<DiscordHttps> {
        #[cfg(feature = "net")]
        let client = reqwest::Client::builder()
            .user_agent(format!(
                "DiscordBot (https://github.com/cquinones100/switchboard, {})",
                env!("CARGO_PKG_VERSION")
            ))
            .connect_timeout(std::time::Duration::from_secs(10))
            .build()
            .map_err(|e| anyhow::anyhow!("couldn't set up HTTPS: {e}"))?;
        Ok(DiscordHttps {
            base,
            bot_token,
            audit: true,
            waits: Default::default(),
            #[cfg(feature = "net")]
            client,
        })
    }
}

/// A route's bucket, as Discord keys its limits: the method and the path
/// with any message's ID and the query left out.
pub fn route_key(method: &str, path: &str) -> String {
    let path = path.split('?').next().unwrap_or(path);
    let parts: Vec<&str> = path.split('/').collect();
    let kept: Vec<&str> = parts
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let after_messages = i > 0 && parts[i - 1] == "messages";
            if after_messages || (i > 0 && parts[i - 1] == "interactions") {
                ":id"
            } else {
                p
            }
        })
        .collect();
    format!("{method} {}", kept.join("/"))
}

/// How long Discord asks to wait: a 429's `retry_after` (seconds), else
/// its `Retry-After` header, else a second.
pub fn retry_after(body: &Value, header: Option<&str>) -> f64 {
    body.get("retry_after")
        .and_then(Value::as_f64)
        .or_else(|| header.and_then(|h| h.trim().parse().ok()))
        .filter(|s| *s > 0.0)
        .unwrap_or(1.0)
}

/// Discord's refusal as an error: the call, its status and message, with
/// `message_not_found` for a 404 so the parts read it as Slack's.
pub fn discord_error(method: &str, path: &str, status: u16, body: &Value) -> anyhow::Error {
    let said = body
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("no reason given");
    let route = path.split('?').next().unwrap_or(path);
    if status == 404 {
        anyhow::anyhow!("{method} {route}: message_not_found ({said})")
    } else {
        anyhow::anyhow!("{method} {route}: HTTP {status}, {said}")
    }
}

#[cfg(feature = "net")]
impl DiscordHttps {
    fn count(&self, protocol: &str, source: &str) {
        if self.audit {
            crate::requests::count("Discord", protocol, source);
        }
    }

    /// Sends one request, waiting out the route's limits, and gives the
    /// answer's JSON.
    async fn send(
        &self,
        method: &str,
        path: &str,
        build: impl Fn() -> reqwest::RequestBuilder,
    ) -> Result<Value> {
        let key = route_key(method, path);
        let mut attempt = 0;
        loop {
            let until = self.waits.borrow().get(&key).copied();
            if let Some(until) = until {
                tokio::time::sleep_until(until.into()).await;
            }
            self.count("REST", &format!("relay: {key}"));
            let response = build()
                .header("Authorization", format!("Bot {}", self.bot_token))
                .timeout(std::time::Duration::from_secs(API_TIMEOUT_SECS))
                .send()
                .await
                .map_err(|e| anyhow::anyhow!("{method} {path}: {}", reason(&e)))?;
            let header = |name: &str| {
                response
                    .headers()
                    .get(name)
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_string)
            };
            let remaining = header("x-ratelimit-remaining");
            let reset = header("x-ratelimit-reset-after").and_then(|r| r.parse::<f64>().ok());
            match (remaining.as_deref(), reset) {
                (Some("0"), Some(after)) => {
                    let until = std::time::Instant::now()
                        + std::time::Duration::from_secs_f64(after.max(0.0));
                    self.waits.borrow_mut().insert(key.clone(), until);
                }
                _ => {
                    self.waits.borrow_mut().remove(&key);
                }
            }
            let status = response.status().as_u16();
            let retry_header = header("retry-after");
            let bytes = response
                .bytes()
                .await
                .map_err(|e| anyhow::anyhow!("{method} {path}: {}", reason(&e)))?;
            let body: Value = if bytes.is_empty() {
                Value::Null
            } else {
                serde_json::from_slice(&bytes).unwrap_or(Value::Null)
            };
            if status == 429 && attempt < 3 {
                let wait = retry_after(&body, retry_header.as_deref());
                tokio::time::sleep(std::time::Duration::from_secs_f64(wait)).await;
                attempt += 1;
                continue;
            }
            if !(200..300).contains(&status) {
                return Err(discord_error(method, path, status, &body));
            }
            return Ok(body);
        }
    }
}

#[cfg(feature = "net")]
impl super::discord::Rest for DiscordHttps {
    fn call<'a>(
        &'a self,
        method: &'a str,
        path: &'a str,
        body: Option<&'a Value>,
    ) -> LocalBoxFuture<'a, Result<Value>> {
        Box::pin(async move {
            let url = format!("{}{path}", self.base);
            let verb = reqwest::Method::from_bytes(method.as_bytes())?;
            self.send(method, path, || {
                let request = self.client.request(verb.clone(), &url);
                match body {
                    Some(b) => request
                        .header("Content-Type", "application/json")
                        .body(crate::util::json_compact(b)),
                    None if method == "PUT" || method == "POST" => {
                        request.header("Content-Length", "0")
                    }
                    None => request,
                }
            })
            .await
        })
    }

    /// The message as multipart: its JSON as `payload_json`, then each file
    /// as `files[n]`.
    fn upload<'a>(
        &'a self,
        path: &'a str,
        payload: &'a Value,
        files: Vec<(String, Vec<u8>)>,
    ) -> LocalBoxFuture<'a, Result<Value>> {
        Box::pin(async move {
            let boundary = format!("switchboard{}", crate::util::now_ms());
            let body = multipart(&boundary, payload, &files);
            let url = format!("{}{path}", self.base);
            self.count("HTTPS", "relay: file upload");
            self.send("POST", path, || {
                self.client
                    .post(&url)
                    .header(
                        "Content-Type",
                        format!("multipart/form-data; boundary={boundary}"),
                    )
                    .timeout(std::time::Duration::from_secs(FILE_TIMEOUT_SECS))
                    .body(body.clone())
            })
            .await
        })
    }

    /// An attachment, from Discord's CDN, whose address is signed: the
    /// bot's token is never sent there.
    fn get<'a>(&'a self, url: &'a str) -> LocalBoxFuture<'a, Result<Vec<u8>>> {
        Box::pin(async move {
            self.count("HTTPS", "relay: file download");
            let response = self
                .client
                .get(url)
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

#[cfg(not(feature = "net"))]
const NO_NET_DISCORD: &str =
    "Switchboard was built without network support, so it can't reach Discord.";

#[cfg(not(feature = "net"))]
impl super::discord::Rest for DiscordHttps {
    fn call<'a>(
        &'a self,
        _: &'a str,
        _: &'a str,
        _: Option<&'a Value>,
    ) -> LocalBoxFuture<'a, Result<Value>> {
        Box::pin(async { Err(anyhow::anyhow!(NO_NET_DISCORD)) })
    }

    fn upload<'a>(
        &'a self,
        _: &'a str,
        _: &'a Value,
        _: Vec<(String, Vec<u8>)>,
    ) -> LocalBoxFuture<'a, Result<Value>> {
        Box::pin(async { Err(anyhow::anyhow!(NO_NET_DISCORD)) })
    }

    fn get<'a>(&'a self, _: &'a str) -> LocalBoxFuture<'a, Result<Vec<u8>>> {
        Box::pin(async { Err(anyhow::anyhow!(NO_NET_DISCORD)) })
    }
}

/// A `multipart/form-data` body: the message's JSON, then each file.
pub fn multipart(boundary: &str, payload: &Value, files: &[(String, Vec<u8>)]) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"payload_json\"\r\nContent-Type: application/json\r\n\r\n").bytes());
    body.extend(crate::util::json_compact(payload).bytes());
    body.extend(b"\r\n");
    for (i, (name, bytes)) in files.iter().enumerate() {
        let name = name.replace(['"', '\r', '\n'], "_");
        body.extend(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"files[{i}]\"; filename=\"{name}\"\r\nContent-Type: application/octet-stream\r\n\r\n").bytes());
        body.extend(bytes);
        body.extend(b"\r\n");
    }
    body.extend(format!("--{boundary}--\r\n").bytes());
    body
}

/// The Gateway: HELLO says how often to beat; the relay identifies (or
/// resumes its session), beats with a jitter before the first, and hands
/// each event to `DiscordChat::on_dispatch`. A heartbeat Discord didn't
/// acknowledge by the next means the connection is a zombie, closed and
/// resumed; Discord's Reconnect (op 7) resumes; an Invalid Session (op 9)
/// resumes when it says so, else identifies afresh after a wait. A dropped
/// connection resumes at `resume_gateway_url`, waiting longer after each
/// failure in a row, and a close that connecting again can't fix (a bad
/// token) stops the relay with why. Runs until the relay stops.
#[cfg(feature = "net")]
pub async fn discord_gateway(cx: super::Cx, chat: super::discord_chat::DiscordChat, token: String) {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message as Ws;

    use super::discord::{
        Gateway, Session, fatal_close, fresh_after, heartbeat, identify, read_gateway, resume,
    };
    use super::slack::RECONNECT_DELAYS_MS;

    let mut session = Session::default();
    let mut failures = 0usize;
    loop {
        if cx.life.borrow().stopping {
            return;
        }
        let base = match (&session.resume_url, session.id.is_some()) {
            (Some(url), true) => Ok(url.clone()),
            _ => gateway_url(&chat).await,
        };
        let why = match base {
            Err(e) => e.to_string(),
            Ok(base) => {
                let url = format!("{}/?v=10&encoding=json", base.trim_end_matches('/'));
                match tokio_tungstenite::connect_async(url.as_str()).await {
                    Err(e) => format!("couldn't connect: {e}"),
                    Ok((mut socket, _)) => {
                        crate::requests::count("Discord", "WebSocket", "relay: Gateway");
                        let mut beat: Option<tokio::time::Interval> = None;
                        let why = loop {
                            let next = async {
                                match beat.as_mut() {
                                    Some(b) => {
                                        b.tick().await;
                                    }
                                    None => std::future::pending::<()>().await,
                                }
                            };
                            tokio::select! {
                                _ = next => {
                                    if !session.acked {
                                        break "Discord stopped answering heartbeats".to_string();
                                    }
                                    session.acked = false;
                                    if socket.send(Ws::text(heartbeat(&session))).await.is_err() {
                                        break "couldn't send a heartbeat".to_string();
                                    }
                                }
                                message = socket.next() => {
                                    if cx.life.borrow().stopping {
                                        let _ = socket.close(None).await;
                                        return;
                                    }
                                    let text = match message {
                                        Some(Ok(Ws::Text(t))) => t.to_string(),
                                        Some(Ok(Ws::Close(frame))) => {
                                            let code = frame.as_ref().map(|f| u16::from(f.code)).unwrap_or(1000);
                                            if let Some(fatal) = fatal_close(code) {
                                                cx.log(&format!("Discord: {fatal} (close {code})"));
                                                super::lifecycle::stop_relay(&cx).await;
                                                return;
                                            }
                                            if fresh_after(code) {
                                                session = Session::default();
                                            }
                                            break format!("closed ({code})");
                                        }
                                        Some(Err(e)) => break format!("connection lost: {e}"),
                                        None => break "connection closed".to_string(),
                                        Some(Ok(_)) => continue,
                                    };
                                    match read_gateway(&mut session, &text) {
                                        Gateway::Hello(every) => {
                                            let jitter = rand_fraction();
                                            let first = std::time::Duration::from_millis((every as f64 * jitter) as u64);
                                            let mut b = tokio::time::interval_at(
                                                tokio::time::Instant::now() + first,
                                                std::time::Duration::from_millis(every),
                                            );
                                            b.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                                            beat = Some(b);
                                            session.acked = true;
                                            let hello = resume(&token, &session).unwrap_or_else(|| identify(&token));
                                            if socket.send(Ws::text(hello)).await.is_err() {
                                                break "couldn't identify".to_string();
                                            }
                                        }
                                        Gateway::Beat => {
                                            if socket.send(Ws::text(heartbeat(&session))).await.is_err() {
                                                break "couldn't send a heartbeat".to_string();
                                            }
                                        }
                                        Gateway::Ack | Gateway::Other => {}
                                        Gateway::Dispatch(name, d) => {
                                            if name == "READY" || name == "RESUMED" {
                                                failures = 0;
                                            }
                                            chat.on_dispatch(&cx, &name, d);
                                        }
                                        Gateway::Reconnect => break "Discord asked for a reconnect".to_string(),
                                        Gateway::Invalid { resumable } => {
                                            if !resumable {
                                                session = Session::default();
                                                let wait = 1_000 + (rand_fraction() * 4_000.0) as u64;
                                                tokio::time::sleep(std::time::Duration::from_millis(wait)).await;
                                            }
                                            break "Discord ended the session".to_string();
                                        }
                                    }
                                }
                            }
                        };
                        let _ = socket.close(None).await;
                        why
                    }
                }
            }
        };
        if cx.life.borrow().stopping {
            return;
        }
        let delay = RECONNECT_DELAYS_MS[failures.min(RECONNECT_DELAYS_MS.len() - 1)];
        failures += 1;
        cx.log(&format!(
            "Discord: {why}; reconnecting in {}s",
            delay / 1000
        ));
        tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
    }
}

/// Where the Gateway is: `SWITCHBOARD_DISCORD_GATEWAY`, else Discord's
/// `/gateway/bot`.
#[cfg(feature = "net")]
async fn gateway_url(chat: &super::discord_chat::DiscordChat) -> Result<String> {
    if let Some(url) = super::discord::gateway_override() {
        return Ok(url);
    }
    let reply = chat.0.discord.api("GET", "/gateway/bot", None).await?;
    reply
        .get("url")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| anyhow::anyhow!("/gateway/bot: no address"))
}

/// A number from 0 to 1, for the heartbeat's jitter.
#[cfg(feature = "net")]
fn rand_fraction() -> f64 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    f64::from(nanos % 1_000_000) / 1_000_000.0
}

#[cfg(not(feature = "net"))]
pub async fn discord_gateway(
    cx: super::Cx,
    _chat: super::discord_chat::DiscordChat,
    _token: String,
) {
    cx.log(&format!("Discord: {NO_NET_DISCORD}"));
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

    #[cfg(feature = "net")]
    #[test]
    fn the_api_client_form_encodes_waits_out_rate_limits_and_reports_refusals() {
        use crate::fake_http::{Fake, Reply, Service};
        // A Slack of the test's own, as strict as Slack about the token and
        // the method, answering each request with the next of these.
        let fake = Fake::canned(
            Service::Slack {
                token: "xoxb-test".into(),
            },
            vec![
                Reply::json(429, json!({ "ok": false, "error": "ratelimited" }))
                    .header("Retry-After", "1"),
                Reply::json(200, json!({ "ok": true, "ts": "1.2" })),
                Reply::json(200, json!({ "ok": false, "error": "channel_not_found" })),
            ],
        );
        let base = format!("{}/api/", fake.url);
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
        let seen = fake.seen();
        assert_eq!(
            (seen[0].method.as_str(), seen[0].path()),
            ("POST", "/api/chat.postMessage")
        );
        assert_eq!(seen[0].header("authorization"), Some("Bearer xoxb-test"));
        assert_eq!(seen[0].text(), "channel=D1&text=hi+there");
        assert_eq!(seen.len(), 3, "three requests in all, none for C2");
    }

    #[cfg(feature = "net")]
    #[test]
    fn discords_api_sends_the_bots_token_and_waits_out_a_429_as_retry_after_says() {
        use crate::fake_http::{Fake, Reply, Service};
        use crate::relay::discord::Rest;
        let fake = Fake::canned(
            Service::Web,
            vec![
                Reply::json(
                    429,
                    json!({ "message": "You are being rate limited.", "retry_after": 0.6, "global": false }),
                ),
                Reply::json(200, json!({ "id": "1" })),
                Reply::json(404, json!({ "message": "Unknown Message", "code": 10008 })),
            ],
        );
        let mut rest = DiscordHttps::new(fake.url.clone(), "tok".into()).unwrap();
        rest.audit = false;
        crate::util::block_on(async {
            let started = std::time::Instant::now();
            let sent = rest
                .call(
                    "POST",
                    "/channels/D1/messages",
                    Some(&json!({ "content": "hi" })),
                )
                .await
                .unwrap();
            assert_eq!(sent["id"], "1");
            assert!(
                started.elapsed() >= std::time::Duration::from_millis(550),
                "the limit was waited out"
            );
            let gone = rest
                .call("DELETE", "/channels/D1/messages/9", None)
                .await
                .unwrap_err();
            assert!(gone.to_string().contains("message_not_found"), "{gone}");
        });
        let seen = fake.seen();
        assert_eq!(seen.len(), 3);
        assert_eq!(seen[0].header("authorization"), Some("Bot tok"));
        assert_eq!(seen[1].path(), "/channels/D1/messages");
        assert_eq!(seen[1].json(), json!({ "content": "hi" }));
    }

    #[test]
    fn a_routes_bucket_leaves_out_message_ids_and_the_query() {
        assert_eq!(
            route_key("PATCH", "/channels/D1/messages/123?x=1"),
            "PATCH /channels/D1/messages/:id"
        );
        assert_eq!(
            route_key("POST", "/interactions/9/tok/callback"),
            "POST /interactions/:id/tok/callback"
        );
        assert_eq!(retry_after(&json!({ "retry_after": 2.5 }), Some("9")), 2.5);
        assert_eq!(retry_after(&json!({}), Some("3")), 3.0);
    }

    #[test]
    fn an_upload_is_its_json_then_each_file() {
        let body = multipart(
            "B",
            &json!({ "a": 1 }),
            &[("x.png".into(), b"PNG".to_vec())],
        );
        let text = String::from_utf8(body).unwrap();
        assert!(text.starts_with("--B\r\nContent-Disposition: form-data; name=\"payload_json\""));
        assert!(text.contains("name=\"files[0]\"; filename=\"x.png\""));
        assert!(text.contains("\r\n\r\nPNG\r\n"));
        assert!(text.ends_with("--B--\r\n"));
    }

    /// A Gateway of the test's own: the relay identifies, Discord closes
    /// the connection, and the relay resumes the same session.
    #[cfg(feature = "net")]
    #[test]
    fn a_dropped_gateway_connection_resumes_its_session() {
        use futures_util::{SinkExt, StreamExt};
        use tokio_tungstenite::tungstenite::Message as Ws;
        use tokio_tungstenite::tungstenite::protocol::CloseFrame;
        use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;

        let rig = crate::relay::testing::Rig::discord();
        let chat = rig.discord_chat.clone().unwrap();
        rig.block_on(async {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url = format!("ws://{}", listener.local_addr().unwrap());
            let given = url.clone();
            *rig.discord.answer.borrow_mut() = Some(Box::new(move |call| {
                (call.path == "/gateway/bot").then(|| Ok(json!({ "url": given })))
            }));
            let gateway =
                tokio::task::spawn_local(discord_gateway(rig.cx.clone(), chat, "tok".into()));
            let read =
                |text: Ws| serde_json::from_str::<Value>(&text.into_text().unwrap()).unwrap();
            let hello =
                || Ws::text(json!({ "op": 10, "d": { "heartbeat_interval": 60_000 } }).to_string());
            let within = std::time::Duration::from_secs(20);
            // The first connection: identified, made ready, then closed.
            let (stream, _) = tokio::time::timeout(within, listener.accept())
                .await
                .unwrap()
                .unwrap();
            let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
            socket.send(hello()).await.unwrap();
            let identify = read(socket.next().await.unwrap().unwrap());
            assert_eq!(identify["op"], 2);
            assert_eq!(identify["d"]["token"], "tok");
            assert_eq!(
                identify["d"]["intents"],
                json!(crate::relay::discord::INTENTS)
            );
            let ready = json!({ "op": 0, "s": 1, "t": "READY", "d": {
                "session_id": "S1", "resume_gateway_url": url,
                "user": { "id": crate::relay::testing::DISCORD_BOT },
            } });
            socket.send(Ws::text(ready.to_string())).await.unwrap();
            socket
                .send(Ws::Close(Some(CloseFrame {
                    code: CloseCode::Library(4000),
                    reason: "".into(),
                })))
                .await
                .unwrap();
            drop(socket);
            // The second: resumed where it got to.
            let (stream, _) = tokio::time::timeout(within, listener.accept())
                .await
                .unwrap()
                .unwrap();
            let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
            socket.send(hello()).await.unwrap();
            let resumed = loop {
                let m = read(socket.next().await.unwrap().unwrap());
                if m["op"] != 1 {
                    break m;
                }
            };
            assert_eq!(resumed["op"], 6, "{resumed}");
            assert_eq!(resumed["d"]["session_id"], "S1");
            assert_eq!(resumed["d"]["seq"], 1);
            rig.cx.life.borrow_mut().stopping = true;
            let _ = socket.send(Ws::text(json!({ "op": 11 }).to_string())).await;
            let _ = socket.close(None).await;
            tokio::time::timeout(within, gateway)
                .await
                .unwrap()
                .unwrap();
        });
        let log = crate::util::read_text(rig.cx.paths.log_file()).unwrap_or_default();
        assert!(log.contains("Discord: ready"), "{log}");
        assert!(log.contains("closed (4000)"), "{log}");
    }
}
