//! `switchboard phone run`: the phone app served over HTTPS on this
//! computer's tailnet addresses alone, never on 127.0.0.1 or 0.0.0.0, so
//! only a tailnet peer reaches it and neither `tailscale serve` nor Funnel
//! can proxy to it; with the certificate `tailscale cert` issues for its
//! `*.ts.net` name, through rustls; axum over hyper for the routes.
//!
//! Every request passes `guard` first (`gate`): its address's `whois`, the
//! owner and a listed device, then, but for the page's own files and
//! pairing, the cookie the device was paired with, and for a write the
//! app's own `Origin`. A refusal is a 403 with a page that says nothing of
//! who is allowed (board 169), and a line in this process's log saying
//! why. No response allows another origin (no CORS headers), and each
//! carries a strict Content-Security-Policy.
//!
//! Nine things change anything, each asked of the server over its socket,
//! which refuses a pane not running Claude: answering a tab's prompt,
//! sending its Claude a message, marking it seen, setting or clearing its
//! reminder, pinning or unpinning it, stopping its working Claude's turn and
//! closing it (the last two asked about first in the app); running an option of one of mission control's menus
//! the phone shows (`phone::mission`), which the server refuses for one
//! the phone leaves out; and opening a tab running Claude in one of the
//! places `remote::places` lists (asked about first in the app), which
//! the server refuses for a place not listed. A pane is
//! taken only when the server's own `watch` lists it as a Claude tab. An
//! image from the phone's photo library is kept on the computer first
//! (`images`, for that tab alone, under a name made here), and the message
//! names it, its path typed after the text. The pictures a tab's Claude
//! showed with `look`, copied into the same folder by the server, come
//! in the stream (`looks`) and go among its turns by time.
//! What the drawer shows comes from that stream (`follow`), passed on as
//! server-sent events, and a tab starting to wait, or a tab's reminder
//! coming due, sends a Web Push naming it.

use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use aws_lc_rs::rand::{SecureRandom, SystemRandom};
use aws_lc_rs::signature::EcdsaKeyPair;
use axum::Router;
use axum::extract::{DefaultBodyLimit, Path, Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, Uri, header};
use axum::middleware::{self, Next};
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde_json::{Value, json};
use tokio::sync::watch;
use tokio_rustls::TlsAcceptor;
use tokio_rustls::rustls::{self, ServerConfig};

use super::build::Noted;
use super::guard::{self, Refusal, Rules, WhoisCache};
use super::pairing;
use super::push;
use super::tailnet::{Peer, Status, peer_from};
use super::turns;
use super::{images, looks};
use crate::remote::notify::{self, Front, Kind};
use crate::remote::places;
use crate::remote::protocol::{self, How, Refused, Remote, Server};
use crate::{assets, platform, settings, util};

/// Who the phone's asks name, for the server's log.
const FROM: &str = "phone";

/// How long the server's stream is given before trying again when it
/// can't be reached.
const RETRY: Duration = Duration::from_secs(2);

const CSP: &str = "default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data: blob:; connect-src 'self'; manifest-src 'self'; worker-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'none'";

/// What every request shares.
pub struct App {
    /// The app's own origin, which a write must name.
    origin: String,
    /// The computer's name before its tailnet's, for the page's header.
    computer: String,
    whois: Mutex<WhoisCache>,
    /// The Claude tabs as the server last sent them, and whether it can be
    /// reached (`live`).
    tabs: watch::Receiver<Value>,
    push_key: Option<Arc<EcdsaKeyPair>>,
    /// Which devices were last said to be behind the build, and how.
    builds: super::build::Seen,
}

/// The address a request came from, put on it as it arrives.
#[derive(Clone, Copy)]
struct From(IpAddr);

/// The device a request passed the guard as.
#[derive(Clone)]
struct Device(String);

/// The addresses served on: the tailnet's alone. A loopback or unspecified
/// address is never one, whatever Tailscale says.
pub fn listen_addrs(status: &Status, port: u16) -> Vec<SocketAddr> {
    status
        .ips
        .iter()
        .filter(|ip| !ip.is_loopback() && !ip.is_unspecified())
        .map(|ip| SocketAddr::new(*ip, port))
        .collect()
}

/// Binds every tailnet address, or fails naming why.
pub async fn bind(status: &Status, port: u16) -> Result<Vec<tokio::net::TcpListener>> {
    let addrs = listen_addrs(status, port);
    if addrs.is_empty() {
        return Err(anyhow!("this computer has no tailnet address to listen on"));
    }
    let mut listeners = Vec::new();
    for addr in addrs {
        listeners.push(
            tokio::net::TcpListener::bind(addr)
                .await
                .with_context(|| format!("listening on {addr}"))?,
        );
    }
    Ok(listeners)
}

/// The certificate Tailscale issues for `name`, fetched (or renewed) now
/// and kept in memory alone, never written to a file.
fn tls(name: &str) -> Result<TlsAcceptor> {
    let pem =
        platform::tailscale_cert(name).context("tailscale cert couldn't issue a certificate")?;
    let (chain, key) = cert_and_key(pem.as_bytes())?;
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let mut config = ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()?
        .with_no_client_auth()
        .with_single_cert(chain, key)?;
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(TlsAcceptor::from(Arc::new(config)))
}

/// The app's origin as a browser sends it, which leaves out the default
/// port.
pub fn origin(name: &str, port: u16) -> String {
    if port == 443 {
        format!("https://{name}")
    } else {
        format!("https://{name}:{port}")
    }
}

/// The certificate chain and the key in what `tailscale cert` printed,
/// both in one PEM stream.
fn cert_and_key(
    pem: &[u8],
) -> Result<(
    Vec<rustls::pki_types::CertificateDer<'static>>,
    rustls::pki_types::PrivateKeyDer<'static>,
)> {
    use rustls::pki_types::pem::PemObject;
    use rustls::pki_types::{CertificateDer, PrivateKeyDer};
    let chain: Vec<CertificateDer<'static>> = CertificateDer::pem_slice_iter(pem)
        .collect::<Result<_, _>>()
        .context("reading the certificate")?;
    if chain.is_empty() {
        return Err(anyhow!("tailscale cert printed no certificate"));
    }
    let key = PrivateKeyDer::from_pem_slice(pem).context("reading the certificate's key")?;
    Ok((chain, key))
}

/// Serves until the process is stopped.
pub async fn serve(status: Status, config: super::config::Config) -> Result<()> {
    let acceptor = tls(&status.name)?;
    images::prune(&images::dir(), std::time::SystemTime::now());
    let listeners = bind(&status, config.port).await?;
    let (tx, rx) = watch::channel(json!({"tabs": [], "live": false}));
    let push_key = match push::key() {
        Ok(key) => Some(Arc::new(key)),
        Err(e) => {
            super::log::line(&format!("no Web Push: {e:#}"));
            None
        }
    };
    tokio::spawn(follow(tx, push_key.clone()));
    let app = Arc::new(App {
        origin: origin(&status.name, config.port),
        computer: status.name.split('.').next().unwrap_or("").to_string(),
        whois: Mutex::new(WhoisCache::default()),
        tabs: rx,
        push_key,
        builds: super::build::Seen::default(),
    });
    let router = routes(app.clone());
    for l in &listeners {
        super::log::line(&format!(
            "serving https://{}:{} on {}",
            status.name,
            config.port,
            l.local_addr()?
        ));
    }
    let mut tasks = Vec::new();
    for listener in listeners {
        let (router, acceptor) = (router.clone(), acceptor.clone());
        tasks.push(tokio::spawn(accept(listener, router, acceptor)));
    }
    for t in tasks {
        let _ = t.await;
    }
    Ok(())
}

async fn accept(listener: tokio::net::TcpListener, router: Router, acceptor: TlsAcceptor) {
    loop {
        let Ok((stream, peer)) = listener.accept().await else {
            tokio::time::sleep(Duration::from_millis(100)).await;
            continue;
        };
        let (router, acceptor) = (router.clone(), acceptor.clone());
        tokio::spawn(async move {
            let Ok(tls) = acceptor.accept(stream).await else {
                return;
            };
            let service =
                hyper::service::service_fn(move |mut req: Request<hyper::body::Incoming>| {
                    req.extensions_mut().insert(From(peer.ip()));
                    let mut router = router.clone();
                    async move {
                        use tower_service::Service;
                        router.call(req.map(axum::body::Body::new)).await
                    }
                });
            let _ = hyper::server::conn::http1::Builder::new()
                .serve_connection(hyper_util::rt::TokioIo::new(tls), service)
                .with_upgrades()
                .await;
        });
    }
}

fn routes(app: Arc<App>) -> Router {
    Router::new()
        .route("/", get(page))
        .route("/app.js", get(|| file("app.js")))
        .route("/markdown-it.min.js", get(|| file("markdown-it.min.js")))
        .route("/app.css", get(|| file("app.css")))
        .route("/sw.js", get(|| file("sw.js")))
        .route(
            "/manifest.webmanifest",
            get(|| file("manifest.webmanifest")),
        )
        .route("/icon.svg", get(|| file("icon.svg")))
        .route("/icon-180.png", get(|| png(ICON_180)))
        .route("/icon-192.png", get(|| png(ICON_192)))
        .route("/icon-512.png", get(|| png(ICON_512)))
        .route("/api/me", get(me))
        .route("/api/pair", post(pair_start).get(pair_look))
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
        .route("/api/push/key", get(push_key))
        .route("/api/push/subscribe", post(subscribe))
        .fallback(|| async { (StatusCode::NOT_FOUND, "Not here") })
        .layer(middleware::from_fn_with_state(app.clone(), gate))
        .layer(middleware::from_fn(headers))
        .with_state(app)
}

/// The headers every response carries.
async fn headers(req: Request, next: Next) -> Response {
    let api = req.uri().path().starts_with("/api/");
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
    h.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    h.insert(
        header::STRICT_TRANSPORT_SECURITY,
        HeaderValue::from_static("max-age=31536000"),
    );
    // An image's name is never reused, so it alone may be kept, by the
    // phone's browser alone.
    if api && !h.contains_key(header::CACHE_CONTROL) {
        h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    res
}

/// The paths a listed device reaches before it is paired: the page's own
/// files, which hold nothing of yours, and pairing.
fn before_pairing(path: &str, method: &Method) -> bool {
    matches!(
        path,
        "/" | "/app.js"
            | "/markdown-it.min.js"
            | "/app.css"
            | "/sw.js"
            | "/manifest.webmanifest"
            | "/icon.svg"
            | "/icon-180.png"
            | "/icon-192.png"
            | "/icon-512.png"
            | "/api/me"
    ) && method == Method::GET
        || path == "/api/pair"
}

/// Every rule in `guard`, before anything else.
async fn gate(State(app): State<Arc<App>>, mut req: Request, next: Next) -> Response {
    let Some(From(ip)) = req.extensions().get::<From>().copied() else {
        return refuse(Refusal::NotOnTailnet, None, req.uri().path());
    };
    let path = req.uri().path().to_string();
    // The look, which holds nothing of yours, for the refused page too.
    if path == "/app.css" && req.method() == Method::GET {
        return next.run(req).await;
    }
    let peer = tokio::task::spawn_blocking({
        let app = app.clone();
        move || {
            let mut cache = app.whois.lock().unwrap_or_else(|e| e.into_inner());
            cache.peer(ip, Instant::now(), whois)
        }
    })
    .await
    .ok()
    .flatten();
    // phone.json read again for each request, so a device taken out of it
    // is refused at once; its port is the one served on until a restart.
    let rules = match super::config::read() {
        Ok(config) => Rules {
            owner: config.owner,
            devices: config.devices,
            origin: app.origin.clone(),
        },
        // Unreadable, it admits no one: the plain refusal, why only in the
        // log, so a device refused learns nothing of what runs here.
        Err(why) => {
            super::log::line(&format!("refused {path} from {ip}: {why}"));
            return refuse(Refusal::NotListed, Some(ip), &path);
        }
    };
    let device = match guard::admit(&rules, peer.as_ref()) {
        Ok(device) => device,
        Err(refusal) => return refuse(refusal, Some(ip), &path),
    };
    // Off, said only to a device it would answer.
    if settings::setting("phone") != Some(Value::Bool(true)) {
        super::log::line(&format!(
            "refused {path} from {ip}: the phone app is off in Settings"
        ));
        return (StatusCode::SERVICE_UNAVAILABLE, "The phone app is off.").into_response();
    }
    if let Err(refusal) = guard::same_origin(
        &rules,
        req.method().as_str(),
        req.headers()
            .get(header::ORIGIN)
            .and_then(|o| o.to_str().ok()),
    ) {
        return refuse(refusal, Some(ip), &path);
    }
    if !before_pairing(&path, req.method()) {
        let cookie = req
            .headers()
            .get(header::COOKIE)
            .and_then(|c| c.to_str().ok());
        if let Err(refusal) = guard::paired(&pairing::devices(), &device, cookie) {
            return refuse(refusal, Some(ip), &path);
        }
    }
    if let Some(builds) = req
        .headers()
        .get(super::build::HEADER)
        .and_then(|h| h.to_str().ok())
    {
        match app
            .builds
            .note(&device, builds, &served_build(), util::now_ms())
        {
            Noted::Quiet => {}
            Noted::Updated(line) => super::log::line(&line),
            // Said only when the page hasn't updated itself by then.
            Noted::FellBehind => {
                let (app, device) = (app.clone(), device.clone());
                tokio::spawn(async move {
                    tokio::time::sleep(super::build::LATE).await;
                    if let Some(line) = app.builds.overdue(&device, util::now_ms()) {
                        super::log::line(&line);
                    }
                });
            }
        }
    }
    req.extensions_mut().insert(Device(device));
    next.run(req).await
}

/// The build serving the app, as `app.js` and `sw.js` are stamped with it.
fn served_build() -> String {
    let assets = assets::dir();
    super::build::of(assets.as_deref().and_then(std::path::Path::parent))
}

fn whois(ip: IpAddr) -> Option<Peer> {
    platform::tailscale_whois(&ip.to_string())
        .ok()
        .and_then(|v| peer_from(&v))
}

/// A 403, logged with why; the page says only that it was refused.
fn refuse(refusal: Refusal, ip: Option<IpAddr>, path: &str) -> Response {
    let from = ip.map(|i| i.to_string()).unwrap_or_else(|| "?".into());
    super::log::line(&format!("refused {path} from {from}: {}", refusal.why()));
    if path.starts_with("/api/") {
        let why = if refusal == Refusal::NotPaired {
            "not paired"
        } else {
            "refused"
        };
        return (StatusCode::FORBIDDEN, axum::Json(json!({"error": why}))).into_response();
    }
    (
        StatusCode::FORBIDDEN,
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        REFUSED,
    )
        .into_response()
}

/// The refused page (board 169): it names neither the computer nor what is
/// checked, and looks the same to every device refused.
const REFUSED: &str = "<!doctype html><meta charset=utf-8><meta name=viewport content=\"width=device-width\"><title>Switchboard</title><link rel=stylesheet href=/app.css><body class=refused><main><h1>Not for this device</h1><p>This page answers one person's devices alone.</p></main></body>";

/// The home screen's icons, drawn from icon.svg: iOS's (180, square, since
/// iOS rounds it itself), and the manifest's 192 and 512 (maskable, its art
/// inside the safe zone).
const ICON_180: &[u8] = include_bytes!("../../assets/phone/icon-180.png");
const ICON_192: &[u8] = include_bytes!("../../assets/phone/icon-192.png");
const ICON_512: &[u8] = include_bytes!("../../assets/phone/icon-512.png");

async fn png(bytes: &'static [u8]) -> Response {
    ([(header::CONTENT_TYPE, "image/png")], bytes).into_response()
}

async fn page() -> Response {
    file("index.html").await
}

async fn file(name: &'static str) -> Response {
    let (text, kind) = match name {
        "index.html" => (
            assets::text(
                "phone/index.html",
                include_str!("../../assets/phone/index.html"),
            ),
            "text/html; charset=utf-8",
        ),
        "app.js" | "sw.js" => {
            let text = if name == "app.js" {
                assets::text("phone/app.js", include_str!("../../assets/phone/app.js"))
            } else {
                assets::text("phone/sw.js", include_str!("../../assets/phone/sw.js"))
            };
            return (
                [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
                super::build::stamped(&text, &served_build()),
            )
                .into_response();
        }
        // markdown-it 15.0.3's browser build, as published, for app.js's
        // `markdown` (its license beside it).
        "markdown-it.min.js" => (
            assets::text(
                "phone/markdown-it.min.js",
                include_str!("../../assets/phone/markdown-it.min.js"),
            ),
            "text/javascript; charset=utf-8",
        ),
        "app.css" => (
            assets::text("phone/app.css", include_str!("../../assets/phone/app.css")),
            "text/css; charset=utf-8",
        ),
        "manifest.webmanifest" => (
            assets::text(
                "phone/manifest.webmanifest",
                include_str!("../../assets/phone/manifest.webmanifest"),
            ),
            "application/manifest+json",
        ),
        _ => (
            assets::text(
                "phone/icon.svg",
                include_str!("../../assets/phone/icon.svg"),
            ),
            "image/svg+xml",
        ),
    };
    ([(header::CONTENT_TYPE, kind)], text.into_owned()).into_response()
}

fn device_of(req_device: Option<axum::Extension<Device>>) -> String {
    req_device.map(|d| d.0.0).unwrap_or_default()
}

/// Whether this device is paired, for the page to show the drawer or the
/// pairing; `revoked` when it carries a cookie that no longer counts.
async fn me(
    State(app): State<Arc<App>>,
    device: Option<axum::Extension<Device>>,
    headers: HeaderMap,
) -> Response {
    let device = device_of(device);
    let cookie = headers.get(header::COOKIE).and_then(|c| c.to_str().ok());
    let paired = guard::paired(&pairing::devices(), &device, cookie).is_ok();
    let revoked = !paired && guard::cookie(cookie, guard::COOKIE).is_some();
    axum::Json(json!({
        "paired": paired,
        "revoked": revoked,
        "device": device,
        "computer": app.computer,
        // What the drawer tells a page or worker behind by.
        "build": served_build(),
    }))
    .into_response()
}

fn random_bytes(n: usize) -> Option<Vec<u8>> {
    let mut out = vec![0u8; n];
    SystemRandom::new().fill(&mut out).ok()?;
    Some(out)
}

/// A pairing started: a code for the phone to show, and the workspace
/// asked to have it typed.
async fn pair_start(device: Option<axum::Extension<Device>>) -> Response {
    let device = device_of(device);
    let Some(bytes) = random_bytes(4) else {
        return (StatusCode::INTERNAL_SERVER_ERROR, "no randomness").into_response();
    };
    let code = format!(
        "{:06}",
        u32::from_be_bytes(bytes.try_into().unwrap_or_default()) % 1_000_000
    );
    if let Err(e) = pairing::start(&device, &code, util::now_ms()) {
        return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
    }
    super::log::line(&format!("{device} asked to pair"));
    let asked = device.clone();
    let _ =
        tokio::task::spawn_blocking(move || Server::current().ask(&["phone-pair", &asked])).await;
    axum::Json(json!({
        "code": pairing::shown(&code),
        "expires": util::now_ms() + pairing::CODE_LIFE_MS,
    }))
    .into_response()
}

/// Whether the pairing was approved: then the device's cookie is set.
async fn pair_look(device: Option<axum::Extension<Device>>) -> Response {
    let device = device_of(device);
    let now = util::now_ms();
    let Some(pending) = pairing::pending().filter(|p| p.device.eq_ignore_ascii_case(&device))
    else {
        return axum::Json(json!({"state": "refused"})).into_response();
    };
    if pending.expires <= now {
        pairing::cancel();
        return axum::Json(json!({"state": "refused"})).into_response();
    }
    if !pairing::take_approved(&device, &pending.code, now) {
        return axum::Json(json!({"state": "waiting"})).into_response();
    }
    let Some(token) = random_bytes(32).map(|b| URL_SAFE_NO_PAD.encode(b)) else {
        return (StatusCode::INTERNAL_SERVER_ERROR, "no randomness").into_response();
    };
    if let Err(e) = pairing::add(&device, &token, now) {
        return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
    }
    super::log::line(&format!("paired {device}"));
    let cookie = format!(
        "{}={token}; HttpOnly; Secure; SameSite=Strict; Path=/; Max-Age=31536000",
        guard::COOKIE
    );
    (
        [(header::SET_COOKIE, cookie)],
        axum::Json(json!({"state": "paired"})),
    )
        .into_response()
}

async fn tabs(State(app): State<Arc<App>>) -> Response {
    axum::Json(app.tabs.borrow().clone()).into_response()
}

/// Whether the phone app is on, phone.json still lists `device` and it
/// carries the cookie it was paired with: an open stream asks before each
/// update, so turning the app off, taking a device out of phone.json or
/// unpairing a phone ends what it is sent.
fn still_allowed(device: &str, cookie: Option<&str>) -> bool {
    settings::setting("phone") == Some(Value::Bool(true))
        && listed(device)
        && guard::paired(&pairing::devices(), device, cookie).is_ok()
}

/// Whether phone.json, read now, still lists `device`.
pub fn listed(device: &str) -> bool {
    super::config::read().is_ok_and(|c| c.devices.iter().any(|d| d.eq_ignore_ascii_case(device)))
}

/// The tabs, now and each time they change, as server-sent events.
async fn events(
    State(app): State<Arc<App>>,
    device: Option<axum::Extension<Device>>,
    headers: HeaderMap,
) -> Response {
    let rx = app.tabs.clone();
    let device = device_of(device);
    let cookie = headers
        .get(header::COOKIE)
        .and_then(|c| c.to_str().ok())
        .map(str::to_string);
    // The build serving the app with each message, and on its own as it
    // changes, so a page open in front updates itself (app.js's `catchUp`).
    let first = (rx, None::<String>);
    let stream = futures_util::stream::unfold(first, move |(mut rx, sent)| {
        let (device, cookie) = (device.clone(), cookie.clone());
        async move {
            let build = match sent {
                None => tokio::task::spawn_blocking(served_build).await.ok()?,
                Some(sent) => super::build::next_on_stream(&mut rx, &sent, served_build).await?,
            };
            let allowed =
                tokio::task::spawn_blocking(move || still_allowed(&device, cookie.as_deref()))
                    .await
                    .unwrap_or(false);
            if !allowed {
                return None;
            }
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

/// A Claude tab as the server's stream last listed it, by its pane's
/// number (the page's path has `/api/tabs/5/...` for `%5`, a `%` being
/// no part of a path): nothing else is ever reached.
fn claude_tab(app: &App, number: &str) -> Option<Value> {
    if number.is_empty() || !number.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let pane = format!("%{number}");
    app.tabs.borrow()["tabs"]
        .as_array()?
        .iter()
        .find(|t| t["pane"] == pane.as_str())
        .cloned()
}

fn pane_of(tab: &Value) -> String {
    tab["pane"].as_str().unwrap_or("").to_string()
}

/// A channel's last turns, or with `?before=<offset>` the turns before
/// the earliest it shows (`turns::window`), `&until=<ms>` the time of the
/// earliest message it shows, which the looks merged in come before: its messages, the offset the
/// next earlier read ends at (`from`), whether there is anything before
/// it (`earlier`), and which conversation they are of, a new one in the
/// pane (`/clear`) having nothing of the last.
async fn turns_of(State(app): State<Arc<App>>, Path(pane): Path<String>, uri: Uri) -> Response {
    let Some(tab) = claude_tab(&app, &pane) else {
        return (StatusCode::NOT_FOUND, "no such Claude tab").into_response();
    };
    let Some(file) = tab["transcript"].as_str().map(PathBuf::from) else {
        return axum::Json(
            json!({"messages": [], "from": 0, "earlier": false, "conversation": ""}),
        )
        .into_response();
    };
    let before = query_number(&uri, "before");
    let until = query_number(&uri, "until");
    let conversation = file
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    // The pictures its Claude showed with `look`, among them by time.
    let shown: Vec<looks::Look> = serde_json::from_value(tab["looks"].clone()).unwrap_or_default();
    let read = tokio::task::spawn_blocking(move || {
        let w = turns::window(&file, before)?;
        let messages = looks::merge(w.messages, &shown, w.from, w.earlier, until);
        Ok::<_, std::io::Error>((messages, w.from, w.earlier))
    })
    .await;
    let Ok(Ok((messages, from, earlier))) = read else {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            "the conversation couldn't be read",
        )
            .into_response();
    };
    // Another tab's message names its sender by its tab, as the drawer
    // lists it now, not by the address the transcript keeps.
    let tabs = app.tabs.borrow()["tabs"].clone();
    let tabs = tabs.as_array().map(Vec::as_slice).unwrap_or_default();
    let messages: Vec<Value> = messages
        .into_iter()
        .map(|m| {
            let tab = (m.who == "tab").then(|| sender_name(&m.from, tabs));
            let mut m = json!(m);
            if let Some((index, name)) = tab {
                m["from"] = json!(name);
                m["tab"] = json!(index);
            }
            m
        })
        .collect();
    axum::Json(json!({
        "messages": messages,
        "from": from,
        "earlier": earlier,
        "conversation": conversation,
    }))
    .into_response()
}

/// A number in a request's query, as a turns request's `before` (the
/// byte offset its turns end at) and `until`; none where it isn't given.
pub fn query_number<T: std::str::FromStr>(uri: &Uri, key: &str) -> Option<T> {
    uri.query()?
        .split('&')
        .find_map(|pair| pair.strip_prefix(key)?.strip_prefix('='))?
        .parse()
        .ok()
}

/// Who sent a message from another tab: the number and name of the tab
/// whose Claude has `address`, else "Another tab" (its tab gone, or a
/// session outside the workspace) with no number.
fn sender_name(address: &str, tabs: &[Value]) -> (Option<u64>, String) {
    tabs.iter()
        .find(|t| !address.is_empty() && t["address"] == address)
        .map(|t| {
            let index = t["index"].as_u64();
            let name = t["name"].as_str().unwrap_or("");
            let words = match index {
                Some(n) => format!("{n} {name}").trim().to_string(),
                None => name.to_string(),
            };
            (index, words)
        })
        .unwrap_or((None, "Another tab".to_string()))
}

/// Asks the server through the protocol, off this task: what it said,
/// or why not.
async fn asked(
    ask: impl FnOnce(&Server) -> Result<String> + Send + 'static,
) -> Result<String, Box<Response>> {
    let answer = tokio::task::spawn_blocking(move || ask(&Server::current())).await;
    match answer {
        Ok(Ok(out)) => Ok(out),
        // Refused in the server's words, shown whole; anything else (no
        // server, or one that hung up) as Switchboard not running.
        Ok(Err(ref e)) if let Some(Refused::No(why)) = e.downcast_ref() => Err(Box::new(
            (StatusCode::CONFLICT, axum::Json(json!({"error": why}))).into_response(),
        )),
        _ => Err(Box::new(
            (
                StatusCode::SERVICE_UNAVAILABLE,
                axum::Json(json!({"error": "Switchboard isn't running"})),
            )
                .into_response(),
        )),
    }
}

/// `asked`, answered as `{"said": ...}`.
async fn ask(ask: impl FnOnce(&Server) -> Result<String> + Send + 'static) -> Response {
    match asked(ask).await {
        Ok(said) => axum::Json(json!({"said": said.trim()})).into_response(),
        Err(refused) => *refused,
    }
}

fn text_of(body: &Value, key: &str) -> String {
    body[key].as_str().unwrap_or("").to_string()
}

async fn answer(
    State(app): State<Arc<App>>,
    Path(pane): Path<String>,
    axum::Json(body): axum::Json<Value>,
) -> Response {
    let Some(tab) = claude_tab(&app, &pane) else {
        return (StatusCode::NOT_FOUND, "no such Claude tab").into_response();
    };
    let pane = pane_of(&tab);
    let reply = text_of(&body, "reply");
    // The prompt the phone showed: an answer is pressed only on that one.
    let id = text_of(&body, "prompt");
    if id.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            axum::Json(json!({"error": "which prompt it answers isn't named"})),
        )
            .into_response();
    }
    // A step over a prompt's questions (`back`, `forward` or a step's
    // name) is a field of its own, never `reply`, which carries words typed
    // in the composer.
    let step = text_of(&body, "step");
    ask(move |s| {
        let how = How {
            from: Some(FROM),
            expect: Some(&id),
            step: Some(step.as_str()).filter(|s| !s.is_empty()),
            ..How::default()
        };
        s.answer(&pane, &reply, how)
    })
    .await
}

async fn message(
    State(app): State<Arc<App>>,
    Path(pane): Path<String>,
    axum::Json(body): axum::Json<Value>,
) -> Response {
    let Some(tab) = claude_tab(&app, &pane) else {
        return (StatusCode::NOT_FOUND, "no such Claude tab").into_response();
    };
    let pane = pane_of(&tab);
    let names: Vec<String> = body["images"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|n| n.as_str().unwrap_or("").to_string())
        .collect();
    let dir = images::dir();
    let mut paths = Vec::new();
    for name in &names {
        let Some(path) = images::path_of(&dir, name) else {
            return (
                StatusCode::BAD_REQUEST,
                axum::Json(json!({"error": "an image sent isn't on the computer"})),
            )
                .into_response();
        };
        paths.push(path);
    }
    let text = images::typed(&text_of(&body, "text"), &paths);
    ask(move |s| {
        let how = How {
            from: Some(FROM),
            ..How::default()
        };
        s.message(&pane, &text, how)
    })
    .await
}

/// An image for a tab's Claude, kept until a message names it; answers
/// with its name.
async fn image(
    State(app): State<Arc<App>>,
    Path(pane): Path<String>,
    body: axum::body::Bytes,
) -> Response {
    if claude_tab(&app, &pane).is_none() {
        return (StatusCode::NOT_FOUND, "no such Claude tab").into_response();
    }
    let Some(random) = random_bytes(16) else {
        return (StatusCode::INTERNAL_SERVER_ERROR, "no randomness").into_response();
    };
    let saved = tokio::task::spawn_blocking(move || images::save(&images::dir(), &body, &random))
        .await
        .map_err(|e| anyhow!("{e}"))
        .and_then(|r| r);
    match saved {
        Ok(name) => axum::Json(json!({"id": name})).into_response(),
        Err(e) => (
            StatusCode::BAD_REQUEST,
            axum::Json(json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

/// An image the phone sent, or one a tab's Claude showed, for its
/// channel: by its name in the images folder alone.
async fn image_of(Path(name): Path<String>) -> Response {
    let read = tokio::task::spawn_blocking({
        let name = name.clone();
        move || images::path_of(&images::dir(), &name).and_then(|p| std::fs::read(p).ok())
    })
    .await
    .ok()
    .flatten();
    match read {
        Some(bytes) => (
            [
                (header::CONTENT_TYPE, images::content_type(&name)),
                (header::CACHE_CONTROL, "private, max-age=2592000, immutable"),
            ],
            bytes,
        )
            .into_response(),
        None => (StatusCode::NOT_FOUND, "Not here").into_response(),
    }
}

async fn seen(State(app): State<Arc<App>>, Path(pane): Path<String>) -> Response {
    let Some(tab) = claude_tab(&app, &pane) else {
        return (StatusCode::NOT_FOUND, "no such Claude tab").into_response();
    };
    let tab = tab["tab"].as_str().unwrap_or("").to_string();
    ask(move |s| s.mark_seen(&tab, Some(FROM)).map(|_| String::new())).await
}

async fn close(State(app): State<Arc<App>>, Path(pane): Path<String>) -> Response {
    let Some(tab) = claude_tab(&app, &pane) else {
        return (StatusCode::NOT_FOUND, "no such Claude tab").into_response();
    };
    let pane = pane_of(&tab);
    ask(move |s| s.close(&pane, FROM)).await
}

/// A working tab's Claude stopped as Esc stops it, which the phone asks
/// about first; the server refuses one that isn't working.
async fn stop(State(app): State<Arc<App>>, Path(pane): Path<String>) -> Response {
    let Some(tab) = claude_tab(&app, &pane) else {
        return (StatusCode::NOT_FOUND, "no such Claude tab").into_response();
    };
    let pane = pane_of(&tab);
    ask(move |s| s.stop(&pane, Some(FROM), false)).await
}

/// A tab's PR, ticket or note as its view on the phone shows it (boards
/// 189 and 190), read only, through `phone-tab`, which refuses a pane not
/// running Claude and a view that is off.
async fn tab_view(
    State(app): State<Arc<App>>,
    Path(pane): Path<String>,
    kind: &'static str,
) -> Response {
    let Some(tab) = claude_tab(&app, &pane) else {
        return (StatusCode::NOT_FOUND, "no such Claude tab").into_response();
    };
    let pane = pane_of(&tab);
    match asked(move |s| s.tab_view(&pane, kind)).await {
        Ok(out) => match serde_json::from_str::<Value>(out.trim()) {
            Ok(v) => axum::Json(v).into_response(),
            Err(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "the view couldn't be read",
            )
                .into_response(),
        },
        Err(refused) => *refused,
    }
}

/// The places a tab can be opened in, as the relay's New tab form lists
/// them (`remote::places`): `{"groups": [{"name", "places": [{"label",
/// "value"}]}]}`, refused while Settings' `phone_new_tab` is off.
async fn places_list() -> Response {
    if !settings::is_on("phone_new_tab") {
        return (
            StatusCode::CONFLICT,
            axum::Json(json!({"error": "opening tabs from the phone is off in Settings"})),
        )
            .into_response();
    }
    let read = tokio::task::spawn_blocking(|| {
        let windows = Server::current().windows()?;
        let known = places::Known::of(&crate::paths::home(), &windows);
        Some(places_json(&places::place_choices(&known)))
    })
    .await;
    match read {
        Ok(Some(groups)) => axum::Json(groups).into_response(),
        _ => (
            StatusCode::SERVICE_UNAVAILABLE,
            axum::Json(json!({"error": "Switchboard isn't running"})),
        )
            .into_response(),
    }
}

/// The places' groups as the phone reads them.
pub fn places_json(groups: &[(String, Vec<places::Choice>)]) -> Value {
    json!({
        "groups": groups
            .iter()
            .map(|(name, choices)| json!({
                "name": name,
                "places": choices
                    .iter()
                    .map(|c| json!({"label": c.label, "value": c.value}))
                    .collect::<Vec<_>>(),
            }))
            .collect::<Vec<_>>(),
    })
}

/// A tab opened in the background running Claude with `prompt` in
/// `place`, one of the places listed, which the app asks about first: what
/// the server said and the new tab's pane, for the app to open its channel.
async fn open_place(axum::Json(body): axum::Json<Value>) -> Response {
    let (place, prompt) = (text_of(&body, "place"), text_of(&body, "prompt"));
    match asked(move |s| {
        let opened = s.open_claude(&place, &prompt, FROM, false)?;
        Ok(format!("{}\t{}", opened.pane, opened.said))
    })
    .await
    {
        Ok(out) => {
            let (pane, said) = out.split_once('\t').unwrap_or((&out, ""));
            axum::Json(json!({
                "pane": pane.trim_start_matches('%'),
                "said": format!("Started Claude in {}", said.replace('`', "")),
            }))
            .into_response()
        }
        Err(refused) => *refused,
    }
}

/// An option of mission control's menu for an item of a view, by the
/// item's ID and the option's label: the server builds the menu again and
/// runs it, refusing one the phone leaves out or no longer offered.
async fn view_item(Path(view): Path<String>, axum::Json(body): axum::Json<Value>) -> Response {
    let (id, label) = (text_of(&body, "id"), text_of(&body, "label"));
    ask(move |s| s.view_option(&view, Some(&id), &label)).await
}

/// An option of a view's own menu, by its label, as `view_item`.
async fn view_act(Path(view): Path<String>, axum::Json(body): axum::Json<Value>) -> Response {
    let label = text_of(&body, "label");
    ask(move |s| s.view_option(&view, None, &label)).await
}

/// A tab's reminder set for `when` (one of the box's times, or one
/// typed, which the server reads) with `note`, or, with `clear`, cleared.
async fn remind(
    State(app): State<Arc<App>>,
    Path(pane): Path<String>,
    axum::Json(body): axum::Json<Value>,
) -> Response {
    let Some(tab) = claude_tab(&app, &pane) else {
        return (StatusCode::NOT_FOUND, "no such Claude tab").into_response();
    };
    let pane = pane_of(&tab);
    let when = (body["clear"] != true).then(|| text_of(&body, "when"));
    let note = text_of(&body, "note");
    ask(move |s| s.remind(&pane, when.as_deref(), &note, Some(FROM))).await
}

/// A tab pinned, or, with `pin` false, unpinned.
async fn pin(
    State(app): State<Arc<App>>,
    Path(pane): Path<String>,
    axum::Json(body): axum::Json<Value>,
) -> Response {
    let Some(tab) = claude_tab(&app, &pane) else {
        return (StatusCode::NOT_FOUND, "no such Claude tab").into_response();
    };
    let pane = pane_of(&tab);
    let pin = body["pin"] == true;
    ask(move |s| s.pin(&pane, pin, Some(FROM))).await
}

async fn push_key(State(app): State<Arc<App>>) -> Response {
    match &app.push_key {
        Some(key) => axum::Json(json!({"key": push::public_key(key)})).into_response(),
        None => (StatusCode::NOT_FOUND, "no push key").into_response(),
    }
}

async fn subscribe(
    device: Option<axum::Extension<Device>>,
    axum::Json(body): axum::Json<Value>,
) -> Response {
    let sub = push::Subscription {
        device: device_of(device),
        endpoint: text_of(&body, "endpoint"),
        p256dh: body["keys"]["p256dh"].as_str().unwrap_or("").to_string(),
        auth: body["keys"]["auth"].as_str().unwrap_or("").to_string(),
    };
    match push::subscribe(sub) {
        Ok(()) => axum::Json(json!({"ok": true})).into_response(),
        Err(e) => (
            StatusCode::BAD_REQUEST,
            axum::Json(json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

/// The tabs that started waiting between two of the server's lines.
pub fn started_waiting(before: &Value, now: &Value) -> Vec<Value> {
    let was_waiting = |tab: &Value| {
        before["tabs"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|b| b["tab"] == tab["tab"] && b["state"] == "waiting")
    };
    now["tabs"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|t| t["state"] == "waiting" && !was_waiting(t))
        .cloned()
        .collect()
}

/// The tabs whose reminder came due between two of the server's lines:
/// due now, and not due before with the same time, so each reminder is
/// pushed once.
pub fn came_due(before: &Value, now: &Value) -> Vec<Value> {
    let was_due = |tab: &Value| {
        before["tabs"].as_array().into_iter().flatten().any(|b| {
            b["tab"] == tab["tab"]
                && b["remind"]["due"] == true
                && b["remind"]["at"] == tab["remind"]["at"]
        })
    };
    now["tabs"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|t| t["remind"]["due"] == true && !was_due(t))
        .cloned()
        .collect()
}

/// The server's `watch`, followed for as long as the process runs: each
/// line handed to the page's stream, a push for each tab that started
/// waiting and each whose reminder came due (its note never in it), and `live` false while the server can't be reached.
async fn follow(tx: watch::Sender<Value>, push_key: Option<Arc<EcdsaKeyPair>>) {
    let mut last: Option<Value> = None;
    loop {
        if let Err(e) = follow_once(&tx, &push_key, &mut last).await {
            super::log::line(&format!("Switchboard's server: {e:#}"));
        }
        tx.send_modify(|v| v["live"] = json!(false));
        tokio::time::sleep(RETRY).await;
    }
}

async fn follow_once(
    tx: &watch::Sender<Value>,
    push_key: &Option<Arc<EcdsaKeyPair>>,
    last: &mut Option<Value>,
) -> Result<()> {
    protocol::watch(|mut now| {
        now["live"] = json!(true);
        if let (Some(before), Some(key)) = (last.as_ref(), push_key) {
            let waiting = started_waiting(before, &now)
                .into_iter()
                .map(|t| (t, Kind::Prompts));
            let due = came_due(before, &now)
                .into_iter()
                .map(|t| (t, Kind::Notices));
            for (tab, kind) in waiting.chain(due) {
                // Read as each push goes, so turned off while it runs, it
                // pushes nothing.
                let hour = util::local_now().hour() as u32;
                if notify::decide(Front::Phone, &settings::setting, kind, false, hour)
                    != notify::Send::Notify
                {
                    continue;
                }
                let title = format!(
                    "Tab {} · {}",
                    tab["index"],
                    tab["name"].as_str().unwrap_or("")
                );
                let body = if kind == Kind::Notices {
                    "Reminder"
                } else {
                    ""
                };
                let (key, id) = (
                    key.clone(),
                    tab["pane"]
                        .as_str()
                        .unwrap_or("")
                        .trim_start_matches('%')
                        .to_string(),
                );
                tokio::spawn(async move {
                    push::send(&key, &title, body, &id).await;
                });
            }
        }
        *last = Some(now.clone());
        let _ = tx.send(now);
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn another_tabs_message_is_named_by_its_tab() {
        let tabs = [
            json!({"index": 6, "name": "review PR 41", "address": "uds:/tmp/six.sock"}),
            json!({"index": 3, "name": "planning", "address": null}),
        ];
        assert_eq!(
            sender_name("uds:/tmp/six.sock", &tabs),
            (Some(6), "6 review PR 41".to_string())
        );
        assert_eq!(
            sender_name("uds:/tmp/gone.sock", &tabs),
            (None, "Another tab".to_string())
        );
        assert_eq!(sender_name("", &tabs), (None, "Another tab".to_string()));
    }

    #[test]
    fn only_tailnet_addresses_are_listened_on() {
        let status = Status {
            ips: vec![
                "100.64.0.1".parse().unwrap(),
                "127.0.0.1".parse().unwrap(),
                "0.0.0.0".parse().unwrap(),
                "::1".parse().unwrap(),
            ],
            name: "studio.tail1.ts.net".into(),
        };
        assert_eq!(
            listen_addrs(&status, 8443),
            vec!["100.64.0.1:8443".parse::<SocketAddr>().unwrap()]
        );
    }

    /// With no tailnet address but loopback, nothing is bound, so a
    /// connection to 127.0.0.1 is refused.
    #[test]
    fn the_listener_refuses_a_loopback_connection() {
        let status = Status {
            ips: vec!["127.0.0.1".parse().unwrap()],
            name: "studio.tail1.ts.net".into(),
        };
        let free = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = free.local_addr().unwrap().port();
        drop(free);
        let bound = util::block_on(bind(&status, port));
        assert!(bound.is_err(), "nothing to listen on");
        assert!(std::net::TcpStream::connect(("127.0.0.1", port)).is_err());
    }

    #[test]
    fn the_certificate_and_key_are_read_from_one_stream() {
        let pem = "-----BEGIN CERTIFICATE-----\nAQID\n-----END CERTIFICATE-----\n-----BEGIN CERTIFICATE-----\nBAUG\n-----END CERTIFICATE-----\n-----BEGIN EC PRIVATE KEY-----\nBwgJ\n-----END EC PRIVATE KEY-----\n";
        let (chain, key) = cert_and_key(pem.as_bytes()).unwrap();
        assert_eq!(chain.len(), 2);
        assert_eq!(key.secret_der(), &[7, 8, 9]);
        assert!(cert_and_key(b"").is_err());
    }

    /// Zooming would throw the channel out of its layout (issue 187).
    #[test]
    fn the_page_viewport_turns_zooming_off() {
        let page = include_str!("../../assets/phone/index.html");
        let viewport = page
            .lines()
            .find(|line| line.contains("name=\"viewport\""))
            .expect("a viewport meta tag");
        assert!(viewport.contains("maximum-scale=1"), "{viewport}");
        assert!(viewport.contains("user-scalable=no"), "{viewport}");
    }

    #[test]
    fn the_origin_leaves_out_the_default_port() {
        assert_eq!(
            origin("studio.tail1.ts.net", 443),
            "https://studio.tail1.ts.net"
        );
        assert_eq!(
            origin("studio.tail1.ts.net", 8443),
            "https://studio.tail1.ts.net:8443"
        );
    }

    #[test]
    fn a_tab_that_starts_waiting_is_pushed_once() {
        let before =
            json!({"tabs": [{"tab": "@1", "state": "working"}, {"tab": "@2", "state": "waiting"}]});
        let now =
            json!({"tabs": [{"tab": "@1", "state": "waiting"}, {"tab": "@2", "state": "waiting"}]});
        let started = started_waiting(&before, &now);
        assert_eq!(started.len(), 1);
        assert_eq!(started[0]["tab"], "@1");
    }

    #[test]
    fn a_reminder_that_comes_due_is_pushed_once() {
        let remind = |at: i64, due: bool| json!({"at": at, "note": "check the deploy", "due": due});
        let before = json!({"tabs": [
            {"tab": "@1", "remind": remind(100, false)},
            {"tab": "@2", "remind": remind(100, true)},
            {"tab": "@3", "remind": remind(100, true)},
            {"tab": "@4", "remind": null},
        ]});
        let now = json!({"tabs": [
            {"tab": "@1", "remind": remind(100, true)},
            {"tab": "@2", "remind": remind(100, true)},
            {"tab": "@3", "remind": remind(200, true)},
            {"tab": "@4", "remind": null},
        ]});
        let due: Vec<_> = came_due(&before, &now)
            .iter()
            .map(|t| t["tab"].clone())
            .collect();
        assert_eq!(
            due,
            [json!("@1"), json!("@3")],
            "come due, or set again and due"
        );
        assert!(came_due(&now, &now).is_empty(), "never twice");
    }

    #[test]
    fn only_the_pages_files_and_pairing_come_before_pairing() {
        assert!(before_pairing("/", &Method::GET));
        assert!(before_pairing("/api/pair", &Method::POST));
        assert!(!before_pairing("/api/tabs", &Method::GET));
        assert!(!before_pairing("/api/tabs/%1/answer", &Method::POST));
        assert!(!before_pairing("/api/tabs/1/close", &Method::POST));
        assert!(!before_pairing("/api/tabs/1/remind", &Method::POST));
        assert!(!before_pairing("/api/tabs/1/pin", &Method::POST));
        assert!(!before_pairing("/api/tabs/1/stop", &Method::POST));
        assert!(!before_pairing("/api/tabs/1/note", &Method::GET));
        assert!(!before_pairing("/", &Method::POST));
    }

    /// A PNG's width and height, from its IHDR.
    fn size_of(png: &[u8]) -> (u32, u32) {
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        let at = |n: usize| u32::from_be_bytes(png[n..n + 4].try_into().unwrap());
        (at(16), at(20))
    }

    /// What an installed app needs: iOS's 180 icon on the page, the
    /// manifest's id and its 192 and 512 icons (one maskable), each served
    /// before pairing as the page's other files are.
    #[test]
    fn the_app_carries_its_home_screen_icons() {
        assert_eq!(size_of(ICON_180), (180, 180));
        assert_eq!(size_of(ICON_192), (192, 192));
        assert_eq!(size_of(ICON_512), (512, 512));
        let index = include_str!("../../assets/phone/index.html");
        assert!(index.contains(r#"rel="apple-touch-icon" sizes="180x180" href="/icon-180.png""#));
        let manifest: Value =
            serde_json::from_str(include_str!("../../assets/phone/manifest.webmanifest")).unwrap();
        assert_eq!(manifest["id"], "/");
        let icons = manifest["icons"].as_array().unwrap();
        let icon = |src: &str| icons.iter().find(|i| i["src"] == src).cloned();
        assert_eq!(icon("/icon-192.png").unwrap()["sizes"], "192x192");
        let large = icon("/icon-512.png").unwrap();
        assert_eq!(
            (large["sizes"].as_str(), large["purpose"].as_str()),
            (Some("512x512"), Some("maskable"))
        );
        for path in ["/icon-180.png", "/icon-192.png", "/icon-512.png"] {
            assert!(before_pairing(path, &Method::GET), "{path}");
        }
    }

    /// The service worker keeps a copy of the page's own files, and lets
    /// everything under /api/ go to the computer untouched.
    #[test]
    fn the_service_worker_keeps_the_apps_files_and_never_the_api() {
        let sw = include_str!("../../assets/phone/sw.js");
        assert!(sw.contains(r#"addEventListener("fetch""#));
        assert!(
            sw.contains(r#"url.pathname.startsWith("/api/") || !isShell(url)) return;"#),
            "a request under /api/ is left to the browser"
        );
        let shell = &sw[sw.find("const SHELL = [").unwrap()..];
        let shell = &shell[..shell.find("];").unwrap()];
        for file in [
            "\"/\"",
            "/app.js",
            "/markdown-it.min.js",
            "/app.css",
            "/icon-180.png",
            "/manifest.webmanifest",
        ] {
            assert!(shell.contains(file), "{file} kept");
        }
        assert!(!shell.contains("/api"), "nothing of the API kept: {shell}");
        // Each file it keeps is one the server serves before pairing.
        for path in shell.split('"').skip(1).step_by(2) {
            assert!(before_pairing(path, &Method::GET), "{path}");
        }
    }
}
