//! A directory's accent color and the name its tab takes, as
//! tmux/accent-color.sh worked them out, and the same color as
//! claude/statusline-command.sh draws it, so the tab and the Claude
//! statusline agree on a project's color.
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
//! about 65% saturation and 78% lightness as the UX 2.0 canvas draws its
//! tabs, the same for the tab and the statusline; or vivid, the scripts'
//! color at about 80% saturation and 45% lightness in the same integer steps
//! the shell's arithmetic took, the tab's then lifted to a perceived
//! luminance of at least 100 for reading on a dark status line, the
//! statusline's not, as it never was.

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

/// The hue a key hashes to, in degrees.
fn hue(key: &str) -> i64 {
    let digest = sha256(key.as_bytes());
    let hash = u32::from_be_bytes([digest[0], digest[1], digest[2], digest[3]]) as usize;
    HUES[hash % HUES.len()]
}

/// The color a key hashes to, as the Claude statusline draws it.
pub fn statusline_color(key: &str, shade: Shade) -> Rgb {
    match shade {
        Shade::Soft => soft_color(hue(key)),
        Shade::Vivid => hue_color(hue(key)),
    }
}

/// A hue made a pastel at about 65% saturation and 78% lightness, in the
/// same integer steps as `hue_color`.
fn soft_color(hue: i64) -> Rgb {
    let segment = hue / 60;
    let frac = (hue % 60) * 255 / 60;
    let c: i64 = 73;
    let x = c * (255 - (frac * 2 - 255).abs()) / 255;
    let m: i64 = 162;
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

/// A hue made a color at about 80% saturation and 45% lightness, in the
/// shell's integer steps.
fn hue_color(hue: i64) -> Rgb {
    let segment = hue / 60;
    let frac = (hue % 60) * 255 / 60;
    let c: i64 = 204;
    let x = c * (255 - (frac * 2 - 255).abs()) / 255;
    let m: i64 = 35;
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

/// The color a key hashes to, as the tab wears it: the statusline's, a
/// vivid one lifted to a perceived luminance of at least 100 (a soft one is
/// far above it).
pub fn accent_color(key: &str, shade: Shade) -> Rgb {
    let color = statusline_color(key, shade);
    let lum = color.luminance() as i64;
    if lum >= 100 {
        return color;
    }
    let boost = 100 - lum;
    let Rgb { r, g, b } = color;
    let (r, g, b) = (r as i64, g as i64, b as i64);
    Rgb::new(channel(r + boost), channel(g + boost), channel(b + boost))
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
                hue(&key)
            })
            .collect();
        assert_eq!(hues.len(), HUES.len());
    }

    #[test]
    fn colors_are_pinned() {
        // The steps of tmux/accent-color.sh, over the hues that aren't red,
        // so a change to the hash or the steps shows up here.
        for (key, hex) in PINNED {
            assert_eq!(accent_color(key, Shade::Vivid).hex(), *hex, "{key}");
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
                assert_eq!(accent_color_of(dir, Shade::Vivid).hex(), *hex, "{key}");
            }
            let place = super::super::git::locate(dir);
            assert_eq!(window_name(&dir_name(dir), place.as_ref()), name, "{key}");
        }
    }
}
