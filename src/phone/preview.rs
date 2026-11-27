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
use axum::http::{HeaderValue, StatusCode, Uri, header};
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
    ("markdown-it.min.js", "text/javascript; charset=utf-8"),
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
    /// Mission control's views, as the panel lists them.
    views: Vec<Value>,
    turns: HashMap<String, Vec<Value>>,
    /// The images the app sent, by the name answered.
    images: HashMap<String, (Vec<u8>, &'static str)>,
    live: bool,
    /// How many questions and images were made, for their names.
    made: u64,
    /// How many times the tabs were reset, so a reply due from before a
    /// reset changes nothing after it.
    resets: u64,
    /// The two-question tab's step shown and the option chosen for each
    /// question so far (`made_up::theme_prompt`).
    theme: (usize, [Option<u64>; 2]),
}

impl Data {
    fn new() -> Data {
        let now = crate::util::now_ms();
        Data {
            tabs: made_up::tabs(now),
            views: made_up::views(now),
            turns: made_up::turns(now).into_iter().collect(),
            images: HashMap::new(),
            live: true,
            made: 0,
            resets: 0,
            theme: (0, [None, None]),
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

    /// An answer or a step on the two-question tab, as Claude Code takes
    /// them: an option answers the question shown and moves on to the next
    /// step, a step moves as ← and → do, and on Submit's step Submit
    /// answers sends every answer while Cancel turns the questions down.
    fn theme_answer(
        &mut self,
        n: &str,
        reply: &str,
        step: &str,
    ) -> Result<(String, bool), (StatusCode, &'static str)> {
        let names: Vec<&str> = made_up::THEME_QUESTIONS
            .iter()
            .map(|(name, ..)| *name)
            .chain(["Submit"])
            .collect();
        let (mut shown, mut answers) = self.theme;
        let submit = names.len() - 1;
        let said = if !step.is_empty() {
            shown = match step {
                "back" if shown > 0 => shown - 1,
                "forward" if shown < submit => shown + 1,
                "back" | "forward" => return Err((StatusCode::CONFLICT, "no step that way")),
                name => names
                    .iter()
                    .position(|s| *s == name)
                    .ok_or((StatusCode::BAD_REQUEST, "no such step"))?,
            };
            format!("Stepped to {}", names[shown])
        } else if shown == submit {
            let words = match reply {
                "1" => made_up::THEME_QUESTIONS
                    .iter()
                    .zip(answers)
                    .filter_map(|((_, _, labels), a)| {
                        labels.get(usize::try_from(a? - 1).ok()?).copied()
                    })
                    .collect::<Vec<_>>()
                    .join(", "),
                "2" => "Not now".to_string(),
                _ => return Err((StatusCode::BAD_REQUEST, "no such option")),
            };
            self.theme = (0, [None, None]);
            self.add_turn(n, "you", &words, Vec::new());
            self.set_state(n, "working", Some("Carrying on after your answer"));
            return Ok((format!("Answered \"{words}\""), true));
        } else {
            let labels = made_up::THEME_QUESTIONS[shown].2;
            let chosen = reply
                .parse::<u64>()
                .ok()
                .filter(|&c| c >= 1 && c <= labels.len() as u64)
                .ok_or((
                    StatusCode::BAD_REQUEST,
                    "the preview takes one of its options here",
                ))?;
            answers[shown] = Some(chosen);
            shown += 1;
            format!("Chose {chosen}. {}", labels[chosen as usize - 1])
        };
        self.theme = (shown, answers);
        if let Some(tab) = self.tab(n) {
            tab["prompt"] = made_up::theme_prompt(shown, answers);
        }
        Ok((said, false))
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

    /// In the sidebar's groups and order, each placed, marked and worded
    /// again, as the server sends them.
    fn sort(&mut self) {
        made_up::restage(&mut self.tabs, crate::util::now_ms());
    }

    /// What the stream sends.
    fn snapshot(&self) -> Value {
        json!({
            "tabs": self.tabs,
            "views": self.views,
            "live": self.live,
            "close": true,
            "new_tab": true,
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
        .route("/api/tabs/{pane}/stop", post(stop))
        .route("/api/tabs/{pane}/pr", get(|s, p| tab_view(s, p, "pr")))
        .route(
            "/api/tabs/{pane}/ticket",
            get(|s, p| tab_view(s, p, "ticket")),
        )
        .route("/api/tabs/{pane}/note", get(|s, p| tab_view(s, p, "note")))
        .route("/api/places", get(places_list).post(open_place))
        .route("/api/views/{view}/item", post(view_item))
        .route("/api/views/{view}/act", post(view_act))
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
        // Stamped with the preview's own build, as the phone server stamps it.
        Ok(bytes) if name == "app.js" => (
            [(header::CONTENT_TYPE, kind)],
            super::build::stamped(&String::from_utf8_lossy(&bytes), &build_of(&preview.dir)),
        )
            .into_response(),
        Ok(bytes) => ([(header::CONTENT_TYPE, kind)], bytes).into_response(),
        Err(_) => (StatusCode::NOT_FOUND, "Not here").into_response(),
    }
}

/// The preview's own build, for the clone whose `assets/phone` is `dir`.
fn build_of(dir: &Path) -> String {
    super::build::of(dir.parent().and_then(Path::parent))
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

async fn me(State(preview): State<Arc<Preview>>) -> Response {
    axum::Json(json!({
        "paired": true,
        "revoked": false,
        "device": "preview",
        "computer": "preview",
        "build": build_of(&preview.dir),
    }))
    .into_response()
}

async fn tabs(State(preview): State<Arc<Preview>>) -> Response {
    axum::Json(preview.data().snapshot()).into_response()
}

/// The tabs, now and each time they change, as the real stream sends them.
async fn events(State(preview): State<Arc<Preview>>) -> Response {
    let rx = preview.tabs.subscribe();
    // With the preview's build, as the real stream sends the server's, so
    // a commit in the clone updates the app in the frame.
    let dir = preview.dir.clone();
    let build_now = move || build_of(&dir);
    let stream = futures_util::stream::unfold((rx, None::<String>), move |(mut rx, sent)| {
        let build_now = build_now.clone();
        async move {
            let build = match sent {
                None => tokio::task::spawn_blocking(build_now).await.ok()?,
                Some(sent) => super::build::next_on_stream(&mut rx, &sent, build_now).await?,
            };
            let data = super::build::with_build(rx.borrow_and_update().clone(), &build);
            Some((
                Ok::<_, std::convert::Infallible>(SseEvent::default().data(data)),
                (rx, Some(build)),
            ))
        }
    });
    Sse::new(stream)
        .keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
        .into_response()
}

/// A channel's turns as the real route reads them from a transcript
/// (`turns::window`), with each message's place in its made-up list as its
/// offset.
async fn turns_of(
    State(preview): State<Arc<Preview>>,
    UrlPath(n): UrlPath<String>,
    uri: Uri,
) -> Response {
    let mut data = preview.data();
    if data.tab(&n).is_none() {
        return no_tab();
    }
    let all = data
        .turns
        .get(&format!("%{n}"))
        .cloned()
        .unwrap_or_default();
    let (messages, from) = window(&all, super::web::query_number(&uri, "before"));
    axum::Json(json!({
        "messages": messages,
        "from": from,
        "earlier": from > 0,
        "conversation": "",
    }))
    .into_response()
}

/// The last `turns::TURNS` turns of `all` before the place `before`, each
/// message given its place as its offset, and the place they start at.
fn window(all: &[Value], before: Option<u64>) -> (Vec<Value>, u64) {
    let end = before.map_or(all.len(), |b| (b as usize).min(all.len()));
    let starts: Vec<usize> = (0..end)
        .filter(|&i| {
            let m = &all[i];
            matches!(m["who"].as_str(), Some("you" | "task"))
                || (m["who"] == "tab" && m["queued"] != true)
        })
        .collect();
    let from = starts
        .len()
        .checked_sub(super::turns::TURNS)
        .map_or(0, |i| starts[i]);
    let messages = (from..end)
        .map(|i| {
            let mut m = all[i].clone();
            m["offset"] = json!(i);
            m
        })
        .collect();
    (messages, from as u64)
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
    let step = text_of(&body, "step");
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
        if prompt["steps"].as_array().is_some_and(|s| !s.is_empty()) {
            return d.theme_answer(&n, &reply, &step);
        }
        if !step.is_empty() {
            return Err((StatusCode::CONFLICT, "its prompt has no steps"));
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

/// A made-up tab's PR, ticket or note, as `phone-tab` answers it.
async fn tab_view(
    State(preview): State<Arc<Preview>>,
    UrlPath(n): UrlPath<String>,
    kind: &'static str,
) -> Response {
    let mut d = preview.data();
    if !d.live {
        return offline();
    }
    if d.tab(&n).is_none() {
        return no_tab();
    }
    match made_up::tab_view(&n, kind) {
        Some(v) => axum::Json(v).into_response(),
        None => refused(StatusCode::CONFLICT, "This tab hasn't one."),
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

/// A working tab's turn stopped, the tab going idle; refused for one that
/// isn't working, in the server's words.
async fn stop(State(preview): State<Arc<Preview>>, UrlPath(n): UrlPath<String>) -> Response {
    preview.change(|d| {
        if !d.live {
            return offline();
        }
        let Some(tab) = d.tab(&n) else {
            return no_tab();
        };
        if tab["state"] != "working" {
            return refused(StatusCode::CONFLICT, "its Claude isn't working");
        }
        let index = tab["index"].clone();
        d.set_state(&n, "idle", None);
        said(format!("Stopped tab {index}"))
    })
}

/// The made-up places, as the real app's sheet lists them.
async fn places_list(State(preview): State<Arc<Preview>>) -> Response {
    if !preview.data().live {
        return offline();
    }
    axum::Json(super::web::places_json(&made_up::places())).into_response()
}

/// A made-up tab opened in a place listed, working on the prompt given, as
/// the computer would open one; a place not listed refused as it is there.
async fn open_place(
    State(preview): State<Arc<Preview>>,
    axum::Json(body): axum::Json<Value>,
) -> Response {
    let (place, prompt) = (text_of(&body, "place"), text_of(&body, "prompt"));
    let response = preview.change(|d| {
        if !d.live {
            return Err((OFFLINE.0, OFFLINE.1.to_string()));
        }
        let Some(group) = made_up::places()
            .into_iter()
            .find(|(_, choices)| choices.iter().any(|c| c.value == place))
            .map(|(group, _)| group)
        else {
            return Err((
                StatusCode::CONFLICT,
                format!("{place} isn't among the places to start a Claude in, so nothing started."),
            ));
        };
        let pane = d
            .tabs
            .iter()
            .filter_map(|t| {
                t["pane"]
                    .as_str()?
                    .trim_start_matches('%')
                    .parse::<u32>()
                    .ok()
            })
            .max()
            .unwrap_or(0)
            + 1;
        let index = d
            .tabs
            .iter()
            .filter_map(|t| t["index"].as_u64())
            .max()
            .unwrap_or(0) as usize
            + 1;
        let repo = (group != "Other directories").then_some(group.as_str());
        let now = crate::util::now_ms();
        d.tabs
            .push(made_up::opened_tab(pane, index, repo, &prompt, now));
        if !prompt.trim().is_empty() {
            d.turns.insert(
                format!("%{pane}"),
                vec![json!({"who": "you", "text": prompt.trim(), "images": [], "at": now})],
            );
        }
        d.sort();
        Ok(pane)
    });
    match response {
        Ok(pane) => {
            if !prompt.trim().is_empty() {
                preview.reply_later(pane.to_string());
            }
            axum::Json(
                json!({"pane": pane.to_string(), "said": format!("Started Claude in {place}")}),
            )
            .into_response()
        }
        Err((status, why)) => refused(status, &why),
    }
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
            d.sort();
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
        d.sort();
        said(format!("Reminder for tab {index} at {}", remind::said(at)))
    })
}

/// A tab pinned, or, with `pin` false, unpinned.
/// What the server says when an option names something that has gone.
const CHANGED: &str = "That has changed since the menu opened.";

/// An option of a made-up item's menu, by the item's ID and its label, as
/// the server refuses one it no longer offers or that the phone does
/// itself; one that takes the item away ("Take it out…", "Dismiss it")
/// takes it out of its view, and any other only says it ran.
async fn view_item(
    State(preview): State<Arc<Preview>>,
    UrlPath(view): UrlPath<String>,
    axum::Json(body): axum::Json<Value>,
) -> Response {
    let (id, label) = (text_of(&body, "id"), text_of(&body, "label"));
    preview.change(|d| {
        if !d.live {
            return offline();
        }
        let Some(v) = d.views.iter_mut().find(|v| v["key"] == view.as_str()) else {
            return refused(StatusCode::CONFLICT, CHANGED);
        };
        let groups = v["groups"]
            .as_array_mut()
            .map(Vec::as_mut_slice)
            .unwrap_or_default();
        for group in groups {
            let Some(items) = group["items"].as_array_mut() else {
                continue;
            };
            let Some(at) = items.iter().position(|i| i["id"] == id.as_str()) else {
                continue;
            };
            let option = items[at]["options"]
                .as_array()
                .and_then(|o| o.iter().find(|o| o["label"] == label.as_str()).cloned());
            let Some(option) = option else {
                return refused(StatusCode::CONFLICT, CHANGED);
            };
            if ["go", "open", "copy"]
                .iter()
                .any(|k| option.get(*k).is_some())
            {
                return refused(StatusCode::CONFLICT, "The phone does that one itself.");
            }
            if label.starts_with("Take it out") || label.starts_with("Dismiss") {
                items.remove(at);
            }
            return said(format!("{label}: done (made up)"));
        }
        refused(StatusCode::CONFLICT, CHANGED)
    })
}

/// An option of a made-up view's own menu, by its label.
async fn view_act(
    State(preview): State<Arc<Preview>>,
    UrlPath(view): UrlPath<String>,
    axum::Json(body): axum::Json<Value>,
) -> Response {
    let label = text_of(&body, "label");
    preview.change(|d| {
        if !d.live {
            return offline();
        }
        let offered = d
            .views
            .iter()
            .find(|v| v["key"] == view.as_str())
            .and_then(|v| v["acts"].as_array())
            .and_then(|a| a.iter().find(|o| o["label"] == label.as_str()));
        match offered {
            None => refused(StatusCode::CONFLICT, CHANGED),
            Some(o) if o.get("open").is_some() => {
                refused(StatusCode::CONFLICT, "The phone does that one itself.")
            }
            Some(_) => said(format!("{label}: done (made up)")),
        }
    })
}

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
    fn every_file_the_page_loads_is_served() {
        let names: Vec<&str> = FILES.iter().map(|(n, _)| *n).collect();
        let page = include_str!("../../assets/phone/index.html");
        let loaded: Vec<&str> = page
            .split(['"', '\''])
            .filter_map(|part| part.strip_prefix('/'))
            .filter(|path| !path.is_empty() && !path.contains(['/', ' ']))
            .collect();
        assert!(loaded.contains(&"markdown-it.min.js"), "{loaded:?}");
        for path in loaded {
            assert!(
                names.contains(&path),
                "{path} is loaded by index.html but not served"
            );
        }
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
    fn the_two_question_tab_steps_answers_and_reviews_as_claude_code_does() {
        let mut data = Data::new();
        let n = made_up::STEPPED.to_string();
        let shown = |d: &mut Data| d.tab(&n).unwrap()["prompt"]["shown"].clone();
        assert_eq!(shown(&mut data), 0);
        assert!(data.theme_answer(&n, "", "back").is_err());
        assert_eq!(data.theme_answer(&n, "2", "").unwrap().0, "Chose 2. Cool");
        assert_eq!(shown(&mut data), 1, "an answer moves on");
        data.theme_answer(&n, "", "Colors").unwrap();
        let prompt = data.tab(&n).unwrap()["prompt"].clone();
        assert_eq!(prompt["cursor"], 2, "the answer given");
        assert_eq!(prompt["steps"][0]["answered"], true);
        data.theme_answer(&n, "2", "").unwrap();
        data.theme_answer(&n, "1", "").unwrap();
        let prompt = data.tab(&n).unwrap()["prompt"].clone();
        assert_eq!(prompt["review"][1]["answer"], "Compact");
        let (said, answered) = data.theme_answer(&n, "1", "").unwrap();
        assert_eq!(said, "Answered \"Cool, Compact\"");
        assert!(answered);
        assert_eq!(data.tab(&n).unwrap()["state"], "working");
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
