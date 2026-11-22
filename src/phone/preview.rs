//! `switchboard phone preview`: the phone app served for developing it,
//! from the clone's `assets/phone/` on 127.0.0.1 alone, over made-up tabs
//! (`demo::phone`), framed at a phone's size, and reloaded as its files
//! are saved. New with the Rust port's phone app; nothing in the tmux
//! version served a page.
//!
//! It is the real app's routes (`web`) with none of what guards them, since
//! nothing here is anyone's: no Tailscale, pairing, cookie, push or
//! phone.json, and nothing read from the state directory. Every route the
//! app calls answers from the made-up data in memory, and what the app
//! does changes only that (an answer clears its question, a message adds a
//! turn, a tab working replies a moment later); no tab, server or service
//! is reached. The stream (`/api/events`) sends the tabs as the real one
//! does, `live` false while the frame's "Server goes offline" holds.
//!
//! The frame (`/preview`, `preview/frame.*`) shows the app in an iframe at
//! a phone's size, with a keyboard that shortens it, and buttons that
//! change the made-up data. A browser reports a desktop's safe-area insets
//! as 0, so the app's CSS is served with each `env(safe-area-inset-*)` as
//! a variable the frame sets on the app's page. The files are looked at
//! every `LOOK` and the frame told on `/preview/reload` when one changes,
//! which reloads the app on the same tab (its `#<pane>`).

use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result};
use axum::Router;
use axum::extract::{DefaultBodyLimit, Path as UrlPath, Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use serde_json::{Value, json};
use tokio::sync::{broadcast, watch};

use super::images;
use crate::demo::phone as made_up;
use crate::mission::remind;

/// The port tried first, so a frame left open finds the next run.
pub const PORT: u16 = 4174;
/// How often the app's files are looked at.
const LOOK: Duration = Duration::from_millis(200);
/// How long a made-up Claude takes to reply to an answer or a message.
const REPLY_AFTER: Duration = Duration::from_secs(3);

/// The real server's policy, but that the frame, from the same origin, may
/// show the app.
const CSP: &str = "default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data: blob:; connect-src 'self'; manifest-src 'self'; worker-src 'self'; frame-ancestors 'self'; base-uri 'none'; form-action 'none'";

/// The app's files served, with their types; `sw.js` is left out, so no
/// service worker of the preview's keeps a copy in the browser.
const FILES: &[(&str, &str)] = &[
    ("index.html", "text/html; charset=utf-8"),
    ("app.js", "text/javascript; charset=utf-8"),
    ("app.css", "text/css; charset=utf-8"),
    ("manifest.webmanifest", "application/manifest+json"),
    ("icon.svg", "image/svg+xml"),
    ("icon-180.png", "image/png"),
    ("icon-192.png", "image/png"),
    ("icon-512.png", "image/png"),
];

/// The phone app's files in the Switchboard clone `dir` is in, or why
/// there are none. The app's code runs in your browser, so a checkout
/// holding someone else's code is refused as `switchboard try` refuses it:
/// a PR `wtpr --untrusted` set up always, another unless it is yours or in
/// trusted-dirs.
pub fn assets_dir(dir: &Path) -> Result<PathBuf, String> {
    let top = dir.ancestors().find(|d| d.join(".git").exists());
    let Some(top) = top.filter(|_| crate::dev::is_switchboard_clone(dir)) else {
        return Err(
            "the phone preview is for Switchboard's clone: run it in a checkout of Switchboard"
                .into(),
        );
    };
    if crate::trust::is_untrusted(top) {
        return Err(format!(
            "{} holds a PR wtpr --untrusted set up, whose code runs only sealed",
            top.display()
        ));
    }
    crate::trust::yours_or_listed(&crate::paths::home(), top)?;
    Ok(top.join("assets/phone"))
}

/// The made-up workspace.
struct Data {
    tabs: Vec<Value>,
    turns: HashMap<String, Vec<Value>>,
    /// The images the app sent, by the name answered.
    images: HashMap<String, (Vec<u8>, &'static str)>,
    live: bool,
    /// How many questions and images were made, for their names.
    made: u64,
    /// How many times the tabs were reset, so a reply due from before a
    /// reset changes nothing after it.
    resets: u64,
}

impl Data {
    fn new() -> Data {
        let now = crate::util::now_ms();
        Data {
            tabs: made_up::tabs(now),
            turns: made_up::turns(now).into_iter().collect(),
            images: HashMap::new(),
            live: true,
            made: 0,
            resets: 0,
        }
    }

    /// The tab whose pane's number is `n`.
    fn tab(&mut self, n: &str) -> Option<&mut Value> {
        let pane = format!("%{n}");
        self.tabs.iter_mut().find(|t| t["pane"] == pane.as_str())
    }

    /// A turn added to a tab's channel, and its last turn set to it, which
    /// is what makes the app read the channel again.
    fn add_turn(&mut self, n: &str, who: &str, text: &str, images: Vec<String>) {
        let at = crate::util::now_ms();
        let pane = format!("%{n}");
        self.turns
            .entry(pane)
            .or_default()
            .push(json!({"who": who, "text": text, "images": images, "at": at}));
        if let Some(tab) = self.tab(n) {
            let prompt = if who == "you" {
                json!(text)
            } else {
                tab["turn"]["prompt"].clone()
            };
            let reply = if who == "claude" {
                json!(text)
            } else {
                Value::Null
            };
            tab["turn"] = json!({"prompt": prompt, "reply": reply, "at": at});
        }
    }

    /// A tab set to `state`, from now.
    fn set_state(&mut self, n: &str, state: &str, wait: Option<&str>) {
        if let Some(tab) = self.tab(n) {
            tab["state"] = json!(state);
            tab["wait"] = json!(wait);
            tab["since"] = json!(crate::util::now_ms() / 1000);
            if state != "waiting" {
                tab["prompt"] = Value::Null;
            }
        }
        self.sort();
    }

    /// Needs you first, the pinned leading it, as the server sorts them.
    fn sort(&mut self) {
        for tab in &mut self.tabs {
            let state = tab["state"].as_str().unwrap_or("");
            tab["needs"] = json!(tab["pinned"] == true || state == "waiting" || state == "done");
        }
        self.tabs
            .sort_by_key(|t| (t["needs"] != true, t["pinned"] != true));
    }

    /// What the stream sends.
    fn snapshot(&self) -> Value {
        json!({
            "tabs": self.tabs,
            "live": self.live,
            "close": true,
            "reminders": true,
            "remind_times": super::serve::remind_times(&crate::util::local_now()),
        })
    }
}

pub struct Preview {
    dir: PathBuf,
    /// The port served on, which a request's Host must name.
    port: u16,
    data: Mutex<Data>,
    tabs: watch::Sender<Value>,
    reload: broadcast::Sender<String>,
}

impl Preview {
    fn data(&self) -> std::sync::MutexGuard<'_, Data> {
        self.data.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// `change` made to the data, and the stream sent what it is now.
    fn change<T>(&self, change: impl FnOnce(&mut Data) -> T) -> T {
        let mut data = self.data();
        let out = change(&mut data);
        self.tabs.send_replace(data.snapshot());
        out
    }

    /// A made-up Claude's reply to what tab `n` was given, a moment later,
    /// if it still works on it.
    fn reply_later(self: &Arc<Self>, n: String) {
        let preview = self.clone();
        let resets = self.data().resets;
        tokio::spawn(async move {
            tokio::time::sleep(REPLY_AFTER).await;
            preview.change(|d| d.resets == resets && reply(d, &n));
        });
    }
}

/// Tab `n`'s Claude finishing its turn, if it is working.
fn reply(data: &mut Data, n: &str) -> bool {
    let working = data
        .tab(n)
        .is_some_and(|t| t["state"] == "working" || t["state"] == "background");
    if working {
        data.add_turn(n, "claude", made_up::REPLY, Vec::new());
        data.set_state(n, "done", None);
    }
    working
}

/// Binds 127.0.0.1 alone: `port` when given, else `PORT` or, taken, any.
pub async fn bind(port: Option<u16>) -> Result<tokio::net::TcpListener> {
    let at = |port| SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    match port {
        Some(port) => tokio::net::TcpListener::bind(at(port))
            .await
            .with_context(|| format!("listening on {}", at(port))),
        None => match tokio::net::TcpListener::bind(at(PORT)).await {
            Ok(listener) => Ok(listener),
            Err(_) => tokio::net::TcpListener::bind(at(0))
                .await
                .context("listening on 127.0.0.1"),
        },
    }
}

/// Serves the app in `dir` on `listener` until the process is stopped.
pub async fn serve(listener: tokio::net::TcpListener, dir: PathBuf) -> Result<()> {
    let data = Data::new();
    let (tabs, _) = watch::channel(data.snapshot());
    let (reload, _) = broadcast::channel(16);
    let port = listener.local_addr()?.port();
    let preview = Arc::new(Preview {
        dir,
        port,
        data: Mutex::new(data),
        tabs,
        reload,
    });
    tokio::spawn(look_at_files(preview.clone()));
    axum::serve(listener, routes(preview)).await?;
    Ok(())
}

/// Each file served's name, size and modified time: only those, so an
/// editor's swap or autosave file beside them reloads nothing.
fn files_now(dir: &Path) -> Vec<(&'static str, u64, Option<SystemTime>)> {
    FILES
        .iter()
        .map(|(name, _)| match std::fs::metadata(dir.join(name)) {
            Ok(meta) => (*name, meta.len(), meta.modified().ok()),
            Err(_) => (*name, 0, None),
        })
        .collect()
}

/// The frame told the name of each file served that is saved, added or
/// removed.
async fn look_at_files(preview: Arc<Preview>) {
    let mut before = files_now(&preview.dir);
    loop {
        tokio::time::sleep(LOOK).await;
        let now = files_now(&preview.dir);
        if now != before {
            let changed = now
                .iter()
                .find(|f| !before.contains(f))
                .or_else(|| before.iter().find(|f| !now.contains(f)))
                .map(|f| f.0.to_string())
                .unwrap_or_default();
            let _ = preview.reload.send(changed);
            before = now;
        }
    }
}

fn routes(preview: Arc<Preview>) -> Router {
    Router::new()
        .route("/", get(|s| app_file(s, "index.html")))
        .route("/{file}", get(named_file))
        .route("/preview", get(|| frame("frame.html")))
        .route("/preview/frame.js", get(|| frame("frame.js")))
        .route("/preview/frame.css", get(|| frame("frame.css")))
        .route("/preview/reload", get(reloads))
        .route("/preview/{what}", post(control))
        .route("/api/me", get(me))
        .route("/api/tabs", get(tabs))
        .route("/api/events", get(events))
        .route("/api/tabs/{pane}/turns", get(turns_of))
        .route("/api/tabs/{pane}/answer", post(answer))
        .route("/api/tabs/{pane}/message", post(message))
        .route(
            "/api/tabs/{pane}/image",
            post(image).layer(DefaultBodyLimit::max(images::MAX_BYTES)),
        )
        .route("/api/images/{name}", get(image_of))
        .route("/api/tabs/{pane}/seen", post(seen))
        .route("/api/tabs/{pane}/close", post(close))
        .route("/api/tabs/{pane}/remind", post(remind))
        .route("/api/tabs/{pane}/pin", post(pin))
        .fallback(|| async { (StatusCode::NOT_FOUND, "Not here") })
        .layer(middleware::from_fn_with_state(
            preview.clone(),
            loopback_host_only,
        ))
        .layer(middleware::from_fn(headers))
        .with_state(preview)
}

/// Whether a request's Host names this preview on the loopback address,
/// so a page whose own name was pointed at 127.0.0.1 (DNS rebinding)
/// can't read what the app sent, such as an image of yours.
pub fn loopback_host(host: Option<&str>, port: u16) -> bool {
    host.is_some_and(|h| {
        [format!("127.0.0.1:{port}"), format!("localhost:{port}")]
            .iter()
            .any(|ok| h.eq_ignore_ascii_case(ok))
    })
}

async fn loopback_host_only(
    State(preview): State<Arc<Preview>>,
    req: Request,
    next: Next,
) -> Response {
    let host = req
        .headers()
        .get(header::HOST)
        .and_then(|h| h.to_str().ok());
    if !loopback_host(host, preview.port) {
        return (StatusCode::FORBIDDEN, "Not for this address").into_response();
    }
    next.run(req).await
}

/// The real server's headers, never kept, so a reload reads the files
/// again.
async fn headers(req: Request, next: Next) -> Response {
    let mut res = next.run(req).await;
    let h = res.headers_mut();
    h.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(CSP),
    );
    h.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    h.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    res
}

/// The app's CSS with the safe area's insets as variables the frame sets.
pub fn with_inset_variables(css: &str) -> String {
    ["top", "right", "bottom", "left"]
        .iter()
        .fold(css.to_string(), |css, side| {
            css.replace(
                &format!("env(safe-area-inset-{side})"),
                &format!("var(--safe-area-inset-{side}, 0px)"),
            )
        })
}

async fn named_file(state: State<Arc<Preview>>, UrlPath(name): UrlPath<String>) -> Response {
    match FILES.iter().find(|(n, _)| *n == name) {
        Some((name, _)) => app_file(state, name).await,
        None => (StatusCode::NOT_FOUND, "Not here").into_response(),
    }
}

/// One of the app's files, read from the clone now.
async fn app_file(State(preview): State<Arc<Preview>>, name: &'static str) -> Response {
    let kind = FILES
        .iter()
        .find(|(n, _)| *n == name)
        .map_or("text/plain", |(_, k)| k);
    match tokio::fs::read(preview.dir.join(name)).await {
        Ok(bytes) if name == "app.css" => (
            [(header::CONTENT_TYPE, kind)],
            with_inset_variables(&String::from_utf8_lossy(&bytes)),
        )
            .into_response(),
        Ok(bytes) => ([(header::CONTENT_TYPE, kind)], bytes).into_response(),
        Err(_) => (StatusCode::NOT_FOUND, "Not here").into_response(),
    }
}

async fn frame(name: &'static str) -> Response {
    let (text, kind) = match name {
        "frame.html" => (
            include_str!("preview/frame.html"),
            "text/html; charset=utf-8",
        ),
        "frame.js" => (
            include_str!("preview/frame.js"),
            "text/javascript; charset=utf-8",
        ),
        _ => (include_str!("preview/frame.css"), "text/css; charset=utf-8"),
    };
    ([(header::CONTENT_TYPE, kind)], text).into_response()
}

/// The name of each file saved, as server-sent events.
async fn reloads(State(preview): State<Arc<Preview>>) -> Response {
    let rx = preview.reload.subscribe();
    let stream = futures_util::stream::unfold(rx, |mut rx| async move {
        loop {
            match rx.recv().await {
                Ok(name) => {
                    return Some((
                        Ok::<_, std::convert::Infallible>(SseEvent::default().data(name)),
                        rx,
                    ));
                }
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => return None,
            }
        }
    });
    Sse::new(stream)
        .keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
        .into_response()
}

fn said(words: impl Into<String>) -> Response {
    axum::Json(json!({"said": words.into()})).into_response()
}

fn refused(status: StatusCode, why: &str) -> Response {
    (status, axum::Json(json!({"error": why}))).into_response()
}

/// What the real server answers while Switchboard's can't be reached.
const OFFLINE: (StatusCode, &str) = (StatusCode::SERVICE_UNAVAILABLE, "Switchboard isn't running");
const NO_TAB: (StatusCode, &str) = (StatusCode::NOT_FOUND, "no such Claude tab");

fn offline() -> Response {
    refused(OFFLINE.0, OFFLINE.1)
}

fn no_tab() -> Response {
    refused(NO_TAB.0, NO_TAB.1)
}

/// The frame's buttons: a tab starts waiting, a reply arrives, the server
/// goes offline and comes back, the data starts again.
async fn control(State(preview): State<Arc<Preview>>, UrlPath(what): UrlPath<String>) -> Response {
    match what.as_str() {
        "wait" => preview.change(|d| {
            let idle = d
                .tabs
                .iter()
                .find(|t| t["state"] != "waiting" && t["state"] != "done")
                .and_then(|t| t["pane"].as_str())
                .map(|p| p.trim_start_matches('%').to_string());
            let Some(n) = idle else {
                return refused(StatusCode::CONFLICT, "every tab already needs you");
            };
            d.made += 1;
            let prompt = made_up::new_prompt(d.made);
            d.set_state(&n, "waiting", Some("AskUserQuestion"));
            let tab = d.tab(&n).expect("the tab just set");
            tab["prompt"] = prompt;
            said(format!("Tab {} started waiting", tab["index"]))
        }),
        "reply" => preview.change(|d| {
            let working = d
                .tabs
                .iter()
                .find(|t| t["state"] == "working" || t["state"] == "background")
                .and_then(|t| t["pane"].as_str())
                .map(|p| p.trim_start_matches('%').to_string());
            match working {
                Some(n) => {
                    reply(d, &n);
                    let index = d.tab(&n).map(|t| t["index"].clone());
                    said(format!("Tab {} replied", index.unwrap_or_default()))
                }
                None => refused(
                    StatusCode::CONFLICT,
                    "no tab is working: answer or message one first",
                ),
            }
        }),
        "offline" => preview.change(|d| {
            d.live = false;
            said("The server is offline")
        }),
        "online" => preview.change(|d| {
            d.live = true;
            said("The server is back")
        }),
        "reset" => preview.change(|d| {
            let resets = d.resets + 1;
            *d = Data::new();
            d.resets = resets;
            said("The made-up tabs are as they started")
        }),
        _ => (StatusCode::NOT_FOUND, "Not here").into_response(),
    }
}

async fn me() -> Response {
    axum::Json(json!({
        "paired": true,
        "revoked": false,
        "device": "preview",
        "computer": "preview",
    }))
    .into_response()
}

async fn tabs(State(preview): State<Arc<Preview>>) -> Response {
    axum::Json(preview.data().snapshot()).into_response()
}

/// The tabs, now and each time they change, as the real stream sends them.
async fn events(State(preview): State<Arc<Preview>>) -> Response {
    let rx = preview.tabs.subscribe();
    let stream = futures_util::stream::unfold((rx, true), |(mut rx, first)| async move {
        if !first && rx.changed().await.is_err() {
            return None;
        }
        let data = rx.borrow_and_update().to_string();
        Some((
            Ok::<_, std::convert::Infallible>(SseEvent::default().data(data)),
            (rx, false),
        ))
    });
    Sse::new(stream)
        .keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
        .into_response()
}

async fn turns_of(State(preview): State<Arc<Preview>>, UrlPath(n): UrlPath<String>) -> Response {
    let mut data = preview.data();
    if data.tab(&n).is_none() {
        return no_tab();
    }
    let messages = data
        .turns
        .get(&format!("%{n}"))
        .cloned()
        .unwrap_or_default();
    axum::Json(json!({"messages": messages})).into_response()
}

fn text_of(body: &Value, key: &str) -> String {
    body[key].as_str().unwrap_or("").to_string()
}

/// An option pressed, the ticked ones submitted, or words of your own, on
/// the question the app showed; as the real server, one no longer showing
/// is refused.
async fn answer(
    State(preview): State<Arc<Preview>>,
    UrlPath(n): UrlPath<String>,
    axum::Json(body): axum::Json<Value>,
) -> Response {
    let reply = text_of(&body, "reply");
    let id = text_of(&body, "prompt");
    let answered = preview.change(|d| {
        if !d.live {
            return Err(OFFLINE);
        }
        let Some(tab) = d.tab(&n) else {
            return Err(NO_TAB);
        };
        let prompt = &mut tab["prompt"];
        if prompt.is_null() || prompt["id"] != id.as_str() {
            return Err((
                StatusCode::CONFLICT,
                "the question it answers is no longer showing",
            ));
        }
        let free = prompt["free"].as_u64();
        let multi = prompt["multi"] == true;
        let chosen = reply.parse::<u64>().ok();
        let options = prompt["options"]
            .as_array_mut()
            .map(Vec::as_mut_slice)
            .unwrap_or_default();
        let option = options
            .iter_mut()
            .find(|o| chosen.is_some() && o["n"].as_u64() == chosen);
        let words = match option {
            Some(o) if multi && chosen != free => {
                let ticked = o["checked"] != true;
                o["checked"] = json!(ticked);
                let verb = if ticked { "Ticked" } else { "Unticked" };
                return Ok((
                    format!("{verb} {}", o["label"].as_str().unwrap_or("")),
                    false,
                ));
            }
            Some(_) if chosen == free => {
                return Err((StatusCode::BAD_REQUEST, "type your own answer"));
            }
            Some(o) => o["label"].as_str().unwrap_or("").to_string(),
            None if reply == "submit" && multi => options
                .iter()
                .filter(|o| o["checked"] == true)
                .filter_map(|o| o["label"].as_str())
                .collect::<Vec<_>>()
                .join(", "),
            None if free.is_some() && chosen.is_none() && !reply.trim().is_empty() => {
                reply.trim().to_string()
            }
            None => return Err((StatusCode::BAD_REQUEST, "no such option")),
        };
        d.add_turn(&n, "you", &words, Vec::new());
        d.set_state(&n, "working", Some("Carrying on after your answer"));
        Ok((format!("Answered \"{words}\""), true))
    });
    match answered {
        Ok((words, answered)) => {
            if answered {
                preview.reply_later(n);
            }
            said(words)
        }
        Err((status, why)) => refused(status, why),
    }
}

/// A message to a tab's Claude: a turn of yours, and its reply a moment
/// later.
async fn message(
    State(preview): State<Arc<Preview>>,
    UrlPath(n): UrlPath<String>,
    axum::Json(body): axum::Json<Value>,
) -> Response {
    let text = text_of(&body, "text");
    let names: Vec<String> = body["images"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|n| n.as_str().unwrap_or("").to_string())
        .collect();
    let sent = preview.change(|d| {
        if !d.live {
            return Err(OFFLINE);
        }
        let Some(tab) = d.tab(&n) else {
            return Err(NO_TAB);
        };
        if tab["state"] == "waiting" && !tab["prompt"].is_null() {
            return Err((
                StatusCode::CONFLICT,
                "its Claude is showing a prompt: answer that first",
            ));
        }
        if text.trim().is_empty() && names.is_empty() {
            return Err((StatusCode::BAD_REQUEST, "nothing to send"));
        }
        if names.iter().any(|name| !d.images.contains_key(name)) {
            return Err((
                StatusCode::BAD_REQUEST,
                "an image sent isn't on the computer",
            ));
        }
        d.add_turn(&n, "you", text.trim(), names.clone());
        d.set_state(&n, "working", Some("Reading your message"));
        Ok(())
    });
    match sent {
        Ok(()) => {
            preview.reply_later(n);
            said(format!("Sent \"{}\"", text.trim()))
        }
        Err((status, why)) => refused(status, why),
    }
}

/// An image the app sent, kept in memory under a name made here.
async fn image(
    State(preview): State<Arc<Preview>>,
    UrlPath(n): UrlPath<String>,
    body: axum::body::Bytes,
) -> Response {
    let Some(ext) = images::kind(&body) else {
        return refused(
            StatusCode::BAD_REQUEST,
            "only a PNG, JPEG, GIF or WebP is taken",
        );
    };
    let mut data = preview.data();
    if !data.live {
        return offline();
    }
    if data.tab(&n).is_none() {
        return no_tab();
    }
    data.made += 1;
    let name = format!("preview-{}.{ext}", data.made);
    let kind = images::content_type(&name);
    data.images.insert(name.clone(), (body.to_vec(), kind));
    axum::Json(json!({"id": name})).into_response()
}

async fn image_of(State(preview): State<Arc<Preview>>, UrlPath(name): UrlPath<String>) -> Response {
    if let Some((bytes, kind)) = preview.data().images.get(&name) {
        return ([(header::CONTENT_TYPE, *kind)], bytes.clone()).into_response();
    }
    match made_up::image(&name) {
        Some(svg) => ([(header::CONTENT_TYPE, "image/svg+xml")], svg).into_response(),
        None => (StatusCode::NOT_FOUND, "Not here").into_response(),
    }
}

async fn seen(State(preview): State<Arc<Preview>>, UrlPath(n): UrlPath<String>) -> Response {
    preview.change(|d| {
        if !d.live {
            return offline();
        }
        let Some(tab) = d.tab(&n) else {
            return no_tab();
        };
        if tab["state"] == "done" {
            d.set_state(&n, "idle", None);
        }
        said("Marked seen")
    })
}

async fn close(State(preview): State<Arc<Preview>>, UrlPath(n): UrlPath<String>) -> Response {
    preview.change(|d| {
        if !d.live {
            return offline();
        }
        let Some(index) = d.tab(&n).map(|t| t["index"].clone()) else {
            return no_tab();
        };
        let pane = format!("%{n}");
        d.tabs.retain(|t| t["pane"] != pane.as_str());
        said(format!("Closed tab {index}"))
    })
}

/// A tab's reminder set for the time the sheet sent (read as the
/// workspace reads it, refused in its words when it can't be) with its
/// note, or cleared; due at once when the time has passed.
async fn remind(
    State(preview): State<Arc<Preview>>,
    UrlPath(n): UrlPath<String>,
    axum::Json(body): axum::Json<Value>,
) -> Response {
    let when = text_of(&body, "when");
    let note = text_of(&body, "note");
    let clear = body["clear"] == true;
    preview.change(|d| {
        if !d.live {
            return offline();
        }
        let Some(tab) = d.tab(&n) else {
            return no_tab();
        };
        let index = tab["index"].clone();
        if clear {
            tab["remind"] = Value::Null;
            return said(format!("Cleared the reminder for tab {index}"));
        }
        let now = crate::util::local_now();
        let Some(at) = remind::when(when.trim(), &now) else {
            return refused(
                StatusCode::CONFLICT,
                &format!("\"{}\" {}", when.trim(), remind::NOT_A_TIME),
            );
        };
        let due = at <= now.timestamp().as_second();
        tab["remind"] = json!({"at": at, "note": note.trim(), "due": due});
        said(format!("Reminder for tab {index} at {}", remind::said(at)))
    })
}

/// A tab pinned, or, with `pin` false, unpinned.
async fn pin(
    State(preview): State<Arc<Preview>>,
    UrlPath(n): UrlPath<String>,
    axum::Json(body): axum::Json<Value>,
) -> Response {
    let pin = body["pin"] == true;
    preview.change(|d| {
        if !d.live {
            return offline();
        }
        let Some(tab) = d.tab(&n) else {
            return no_tab();
        };
        let index = tab["index"].clone();
        tab["pinned"] = json!(pin);
        d.sort();
        said(format!(
            "{} tab {index}",
            if pin { "Pinned" } else { "Unpinned" }
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_safe_area_inset_becomes_a_variable() {
        let css = with_inset_variables(
            "a { padding: env(safe-area-inset-top) 0 env(safe-area-inset-bottom); top: calc(12px + env(safe-area-inset-top)); }",
        );
        assert!(!css.contains("env("), "{css}");
        assert_eq!(css.matches("var(--safe-area-inset-top, 0px)").count(), 2);
        assert!(css.contains("var(--safe-area-inset-bottom, 0px)"));
    }

    #[test]
    fn outside_switchboards_clone_it_says_what_it_is_for() {
        let dir = std::env::temp_dir();
        let why = assets_dir(&dir).unwrap_err();
        assert!(why.contains("Switchboard's clone"), "{why}");
    }

    #[test]
    fn the_app_and_its_icons_are_served_but_not_its_service_worker() {
        let names: Vec<&str> = FILES.iter().map(|(n, _)| *n).collect();
        for name in ["index.html", "app.js", "app.css", "icon-180.png"] {
            assert!(names.contains(&name), "{name}");
        }
        assert!(!names.contains(&"sw.js"));
    }

    #[test]
    fn only_a_loopback_host_on_its_port_is_answered() {
        assert!(loopback_host(Some("127.0.0.1:4174"), 4174));
        assert!(loopback_host(Some("localhost:4174"), 4174));
        assert!(!loopback_host(Some("evil.example:4174"), 4174));
        assert!(!loopback_host(Some("127.0.0.1:4175"), 4174));
        assert!(!loopback_host(None, 4174));
    }

    #[test]
    fn a_working_tab_replies_and_finishes() {
        let mut data = Data::new();
        let n = "16";
        assert_eq!(data.tab(n).unwrap()["state"], "working");
        assert!(reply(&mut data, n));
        let tab = data.tab(n).unwrap();
        assert_eq!(tab["state"], "done");
        assert_eq!(tab["needs"], true);
        assert_eq!(tab["turn"]["reply"], made_up::REPLY);
        assert!(!reply(&mut data, n), "a finished tab replies no more");
    }
}
