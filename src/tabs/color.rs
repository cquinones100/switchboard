//! A directory's accent color and the name its tab takes, as
//! tmux/accent-color.sh worked them out, and the same color as
//! claude/statusline-command.sh draws it, so the tab and the Claude
//! statusline agree on a project's color.
//!
//! A tab's hue is its own (Settings' `tab_colors_apart`): chosen as the tab
//! opens (`apart`), the one farthest from the open tabs' hues, and kept on
//! the tab (`@tab_hue`, `@tab_step`) with the layout, so it lasts through
//! restarts and upgrades and never changes while the tab is open. The
//! directory's hash is only the tie-break among the farthest hues, the hue
//! of a lone tab, the color with the setting off, and the statusline's
//! outside the workspace; the key stays byte for byte, since that hue still
//! hangs on it.
//!
//! A hue is drawn in one of two lightness steps (`Step`): light, as it
//! always was, or deep, darker, so two tabs whose hues are close still tell
//! apart. A tab whose free hue is within `NEAR` of an open tab's takes the
//! other step from that tab's; with the setting off, the step is the next
//! bit of the key's hash after the hue's. A hue whose deep step would read
//! as red keeps the light one.
//!
//! The color key and the name are separate strings. The key is hashed, so it
//! has to stay byte for byte what the scripts built, which is why it spells
//! out `worktree:` and repeats the directory's name; nobody reads it. The
//! name is only read, so it says each thing once, and is free to change.
//!
//! The hash is SHA-256, as `shasum -a 256` gave it, written out here since
//! the color needs only this one use of it: the first eight hex digits of
//! the key's hash, as a number, picks the hue from those whose color doesn't
//! read as red (the scripts took any of the 360). Settings' `tab_colors`
//! picks how that hue is drawn, never the key or the hue: soft, a pastel at
//! about 65% saturation and 78% lightness (deep: 62%) as the UX 2.0 canvas
//! draws its tabs, the same for the tab and the statusline; or vivid, the
//! scripts' color at about 80% saturation and 45% lightness (deep: about
//! 33%) in the same integer steps the shell's arithmetic took, the tab's
//! then lifted to a perceived luminance of at least 100 for reading on a
//! dark status line, the statusline's not, as it never was.

use std::path::Path;
use std::sync::LazyLock;

use super::git::GitPlace;

/// A color as three channels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    pub const fn new(r: u8, g: u8, b: u8) -> Rgb {
        Rgb { r, g, b }
    }

    /// Its perceived luminance, 0 to 255.
    pub fn luminance(self) -> u8 {
        let (r, g, b) = (self.r as u32, self.g as u32, self.b as u32);
        ((299 * r + 587 * g + 114 * b) / 1000) as u8
    }

    /// The same hue at 55% of each channel, for words on a light background.
    pub fn darker(self) -> Rgb {
        let dim = |c: u8| (c as u16 * 55 / 100) as u8;
        Rgb::new(dim(self.r), dim(self.g), dim(self.b))
    }

    /// Light enough that words on it are drawn dark rather than bright:
    /// every soft color, and the lightest vivid ones.
    pub fn takes_dark_words(self) -> bool {
        self.luminance() > 140
    }

    /// `23ef59`, as accent-color.sh printed it.
    pub fn hex(self) -> String {
        format!("{:02x}{:02x}{:02x}", self.r, self.g, self.b)
    }
}

/// The color of words on a fill of a tab's color, whatever the theme: dark
/// on a light color (every soft one), light on a dark vivid one; a color
/// that isn't RGB keeps the dark words.
pub fn words_on(fill: ratatui::style::Color) -> ratatui::style::Color {
    let light = match fill {
        ratatui::style::Color::Rgb(r, g, b) => !Rgb::new(r, g, b).takes_dark_words(),
        _ => false,
    };
    ratatui::style::Color::Indexed(if light { 255 } else { 232 })
}

/// A tab's color as words on the light theme's pale backgrounds, where a
/// soft color (or a light vivid one) would barely show: the same hue at 55%
/// of each channel. Any other color, or on the dark theme, as it is.
pub fn as_words(color: ratatui::style::Color, light: bool) -> ratatui::style::Color {
    match color {
        ratatui::style::Color::Rgb(r, g, b) if light => Rgb::new(r, g, b).darker().into(),
        other => other,
    }
}

impl From<Rgb> for ratatui::style::Color {
    fn from(c: Rgb) -> Self {
        ratatui::style::Color::Rgb(c.r, c.g, c.b)
    }
}

/// How a project's hue is drawn: Settings' `tab_colors`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Shade {
    /// A pastel, as the UX 2.0 canvas draws its tabs.
    #[default]
    Soft,
    /// The stronger colors from before.
    Vivid,
}

impl Shade {
    /// The shade a `tab_colors` value names: soft unless it says vivid.
    pub fn of(value: Option<&serde_json::Value>) -> Shade {
        match value.and_then(|v| v.as_str()) {
            Some("vivid") => Shade::Vivid,
            _ => Shade::Soft,
        }
    }

    /// The shade the settings file names now.
    pub fn read() -> Shade {
        Shade::of(crate::settings::setting("tab_colors").as_ref())
    }
}

/// The one gray every tab wears with the `accent_colors` setting off. Only
/// the tabs follow the setting; the statusline keeps its colors.
pub const GRAY: Rgb = Rgb::new(0x8a, 0x8a, 0x8a);

/// The last part of a directory's path, as `basename` gives it (`/` for the
/// root).
pub fn dir_name(dir: &Path) -> String {
    dir.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "/".into())
}

/// The string hashed into the color: the directory's name, then for a
/// linked worktree ` | worktree:<its name>`, else on any branch
/// ` | branch:<branch>`.
pub fn color_key(dir_name: &str, place: Option<&GitPlace>) -> String {
    match place {
        None => dir_name.to_string(),
        Some(p) => match p.worktree() {
            Some(worktree) => format!("{dir_name} | worktree:{worktree}"),
            None => format!("{dir_name} | branch:{}", p.branch),
        },
    }
}

/// The name a tab takes from its directory (`accent-color.sh --name`): a
/// worktree's name, else the directory's, with the branch after a colon
/// unless it is the trunk, where a main clone usually sits, so naming it
/// costs width and says nothing.
pub fn window_name(dir_name: &str, place: Option<&GitPlace>) -> String {
    match place {
        None => dir_name.to_string(),
        Some(p) => match p.worktree() {
            Some(worktree) => worktree,
            None if p.branch == "main" || p.branch == "master" => dir_name.to_string(),
            None => format!("{dir_name}:{}", p.branch),
        },
    }
}

/// The color key for a directory, read from its git files: what the Claude
/// statusline hashes for its working directory.
pub fn color_key_of(dir: &Path) -> String {
    color_key(&dir_name(dir), super::git::locate(dir).as_ref())
}

/// The hues a key can hash to: those of the 360 whose color doesn't read as
/// red, since red is what a failing build and an error wear. The shell's
/// steps run each 60° segment out and back, so red is at both ends of the
/// two segments either side of 0° (0°, 60°, 300°, 360°), not only around 0°;
/// what is left is 302 hues, orange (15° to 45°) and magenta (315° to 345°)
/// among them.
static HUES: LazyLock<Vec<i64>> =
    LazyLock::new(|| (0..360).filter(|h| !reads_as_red(hue_color(*h))).collect());

/// Red, its other two channels left too low to make it orange or pink.
fn reads_as_red(c: Rgb) -> bool {
    c.r > c.g.max(c.b) && c.g.max(c.b) < 135
}

/// One of a hue's two lightness steps.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Step {
    /// As a hue was always drawn.
    #[default]
    Light,
    /// Darker, for a tab whose hue is close to an open tab's.
    Deep,
}

impl Step {
    /// The value kept in `@tab_step`.
    pub fn name(self) -> &'static str {
        match self {
            Step::Light => "light",
            Step::Deep => "deep",
        }
    }

    /// The step a `@tab_step` value names: light unless it says deep.
    pub fn of(value: &str) -> Step {
        match value.trim() {
            "deep" => Step::Deep,
            _ => Step::Light,
        }
    }

    fn other(self) -> Step {
        match self {
            Step::Light => Step::Deep,
            Step::Deep => Step::Light,
        }
    }
}

/// A hue and the step it is drawn in: what a tab keeps of its color, Settings'
/// `tab_colors` deciding how it is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Tint {
    pub hue: i64,
    pub step: Step,
}

impl Tint {
    /// A hue in a step, the deep step falling back to the light one where it
    /// would read as red.
    pub fn new(hue: i64, step: Step) -> Tint {
        let step = if step == Step::Deep && !deep_allowed(hue) {
            Step::Light
        } else {
            step
        };
        Tint { hue, step }
    }

    /// The tint a tab's options keep (`@tab_hue`, `@tab_step`); none
    /// without a hue that a key could hash to.
    pub fn of_opts(opts: &std::collections::BTreeMap<String, String>) -> Option<Tint> {
        let hue = opts.get(HUE)?.trim().parse::<i64>().ok()?;
        HUES.contains(&hue).then(|| {
            Tint::new(
                hue,
                Step::of(opts.get(STEP).map(String::as_str).unwrap_or("")),
            )
        })
    }
}

/// The tab options its tint is kept in, saved with the layout.
pub const HUE: &str = "@tab_hue";
pub const STEP: &str = "@tab_step";

/// How close two hues are, in degrees, for a new tab to take the other step
/// from the open tab's.
pub const NEAR: i64 = 20;

/// Whether a hue's deep step reads as other than red, soft and vivid, the
/// tab's lifted color too.
fn deep_allowed(hue: i64) -> bool {
    let tint = Tint {
        hue,
        step: Step::Deep,
    };
    [Shade::Soft, Shade::Vivid].into_iter().all(|shade| {
        !reads_as_red(line_color(tint, shade)) && !reads_as_red(tab_color(tint, shade))
    })
}

/// The tint a key hashes to: the hue from the hash's first four bytes, the
/// step from the next bit.
pub fn tint_of(key: &str) -> Tint {
    let digest = sha256(key.as_bytes());
    let hash = u32::from_be_bytes([digest[0], digest[1], digest[2], digest[3]]) as usize;
    let step = if digest[4] & 1 == 1 {
        Step::Deep
    } else {
        Step::Light
    };
    Tint::new(HUES[hash % HUES.len()], step)
}

/// How far apart the colors two hues are drawn in are around the circle, in
/// degrees: the shell's steps run each 60° segment out and back, so 315° and
/// 345° draw one color, and the hue numbers alone would set them apart.
fn distance(a: i64, b: i64) -> i64 {
    let d = (drawn_hue(a) - drawn_hue(b)).rem_euclid(360);
    d.min(360 - d)
}

/// The hue a hue's color is drawn in (`hsl_steps`), in degrees: each
/// segment covers its own 60°, there and back.
fn drawn_hue(hue: i64) -> i64 {
    let segment = hue / 60;
    let frac = (hue % 60) * 255 / 60;
    let c = 204;
    let x = c * (255 - (frac * 2 - 255).abs()) / 255;
    let along = 60 * x / c;
    segment * 60 + if segment % 2 == 0 { along } else { 60 - along }
}

/// The tint for a tab opening while tabs of the `taken` tints are open: the
/// hue farthest from all of theirs, the tie going to the one nearest its
/// directory's (`home`), never a red one; in the other step from the
/// nearest open tab's when that is within `NEAR`, else in the light one.
/// A lone tab wears its directory's tint.
pub fn apart(taken: &[Tint], home: Tint) -> Tint {
    if taken.is_empty() {
        return home;
    }
    let nearest = |h: i64| {
        taken
            .iter()
            .min_by_key(|t| distance(h, t.hue))
            .expect("not empty")
    };
    let hue = HUES
        .iter()
        .copied()
        .max_by_key(|h| {
            (
                distance(*h, nearest(*h).hue),
                std::cmp::Reverse(distance(*h, home.hue)),
                std::cmp::Reverse(*h),
            )
        })
        .expect("hues");
    let close = nearest(hue);
    let step = if distance(hue, close.hue) <= NEAR {
        close.step.other()
    } else {
        Step::Light
    };
    Tint::new(hue, step)
}

/// A tint as the Claude statusline draws it.
pub fn line_color(tint: Tint, shade: Shade) -> Rgb {
    let (c, m) = match (shade, tint.step) {
        (Shade::Soft, Step::Light) => (73, 162),
        (Shade::Soft, Step::Deep) => (126, 95),
        (Shade::Vivid, Step::Light) => (204, 35),
        (Shade::Vivid, Step::Deep) => (160, 22),
    };
    hsl_steps(tint.hue, c, m)
}

/// A tint as the tab wears it: the statusline's, a vivid one lifted to a
/// perceived luminance of at least 100 (a soft one is far above it).
pub fn tab_color(tint: Tint, shade: Shade) -> Rgb {
    let color = line_color(tint, shade);
    let lum = color.luminance() as i64;
    if lum >= 100 {
        return color;
    }
    let boost = 100 - lum;
    let Rgb { r, g, b } = color;
    let (r, g, b) = (r as i64, g as i64, b as i64);
    Rgb::new(channel(r + boost), channel(g + boost), channel(b + boost))
}

/// The color a key hashes to, as the Claude statusline draws it.
pub fn statusline_color(key: &str, shade: Shade) -> Rgb {
    line_color(tint_of(key), shade)
}

/// A hue made a pastel at about 65% saturation and 78% lightness, in the
/// same integer steps as `hue_color`.
#[cfg(test)]
fn soft_color(hue: i64) -> Rgb {
    hsl_steps(hue, 73, 162)
}

/// A hue made a color at about 80% saturation and 45% lightness, in the
/// shell's integer steps.
fn hue_color(hue: i64) -> Rgb {
    hsl_steps(hue, 204, 35)
}

/// A hue as the shell's integer steps drew it, with chroma `c` and the
/// floor `m` every channel starts from.
fn hsl_steps(hue: i64, c: i64, m: i64) -> Rgb {
    let segment = hue / 60;
    let frac = (hue % 60) * 255 / 60;
    let x = c * (255 - (frac * 2 - 255).abs()) / 255;
    let (r, g, b) = match segment {
        0 => (c + m, x + m, m),
        1 => (x + m, c + m, m),
        2 => (m, c + m, x + m),
        3 => (m, x + m, c + m),
        4 => (x + m, m, c + m),
        _ => (c + m, m, x + m),
    };
    Rgb::new(channel(r), channel(g), channel(b))
}

/// The color a key hashes to, as the tab wears it.
pub fn accent_color(key: &str, shade: Shade) -> Rgb {
    tab_color(tint_of(key), shade)
}

/// The tab's color for a directory, read from its git files.
pub fn accent_color_of(dir: &Path, shade: Shade) -> Rgb {
    accent_color(&color_key_of(dir), shade)
}

fn channel(v: i64) -> u8 {
    v.clamp(0, 255) as u8
}

// ── SHA-256 ─────────────────────────────────────────────────────────────────

const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

/// SHA-256 of `data` (FIPS 180-4), for the color key's hash.
pub fn sha256(data: &[u8]) -> [u8; 32] {
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let mut message = data.to_vec();
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&((data.len() as u64).wrapping_mul(8)).to_be_bytes());
    for block in message.as_chunks::<64>().0 {
        let mut w = [0u32; 64];
        for (i, word) in block.as_chunks::<4>().0.iter().enumerate() {
            w[i] = u32::from_be_bytes(*word);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = h;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ (!e & g);
            let t1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        for (slot, v) in h.iter_mut().zip([a, b, c, d, e, f, g, hh]) {
            *slot = slot.wrapping_add(v);
        }
    }
    let mut out = [0u8; 32];
    for (i, word) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::super::git::tests::{Scratch, git, repo};
    use super::*;

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn sha256_matches_the_standard_vectors() {
        assert_eq!(
            hex(&sha256(b"")),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            hex(&sha256(b"abc")),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        // A message whose padding needs a second block.
        assert_eq!(
            hex(&sha256(
                b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"
            )),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
        assert_eq!(
            hex(&sha256(&[b'a'; 1000])),
            "41edece42d63e8d9bf515a9ba6932e1c20cbc9f5a5d134645adb5db1b9737ea3"
        );
    }

    #[test]
    fn keys_and_names() {
        let place = |branch: &str, worktree: bool| GitPlace {
            branch: branch.into(),
            git_dir: if worktree {
                "/r/.git/worktrees/eng-7".into()
            } else {
                "/r/.git".into()
            },
            common_dir: "/r/.git".into(),
        };
        assert_eq!(color_key("notes", None), "notes");
        assert_eq!(window_name("notes", None), "notes");
        let main = place("main", false);
        assert_eq!(color_key("app", Some(&main)), "app | branch:main");
        assert_eq!(window_name("app", Some(&main)), "app");
        let master = place("master", false);
        assert_eq!(window_name("app", Some(&master)), "app");
        let feature = place("eng-42-fix", false);
        assert_eq!(color_key("app", Some(&feature)), "app | branch:eng-42-fix");
        assert_eq!(window_name("app", Some(&feature)), "app:eng-42-fix");
        let wt = place("eng-7-thing", true);
        assert_eq!(color_key("eng-7", Some(&wt)), "eng-7 | worktree:eng-7");
        assert_eq!(window_name("eng-7", Some(&wt)), "eng-7");
        let detached = place("HEAD", false);
        assert_eq!(window_name("app", Some(&detached)), "app:HEAD");
        assert_eq!(dir_name(Path::new("/")), "/");
        assert_eq!(dir_name(Path::new("/a/b/")), "b");
    }

    #[test]
    fn no_key_hashes_to_a_red() {
        // Every hue a key can take is a color that isn't red, the tab's
        // lifted one too, and the reds at 0°, 60° and 300° are left out.
        for h in HUES.iter() {
            assert!(!reads_as_red(hue_color(*h)), "{h}°");
        }
        assert_eq!(hue_color(300), Rgb::new(239, 35, 35));
        for red in [0, 14, 46, 59, 300, 314, 346, 359] {
            assert!(!HUES.contains(&red), "{red}°");
        }
        assert_eq!(HUES.len(), 302);
        // The reddest left are orange and pink.
        assert_eq!(hue_color(15), Rgb::new(239, 135, 35));
        assert_eq!(hue_color(345), Rgb::new(239, 35, 137));
        // Thousands of made-up directories, spread over all of them.
        let hues: std::collections::HashSet<i64> = (0..5000)
            .map(|i| {
                let key = format!("project-{i} | branch:eng-{i}-fix");
                assert!(!reads_as_red(statusline_color(&key, Shade::Vivid)), "{key}");
                assert!(!reads_as_red(accent_color(&key, Shade::Vivid)), "{key}");
                tint_of(&key).hue
            })
            .collect();
        assert_eq!(hues.len(), HUES.len());
    }

    #[test]
    fn colors_are_pinned() {
        // The steps of tmux/accent-color.sh, over the hues that aren't red,
        // so a change to the hash or the steps shows up here.
        for (key, hex) in PINNED {
            assert_eq!(light(key, Shade::Vivid).hex(), *hex, "{key}");
        }
        // Lifting never lowers a channel; a light color is left alone.
        for (key, _) in PINNED {
            let (s, a) = (
                statusline_color(key, Shade::Vivid),
                accent_color(key, Shade::Vivid),
            );
            assert!(a.r >= s.r && a.g >= s.g && a.b >= s.b);
        }
    }

    #[test]
    fn soft_colors_are_the_canvas_pastels() {
        // Every hue, in steps of 15, a pastel in the canvas's range.
        for h in (0..360).step_by(15) {
            let Rgb { r, g, b } = soft_color(h);
            let (max, min) = (r.max(g).max(b) as f64, r.min(g).min(b) as f64);
            let lightness = (max + min) / 2.0 / 255.0;
            let saturation = (max - min) / 255.0 / (1.0 - (2.0 * lightness - 1.0).abs());
            assert!((0.70..=0.85).contains(&lightness), "{h}°: {lightness}");
            assert!((0.50..=0.85).contains(&saturation), "{h}°: {saturation}");
            assert!(soft_color(h).takes_dark_words(), "{h}°");
        }
        // The same key keeps its hue under both shades: the soft color's
        // strongest channel is the vivid one's.
        for (key, _) in PINNED {
            let (soft, vivid) = (
                statusline_color(key, Shade::Soft),
                statusline_color(key, Shade::Vivid),
            );
            let strongest = |c: Rgb| {
                let channels = [c.r, c.g, c.b];
                (0..3).max_by_key(|i| channels[*i]).unwrap()
            };
            assert_eq!(strongest(soft), strongest(vivid), "{key}");
            // The tab and the statusline agree under soft.
            assert_eq!(accent_color(key, Shade::Soft), soft, "{key}");
        }
        // As words on the light theme, darker in the same hue; elsewhere
        // as it is.
        use ratatui::style::Color;
        let soft = Color::from(statusline_color("a", Shade::Soft));
        assert_eq!(as_words(soft, false), soft);
        let Color::Rgb(r, g, b) = as_words(soft, true) else {
            panic!("not RGB")
        };
        assert!(Rgb::new(r, g, b).luminance() < 110, "{r} {g} {b}");
        assert_eq!(as_words(Color::Indexed(3), true), Color::Indexed(3));
        assert_eq!(Shade::of(None), Shade::Soft);
        assert_eq!(Shade::of(Some(&serde_json::json!("vivid"))), Shade::Vivid);
        assert_eq!(Shade::of(Some(&serde_json::json!("soft"))), Shade::Soft);
    }

    #[test]
    fn a_new_tab_takes_the_farthest_hue() {
        let light = |hue| Tint::new(hue, Step::Light);
        let home = light(267);
        // A lone tab wears its directory's tint.
        assert_eq!(apart(&[], home), home);
        // A second, the hue whose color is drawn farthest from the first's.
        let far = |h: i64| HUES.iter().map(|o| distance(h, *o)).max().unwrap();
        let second = apart(&[home], home);
        assert_eq!(distance(second.hue, 267), far(267));
        assert!(far(267) >= 170, "{}", far(267));
        // Among hues as far, the one drawn nearest the directory's decides.
        let ties: Vec<i64> = HUES
            .iter()
            .copied()
            .filter(|h| distance(*h, 267) == far(267))
            .collect();
        for h in &ties {
            assert_eq!(distance(apart(&[home], light(*h)).hue, *h), 0, "{ties:?}");
        }
        // The shell's steps run each segment out and back: 315° and 345°
        // draw one color, so they count as one hue.
        assert_eq!(soft_color(315), soft_color(345));
        assert!(distance(315, 345) <= 1);
        assert_eq!(apart(&[light(315)], light(345)).step, Step::Light);
        assert!(distance(apart(&[light(315)], light(345)).hue, 315) >= 170);
        // Three tabs of one directory, whatever its hue, each drawn about 90°
        // from the others (89° at worst, the drawn hues in whole steps).
        for home in HUES.iter().map(|h| light(*h)) {
            let mut open = vec![home];
            for _ in 0..2 {
                open.push(apart(&open, home));
            }
            for (i, a) in open.iter().enumerate() {
                for b in &open[i + 1..] {
                    assert!(distance(a.hue, b.hue) >= 85, "{open:?}");
                }
            }
        }
        // However many are open, never a red one.
        let mut open = vec![];
        for i in 0..40 {
            let tint = apart(&open, tint_of(&format!("p{i}")));
            assert!(HUES.contains(&tint.hue), "{tint:?}");
            open.push(tint);
        }
    }

    #[test]
    fn a_hue_close_to_an_open_tabs_takes_the_other_step() {
        // Every hue taken but those drawn 126° to 154°, in the light step:
        // the next, drawn about 140°, is within 20° of the nearest, and takes
        // the deep step.
        let taken = |step| -> Vec<Tint> {
            HUES.iter()
                .copied()
                .filter(|h| !(126..=154).contains(&drawn_hue(*h)))
                .map(|h| Tint::new(h, step))
                .collect()
        };
        let next = apart(&taken(Step::Light), Tint::new(10, Step::Light));
        assert!((135..=145).contains(&drawn_hue(next.hue)), "{next:?}");
        assert_eq!(next.step, Step::Deep);
        // Taken in the deep step, the light one.
        let next = apart(&taken(Step::Deep), Tint::new(10, Step::Light));
        assert_eq!(next.step, Step::Light);
        // Far from every open tab, the light step.
        assert_eq!(
            apart(&[Tint::new(267, Step::Deep)], Tint::new(10, Step::Deep)).step,
            Step::Light
        );
    }

    #[test]
    fn both_steps_read_and_are_not_red() {
        let mut deep = 0;
        for h in HUES.iter().copied() {
            let (l, d) = (Tint::new(h, Step::Light), Tint::new(h, Step::Deep));
            for shade in [Shade::Soft, Shade::Vivid] {
                for tint in [l, d] {
                    assert!(!reads_as_red(line_color(tint, shade)), "{tint:?} {shade:?}");
                    assert!(!reads_as_red(tab_color(tint, shade)), "{tint:?} {shade:?}");
                }
            }
            if d.step == Step::Light {
                // A hue whose deep step reads as red keeps the light one.
                assert!(!deep_allowed(h), "{h}°");
                continue;
            }
            deep += 1;
            for shade in [Shade::Soft, Shade::Vivid] {
                assert!(
                    line_color(d, shade).luminance() < line_color(l, shade).luminance(),
                    "{h}° {shade:?}"
                );
            }
            // Soft's deep step, a darker pastel whose words are chosen for it.
            let Rgb { r, g, b } = line_color(d, Shade::Soft);
            let (max, min) = (r.max(g).max(b) as f64, r.min(g).min(b) as f64);
            let lightness = (max + min) / 2.0 / 255.0;
            assert!((0.55..=0.68).contains(&lightness), "{h}°: {lightness}");
            let words = words_on(line_color(d, Shade::Soft).into());
            let dark = words == ratatui::style::Color::Indexed(232);
            assert_eq!(dark, line_color(d, Shade::Soft).takes_dark_words(), "{h}°");
        }
        assert!(deep > HUES.len() / 2, "most hues have a deep step: {deep}");
        // The hash's step is the key's own, and both come up.
        assert_eq!(
            tint_of("switchboard | branch:main"),
            tint_of("switchboard | branch:main")
        );
        let steps: std::collections::HashSet<Step> =
            (0..50).map(|i| tint_of(&format!("k{i}")).step).collect();
        assert_eq!(steps.len(), 2);
        // Kept on a tab and read back.
        let opts = std::collections::BTreeMap::from([
            (HUE.to_string(), "87".to_string()),
            (STEP.to_string(), "deep".to_string()),
        ]);
        assert_eq!(Tint::of_opts(&opts), Some(Tint::new(87, Step::Deep)));
        assert_eq!(Tint::of_opts(&Default::default()), None);
    }

    /// A key's hue in the light step, as the tab wears it.
    fn light(key: &str, shade: Shade) -> Rgb {
        tab_color(Tint::new(tint_of(key).hue, Step::Light), shade)
    }

    // Some lifted for a dark status line (a), some not.
    const PINNED: &[(&str, &str)] = &[
        ("switchboard | branch:main", "d923ef"),
        ("app", "efdb23"),
        ("web | branch:eng-42-fix-login", "67ef23"),
        ("eng-7 | worktree:eng-7", "a3ef23"),
        ("notes", "5fef23"),
        ("api | branch:HEAD", "ef238f"),
        ("deep-blue", "29ef23"),
        ("a", "6f3fff"),
        ("c", "4bef23"),
        ("f", "6eef23"),
    ];

    /// Real directories, read through git's own files, take the keys and
    /// names accent-color.sh gave them, and the colors pinned above.
    #[test]
    fn real_directories_take_their_keys_names_and_colors() {
        let scratch = Scratch::new("color");
        let main = scratch.0.join("switchboard");
        if !repo(&main) {
            eprintln!("skipped: no git");
            return;
        }
        let plain = scratch.0.join("app");
        std::fs::create_dir_all(&plain).unwrap();
        let web = scratch.0.join("web");
        assert!(repo(&web));
        assert!(git(&web, &["checkout", "-q", "-b", "eng-42-fix-login"]));
        let sub = web.join("src");
        std::fs::create_dir_all(&sub).unwrap();
        let wt = scratch.0.join("eng-7");
        assert!(git(
            &web,
            &["worktree", "add", "-q", "-b", "eng-7", wt.to_str().unwrap()]
        ));
        let detached = scratch.0.join("api");
        assert!(repo(&detached));
        assert!(git(&detached, &["checkout", "-q", "--detach"]));
        for (dir, key, name) in [
            (&main, "switchboard | branch:main", "switchboard"),
            (&plain, "app", "app"),
            (
                &web,
                "web | branch:eng-42-fix-login",
                "web:eng-42-fix-login",
            ),
            (
                &sub,
                "src | branch:eng-42-fix-login",
                "src:eng-42-fix-login",
            ),
            (&wt, "eng-7 | worktree:eng-7", "eng-7"),
            (&detached, "api | branch:HEAD", "api:HEAD"),
        ] {
            assert_eq!(color_key_of(dir), key, "{}", dir.display());
            if let Some((_, hex)) = PINNED.iter().find(|(k, _)| *k == key) {
                assert_eq!(light(&color_key_of(dir), Shade::Vivid).hex(), *hex, "{key}");
            }
            let place = super::super::git::locate(dir);
            assert_eq!(window_name(&dir_name(dir), place.as_ref()), name, "{key}");
        }
    }
}
