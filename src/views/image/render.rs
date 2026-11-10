//! The page drawn as pictures the pane's size, ported from
//! tmux/pr-image.js's `layOut`. pr-image.js drove mermaid-cli's headless
//! Chrome through puppeteer; this runs Chrome itself, twice, with no
//! DevTools protocol: once with `--dump-dom` to lay the page out and read
//! back where its blocks start and end (the page writes them into itself,
//! `super::page`), and once with `--screenshot`, the window as tall as the
//! whole page, for one picture of all of it. The pages are cut from that
//! picture here, each ending where a block does, and a diagram taller than
//! a page is drawn smaller to fit one, so it is never cut in two.
//!
//! With more than one page, each says which it is, and that more follow,
//! in a strip of its own above its text, drawn into the picture itself,
//! since a footer below it is easy to miss. Chrome draws every page's strip
//! below the page in the same screenshot, where nothing of the page is, and
//! each is laid over its page's top here, so nothing in the description, a
//! table or code block cut across pages included, is ever under one.
//!
//! The page is laid out as wide as the pane, in CSS pixels, and drawn at
//! the screen's own pixels (twice as many on a Retina screen), so it fills
//! the pane at its own size, sharp. A page so long that its picture would
//! be taller than Chrome draws in one is drawn at half the pixels instead,
//! the same layout, softer. Chrome is used only when it is installed: the
//! view says so plainly when it isn't (`NEEDS_CHROME`).
//!
//! Every run has a profile of its own, thrown away with the view's working
//! directory, and asks for no keychain (`SWITCHES`): a new profile's Chrome
//! otherwise makes a key and stores it in the login keychain as "Chrome Safe
//! Storage", and macOS, with no keychain to offer a process the server
//! started, shows "Keychain Not Found" over whatever is in front.
//!
//! With the image cache on, what the page loads is fetched before Chrome
//! runs and named by its copies (`super::cache`), so neither run reaches
//! the web for it, and the pages are kept once drawn with everything
//! loaded.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::LazyLock;
use std::sync::atomic::{AtomicI32, Ordering};
use std::time::{Duration, Instant, SystemTime};

use image::{GenericImageView, ImageFormat, Rgba, RgbaImage, imageops};
use regex::Regex;
use serde::Deserialize;

use super::cache::{self, Cache};
use super::page::{self, Bands, Local, Parts};

/// How many device pixels to a CSS pixel: two on a Retina screen.
pub const DPR: u32 = 2;
/// The page marker's strip, in CSS pixels, when there are several pages.
pub const BAND: u32 = 30;
/// The tallest picture asked of Chrome, in device pixels.
const TALLEST: f64 = 32_000.0;
/// How long one run of Chrome may take, loading the page's scripts from
/// the web included.
const CHROME_TIMEOUT: Duration = Duration::from_secs(60);
/// How long Chrome lets the page's scripts run before it reads the page.
const SCRIPT_BUDGET_MS: u32 = 15_000;

/// What every run of Chrome is started with. The last two keep it from the
/// computer's keychain, the first on macOS and the second on Linux's secret
/// service: the profile's key stays in memory, and the pages drawn keep no
/// password or cookie that would need one.
const SWITCHES: [&str; 9] = [
    "--headless",
    "--hide-scrollbars",
    "--mute-audio",
    "--no-first-run",
    "--no-default-browser-check",
    "--disable-extensions",
    "--run-all-compositor-stages-before-draw",
    "--use-mock-keychain",
    "--password-store=basic",
];

/// The window a diagram is drawn in, in CSS pixels: as wide as a diagram
/// is drawn at its own size, which the view makes smaller to fit the pane.
const DIAGRAM_WIDTH: u32 = 1200;
const DIAGRAM_HEIGHT: u32 = 3000;
/// The diagrams drawn so far, each in files of its own.
static DIAGRAMS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// A picture cut to what is drawn in it, a few pixels around; `None` when
/// nothing is.
pub fn trimmed(img: &RgbaImage) -> Option<RgbaImage> {
    let (w, h) = img.dimensions();
    let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0, 0);
    for (x, y, px) in img.enumerate_pixels() {
        if px.0[3] > 0 {
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x + 1);
            y1 = y1.max(y + 1);
        }
    }
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    let pad = 4;
    let (x0, y0) = (x0.saturating_sub(pad), y0.saturating_sub(pad));
    let (x1, y1) = ((x1 + pad).min(w), (y1 + pad).min(h));
    Some(img.view(x0, y0, x1 - x0, y1 - y0).to_image())
}

/// Whether anything is drawn on a picture's last row: a diagram taller than
/// the window it was drawn in, cut off there.
pub fn reaches_bottom(img: &RgbaImage) -> bool {
    let h = img.height();
    h > 0 && (0..img.width()).any(|x| img.get_pixel(x, h - 1).0[3] > 0)
}

/// The process groups of the Chromes this process has running, each led
/// by its Chrome, so a view closing can stop them (`stop_all`): slots of
/// atomics rather than a locked set, since a signal handler reads them as
/// the workspace hangs a view's pane up (`stop_on_hangup`). A Chrome past
/// the slots, which no view starts, isn't tracked.
static RUNNING: [AtomicI32; 8] = [const { AtomicI32::new(0) }; 8];

fn track(pid: i32) {
    let _ = RUNNING.iter().find(|slot| {
        slot.compare_exchange(0, pid, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
    });
}

fn untrack(pid: i32) {
    for slot in &RUNNING {
        let _ = slot.compare_exchange(pid, 0, Ordering::SeqCst, Ordering::SeqCst);
    }
}

/// Every Chrome this process still has running stopped, with what it
/// started, as a view closes, so none goes on drawing for a view that is
/// gone. Safe in a signal handler: atomics and `kill` alone.
pub fn stop_all() {
    for slot in &RUNNING {
        let pid = slot.swap(0, Ordering::SeqCst);
        if pid > 0 {
            // SAFETY: a signal to a group a Chrome this process started leads.
            unsafe {
                libc::kill(-pid, libc::SIGKILL);
            }
        }
    }
}

extern "C" fn on_hangup(signal: libc::c_int) {
    stop_all();
    // SAFETY: the signal's own action put back and the signal sent again,
    // so the process ends as it would have; both are async-signal-safe.
    unsafe {
        libc::signal(signal, libc::SIG_DFL);
        libc::raise(signal);
    }
}

/// Stops this process's Chromes when the workspace hangs its pane up or
/// ends it (SIGHUP, SIGTERM), as closing a view with its key does, before
/// the process ends as the signal says; Chrome runs in a group of its own,
/// which the hangup doesn't reach.
pub fn stop_on_hangup() {
    for signal in [libc::SIGHUP, libc::SIGTERM] {
        // SAFETY: a handler that only reads atomics and sends signals.
        unsafe {
            libc::signal(signal, on_hangup as *const () as libc::sighandler_t);
        }
    }
}

/// What the view says when there is no Chrome to draw with.
pub const NEEDS_CHROME: &str = "The image view needs Google Chrome or Chromium to draw the page, and neither is installed. M-v shows the description as text.";

/// What asked, for the request audit.
const SOURCE: &str = "PR as an image: page";

// ── Finding Chrome ──────────────────────────────────────────────────────────

/// Chrome's programs by name, on the PATH, as src/links.rs runs it on Linux.
const NAMES: [&str; 4] = [
    "google-chrome",
    "google-chrome-stable",
    "chromium",
    "chromium-browser",
];

/// Where macOS keeps Chrome and Chromium's programs, under `/` and your
/// home: `open -a` finds them there, but headless Chrome is run as a
/// program, from inside its app.
const APPS: [&str; 2] = [
    "Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
    "Applications/Chromium.app/Contents/MacOS/Chromium",
];

/// The first Chrome among the apps, then the names in `path`'s
/// directories.
pub fn chrome_in(path: Option<&OsStr>, apps: &[PathBuf]) -> Option<PathBuf> {
    if let Some(app) = apps.iter().find(|a| a.is_file()) {
        return Some(app.clone());
    }
    let dirs: Vec<PathBuf> = path
        .map(|p| std::env::split_paths(p).collect())
        .unwrap_or_default();
    NAMES
        .iter()
        .find_map(|name| dirs.iter().map(|d| d.join(name)).find(|f| is_program(f)))
}

fn is_program(file: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(file).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// Chrome on this computer, if any.
pub fn find_chrome() -> Option<PathBuf> {
    let apps: Vec<PathBuf> = if cfg!(target_os = "macos") {
        APPS.iter()
            .flat_map(|a| [Path::new("/").join(a), crate::paths::home().join(a)])
            .collect()
    } else {
        Vec::new()
    };
    chrome_in(std::env::var_os("PATH").as_deref(), &apps)
}

// ── Pages ───────────────────────────────────────────────────────────────────

/// The pane as the pages are laid out for it: its columns and the rows
/// above the footer, as many as kitty's placeholders can mark, and the page
/// in CSS pixels, as wide as those columns and as tall as one page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct View {
    pub cols: u16,
    pub rows: u16,
    pub width: u32,
    pub page_height: u32,
}

impl View {
    /// For a pane of `cols` by `rows` cells of `cell` device pixels, a row
    /// kept for the footer. No wider or taller than the marks go, since a
    /// cell past the last has no mark to say where it is, and that part of
    /// the picture would never show.
    pub fn new(cols: u16, rows: u16, cell: (u32, u32)) -> View {
        let most = crate::notes::images::MARKS.len() as u16;
        let cols = cols.clamp(1, most);
        let rows = rows.saturating_sub(1).clamp(1, most);
        View {
            cols,
            rows,
            width: (cols as u32 * cell.0 / DPR).max(480),
            page_height: (rows as u32 * cell.1 / DPR).max(100),
        }
    }
}

/// The tallest a diagram is drawn: what a page holds below the marker's
/// strip, so it stays whole however many pages there turn out to be.
pub fn fit(view: &View) -> u32 {
    view.page_height.saturating_sub(BAND + 40).max(40)
}

/// One page, drawn: its picture, how many rows it takes, and where it
/// starts as a share of the whole, to find it again in the next layout.
#[derive(Clone, Debug, PartialEq)]
pub struct Shot {
    pub file: PathBuf,
    pub rows: u16,
    pub from: f64,
}

/// The pages drawn, whether the page's scripts loaded, and whether
/// everything it loads did (the scripts, the stylesheet and every
/// picture), without which the pages aren't kept.
#[derive(Clone, Debug, PartialEq)]
pub struct Drawing {
    pub shots: Vec<Shot>,
    pub loaded: bool,
    pub complete: bool,
}

/// Where the page was cut: the starts of each page, then the end of the
/// last. Each page ends at the last block that starts within `room` of its
/// start, or `room` on when no block does.
pub fn cut_at(height: f64, blocks: &[(f64, f64)], room: f64) -> Vec<f64> {
    let room = room.max(1.0);
    let mut cuts = vec![0.0];
    while let Some(&start) = cuts.last()
        && start + room < height
    {
        let next = blocks
            .iter()
            .map(|b| b.0)
            .filter(|top| *top > start && *top <= start + room)
            .fold(None, |most: Option<f64>, top| {
                Some(most.map_or(top, |m| m.max(top)))
            });
        cuts.push(next.unwrap_or(start + room));
    }
    cuts.push(height);
    cuts
}

/// The cuts for a page `page_height` tall: one page if it fits, else pages
/// a strip shorter, to make room for the strip that says which is which.
pub fn cuts_for(height: f64, blocks: &[(f64, f64)], page_height: f64) -> Vec<f64> {
    let cuts = cut_at(height, blocks, page_height);
    if cuts.len() > 2 {
        cut_at(height, blocks, page_height - BAND as f64)
    } else {
        cuts
    }
}

/// What the page wrote about itself once drawn.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Measured {
    /// Where the body starts on the page, in CSS pixels.
    pub top: f64,
    pub height: f64,
    /// Each block of the body's top and bottom, from the body's top.
    pub blocks: Vec<(f64, f64)>,
    /// Whether the page's scripts loaded.
    pub loaded: bool,
    /// Whether the stylesheet and every picture did.
    pub complete: bool,
}

static MEASURED: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"<pre id="measured"[^>]*>([^<]*)</pre>"#).unwrap());

/// The measurements in the page Chrome printed, once its scripts ran.
pub fn measured(dom: &str) -> Option<Measured> {
    let text = MEASURED.captures(dom)?.get(1)?.as_str();
    let json = text
        .replace("&quot;", "\"")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&");
    serde_json::from_str(&json).ok()
}

/// The pages cut from Chrome's one picture of the whole page, `scale`
/// device pixels to a CSS pixel: each page's part, with its strip laid
/// over its top when there are several, as a PNG named for the layout `n`
/// in `dir`. Gives each page's file and height in CSS pixels.
pub fn compose(
    picture: &Path,
    scale: f64,
    body_top: f64,
    cuts: &[f64],
    bands_top: f64,
    dir: &Path,
    n: u64,
) -> Result<Vec<(PathBuf, f64)>, String> {
    let whole = image::open(picture).map_err(|e| format!("Chrome's picture of the page: {e}"))?;
    let (width, tall) = whole.dimensions();
    let px = |css: f64| ((css * scale).round().max(0.0) as u32).min(tall);
    let total = cuts.len().saturating_sub(1);
    let band = if total > 1 { BAND as f64 } else { 0.0 };
    let [r, g, b] = page::BACKGROUND;
    let mut out = Vec::new();
    for i in 0..total {
        let (from, to) = (px(body_top + cuts[i]), px(body_top + cuts[i + 1]));
        let strip = px(band);
        let mut canvas =
            RgbaImage::from_pixel(width, (to - from + strip).max(1), Rgba([r, g, b, 255]));
        if strip > 0 {
            let at = px(bands_top + (i as f64) * band);
            let shown = strip.min(tall - at);
            imageops::replace(
                &mut canvas,
                &whole.view(0, at, width, shown).to_image(),
                0,
                0,
            );
        }
        if to > from {
            imageops::replace(
                &mut canvas,
                &whole.view(0, from, width, to - from).to_image(),
                0,
                strip as i64,
            );
        }
        let file = dir.join(format!("page-{n}-{}.png", i + 1));
        canvas
            .save_with_format(&file, ImageFormat::Png)
            .map_err(|e| format!("couldn't save a page: {e}"))?;
        out.push((file, cuts[i + 1] - cuts[i] + band));
    }
    Ok(out)
}

// ── Chrome ──────────────────────────────────────────────────────────────────

/// Draws the page with Chrome into a working directory of the view's own,
/// keeping what it can in `cache` while the image cache is on. Without
/// Chrome, every drawing fails with `NEEDS_CHROME`, and only pages kept
/// before can be shown.
#[derive(Clone, Debug)]
pub struct Renderer {
    pub chrome: Option<PathBuf>,
    pub work: PathBuf,
    pub cache: Option<Cache>,
}

impl Renderer {
    /// Runs Chrome on the page in `file`, the window `width` by `height`
    /// CSS pixels, with `what` (`--dump-dom`, or `--screenshot=<file>`,
    /// and any other flags);
    /// gives what it printed. Its own profile in the working directory, so
    /// it never touches yours, nor waits on a Chrome you have open, and no
    /// keychain asked for (`SWITCHES`). `cdn`
    /// is how many of the pinned files it loads from the CDN, for the
    /// request audit.
    fn chrome(
        &self,
        file: &Path,
        width: u32,
        height: u32,
        scale: f64,
        what: &[String],
        cdn: usize,
    ) -> Result<String, String> {
        let program = self.chrome.as_ref().ok_or(NEEDS_CHROME)?;
        if cdn > 0 {
            crate::requests::count_n("cdn.jsdelivr.net", "HTTPS", SOURCE, cdn);
        }
        let mut cmd = Command::new(program);
        cmd.args(SWITCHES)
            .arg(format!(
                "--user-data-dir={}",
                self.work.join("chrome").display()
            ))
            .arg(format!("--force-device-scale-factor={scale}"))
            .arg(format!("--window-size={width},{height}"))
            .arg(format!("--virtual-time-budget={SCRIPT_BUDGET_MS}"))
            .args(what)
            .arg(format!("file://{}", file.display()))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        {
            use std::os::unix::process::CommandExt;
            // A group of its own, so its helpers go with it if it hangs.
            cmd.process_group(0);
        }
        let mut child = cmd
            .spawn()
            .map_err(|e| format!("couldn't start Chrome ({}): {e}", program.display()))?;
        let pid = child.id() as i32;
        track(pid);
        let mut stdout = child.stdout.take();
        let reader = std::thread::spawn(move || {
            let mut text = String::new();
            if let Some(out) = stdout.as_mut() {
                use std::io::Read;
                let _ = out.read_to_string(&mut text);
            }
            text
        });
        let deadline = Instant::now() + CHROME_TIMEOUT;
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(50));
                }
                _ => {
                    // SAFETY: a signal to the group Chrome leads, which this
                    // started.
                    unsafe {
                        libc::kill(-pid, libc::SIGKILL);
                    }
                    let _ = child.wait();
                    untrack(pid);
                    return Err("Chrome took too long to draw the page.".into());
                }
            }
        }
        untrack(pid);
        Ok(reader.join().unwrap_or_default())
    }

    /// The page laid out for `view` and cut into pages, as layout `n`'s
    /// files, and how much of what it loads loaded. With the cache, what
    /// the page loads is had first, and the pages kept when all of it did.
    pub fn render(&self, parts: &Parts, view: &View, n: u64) -> Result<Drawing, String> {
        if self.chrome.is_none() {
            return Err(NEEDS_CHROME.into());
        }
        let _ = std::fs::create_dir_all(&self.work);
        let file = self.work.join(format!("page-{n}.html"));
        let local = match &self.cache {
            Some(cache) => cache.local_copies(&page::PINS, &page::pictures(parts)),
            None => Local::new(),
        };
        let cdn = page::PINS
            .iter()
            .filter(|p| !local.contains_key(p.url))
            .count();
        let write = |bands: &Bands| {
            std::fs::write(
                &file,
                page::html(parts, view.width, fit(view), BAND, bands, &local),
            )
            .map_err(|e| format!("couldn't write the page: {e}"))
        };
        write(&Bands::default())?;
        let dom = self.chrome(
            &file,
            view.width,
            800,
            DPR as f64,
            &["--dump-dom".into()],
            cdn,
        )?;
        let m = measured(&dom).ok_or("Chrome couldn't draw the page.")?;
        let cuts = cuts_for(m.height, &m.blocks, view.page_height as f64);
        let total = cuts.len() - 1;
        let bands = Bands {
            top: (m.top + m.height).ceil(),
            labels: if total > 1 {
                (1..=total).map(|i| page::band_label(i, total)).collect()
            } else {
                Vec::new()
            },
        };
        write(&bands)?;
        let tall = (bands.top + (bands.labels.len() as u32 * BAND) as f64).ceil();
        let scale = if tall * DPR as f64 <= TALLEST {
            DPR as f64
        } else {
            1.0
        };
        let tall = tall.min(TALLEST / scale) as u32;
        let picture = self.work.join(format!("page-{n}.png"));
        self.chrome(
            &file,
            view.width,
            tall,
            scale,
            &[format!("--screenshot={}", picture.display())],
            cdn,
        )?;
        let pages = compose(&picture, scale, m.top, &cuts, bands.top, &self.work, n);
        let _ = std::fs::remove_file(&picture);
        let _ = std::fs::remove_file(&file);
        let shots = pages?
            .into_iter()
            .zip(&cuts)
            .map(|((file, h), from)| Shot {
                file,
                rows: ((h / view.page_height as f64) * view.rows as f64)
                    .round()
                    .clamp(1.0, view.rows as f64) as u16,
                from: if m.height > 0.0 { from / m.height } else { 0.0 },
            })
            .collect();
        let drawing = Drawing {
            shots,
            loaded: m.loaded,
            complete: m.loaded && m.complete,
        };
        if let Some(cache) = &self.cache {
            if drawing.complete {
                cache.save_pages(parts, view, &drawing.shots);
            }
            // Held to the limit again with what this drawing added, its
            // pages kept.
            let key = cache::page_key(parts, view);
            cache.prune(cache::LIMIT, SystemTime::now(), Some(&key));
        }
        Ok(drawing)
    }

    /// A Mermaid diagram drawn by Mermaid itself, as GitHub draws it, in
    /// the dark theme or the light one, at twice the CSS pixels: Chrome
    /// draws it on nothing (`--default-background-color` clear), in a window
    /// `DIAGRAM_WIDTH` wide, and the picture is cut to what it drew, so a
    /// diagram that can't be drawn (Mermaid not loaded, or a source it
    /// refuses) is an error rather than a picture of nothing. Mermaid comes
    /// from the cache, where the image cache keeps it, else the CDN.
    pub fn diagram(&self, source: &str, light: bool) -> Result<RgbaImage, String> {
        if self.chrome.is_none() {
            return Err(NEEDS_CHROME.into());
        }
        let _ = std::fs::create_dir_all(&self.work);
        let local = match &self.cache {
            Some(cache) => cache.local_copies(&[page::MERMAID], &[]),
            None => Local::new(),
        };
        let cdn = usize::from(!local.contains_key(page::MERMAID.url));
        let n = DIAGRAMS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let file = self.work.join(format!("diagram-{n}.html"));
        let picture = self.work.join(format!("diagram-{n}.png"));
        crate::util::write_atomic(&file, page::diagram_html(source, light, &local))
            .map_err(|e| format!("couldn't write the diagram's page: {e}"))?;
        let ran = self.chrome(
            &file,
            DIAGRAM_WIDTH,
            DIAGRAM_HEIGHT,
            DPR as f64,
            &[
                "--default-background-color=00000000".into(),
                format!("--screenshot={}", picture.display()),
            ],
            cdn,
        );
        let drawn = ran.and_then(|_| {
            image::open(&picture)
                .map(|i| i.to_rgba8())
                .map_err(|e| format!("Chrome's picture of the diagram: {e}"))
        });
        let _ = std::fs::remove_file(&file);
        let _ = std::fs::remove_file(&picture);
        let drawn = drawn?;
        // One taller than the window is cut off: its text stays instead.
        if reaches_bottom(&drawn) {
            return Err("The diagram is too tall to draw whole.".into());
        }
        trimmed(&drawn).ok_or_else(|| "Mermaid couldn't draw the diagram.".into())
    }

    /// Every page file but layout `n`'s, which the terminal has read
    /// already or never will.
    pub fn forget_others(&self, n: u64) {
        let keep = format!("page-{n}-");
        if let Ok(entries) = std::fs::read_dir(&self.work) {
            for e in entries.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                if name.starts_with("page-") && name.ends_with(".png") && !name.starts_with(&keep) {
                    let _ = std::fs::remove_file(e.path());
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_diagram_drawn_to_the_last_row_is_cut_off() {
        let mut img = RgbaImage::new(4, 4);
        img.put_pixel(1, 1, image::Rgba([0, 0, 0, 255]));
        assert!(!reaches_bottom(&img), "drawn above the last row");
        img.put_pixel(2, 3, image::Rgba([0, 0, 0, 255]));
        assert!(reaches_bottom(&img), "drawn on the last row");
    }

    #[test]
    fn pages_break_between_blocks_and_a_tall_block_is_cut_where_it_must() {
        let blocks = [
            (0.0, 40.0),
            (50.0, 90.0),
            (100.0, 180.0),
            (190.0, 400.0),
            (410.0, 430.0),
        ];
        assert_eq!(cut_at(430.0, &blocks, 1000.0), vec![0.0, 430.0], "one page");
        assert_eq!(
            cut_at(430.0, &blocks, 120.0),
            vec![0.0, 100.0, 190.0, 310.0, 430.0],
            "each page ends where the last block starting in it does, a block taller than a page cut a page on"
        );
        assert_eq!(
            cuts_for(430.0, &blocks, 150.0),
            cut_at(430.0, &blocks, 120.0),
            "several pages leave room for their strips"
        );
        assert_eq!(cuts_for(100.0, &blocks, 150.0), vec![0.0, 100.0]);
    }

    #[test]
    fn the_view_fits_the_pane_and_the_placeholders_marks() {
        let v = View::new(100, 41, (16, 32));
        assert_eq!(
            (v.cols, v.rows, v.width, v.page_height),
            (100, 40, 800, 640)
        );
        let narrow = View::new(20, 10, (16, 32));
        assert_eq!((narrow.width, narrow.page_height), (480, 144));
        let huge = View::new(1000, 1000, (16, 32));
        assert_eq!(huge.cols as usize, crate::notes::images::MARKS.len());
    }

    #[test]
    fn the_measurements_are_read_from_the_page_chrome_printed() {
        let dom = r#"<html><body><pre id="measured" hidden="">{"top":0,"height":407.5,"blocks":[[0,37],[58.4,76.4]],"loaded":true,"complete":false}</pre></body></html>"#;
        assert_eq!(
            measured(dom),
            Some(Measured {
                top: 0.0,
                height: 407.5,
                blocks: vec![(0.0, 37.0), (58.4, 76.4)],
                loaded: true,
                complete: false,
            })
        );
        assert_eq!(measured(r#"<pre id="measured" hidden=""></pre>"#), None);
    }

    #[test]
    fn each_page_is_cut_from_the_picture_with_its_strip_over_it() {
        let dir = std::env::temp_dir().join(format!("sb-pr-image-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // A page 100 CSS pixels wide: body 0..300, red then green then blue
        // a hundred each, and two strips below it, white then grey, drawn
        // at two device pixels to one.
        let scale = 2.0;
        let mut whole = RgbaImage::new(200, (300 + 2 * BAND) * 2);
        for (_, y, p) in whole.enumerate_pixels_mut() {
            *p = match y / 2 {
                0..100 => Rgba([255, 0, 0, 255]),
                100..200 => Rgba([0, 255, 0, 255]),
                200..300 => Rgba([0, 0, 255, 255]),
                y if y < 300 + BAND => Rgba([255, 255, 255, 255]),
                _ => Rgba([128, 128, 128, 255]),
            };
        }
        let picture = dir.join("whole.png");
        whole.save(&picture).unwrap();
        let cuts = vec![0.0, 200.0, 300.0];
        let pages = compose(&picture, scale, 0.0, &cuts, 300.0, &dir, 7).unwrap();
        assert_eq!(pages.len(), 2);
        assert_eq!(pages[0].1, 200.0 + BAND as f64);
        let first = image::open(&pages[0].0).unwrap().to_rgba8();
        assert_eq!(first.dimensions(), (200, (200 + BAND) * 2));
        assert_eq!(
            first.get_pixel(5, 1),
            &Rgba([255, 255, 255, 255]),
            "its strip on top"
        );
        assert_eq!(
            first.get_pixel(5, BAND * 2 + 1),
            &Rgba([255, 0, 0, 255]),
            "then its part"
        );
        assert_eq!(
            first.get_pixel(5, (BAND + 199) * 2),
            &Rgba([0, 255, 0, 255])
        );
        let second = image::open(&pages[1].0).unwrap().to_rgba8();
        assert_eq!(
            second.get_pixel(5, 1),
            &Rgba([128, 128, 128, 255]),
            "the second strip"
        );
        assert_eq!(second.get_pixel(5, BAND * 2 + 1), &Rgba([0, 0, 255, 255]));
        assert!(pages[0].0.ends_with("page-7-1.png"));
        let one = compose(&picture, scale, 0.0, &[0.0, 300.0], 300.0, &dir, 8).unwrap();
        let only = image::open(&one[0].0).unwrap().to_rgba8();
        assert_eq!(only.dimensions(), (200, 600), "one page has no strip");
        let renderer = Renderer {
            chrome: None,
            work: dir.clone(),
            cache: None,
        };
        renderer.forget_others(8);
        assert_eq!(
            renderer.render(&Parts::default(), &View::new(80, 24, (16, 32)), 9),
            Err(NEEDS_CHROME.to_string()),
            "no Chrome, no drawing"
        );
        assert!(!pages[0].0.exists() && one[0].0.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn chrome_is_started_asking_for_no_keychain() {
        assert!(SWITCHES.contains(&"--headless"));
        assert!(SWITCHES.contains(&"--use-mock-keychain"));
        assert!(SWITCHES.contains(&"--password-store=basic"));
    }

    #[test]
    fn with_no_chrome_there_is_none_to_draw_with() {
        let dir = std::env::temp_dir().join(format!("sb-no-chrome-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(
            chrome_in(Some(dir.as_os_str()), &[dir.join("Chrome.app")]),
            None
        );
        assert_eq!(chrome_in(None, &[]), None);
        let fake = dir.join("chromium");
        std::fs::write(&fake, "#!/bin/sh\n").unwrap();
        assert_eq!(
            chrome_in(Some(dir.as_os_str()), &[]),
            None,
            "not a program until it can run"
        );
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(chrome_in(Some(dir.as_os_str()), &[]), Some(fake));
        assert!(NEEDS_CHROME.contains("needs Google Chrome or Chromium"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_chrome_tracked_is_stopped_with_its_group() {
        use std::os::unix::process::CommandExt;
        let mut child = Command::new("sleep")
            .arg("30")
            .process_group(0)
            .spawn()
            .unwrap();
        let pid = child.id() as i32;
        track(pid);
        stop_all();
        let status = child.wait().unwrap();
        assert!(!status.success(), "killed");
        assert!(RUNNING.iter().all(|s| s.load(Ordering::SeqCst) != pid));
    }
}
