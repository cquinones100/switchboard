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
//! the key's hash, as a number, modulo 360 is the hue, turned into a color
//! at about 80% saturation and 45% lightness in the same integer steps the
//! shell's arithmetic took. The tab's color is then lifted to a perceived
//! luminance of at least 100, for reading on a dark status line; the
//! statusline's isn't, as it never was.

use std::path::Path;

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

    /// `23ef59`, as accent-color.sh printed it.
    pub fn hex(self) -> String {
        format!("{:02x}{:02x}{:02x}", self.r, self.g, self.b)
    }
}

impl From<Rgb> for ratatui::style::Color {
    fn from(c: Rgb) -> Self {
        ratatui::style::Color::Rgb(c.r, c.g, c.b)
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

/// The color a key hashes to, as the Claude statusline draws it.
pub fn statusline_color(key: &str) -> Rgb {
    let digest = sha256(key.as_bytes());
    let hash = u32::from_be_bytes([digest[0], digest[1], digest[2], digest[3]]) as i64;
    let hue = hash % 360;
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

/// The color a key hashes to, as the tab wears it: the statusline's, lifted
/// to a perceived luminance of at least 100.
pub fn accent_color(key: &str) -> Rgb {
    let Rgb { r, g, b } = statusline_color(key);
    let (r, g, b) = (r as i64, g as i64, b as i64);
    let lum = (299 * r + 587 * g + 114 * b) / 1000;
    if lum >= 100 {
        return Rgb::new(r as u8, g as u8, b as u8);
    }
    let boost = 100 - lum;
    Rgb::new(channel(r + boost), channel(g + boost), channel(b + boost))
}

/// The tab's color for a directory, read from its git files.
pub fn accent_color_of(dir: &Path) -> Rgb {
    accent_color(&color_key_of(dir))
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
    fn colors_are_pinned() {
        // Worked out by tmux/accent-color.sh, so a change to the hash or the
        // steps shows up here.
        for (key, hex) in PINNED {
            assert_eq!(accent_color(key).hex(), *hex, "{key}");
        }
        // Lifting never lowers a channel; a light color is left alone.
        for (key, _) in PINNED {
            let (s, a) = (statusline_color(key), accent_color(key));
            assert!(a.r >= s.r && a.g >= s.g && a.b >= s.b);
        }
    }

    // Some lifted for a dark status line (app, deep-blue, a, f), some not.
    const PINNED: &[(&str, &str)] = &[
        ("switchboard | branch:main", "23d4ef"),
        ("app", "f0243f"),
        ("web | branch:eng-42-fix-login", "23a3ef"),
        ("eng-7 | worktree:eng-7", "ef7423"),
        ("notes", "efe123"),
        ("api | branch:HEAD", "23efb7"),
        ("deep-blue", "8f31fd"),
        ("a", "7d39ff"),
        ("c", "beef23"),
        ("f", "f42828"),
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
                assert_eq!(accent_color_of(dir).hex(), *hex, "{key}");
            }
            let place = super::super::git::locate(dir);
            assert_eq!(window_name(&dir_name(dir), place.as_ref()), name, "{key}");
        }
    }
}
