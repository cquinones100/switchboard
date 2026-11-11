//! The pictures and Mermaid diagrams the Markdown views (the PR, ticket,
//! link and file views, `super::pager`) draw as images, where the terminal
//! speaks kitty's graphics protocol (Ghostty, kitty) and Settings' "Pictures
//! and diagrams in the views" (`view_images`) is on; anywhere else each stays
//! the text `super::markdown` draws it as.
//!
//! A picture on a line of its own is fetched from its link (counted in the
//! request audit by src/net.rs), GitHub's attachments with your GitHub token
//! and Linear's uploads with your Linear key, each sent to that host alone
//! (checked on the link as the HTTP client parses it), and no picture on
//! GitHub or Linear fetched while that integration is off, four at a time
//! and none over 20 MB; no picture is fetched for a file of yours or a note
//! (`pager::run_local`), whose links are your words and stay on the
//! computer; a diagram is drawn by Mermaid in
//! Chrome, as the image views draw a PR's (`super::image::render`). Both are
//! kept in the image views' cache while Settings' `image_cache` is on, a
//! picture under its link and a diagram under its source and theme, so a
//! view opened again shows them at once, the cache held to its limit as a
//! view starts. Until one is had, and for one that
//! can't be (a picture that isn't one, an SVG, a diagram Mermaid refuses),
//! the text stays. Mission control's boards draw the diagrams of the
//! description beside them the same way, on the server (`draw_diagrams`,
//! src/mission/diagrams.rs).
//!
//! Each is drawn as Images' picture atom (`ui::images::raster_picture`, in
//! a rounded border on the sunken surface) and sent with its data inline,
//! never a path, through Images' `ImageCache`, so an image is sent once and
//! again only when its size changes. The view's pane is marked
//! `@allow_passthrough` while it runs (src/mux/passthrough.rs), and every
//! line the view writes leaves control characters out
//! (`markdown::Line::ansi`), so nothing in a PR or ticket can send the
//! terminal a command of its own.

use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use image::RgbaImage;

use super::image::cache::{self, Cache};
use super::image::render::{self, Renderer};
use super::markdown::{Images, Placement, Wanted};
use crate::ui::images::{ImageCache, Placed, Spot, Tones};
use crate::{net, photos, settings, util};

/// What a photo's key starts with (`markdown::photo_key`).
const PHOTO: &str = "photo:";

/// What asked, for the request audit.
const SOURCE: &str = "Markdown view: pictures";
/// How long one picture may take to arrive.
const FETCH_TIMEOUT: Duration = Duration::from_secs(30);
/// How many pictures are fetched at once.
const WORKERS: usize = 4;
/// The largest picture fetched, refused as it arrives; the most pixels one
/// decoded may have, and the most memory decoding it may take, so with its
/// RGBA copy it stays under 256 MB.
const LARGEST: usize = 20 * 1024 * 1024;
const MOST_PIXELS: u64 = 16 * 1024 * 1024;
const DECODE_LIMIT: u64 = 128 * 1024 * 1024;
/// The longest side a picture is kept at, larger than a pane shows one,
/// and the most memory a view's pictures may hold together; one past it
/// stays its text.
const LONGEST: u32 = 2048;
const HELD: usize = 256 * 1024 * 1024;
/// How long after a photo failed to load the view asks for it again, as
/// the server's Photos does (src/photos/serve.rs's `RETRY_MS`).
const PHOTO_RETRY: Duration = Duration::from_secs(15 * 60);

/// Whether the views draw pictures here: the terminal can and the setting
/// is on, and not in a pane while Switchboard draws pictures as sixels,
/// whose receiver takes only a picture given by its path, never the inline
/// ones the views send, so each would be a blank where its text was.
pub fn on() -> bool {
    draws(
        settings::is_on("view_images"),
        crate::notes::images::terminal_can(),
        crate::mux::socket::in_pane() && crate::mux::sixel::chosen_now(),
    )
}

fn draws(setting: bool, terminal_can: bool, sixels_in_pane: bool) -> bool {
    setting && terminal_can && !sixels_in_pane
}

/// An image's id: the top bit of its 24 clear, as a note's are, and the
/// rest from the view's process and the picture's key, so two views open at
/// once don't take each other's pictures. Never 0.
pub fn id_for(pid: u32, key: &str) -> u32 {
    let mut h: u32 = 2_166_136_261;
    for b in pid.to_le_bytes().iter().chain(key.as_bytes()) {
        h = (h ^ *b as u32).wrapping_mul(16_777_619);
    }
    (h & 0x7f_ffff).max(1)
}

/// A picture's link as the request for it goes, parsed by the parser the
/// HTTP client uses, so the host checked is the host reached: https, with
/// a host and no user name or password.
fn parsed(url: &str) -> Option<url::Url> {
    let u = url::Url::parse(url).ok()?;
    (u.scheme() == "https"
        && u.host_str().is_some()
        && u.username().is_empty()
        && u.password().is_none())
    .then_some(u)
}

/// The integration whose service a host belongs to, whose setting decides
/// whether the view reaches it: GitHub's hosts and Linear's.
pub(crate) fn integration_of(host: &str) -> Option<&'static str> {
    if host == "github.com"
        || host.ends_with(".github.com")
        || host.ends_with(".githubusercontent.com")
    {
        Some("github")
    } else if host == "linear.app" || host.ends_with(".linear.app") {
        Some("linear")
    } else {
        None
    }
}

/// Whether a host may be reached for a picture, a redirect's included:
/// any but one of an integration turned off.
pub(crate) fn reachable(host: &str) -> bool {
    integration_of(host).is_none_or(settings::is_on)
}

/// The header that signs a request for `url` in, if any: your GitHub token
/// for an attachment on github.com, your Linear key for an upload on
/// uploads.linear.app. Only called while that integration is on.
async fn sign_in(url: &url::Url) -> Option<(&'static str, String)> {
    let host = url.host_str()?;
    if host == "github.com" && url.path().starts_with("/user-attachments/") {
        let token = crate::integrations::github::token().await.ok()?;
        return Some(("Authorization", format!("Bearer {token}")));
    }
    if host == "uploads.linear.app" {
        let key = crate::integrations::linear::api_key().await.ok()?;
        return Some(("Authorization", key));
    }
    None
}

/// The picture `url` names, from the cache or fetched (and kept there),
/// or why it can't be had. A picture on GitHub or Linear isn't fetched
/// while that integration is off.
pub fn picture(url: &str, kept: Option<&Cache>) -> Result<RgbaImage, String> {
    if let Some(file) = kept.and_then(|c| c.asset(url))
        && let Some(img) = std::fs::read(&file).ok().and_then(|b| decode(&b).ok())
    {
        return Ok(img);
    }
    let link = parsed(url).ok_or("not an https link")?;
    let host = link.host_str().unwrap_or_default().to_string();
    if !reachable(&host) {
        return Err(format!("{host} belongs to an integration turned off"));
    }
    let got = crate::util::block_on(async {
        let header = sign_in(&link).await;
        let headers: Vec<(&str, &str)> = header.iter().map(|(n, v)| (*n, v.as_str())).collect();
        let audit = net::Audit {
            service: &host,
            protocol: "HTTPS",
            source: SOURCE,
        };
        net::get_file_following(
            link.as_str(),
            &headers,
            FETCH_TIMEOUT,
            audit,
            reachable,
            LARGEST,
        )
        .await
    })
    .map_err(|e| e.to_string())?;
    if got.status != 200 {
        return Err(format!("{host} answered {}", got.status));
    }
    let kind =
        cache::picture_kind(got.content_type.as_deref().unwrap_or("")).ok_or("not a picture")?;
    let img = decode(&got.bytes)?;
    if let Some(c) = kept {
        c.keep_asset(url, kind, &got.bytes);
    }
    Ok(img)
}

/// A picture's bytes decoded as RGBA: one over `MOST_PIXELS` refused from
/// its header before anything is decoded, the decoding held to
/// `DECODE_LIMIT`, and the decoded image turned into RGBA in place where it
/// is RGBA already.
fn decode(bytes: &[u8]) -> Result<RgbaImage, String> {
    let reader = || {
        image::ImageReader::new(std::io::Cursor::new(bytes))
            .with_guessed_format()
            .map_err(|e| e.to_string())
    };
    let (w, h) = reader()?.into_dimensions().map_err(|e| e.to_string())?;
    if w as u64 * h as u64 > MOST_PIXELS {
        return Err("more pixels than a picture the view shows".into());
    }
    let mut reader = reader()?;
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(DECODE_LIMIT);
    reader.limits(limits);
    Ok(reader.decode().map_err(|e| e.to_string())?.into_rgba8())
}

/// The bytes a view's pictures hold together, reserved by the worker that
/// decodes one before it hands it over, so pictures on their way count too.
type Held = Arc<std::sync::atomic::AtomicUsize>;

/// A picture with its bytes reserved, or refused when they would take the
/// view's pictures past `HELD`.
fn reserve(held: &Held, img: RgbaImage) -> Result<RgbaImage, String> {
    use std::sync::atomic::Ordering;
    let size = img.as_raw().len();
    let before = held.fetch_add(size, Ordering::Relaxed);
    if before + size > HELD {
        held.fetch_sub(size, Ordering::Relaxed);
        return Err("past the memory the view's pictures may hold".into());
    }
    Ok(img)
}

/// The working folders in `temp` of views no longer running, removed: a
/// view closed by its key ends from a signal, before it can remove its
/// own. Only a folder of yours (not a link) named for a view whose process
/// is gone.
fn remove_dead_dirs(temp: &std::path::Path) {
    use std::os::unix::fs::MetadataExt;
    let Ok(entries) = std::fs::read_dir(temp) else {
        return;
    };
    // SAFETY: geteuid has no preconditions.
    let me = unsafe { libc::geteuid() };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(pid) = name
            .strip_prefix("switchboard-view-")
            .and_then(|rest| rest.split_once('-'))
            .and_then(|(pid, _)| pid.parse::<i32>().ok())
        else {
            continue;
        };
        let Ok(meta) = std::fs::symlink_metadata(entry.path()) else {
            continue;
        };
        // SAFETY: signal 0 only asks whether the process is there.
        let gone = unsafe { libc::kill(pid, 0) } != 0
            && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH);
        if meta.is_dir() && meta.uid() == me && gone {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

/// A folder of the view's own for Chrome's work, made fresh under the temp
/// folder with a name no one can guess and only you can open, refusing one
/// that is there already, so nothing another account made first is written
/// through.
fn private_dir(pid: u32) -> PathBuf {
    use std::os::unix::fs::DirBuilderExt;
    let mut dir = std::env::temp_dir().join(format!("switchboard-view-{pid}"));
    for attempt in 0u32..16 {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (pid, attempt, nanos, &dir).hash(&mut h);
        dir = std::env::temp_dir().join(format!("switchboard-view-{pid}-{:016x}", h.finish()));
        if std::fs::DirBuilder::new().mode(0o700).create(&dir).is_ok() {
            break;
        }
    }
    dir
}

/// A picture's name as a file's, which the full-size view shows as its
/// title: no slashes, dots at its start or control characters, and no
/// longer than `NAME_BYTES`, well inside the 255 bytes a file's name may
/// take with its extension.
const NAME_BYTES: usize = 200;

fn file_name(title: &str) -> String {
    let name: String = title
        .chars()
        .map(|c| if c == '/' || c.is_control() { ' ' } else { c })
        .collect::<String>()
        .trim()
        .trim_start_matches('.')
        .chars()
        .scan(0, |bytes, c| {
            *bytes += c.len_utf8();
            (*bytes <= NAME_BYTES).then_some(c)
        })
        .collect();
    if name.trim().is_empty() {
        "Picture".into()
    } else {
        name.trim().to_string()
    }
}

/// The image cache held to its limit, the least recently used going first.
fn prune(kept: Option<&Cache>) {
    if let Some(c) = kept {
        c.prune(cache::LIMIT, std::time::SystemTime::now(), None);
    }
}

/// A picture made no longer than `LONGEST` on its longest side, its shape
/// kept.
fn shrunk(img: RgbaImage) -> RgbaImage {
    let (w, h) = img.dimensions();
    let longest = w.max(h);
    if longest <= LONGEST {
        return img;
    }
    let scale = LONGEST as f64 / longest as f64;
    let (sw, sh) = (
        ((w as f64 * scale) as u32).max(1),
        ((h as f64 * scale) as u32).max(1),
    );
    image::imageops::resize(&img, sw, sh, image::imageops::FilterType::Triangle)
}

/// The pictures waiting to be fetched, and how many workers fetch them.
#[derive(Default)]
struct Queue {
    links: VecDeque<String>,
    workers: usize,
}

/// One of the workers: fetches the pictures waiting until there are none,
/// or the view has closed; the last to stop holds the cache to its limit
/// again, with what they added.
fn fetch_all(queue: Arc<Mutex<Queue>>, tx: Sender<Got>, kept: Option<Cache>, held: Held) {
    loop {
        let next = {
            let mut q = queue.lock().unwrap_or_else(|e| e.into_inner());
            let next = q.links.pop_front();
            if next.is_none() {
                q.workers -= 1;
            }
            (next, q.workers == 0)
        };
        let (Some(url), _) = next else {
            if next.1 {
                prune(kept.as_ref());
            }
            return;
        };
        let got = picture(&url, kept.as_ref()).and_then(|img| reserve(&held, shrunk(img)));
        if tx.send((url, got)).is_err() {
            queue.lock().unwrap_or_else(|e| e.into_inner()).workers -= 1;
            return;
        }
    }
}

/// The link a diagram is kept under in the cache: its source's hash and
/// its theme, under a version that changes when diagrams are drawn another
/// way (`v2`: at their own size), so the ones kept before are drawn again.
pub fn diagram_key(source: &str, light: bool) -> String {
    use sha2::{Digest, Sha256};
    let hash: String = Sha256::digest(source.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let theme = if light { "light" } else { "dark" };
    format!("mermaid-v2:{hash}:{theme}")
}

/// The diagram drawn, from the cache or by Mermaid in Chrome (and kept).
pub fn diagram(source: &str, light: bool, renderer: &Renderer) -> Result<RgbaImage, String> {
    let key = diagram_key(source, light);
    let kept = renderer.cache.as_ref();
    if let Some(img) = kept
        .and_then(|c| c.asset(&key))
        .and_then(|f| image::open(f).ok())
    {
        return Ok(img.to_rgba8());
    }
    let img = renderer.diagram(source, light)?;
    if let Some(c) = kept {
        let mut png = Vec::new();
        if img
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .is_ok()
        {
            c.keep_asset(&key, "png", &png);
        }
    }
    Ok(img)
}

/// Each of `sources` drawn in turn, for what draws diagrams without a view
/// of its own (mission control's boards, on the server): from the image
/// cache while Settings' `image_cache` is on, else by Mermaid in Chrome in
/// a folder made for the run and removed after it, each handed to `got` as
/// it is had, and the cache held to its limit at the end. It reads files
/// and runs Chrome, so never on the server's own task.
pub fn draw_diagrams(
    sources: &[String],
    light: bool,
    mut got: impl FnMut(&str, Result<RgbaImage, String>),
) {
    let renderer = Renderer {
        chrome: render::find_chrome(),
        work: private_dir(std::process::id()),
        cache: settings::is_on("image_cache").then(Cache::here),
    };
    for source in sources {
        got(source, diagram(source, light, &renderer).map(shrunk));
    }
    let _ = std::fs::remove_dir_all(&renderer.work);
    prune(renderer.cache.as_ref());
}

/// A picture had, or why not, by its key.
pub type Got = (String, Result<RgbaImage, String>);

/// The pictures of one view: those ready, those asked for, what the
/// terminal holds, and where the work off the view's thread answers.
pub struct Pictures {
    pid: u32,
    cell: (u32, u32),
    tones: Tones,
    light: bool,
    renderer: Renderer,
    images: HashMap<String, Arc<RgbaImage>>,
    /// The bytes the pictures hold together, kept and on their way.
    held: Held,
    asked: HashSet<String>,
    /// Each photo that failed to load and when, asked for again once
    /// `PHOTO_RETRY` has passed, while the view stays open.
    failed: HashMap<String, Instant>,
    sent: ImageCache,
    tx: Sender<Got>,
    /// Whether pictures are fetched: not for a file of yours or a note,
    /// whose links are your words and stay on the computer.
    fetch: bool,
    queue: Arc<Mutex<Queue>>,
}

impl Pictures {
    /// The pictures of a view; `fetch` says whether its pictures are
    /// fetched, its diagrams being drawn either way. The image cache is held
    /// to its limit as it starts, off the view's thread.
    pub fn new(tx: Sender<Got>, fetch: bool) -> Pictures {
        let pid = std::process::id();
        let light = crate::status::theme::light();
        let kept = settings::is_on("image_cache").then(Cache::here);
        std::thread::spawn(|| remove_dead_dirs(&std::env::temp_dir()));
        if let Some(c) = kept.clone() {
            std::thread::spawn(move || prune(Some(&c)));
        }
        Pictures {
            pid,
            cell: crate::notes::images::cell_size(),
            tones: Tones::now(),
            light,
            renderer: Renderer {
                chrome: render::find_chrome(),
                work: private_dir(pid),
                cache: kept,
            },
            images: HashMap::new(),
            held: Held::default(),
            asked: HashSet::new(),
            failed: HashMap::new(),
            sent: ImageCache::default(),
            tx,
            fetch,
            queue: Arc::new(Mutex::new(Queue::default())),
        }
    }

    /// Asks for each picture and diagram not asked for yet, off the view's
    /// thread: the pictures by `WORKERS` at once, the diagrams one after
    /// another, since each is a run of Chrome.
    pub fn ask(&mut self, wanted: Vec<Wanted>) {
        let mut diagrams = Vec::new();
        for w in wanted {
            if matches!(w, Wanted::Picture(_) | Wanted::Photo(_)) && !self.fetch {
                continue;
            }
            if matches!(w, Wanted::Photo(_)) && !settings::is_on("photos") {
                continue;
            }
            if !self.asked.insert(w.key()) {
                continue;
            }
            match w {
                Wanted::Picture(url) => {
                    let mut q = self.queue.lock().unwrap_or_else(|e| e.into_inner());
                    q.links.push_back(url);
                    while q.workers < WORKERS && q.workers < q.links.len() {
                        q.workers += 1;
                        let (queue, tx, kept, held) = (
                            self.queue.clone(),
                            self.tx.clone(),
                            self.renderer.cache.clone(),
                            self.held.clone(),
                        );
                        std::thread::spawn(move || fetch_all(queue, tx, kept, held));
                    }
                }
                Wanted::Diagram(source) => diagrams.push(source),
                // A photo from the cache Photos keeps (src/photos), fetched
                // and kept there when it isn't, its request counted there.
                Wanted::Photo(who) => {
                    let tx = self.tx.clone();
                    std::thread::spawn(move || {
                        let key = Wanted::Photo(who.clone()).key();
                        let got = util::block_on(photos::load(&photos::Store::here(), &who))
                            .map_err(|e| e.to_string())
                            .and_then(|l| l.photo.ok_or_else(|| "no photo".to_string()));
                        let _ = tx.send((key, got));
                    });
                }
            }
        }
        if diagrams.is_empty() {
            return;
        }
        let (tx, renderer, light, held) = (
            self.tx.clone(),
            self.renderer.clone(),
            self.light,
            self.held.clone(),
        );
        std::thread::spawn(move || {
            for source in diagrams {
                let got =
                    diagram(&source, light, &renderer).and_then(|img| reserve(&held, shrunk(img)));
                let key = Wanted::Diagram(source).key();
                if tx.send((key, got)).is_err() {
                    break;
                }
            }
            prune(renderer.cache.as_ref());
        });
    }

    /// A picture had: kept to draw, its bytes reserved already, or, when it
    /// couldn't be had or was past `HELD`, its text left in its place, a
    /// photo's asked for again after `PHOTO_RETRY` (`retry_failed`).
    /// Whether the layout changes.
    pub fn got(&mut self, (key, got): Got) -> bool {
        match got {
            Ok(img) => {
                self.images.insert(key, Arc::new(img));
                true
            }
            Err(_) => {
                if key.starts_with(PHOTO) {
                    self.failed.insert(key, Instant::now());
                }
                false
            }
        }
    }

    /// Forgets having asked for each photo that failed `PHOTO_RETRY` before
    /// `now` or longer, so the next `ask` asks for it again. Whether any was.
    pub fn retry_failed(&mut self, now: Instant) -> bool {
        let before = self.failed.len();
        let asked = &mut self.asked;
        self.failed.retain(|key, at| {
            let due = now.saturating_duration_since(*at) >= PHOTO_RETRY;
            if due {
                asked.remove(key);
            }
            !due
        });
        self.failed.len() != before
    }

    /// What the layout draws as pictures, in a pane `rows` tall.
    pub fn layout(&mut self, rows: usize) -> Images {
        self.cell = crate::notes::images::cell_size();
        Images {
            cell: self.cell,
            rows,
            ready: self
                .images
                .iter()
                .map(|(key, img)| {
                    (
                        key.clone(),
                        (id_for(self.pid, key), (img.width(), img.height())),
                    )
                })
                .collect(),
        }
    }

    /// The graphics commands that give the terminal the pictures `placed`
    /// (a document's, and a board's panel's) and take back those it no
    /// longer places: a photo cropped to a circle, as Images' avatar, any
    /// other in its frame.
    pub fn commands(&mut self, placed: &[Placement]) -> Vec<u8> {
        let cell = (
            self.cell.0.min(u16::MAX as u32) as u16,
            self.cell.1.min(u16::MAX as u32) as u16,
        );
        let tones = self.tones;
        let placed: Vec<Placed> = placed
            .iter()
            .filter_map(|p| {
                let img = self.images.get(&p.key)?.clone();
                let mut h = std::collections::hash_map::DefaultHasher::new();
                (&p.key, p.cols, p.rows, cell, tones).hash(&mut h);
                let (cols, rows) = (p.cols, p.rows);
                Some(Placed {
                    id: p.id,
                    cols,
                    rows,
                    spot: Spot::Placeholders,
                    key: h.finish(),
                    draw: if p.key.starts_with(PHOTO) {
                        Box::new(move || crate::ui::images::raster_avatar(&img, cols, rows, cell))
                    } else {
                        Box::new(move || {
                            crate::ui::images::raster_picture(&img, cols, rows, cell, &tones)
                        })
                    },
                })
            })
            .collect();
        self.sent.commands(&placed)
    }

    /// The picture or diagram `key` names shown full size over the view's
    /// pane by the workspace (`switchboard picture`, src/ui/picture.rs),
    /// written as a PNG named `title` into the view's own folder for it,
    /// off the view's thread; where the workspace can't show it (no
    /// Switchboard, Settings' `server_images` or `picture_full_size`
    /// off), opened in Quick Look or the desktop's viewer, as a note's
    /// picture is. Only a picture the view draws can be chosen, so a
    /// terminal that draws none never gets here.
    pub fn show_full(&self, key: &str, title: &str) {
        let Some(img) = self.images.get(key).cloned() else {
            return;
        };
        let file = self.renderer.work.join(format!("{}.png", file_name(title)));
        std::thread::spawn(move || {
            let mut png = Vec::new();
            let encoded =
                img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png);
            if encoded.is_err() || crate::util::write_atomic(&file, &png).is_err() {
                return;
            }
            let shown_in_place = crate::mux::socket::in_pane()
                && settings::is_on("picture_full_size")
                && file
                    .to_str()
                    .is_some_and(|f| crate::mux::socket::ask_ok(&["picture", f]).is_ok());
            if !shown_in_place {
                let _ = crate::platform::preview(&file);
            }
        });
    }

    /// Every picture taken back, as the view closes, and any Chrome still
    /// drawing a diagram for it stopped.
    pub fn forget(&mut self) -> Vec<u8> {
        render::stop_all();
        let _ = std::fs::remove_dir_all(&self.renderer.work);
        self.sent.forget()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A view's pictures with nothing spawned and nothing read from home.
    fn bare() -> Pictures {
        Pictures {
            pid: 1,
            cell: (10, 20),
            tones: Tones::now(),
            light: false,
            renderer: Renderer {
                chrome: None,
                work: PathBuf::from("/nonexistent"),
                cache: None,
            },
            images: HashMap::new(),
            held: Held::default(),
            asked: HashSet::new(),
            failed: HashMap::new(),
            sent: ImageCache::default(),
            tx: std::sync::mpsc::channel().0,
            fetch: false,
            queue: Arc::new(Mutex::new(Queue::default())),
        }
    }

    #[test]
    fn a_photo_that_failed_is_asked_for_again_once_the_retry_has_passed() {
        let mut p = bare();
        let photo = format!("{PHOTO}github:someone");
        let picture = "https://x/a.png".to_string();
        p.asked.insert(photo.clone());
        p.asked.insert(picture.clone());
        assert!(!p.got((photo.clone(), Err("offline".into()))));
        assert!(!p.got((picture.clone(), Err("offline".into()))));
        let failed_at = p.failed[&photo];
        assert!(!p.failed.contains_key(&picture), "only photos are retried");

        assert!(!p.retry_failed(failed_at + PHOTO_RETRY / 2));
        assert!(p.asked.contains(&photo), "not before the retry is due");

        assert!(p.retry_failed(failed_at + PHOTO_RETRY));
        assert!(!p.asked.contains(&photo), "the next ask asks for it again");
        assert!(p.asked.contains(&picture));
        assert!(p.failed.is_empty());
        assert!(!p.retry_failed(failed_at + PHOTO_RETRY * 2));
    }

    #[test]
    fn the_views_keep_their_text_where_switchboard_draws_sixels() {
        assert!(draws(true, true, false));
        assert!(!draws(true, true, true), "sixels in a pane: the text stays");
        assert!(!draws(false, true, false));
        assert!(!draws(true, false, false));
    }

    #[test]
    fn an_id_is_in_the_lower_half_never_zero_and_the_views_own() {
        let a = id_for(10, "https://x/a.png");
        assert!(a > 0 && a < crate::ui::images::SERVER_IDS);
        assert_ne!(a, id_for(11, "https://x/a.png"));
        assert_ne!(a, id_for(10, "https://x/b.png"));
    }

    #[test]
    fn a_diagram_is_kept_by_its_source_and_theme() {
        assert_ne!(
            diagram_key("graph TD; A-->B", true),
            diagram_key("graph TD; A-->B", false)
        );
        assert_ne!(
            diagram_key("graph TD; A-->B", true),
            diagram_key("graph TD; A-->C", true)
        );
        assert!(!diagram_key("graph TD; A#B", true).contains('#'));
        assert!(
            diagram_key("graph TD; A-->B", true).starts_with("mermaid-v2:"),
            "not the key the diagrams drawn small were kept under"
        );
    }

    #[test]
    fn a_link_is_checked_as_the_request_will_go() {
        // A backslash before the @ is a path for the fetch, so the host is
        // the one before it, never the one after.
        let tricked = parsed("https://attacker.example\\@uploads.linear.app/a.png");
        assert_eq!(
            tricked
                .and_then(|u| u.host_str().map(String::from))
                .as_deref(),
            Some("attacker.example")
        );
        assert!(parsed("https://me:pw@uploads.linear.app/a.png").is_none());
        assert!(parsed("http://uploads.linear.app/a.png").is_none());
        assert_eq!(integration_of("uploads.linear.app"), Some("linear"));
        assert_eq!(
            integration_of("private-user-images.githubusercontent.com"),
            Some("github")
        );
        assert_eq!(integration_of("github.com"), Some("github"));
        assert_eq!(integration_of("api.github.com"), Some("github"));
        assert_eq!(integration_of("uploads.linear.app.example.com"), None);
        let none = |url: &str| crate::util::block_on(sign_in(&parsed(url).unwrap())).is_none();
        assert!(none("https://example.com/user-attachments/x.png"));
        assert!(none("https://github.com/o/r/raw/main/x.png"));
        assert!(none(
            "https://attacker.example\\@github.com/user-attachments/a.png"
        ));
    }

    #[test]
    fn a_picture_with_too_many_pixels_is_refused_before_decoding() {
        let png = |w: u32, h: u32| {
            let mut out = Vec::new();
            image::GrayImage::new(w, h)
                .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
                .unwrap();
            out
        };
        assert_eq!(decode(&png(3, 2)).unwrap().dimensions(), (3, 2));
        assert!(decode(&png(5000, 4000)).is_err());
        assert!(decode(b"not a picture").is_err());
    }

    #[cfg(feature = "net")]
    #[test]
    fn a_download_stops_at_its_limit_and_at_a_redirect_not_taken() {
        use crate::fake_http::{Fake, Reply, Service};
        let web = Fake::start(Service::Web, |request| {
            Some(match request.target.as_str() {
                "/moved" => Reply::bytes(302, "text/html", Vec::new()).header("Location", "/big"),
                _ => Reply::bytes(200, "image/png", vec![0; 1000]),
            })
        });
        let audit = net::Audit {
            service: "test",
            protocol: "HTTPS",
            source: "test",
        };
        let get = |path: &str, largest: usize, follow: fn(&str) -> bool| {
            crate::util::block_on(net::get_file_following(
                &format!("{}{path}", web.url),
                &[],
                Duration::from_secs(5),
                audit,
                follow,
                largest,
            ))
        };
        assert_eq!(get("/big", 1000, |_| true).unwrap().bytes.len(), 1000);
        assert!(get("/big", 999, |_| true).is_err());
        assert_eq!(get("/moved", 1000, |_| false).unwrap().status, 302);
    }

    #[test]
    fn a_pictures_title_is_a_safe_file_name() {
        assert_eq!(file_name("Login screen"), "Login screen");
        assert_eq!(file_name("../../etc/passwd"), ".. etc passwd");
        assert_eq!(file_name("a\x1b[2Jb"), "a [2Jb");
        assert_eq!(file_name(".."), "Picture");
        assert_eq!(file_name(&"x".repeat(300)).len(), NAME_BYTES);
        assert!(file_name(&"写".repeat(100)).len() <= NAME_BYTES);
    }

    #[test]
    fn a_large_picture_is_kept_smaller_its_shape_kept() {
        assert_eq!(shrunk(RgbaImage::new(4096, 1024)).dimensions(), (2048, 512));
        assert_eq!(shrunk(RgbaImage::new(300, 200)).dimensions(), (300, 200));
    }

    #[test]
    fn pictures_past_the_budget_are_refused_as_they_are_reserved() {
        let held = Held::default();
        let side = 4096u32;
        let mut kept = 0;
        while reserve(&held, RgbaImage::new(side, side)).is_ok() {
            kept += 1;
        }
        assert_eq!(kept, HELD / (side as usize * side as usize * 4));
        assert!(held.load(std::sync::atomic::Ordering::Relaxed) <= HELD);
    }

    #[test]
    fn chromes_folder_is_made_fresh_and_yours_alone() {
        use std::os::unix::fs::PermissionsExt;
        let (a, b) = (private_dir(1), private_dir(1));
        assert_ne!(a, b);
        for dir in [&a, &b] {
            let mode = std::fs::metadata(dir).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o700);
            std::fs::remove_dir(dir).unwrap();
        }
    }

    #[test]
    fn the_folders_of_views_gone_are_removed_and_a_running_ones_kept() {
        let temp = std::env::temp_dir().join(format!("sb-dead-dirs-{}", std::process::id()));
        let dead = temp.join("switchboard-view-2147483646-00ff");
        let alive = temp.join(format!("switchboard-view-{}-00ff", std::process::id()));
        let other = temp.join("switchboard-view-notapid-00ff");
        for d in [&dead, &alive, &other] {
            std::fs::create_dir_all(d.join("chrome")).unwrap();
        }
        remove_dead_dirs(&temp);
        assert!(!dead.exists());
        assert!(alive.exists() && other.exists());
        std::fs::remove_dir_all(&temp).unwrap();
    }
}
