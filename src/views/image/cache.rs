//! The image view's caches (the image_cache setting), so a PR or ticket
//! opened again shows at once, ported from tmux/pr-image.js's `assetCache`,
//! `cachedPages`, `savePages` and `prune`:
//!
//! - `~/.cache/switchboard/assets/`: what the page loads from the web, the
//!   stylesheet and scripts it is pinned to and the PR's or ticket's
//!   pictures, each a file named by the SHA-256 of its link, without the
//!   query for GitHub's private pictures and Linear's uploads
//!   (`SIGNED_HOSTS`), whose signed links change their token on every read
//!   while the path stays the file's. pr-image.js answered the page's
//!   requests from it through puppeteer's request interception; Chrome here
//!   runs with no DevTools protocol (`super::render`), so the view fetches
//!   them itself before Chrome draws (`local_copies`, each request counted
//!   in the request audit by src/net.rs, a copy found counting nothing) and
//!   the page names the copies in place of the links (`super::page`). A
//!   pinned file is kept only once its hash is the pin's, and used only
//!   while it still is; a picture only when it comes back whole as a
//!   picture, under the link the page names however it was redirected. One
//!   that can't be fetched is left for Chrome to load, as before. Each copy
//!   is named with its kind's extension, which Chrome reads a `file://`
//!   link's type from.
//! - `~/.cache/switchboard/pr-image/pages/<key>/`: the pages drawn, keyed
//!   by the page with those signatures left out (its text and look), the
//!   pane's size and the build (`page_key`), so the same PR or ticket at
//!   the same size shows from there without running Chrome. A drawing in
//!   which something failed to load is shown but not kept, so the next
//!   open tries again.
//!
//! Both are held to `LIMIT` together as the view starts and after each
//! drawing, the least recently used first (by their time, touched on each
//! use), the pages on screen kept, and page sets over 30 days old dropped
//! whatever the size (`prune`). M-p's "Clear the PR and ticket image cache"
//! removes them, and the text last read (`clear`).

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::{Duration, SystemTime};

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256, Sha384};

use super::page::{self, Bands, Local, Parts, Pin};
use super::render::{BAND, DPR, Shot, View};
use crate::{links, net, paths, util};

/// What the caches are held to together.
pub const LIMIT: u64 = 300 * 1024 * 1024;
/// How old a page set may get, unused, before it is dropped.
pub const PAGES_MAX_AGE: Duration = Duration::from_secs(30 * 24 * 60 * 60);
/// The hosts whose query is a signature rather than part of what the file
/// is: GitHub's private pictures (`jwt=`) and Linear's uploads. Anywhere
/// else the query is kept, since it may name the picture (`/image?id=1`).
pub const SIGNED_HOSTS: [&str; 2] = [
    "private-user-images.githubusercontent.com",
    "uploads.linear.app",
];
/// What asked, for the request audit, as the page's own requests are
/// counted (`super::render`).
const SOURCE: &str = "PR as an image: page";
/// How long one file may take to arrive.
const FETCH_TIMEOUT: Duration = Duration::from_secs(60);
/// The kinds a copy is kept as, by its extension.
const KINDS: [&str; 11] = [
    "css", "js", "png", "jpg", "gif", "webp", "svg", "avif", "bmp", "ico", "bin",
];

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// The name a link's copy is kept under: its link without the fragment,
/// and without the query on a signed host.
pub fn asset_key(url: &str) -> String {
    let url = url.split('#').next().unwrap_or(url);
    let (place, query) = url
        .split_once('?')
        .map_or((url, None), |(p, q)| (p, Some(q)));
    let signed = links::host_of(place).is_some_and(|h| SIGNED_HOSTS.contains(&h.as_str()));
    match query {
        Some(q) if !signed => sha256_hex(format!("{place}?{q}").as_bytes()),
        _ => sha256_hex(place.as_bytes()),
    }
}

/// A signed host's link with its signature.
static SIGNED_LINK: LazyLock<Regex> = LazyLock::new(|| {
    let hosts: Vec<String> = SIGNED_HOSTS.iter().map(|h| regex::escape(h)).collect();
    Regex::new(&format!(
        r#"(https://(?:{})/[^"'\s?<>\\]*)\?[^"'\s<>)\\]*"#,
        hosts.join("|")
    ))
    .unwrap()
});

/// Text with the signed hosts' links' signatures left out, which GitHub
/// and Linear write anew on every read, so a PR read again unchanged is the
/// same text.
pub fn unsigned(text: &str) -> String {
    SIGNED_LINK.replace_all(text, "$1").into_owned()
}

/// The PR or ticket as what it shows, to tell whether a read changed it.
pub fn same_text(parts: &Parts) -> String {
    unsigned(&util::json_compact(parts))
}

/// The key of the pages drawn for these parts in this view: the page as
/// Chrome is given it (the text and its look) with the signatures left
/// out, the pane's size in cells and pixels, the pixel ratio and the
/// build, whose drawing may differ from another's.
pub fn page_key(parts: &Parts, view: &View) -> String {
    let page = page::html(
        parts,
        view.width,
        super::render::fit(view),
        BAND,
        &Bands::default(),
        &Local::new(),
    );
    let key = json!({
        "page": unsigned(&page),
        "cols": view.cols,
        "rows": view.rows,
        "width": view.width,
        "pageHeight": view.page_height,
        "dpr": DPR,
        "build": env!("CARGO_PKG_VERSION"),
    });
    sha256_hex(util::json_compact(&key).as_bytes())
}

/// Whether a file is the one a pin names, by its hash.
fn pinned(pin: Pin, bytes: &[u8]) -> bool {
    format!("sha384-{}", util::base64(&Sha384::digest(bytes))) == pin.integrity
}

/// The extension a pinned file is kept with.
fn pin_kind(pin: Pin) -> &'static str {
    if pin.url.ends_with(".css") {
        "css"
    } else {
        "js"
    }
}

/// The extension a picture of this type is kept with, or `None` for an
/// answer that isn't a picture (a sign-in page).
fn picture_kind(content_type: &str) -> Option<&'static str> {
    let kind = content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    let sub = kind.strip_prefix("image/")?;
    Some(match sub {
        "png" => "png",
        "jpeg" | "jpg" => "jpg",
        "gif" => "gif",
        "webp" => "webp",
        "svg+xml" => "svg",
        "avif" => "avif",
        "bmp" => "bmp",
        "x-icon" | "vnd.microsoft.icon" => "ico",
        _ => "bin",
    })
}

/// A file's time set to now, which `prune` takes as when it was last used.
fn touch(path: &Path) {
    if let Ok(file) = fs::File::open(path) {
        let _ = file.set_modified(SystemTime::now());
    }
}

/// A copy's name: the key and a kind's extension.
fn is_asset_name(name: &str) -> bool {
    name.split_once('.').is_some_and(|(key, kind)| {
        key.len() == 64 && key.bytes().all(|b| b.is_ascii_hexdigit()) && KINDS.contains(&kind)
    })
}

/// What a link fetched gave: its type and bytes, or nothing for anything
/// but a whole answer.
async fn fetch(url: &str) -> Option<(String, Vec<u8>)> {
    let host = links::host_of(url)?;
    let audit = net::Audit {
        service: &host,
        protocol: "HTTPS",
        source: SOURCE,
    };
    let got = net::get_file(url, &[], FETCH_TIMEOUT, audit).await.ok()?;
    (got.status == 200).then(|| (got.content_type.unwrap_or_default(), got.bytes))
}

/// A link fetched, and when it came back as wanted, the kind it is kept as
/// and its bytes.
type Fetched = (String, Option<(&'static str, Vec<u8>)>);

/// A page's rows and where it starts, as kept beside its picture.
#[derive(Serialize, Deserialize)]
struct Kept {
    rows: u16,
    from: f64,
}

/// The caches, under a root: `~/.cache/switchboard` (`here`), or a test's.
#[derive(Clone, Debug)]
pub struct Cache {
    root: PathBuf,
}

impl Cache {
    pub fn at(root: impl Into<PathBuf>) -> Cache {
        Cache { root: root.into() }
    }

    /// Switchboard's own.
    pub fn here() -> Cache {
        Cache::at(paths::cache_dir())
    }

    pub fn assets(&self) -> PathBuf {
        self.root.join("assets")
    }

    pub fn pages(&self) -> PathBuf {
        self.root.join("pr-image").join("pages")
    }

    /// The copy kept of what `url` names, touched as it is used.
    pub fn asset(&self, url: &str) -> Option<PathBuf> {
        let key = asset_key(url);
        let dir = self.assets();
        let file = KINDS
            .iter()
            .map(|kind| dir.join(format!("{key}.{kind}")))
            .find(|f| f.is_file())?;
        touch(&file);
        Some(file)
    }

    /// `bytes` kept as the copy of what `url` names, as a file of `kind`.
    pub fn keep_asset(&self, url: &str, kind: &str, bytes: &[u8]) -> Option<PathBuf> {
        let file = self.assets().join(format!("{}.{kind}", asset_key(url)));
        util::write_atomic(&file, bytes).ok()?;
        Some(file)
    }

    /// The copies of `pins` (the page's, `page::PINS`) and `pictures`,
    /// from here where they are kept, the rest fetched, all at once, and
    /// kept: a pinned file only when its hash is the pin's, a picture only
    /// when it is one. What can't be had is left out, for the page to load
    /// from the web.
    pub fn local_copies(&self, pins: &[Pin], pictures: &[String]) -> Local {
        let mut local = Local::new();
        let mut wanted_pins = Vec::new();
        for &pin in pins {
            let copy = self
                .asset(pin.url)
                .filter(|f| fs::read(f).is_ok_and(|b| pinned(pin, &b)));
            match copy {
                Some(file) => {
                    local.insert(pin.url.to_string(), file);
                }
                None => wanted_pins.push(pin),
            }
        }
        let mut wanted = Vec::new();
        for url in pictures {
            match self.asset(url) {
                Some(file) => {
                    local.insert(url.clone(), file);
                }
                None => wanted.push(url.clone()),
            }
        }
        if wanted_pins.is_empty() && wanted.is_empty() {
            return local;
        }
        let fetched: Vec<Fetched> = util::block_on(async {
            let pins = futures_util::future::join_all(wanted_pins.iter().map(|pin| async move {
                let got = fetch(pin.url).await.filter(|(_, b)| pinned(*pin, b));
                (pin.url.to_string(), got.map(|(_, b)| (pin_kind(*pin), b)))
            }));
            let pictures = futures_util::future::join_all(wanted.iter().map(|url| async move {
                let got = fetch(url).await;
                let kept = got.and_then(|(kind, b)| picture_kind(&kind).map(|k| (k, b)));
                (url.clone(), kept)
            }));
            let (pins, pictures) = futures_util::future::join(pins, pictures).await;
            pins.into_iter().chain(pictures).collect()
        });
        for (url, got) in fetched {
            if let Some((kind, bytes)) = got
                && let Some(file) = self.keep_asset(&url, kind, &bytes)
            {
                local.insert(url, file);
            }
        }
        local
    }

    /// The pages drawn before for these parts in this view, touched, or
    /// `None` when there are none (or one of them has gone).
    pub fn pages_for(&self, parts: &Parts, view: &View) -> Option<Vec<Shot>> {
        let dir = self.pages().join(page_key(parts, view));
        let kept: Vec<Kept> =
            serde_json::from_str(&fs::read_to_string(dir.join("rows.json")).ok()?).ok()?;
        let shots: Vec<Shot> = kept
            .iter()
            .enumerate()
            .map(|(i, k)| Shot {
                file: dir.join(format!("page-{}.png", i + 1)),
                rows: k.rows,
                from: k.from,
            })
            .collect();
        if shots.is_empty() || !shots.iter().all(|s| s.file.is_file()) {
            return None;
        }
        touch(&dir);
        Some(shots)
    }

    /// The pages drawn for these parts in this view kept, unless they are
    /// already: written beside and renamed into place, so a reader never
    /// finds half a set.
    pub fn save_pages(&self, parts: &Parts, view: &View, shots: &[Shot]) {
        let key = page_key(parts, view);
        let dir = self.pages().join(&key);
        if shots.is_empty() || dir.exists() {
            return;
        }
        let part = self
            .pages()
            .join(format!("{key}.{}.part", std::process::id()));
        let saved = (|| -> std::io::Result<()> {
            fs::create_dir_all(&part)?;
            for (i, shot) in shots.iter().enumerate() {
                fs::copy(&shot.file, part.join(format!("page-{}.png", i + 1)))?;
            }
            let kept: Vec<Kept> = shots
                .iter()
                .map(|s| Kept {
                    rows: s.rows,
                    from: s.from,
                })
                .collect();
            fs::write(part.join("rows.json"), util::json_compact(&kept))?;
            fs::rename(&part, &dir)
        })();
        if saved.is_err() {
            let _ = fs::remove_dir_all(&part);
        }
    }

    /// Both caches held to `limit` bytes together, the least recently used
    /// removed first, and page sets older than `PAGES_MAX_AGE` removed
    /// whatever the size; `keep` is the page set on screen, never removed.
    pub fn prune(&self, limit: u64, now: SystemTime, keep: Option<&str>) {
        struct Entry {
            path: PathBuf,
            size: u64,
            time: SystemTime,
            kept: bool,
        }
        let listed = |dir: &Path| -> Vec<(String, PathBuf, fs::Metadata)> {
            let Ok(entries) = fs::read_dir(dir) else {
                return Vec::new();
            };
            entries
                .flatten()
                .filter_map(|e| {
                    let meta = e.metadata().ok()?;
                    Some((e.file_name().to_string_lossy().into_owned(), e.path(), meta))
                })
                .collect()
        };
        let time = |meta: &fs::Metadata| meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        let mut entries = Vec::new();
        for (name, path, meta) in listed(&self.assets()) {
            if meta.is_file() && is_asset_name(&name) {
                entries.push(Entry {
                    path,
                    size: meta.len(),
                    time: time(&meta),
                    kept: false,
                });
            }
        }
        for (name, path, meta) in listed(&self.pages()) {
            if !meta.is_dir() || name.contains('.') {
                continue;
            }
            let kept = keep == Some(name.as_str());
            let at = time(&meta);
            if !kept && now.duration_since(at).is_ok_and(|age| age > PAGES_MAX_AGE) {
                let _ = fs::remove_dir_all(&path);
                continue;
            }
            let size = listed(&path).iter().map(|(_, _, m)| m.len()).sum();
            entries.push(Entry {
                path,
                size,
                time: at,
                kept,
            });
        }
        let mut total: u64 = entries.iter().map(|e| e.size).sum();
        entries.sort_by_key(|e| e.time);
        for entry in entries {
            if total <= limit {
                break;
            }
            if entry.kept {
                continue;
            }
            let removed = if entry.path.is_dir() {
                fs::remove_dir_all(&entry.path)
            } else {
                fs::remove_file(&entry.path)
            };
            if removed.is_ok() {
                total -= entry.size;
            }
        }
    }

    /// Everything the image views keep: the copies, the pages drawn and
    /// the text last read.
    pub fn clear(&self) {
        let _ = fs::remove_dir_all(self.assets());
        let _ = fs::remove_dir_all(self.root.join("pr-image"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Root(PathBuf);

    impl Root {
        fn new(name: &str) -> Root {
            let dir =
                std::env::temp_dir().join(format!("sb-image-cache-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Root(dir)
        }
    }

    impl Drop for Root {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn ago(path: &Path, secs: u64) {
        let file = fs::File::open(path).unwrap();
        file.set_modified(SystemTime::now() - Duration::from_secs(secs))
            .unwrap();
    }

    fn parts(html: &str) -> Parts {
        Parts {
            title: "T".into(),
            number: "#1".into(),
            html: html.into(),
            ..Parts::default()
        }
    }

    /// A page drawn, as the renderer leaves one in its working directory.
    fn shot(dir: &Path, name: &str, body: &str, rows: u16, from: f64) -> Shot {
        let file = dir.join(name);
        fs::write(&file, body).unwrap();
        Shot { file, rows, from }
    }

    #[test]
    fn a_signed_hosts_link_is_kept_by_its_path_and_anothers_by_its_query_too() {
        assert_eq!(
            asset_key("https://private-user-images.githubusercontent.com/1/a.png?jwt=x"),
            asset_key("https://private-user-images.githubusercontent.com/1/a.png?jwt=y")
        );
        assert_eq!(
            asset_key("https://uploads.linear.app/u/1.png?signature=a"),
            asset_key("https://uploads.linear.app/u/1.png")
        );
        assert_ne!(
            asset_key("https://images.example.com/image?id=1"),
            asset_key("https://images.example.com/image?id=2")
        );
        assert_eq!(
            asset_key("https://images.example.com/a.png#top"),
            asset_key("https://images.example.com/a.png")
        );
    }

    #[test]
    fn a_copy_is_found_again_under_its_link_with_a_new_token() {
        let root = Root::new("asset");
        let cache = Cache::at(&root.0);
        assert_eq!(
            cache.asset("https://uploads.linear.app/u/1.png?token=a"),
            None
        );
        let kept = cache
            .keep_asset(
                "https://uploads.linear.app/u/1.png?token=a",
                "png",
                b"picture",
            )
            .unwrap();
        assert!(kept.to_string_lossy().ends_with(".png"));
        let found = cache
            .asset("https://uploads.linear.app/u/1.png?token=b")
            .unwrap();
        assert_eq!(found, kept);
        assert_eq!(fs::read(found).unwrap(), b"picture");
        assert_eq!(cache.asset("https://uploads.linear.app/u/2.png"), None);
    }

    #[test]
    fn a_pinned_file_is_known_by_its_hash_and_a_picture_by_its_type() {
        assert!(!pinned(page::MERMAID, b"not mermaid"));
        assert_eq!(pin_kind(page::STYLESHEET), "css");
        assert_eq!(pin_kind(page::MARKED), "js");
        assert_eq!(picture_kind("image/png"), Some("png"));
        assert_eq!(picture_kind("image/svg+xml; charset=utf-8"), Some("svg"));
        assert_eq!(picture_kind("IMAGE/JPEG"), Some("jpg"));
        assert_eq!(picture_kind("image/tiff"), Some("bin"));
        assert_eq!(picture_kind("text/html; charset=utf-8"), None);
        assert_eq!(picture_kind(""), None);
    }

    /// A stand-in for the web on 127.0.0.1 (crate::fake_http), answering
    /// each path as `answer` says (status, type, body), a 302 sending on to
    /// /landed.png, and keeping what it was asked.
    #[cfg(feature = "net")]
    fn stand_in(answer: fn(&str) -> (u16, &'static str, Vec<u8>)) -> crate::fake_http::Fake {
        use crate::fake_http::{Fake, Reply, Service};
        Fake::start(Service::Web, move |request| {
            let (status, kind, body) = answer(&request.target);
            let reply = Reply::bytes(status, kind, body);
            Some(if status == 302 {
                reply.header("Location", "/landed.png")
            } else {
                reply
            })
        })
    }

    #[cfg(feature = "net")]
    #[test]
    fn what_the_page_loads_is_fetched_once_and_kept_only_when_whole() {
        let web = stand_in(|path| match path {
            "/a.png" | "/landed.png" => (200, "image/png", b"picture".to_vec()),
            "/script.js" => (200, "application/javascript", b"let x = 1;".to_vec()),
            "/sign-in" => (200, "text/html", b"<html>".to_vec()),
            "/moved" => (302, "text/html", Vec::new()),
            _ => (404, "text/plain", b"no".to_vec()),
        });
        let root = Root::new("fetch");
        let cache = Cache::at(&root.0);
        let url = |p: &str| format!("{}{p}", web.url);
        let script = Pin {
            url: Box::leak(url("/script.js").into_boxed_str()),
            integrity: Box::leak(
                format!("sha384-{}", util::base64(&Sha384::digest(b"let x = 1;"))).into_boxed_str(),
            ),
        };
        let changed = Pin {
            url: Box::leak(url("/script.js?v=2").into_boxed_str()),
            integrity: "sha384-AAAA",
        };
        let pictures = [
            url("/a.png"),
            url("/sign-in"),
            url("/gone.png"),
            url("/moved"),
        ];
        let local = cache.local_copies(&[script, changed], &pictures);
        let kept: Vec<&str> = local.keys().map(String::as_str).collect();
        let mut wanted = vec![script.url, &pictures[0], &pictures[3]];
        wanted.sort();
        assert_eq!(
            kept, wanted,
            "a sign-in page, a 404 and a file not as pinned are left out"
        );
        assert_eq!(fs::read(&local[&pictures[3]]).unwrap(), b"picture");
        assert!(
            local[&pictures[3]].starts_with(cache.assets()),
            "a redirected picture is kept under the link asked for"
        );
        assert!(local[script.url].to_string_lossy().ends_with(".js"));
        let first = web.seen().len();
        // Asked again: the copies are used and nothing is fetched but what
        // wasn't kept.
        let again = cache.local_copies(&[script], &pictures[..1]);
        assert_eq!(again.len(), 2);
        assert_eq!(web.seen().len(), first, "nothing fetched again");
    }

    #[test]
    fn text_read_again_with_only_its_signatures_new_is_the_same() {
        let read = |sig: &str| Parts {
            html: format!(
                r#"<img src="https://private-user-images.githubusercontent.com/1/a.png?jwt={sig}&amp;x=1">"#
            ),
            markdown: vec![format!(
                "![](https://uploads.linear.app/a/b.png?signature={sig})"
            )],
            ..parts("")
        };
        assert_eq!(same_text(&read("one")), same_text(&read("two")));
        assert_ne!(
            same_text(&parts(r#"<img src="https://images.example.com/i?id=1">"#)),
            same_text(&parts(r#"<img src="https://images.example.com/i?id=2">"#))
        );
        let view = View::new(80, 24, (16, 32));
        assert_eq!(page_key(&read("one"), &view), page_key(&read("two"), &view));
    }

    #[test]
    fn the_pages_drawn_are_found_again_for_the_same_text_and_size() {
        let root = Root::new("pages");
        let cache = Cache::at(root.0.join("cache"));
        let work = root.0.join("work");
        fs::create_dir_all(&work).unwrap();
        let p = parts("<p>hi</p>");
        let view = View::new(80, 24, (16, 32));
        let shots = vec![
            shot(&work, "page-1-1.png", "one", 10, 0.0),
            shot(&work, "page-1-2.png", "two", 8, 0.5),
        ];
        assert_eq!(cache.pages_for(&p, &view), None);
        cache.save_pages(&p, &view, &shots);
        let found = cache.pages_for(&p, &view).unwrap();
        let read: Vec<(String, u16, f64)> = found
            .iter()
            .map(|s| (fs::read_to_string(&s.file).unwrap(), s.rows, s.from))
            .collect();
        assert_eq!(
            read,
            vec![("one".to_string(), 10, 0.0), ("two".to_string(), 8, 0.5)]
        );
        assert!(found.iter().all(|s| s.file.starts_with(cache.pages())));
        assert_eq!(cache.pages_for(&p, &View::new(100, 24, (16, 32))), None);
        assert_eq!(cache.pages_for(&parts("<p>changed</p>"), &view), None);
        // A set missing a page is no set.
        fs::remove_file(&found[1].file).unwrap();
        assert_eq!(cache.pages_for(&p, &view), None);
        let left: Vec<_> = fs::read_dir(cache.pages()).unwrap().flatten().collect();
        assert_eq!(left.len(), 1, "nothing left half-written beside it");
    }

    #[test]
    fn pruning_removes_the_least_recently_used_first_and_old_page_sets() {
        let root = Root::new("prune");
        let cache = Cache::at(&root.0);
        let asset = |name: &str, secs: u64| {
            let file = cache.keep_asset(name, "png", &[0; 100]).unwrap();
            ago(&file, secs);
            file
        };
        let old = asset("https://example.com/old.png", 3000);
        let middle = asset("https://example.com/middle.png", 2000);
        let new = asset("https://example.com/new.png", 1000);
        let page_set = |name: &str, secs: u64| {
            let dir = cache.pages().join(name);
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("page-1.png"), [0; 10]).unwrap();
            ago(&dir, secs);
            dir
        };
        let recent = page_set("recent", 500);
        let stale = page_set("stale", 31 * 24 * 60 * 60);
        cache.prune(250, SystemTime::now(), None);
        assert!(!old.exists(), "the least recently used goes first");
        assert!(middle.exists() && new.exists());
        assert!(recent.exists());
        assert!(
            !stale.exists(),
            "a page set over 30 days old goes whatever the size"
        );
    }

    #[test]
    fn pruning_keeps_the_pages_on_screen_and_counts_them() {
        let root = Root::new("keep");
        let cache = Cache::at(&root.0);
        let dir = cache.pages().join("showing");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("page-1.png"), [0; 200]).unwrap();
        ago(&dir, 40 * 24 * 60 * 60);
        let picture = cache
            .keep_asset("https://example.com/p.png", "png", &[0; 100])
            .unwrap();
        cache.prune(250, SystemTime::now(), Some("showing"));
        assert!(dir.exists(), "the set on screen stays, however old");
        assert!(!picture.exists(), "and counts toward the limit");
    }

    #[test]
    fn clearing_removes_the_copies_the_pages_and_the_text() {
        let root = Root::new("clear");
        let cache = Cache::at(&root.0);
        cache
            .keep_asset("https://example.com/p.png", "png", b"p")
            .unwrap();
        fs::create_dir_all(cache.pages().join("k")).unwrap();
        fs::write(root.0.join("pr-image/ENG-1.json"), "{}").unwrap();
        fs::create_dir_all(root.0.join("diff-story")).unwrap();
        cache.clear();
        assert!(!cache.assets().exists() && !root.0.join("pr-image").exists());
        assert!(
            root.0.join("diff-story").exists(),
            "nothing else of the cache's"
        );
    }
}
