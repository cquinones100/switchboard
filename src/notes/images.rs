//! Images in a note, ported from notes/lua/notes/images.lua: one pasted
//! from the clipboard is saved beside the note, in `.attachments/<the
//! note's name>/` in its folder (a folder starting with a dot, which the
//! list leaves out), and written into the note as Markdown,
//! `![name](path)`, its path from the note's own folder, so Claude, reading
//! the note, can read the image too, and nothing about it leaves the
//! computer. Drawn in the note under its line, and opened in Quick Look by
//! Enter or a click (links.rs).
//!
//! A picture is drawn with the kitty graphics protocol's Unicode
//! placeholders, as M-p's PR image view drew its pages: the terminal is
//! given the PNG's path once (it reads the file itself), as an image laid
//! over cells, and the note shows, under the image's line, rows of a
//! placeholder character (U+10EEEE), each cell marked with its row and
//! column and coloured with the image's id. The multiplexer places those
//! like any other text, so the picture sits exactly where its rows are,
//! scrolls with them, and is covered by a box over them as text would be.
//! It needs a terminal that speaks kitty's protocol with placeholders
//! (Ghostty, kitty); elsewhere, or with the notes_images setting off, an
//! image is its name alone. A picture other than a PNG is made one first,
//! once for each version of the file, in the cache, as macOS's sips did.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use ratatui::style::Color;

use super::format::Doc;
use super::render::{self, Kind};
use super::store::{self, Store};
use crate::{paths, util};

/// kitty's gen/rowcolumn-diacritics.txt: the nth mark means row or column n.
const MARKS: &[u32] = &[
    0x0305, 0x030D, 0x030E, 0x0310, 0x0312, 0x033D, 0x033E, 0x033F, 0x0346, 0x034A, 0x034B, 0x034C,
    0x0350, 0x0351, 0x0352, 0x0357, 0x035B, 0x0363, 0x0364, 0x0365, 0x0366, 0x0367, 0x0368, 0x0369,
    0x036A, 0x036B, 0x036C, 0x036D, 0x036E, 0x036F, 0x0483, 0x0484, 0x0485, 0x0486, 0x0487, 0x0592,
    0x0593, 0x0594, 0x0595, 0x0597, 0x0598, 0x0599, 0x059C, 0x059D, 0x059E, 0x059F, 0x05A0, 0x05A1,
    0x05A8, 0x05A9, 0x05AB, 0x05AC, 0x05AF, 0x05C4, 0x0610, 0x0611, 0x0612, 0x0613, 0x0614, 0x0615,
    0x0616, 0x0617, 0x0657, 0x0658, 0x0659, 0x065A, 0x065B, 0x065D, 0x065E, 0x06D6, 0x06D7, 0x06D8,
    0x06D9, 0x06DA, 0x06DB, 0x06DC, 0x06DF, 0x06E0, 0x06E1, 0x06E2, 0x06E4, 0x06E7, 0x06E8, 0x06EB,
    0x06EC, 0x0730, 0x0732, 0x0733, 0x0735, 0x0736, 0x073A, 0x073D, 0x073F, 0x0740, 0x0741, 0x0743,
    0x0745, 0x0747, 0x0749, 0x074A, 0x07EB, 0x07EC, 0x07ED, 0x07EE, 0x07EF, 0x07F0, 0x07F1, 0x07F3,
    0x0816, 0x0817, 0x0818, 0x0819, 0x081B, 0x081C, 0x081D, 0x081E, 0x081F, 0x0820, 0x0821, 0x0822,
    0x0823, 0x0825, 0x0826, 0x0827, 0x0829, 0x082A, 0x082B, 0x082C, 0x082D, 0x0951, 0x0953, 0x0954,
    0x0F82, 0x0F83, 0x0F86, 0x0F87, 0x135D, 0x135E, 0x135F, 0x17DD, 0x193A, 0x1A17, 0x1A75, 0x1A76,
    0x1A77, 0x1A78, 0x1A79, 0x1A7A, 0x1A7B, 0x1A7C, 0x1B6B, 0x1B6D, 0x1B6E, 0x1B6F, 0x1B70, 0x1B71,
    0x1B72, 0x1B73, 0x1CD0, 0x1CD1, 0x1CD2, 0x1CDA, 0x1CDB, 0x1CE0, 0x1DC0, 0x1DC1, 0x1DC3, 0x1DC4,
    0x1DC5, 0x1DC6, 0x1DC7, 0x1DC8, 0x1DC9, 0x1DCB, 0x1DCC, 0x1DD1, 0x1DD2, 0x1DD3, 0x1DD4, 0x1DD5,
    0x1DD6, 0x1DD7, 0x1DD8, 0x1DD9, 0x1DDA, 0x1DDB, 0x1DDC, 0x1DDD, 0x1DDE, 0x1DDF, 0x1DE0, 0x1DE1,
    0x1DE2, 0x1DE3, 0x1DE4, 0x1DE5, 0x1DE6, 0x1DFE, 0x20D0, 0x20D1, 0x20D4, 0x20D5, 0x20D6, 0x20D7,
    0x20DB, 0x20DC, 0x20E1, 0x20E7, 0x20E9, 0x20F0, 0x2CEF, 0x2CF0, 0x2CF1, 0x2DE0, 0x2DE1, 0x2DE2,
    0x2DE3, 0x2DE4, 0x2DE5, 0x2DE6, 0x2DE7, 0x2DE8, 0x2DE9, 0x2DEA, 0x2DEB, 0x2DEC, 0x2DED, 0x2DEE,
    0x2DEF, 0x2DF0, 0x2DF1, 0x2DF2, 0x2DF3, 0x2DF4, 0x2DF5, 0x2DF6, 0x2DF7, 0x2DF8, 0x2DF9, 0x2DFA,
    0x2DFB, 0x2DFC, 0x2DFD, 0x2DFE, 0x2DFF, 0xA66F, 0xA67C, 0xA67D, 0xA6F0, 0xA6F1, 0xA8E0, 0xA8E1,
    0xA8E2, 0xA8E3, 0xA8E4, 0xA8E5, 0xA8E6, 0xA8E7, 0xA8E8, 0xA8E9, 0xA8EA, 0xA8EB, 0xA8EC, 0xA8ED,
    0xA8EE, 0xA8EF, 0xA8F0, 0xA8F1, 0xAAB0, 0xAAB2, 0xAAB3, 0xAAB7, 0xAAB8, 0xAABE, 0xAABF, 0xAAC1,
    0xFE20, 0xFE21, 0xFE22, 0xFE23, 0xFE24, 0xFE25, 0xFE26, 0x10A0F, 0x10A38, 0x1D185, 0x1D186,
    0x1D187, 0x1D188, 0x1D189, 0x1D1AA, 0x1D1AB, 0x1D1AC, 0x1D1AD, 0x1D242, 0x1D243, 0x1D244,
];

pub const PLACEHOLDER: char = '\u{10EEEE}';

/// A name made only of letters, digits, dots and dashes, so the path in the
/// Markdown never holds a space or a bracket that would end it.
fn slug(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars() {
        if c.is_ascii_alphanumeric() || c == '.' || c == '-' {
            out.push(c);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let mut made = out;
    while made.contains("--") {
        made = made.replace("--", "-");
    }
    let made = made.trim_start_matches(['-', '.']).trim_end_matches('-');
    if made.is_empty() {
        "note".into()
    } else {
        made.into()
    }
}

/// Pastes the clipboard's image into a note on a line of its own below the
/// cursor's, indented under it when that is a list or checklist item, so
/// the image goes with the item (ticked, moved) and is read as part of it.
/// `grab` saves the clipboard's image to a file (the platform's, or a
/// test's). Gives the path written, or why not.
pub fn paste(doc: &mut Doc, note: &Path, grab: &dyn Fn(&Path) -> bool) -> Result<String, String> {
    let folder = note.parent().ok_or("not a note")?;
    let stem = note
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let name = slug(&stem);
    let dir = folder.join(".attachments").join(&name);
    let _ = fs::create_dir_all(&dir);
    let now = util::local_now();
    let stamp = format!(
        "{}-{:02}{:02}{:02}",
        util::ymd(&now),
        now.hour(),
        now.minute(),
        now.second()
    );
    let mut file = dir.join(format!("image-{stamp}.png"));
    let mut n = 1;
    while file.exists() {
        n += 1;
        file = dir.join(format!("image-{stamp}-{n}.png"));
    }
    if !grab(&file) {
        let _ = fs::remove_dir(&dir);
        return Err("no image on the clipboard".into());
    }
    let base = file
        .file_name()
        .map(|f| f.to_string_lossy().into_owned())
        .unwrap_or_default();
    let shown = base.trim_end_matches(".png").to_string();
    let rel = format!(".attachments/{name}/{base}");
    let line = doc.line(doc.row);
    let mut indent: String = line.chars().take_while(|c| c.is_whitespace()).collect();
    let rest = line.trim_start();
    let listed = (rest.starts_with(['-', '*', '+']) && rest.chars().nth(1) == Some(' ')) || {
        let digits = rest.chars().take_while(char::is_ascii_digit).count();
        digits > 0
            && matches!(rest[digits..].chars().next(), Some('.' | ')'))
            && rest[digits + 1..].starts_with(' ')
    };
    if listed {
        indent.push_str("  ");
    }
    doc.text.set_lines(
        doc.row + 1,
        doc.row + 1,
        &[format!("{indent}![{shown}]({rel})")],
    );
    Ok(rel)
}

/// An image's path in a note made a file on disk, from the note's folder,
/// or `None` for one outside the notes directory or not there: a web
/// address, a data: one, or a path that leads out of the notes. Only what
/// this gives is drawn, so the terminal is never handed a file outside the
/// notes or one a note's text made up.
pub fn resolve(root: &Path, note: &Path, path: &str) -> Option<PathBuf> {
    let b = path.as_bytes();
    let scheme = b.first().is_some_and(u8::is_ascii_alphabetic)
        && b.iter()
            .skip(1)
            .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, b'+' | b'.' | b'-'))
            .count()
            .checked_add(1)
            .is_some_and(|n| b.get(n) == Some(&b':'));
    if scheme {
        return None;
    }
    // Real paths, so neither a .. nor a link leads outside the notes.
    let full = fs::canonicalize(note.parent()?.join(path)).ok()?;
    let root = fs::canonicalize(root).ok()?;
    (full.starts_with(&root) && full != root).then_some(full)
}

// ── Drawing ─────────────────────────────────────────────────────────────────

/// A picture drawn under a line: its id, its size in cells, and how far it
/// is indented.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Picture {
    pub id: u32,
    pub cols: u16,
    pub rows: u16,
    pub indent: u16,
    /// The PNG the terminal is given.
    pub png: PathBuf,
}

impl Picture {
    /// Its colour: the id, as the placeholders carry it.
    pub fn color(&self) -> Color {
        Color::Rgb(
            (self.id >> 16) as u8,
            (self.id >> 8 & 0xff) as u8,
            (self.id & 0xff) as u8,
        )
    }

    /// The placeholder cell at a row and column of it.
    pub fn cell(row: u16, col: u16) -> String {
        let mark = |n: u16| char::from_u32(MARKS[n as usize]).unwrap_or('\u{0305}');
        format!("{PLACEHOLDER}{}{}", mark(row), mark(col))
    }
}

/// A PNG's size in pixels, from its header.
pub fn png_size(file: &Path) -> Option<(u32, u32)> {
    use std::io::Read;
    let mut head = [0u8; 24];
    fs::File::open(file).ok()?.read_exact(&mut head).ok()?;
    if &head[..8] != b"\x89PNG\r\n\x1a\n" {
        return None;
    }
    let u32_at =
        |at: usize| u32::from_be_bytes([head[at], head[at + 1], head[at + 2], head[at + 3]]);
    Some((u32_at(16), u32_at(20)))
}

/// An id for an image, 24 bits, never 0: FNV-1a, as images.lua made it.
fn id_of(text: &str) -> u32 {
    let mut h: u32 = 2_166_136_261;
    for b in text.bytes() {
        h = (h ^ b as u32).wrapping_mul(16_777_619);
    }
    let id = h % 16_777_215;
    if id == 0 { 1 } else { id }
}

const KINDS: [&str; 8] = ["jpg", "jpeg", "gif", "webp", "heic", "tif", "tiff", "bmp"];

/// A picture the terminal can't take as it is (it reads PNG alone): made a
/// PNG in the cache, once for each version of the file.
fn as_png(file: &Path) -> Option<PathBuf> {
    let ext = file.extension()?.to_string_lossy().to_lowercase();
    if !KINDS.contains(&ext.as_str()) {
        return None;
    }
    let meta = fs::metadata(file).ok()?;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    (file, store::mtime(file), meta.len()).hash(&mut hasher);
    let dir = paths::cache_dir().join("notes-images");
    let out = dir.join(format!("{:016x}.png", hasher.finish()));
    if out.exists() {
        return Some(out);
    }
    let _ = fs::create_dir_all(&dir);
    let made = image::open(file)
        .ok()
        .and_then(|img| img.save_with_format(&out, image::ImageFormat::Png).ok())
        .is_some();
    #[cfg(target_os = "macos")]
    let made = made
        || util::try_run(
            "sips",
            [
                "-s",
                "format",
                "png",
                &file.to_string_lossy(),
                "--out",
                &out.to_string_lossy(),
            ],
        )
        .is_some();
    if made {
        Some(out)
    } else {
        let _ = fs::remove_file(&out);
        None
    }
}

/// Whether the terminal speaks kitty's graphics protocol with
/// placeholders: Ghostty or kitty, by `$TERM` or `$TERM_PROGRAM` (the
/// multiplexer's panes keep the terminal the workspace started in as
/// `TERM_PROGRAM`).
pub fn terminal_can() -> bool {
    let name = format!(
        "{} {}",
        std::env::var("TERM").unwrap_or_default(),
        std::env::var("TERM_PROGRAM").unwrap_or_default()
    )
    .to_lowercase();
    name.contains("ghostty") || name.contains("kitty")
}

/// The terminal's cell in pixels, from its window size, else a common one.
pub fn cell_size() -> (u32, u32) {
    match crossterm::terminal::window_size() {
        Ok(s) if s.width > 0 && s.height > 0 && s.columns > 0 && s.rows > 0 => (
            (s.width / s.columns).max(1) as u32,
            (s.height / s.rows).max(1) as u32,
        ),
        _ => (8, 16),
    }
}

/// A PNG to draw, and its size in pixels.
type Png = (PathBuf, u32, u32);

/// The pictures given to the terminal and drawn now, so one already sent
/// isn't sent again, and one no longer shown is taken back (Ghostty holds
/// at most 320 MB of them).
#[derive(Default)]
pub struct Images {
    /// Drawing them at all: the terminal can, and notes_images is on.
    pub on: bool,
    pub cell: (u32, u32),
    /// Each picture sent, by id, with the size it was sent at.
    sent: HashMap<u32, (u16, u16)>,
    /// What each file read as: its PNG and size, by the file and its time.
    seen: HashMap<(PathBuf, Option<i64>), Option<Png>>,
}

impl Images {
    pub fn new(on: bool) -> Images {
        Images {
            on,
            cell: cell_size(),
            ..Images::default()
        }
    }

    /// The terminal may have lost them (another one attached, a resize that
    /// changes the cell size): sent again on the next draw.
    pub fn forget_sent(&mut self) {
        self.sent.clear();
        self.cell = cell_size();
    }

    fn png_of(&mut self, file: &Path) -> Option<Png> {
        let key = (file.to_path_buf(), store::mtime(file));
        if let Some(found) = self.seen.get(&key) {
            return found.clone();
        }
        let found = match png_size(file) {
            Some((w, h)) => Some((file.to_path_buf(), w, h)),
            None => as_png(file).and_then(|png| png_size(&png).map(|(w, h)| (png, w, h))),
        };
        self.seen.insert(key, found.clone());
        found
    }

    /// The pictures a line names, under it, sized to a window `room` cells
    /// wide and `height` tall: as wide as it lets them (at most their own
    /// width) and at most two fifths of its height.
    pub fn pictures(
        &mut self,
        root: &Path,
        note: &Path,
        line: &str,
        kind: Kind,
        room: u16,
        height: u16,
    ) -> Vec<Picture> {
        if !self.on || matches!(kind, Kind::Code | Kind::Fence) {
            return Vec::new();
        }
        let indent = render::indent_of(line) as u16;
        let (cw, ch) = self.cell;
        let mut out = Vec::new();
        for span in render::inline(line, 0) {
            let Some(path) = &span.image else { continue };
            let Some(file) = resolve(root, note, path) else {
                continue;
            };
            let Some((png, pw, ph)) = self.png_of(&file) else {
                continue;
            };
            if pw == 0 || ph == 0 {
                continue;
            }
            let room = room.saturating_sub(indent + 1).min(MARKS.len() as u16) as u32;
            let mut cols = room.min(pw.div_ceil(cw)).max(1);
            let mut rows = (cols * cw * ph).div_ceil(pw * ch).max(1);
            let most = ((height as u32 * 2) / 5).clamp(1, MARKS.len() as u32);
            if rows > most {
                rows = most;
                cols = ((rows * ch * pw) / (ph * cw)).clamp(1, room.max(1));
            }
            // An id for this picture at this size: the same image drawn at
            // two sizes is two images to the terminal, and a file changed
            // since is a new one.
            let stamp = format!(
                "{}:{cols}x{rows}:{}:{}",
                png.to_string_lossy(),
                store::mtime(&png).unwrap_or(0),
                fs::metadata(&png).map(|m| m.len()).unwrap_or(0)
            );
            out.push(Picture {
                id: id_of(&stamp),
                cols: cols as u16,
                rows: rows as u16,
                indent,
                png,
            });
        }
        out
    }

    /// The commands that give the terminal the pictures `near` the screen
    /// it hasn't been given, and take back those it was given that aren't.
    pub fn commands(&mut self, near: &[Picture]) -> Vec<u8> {
        let mut out = Vec::new();
        let keep: HashSet<u32> = near.iter().map(|p| p.id).collect();
        let gone: Vec<u32> = self
            .sent
            .keys()
            .filter(|id| !keep.contains(id))
            .copied()
            .collect();
        for id in gone {
            out.extend(command(&format!("a=d,d=I,i={id},q=2")));
            self.sent.remove(&id);
        }
        for p in near {
            if self.sent.get(&p.id) != Some(&(p.cols, p.rows)) {
                out.extend(command(&format!(
                    "a=T,U=1,f=100,t=f,q=2,i={},c={},r={};{}",
                    p.id,
                    p.cols,
                    p.rows,
                    util::base64(p.png.to_string_lossy().as_bytes())
                )));
                self.sent.insert(p.id, (p.cols, p.rows));
            }
        }
        out
    }

    /// Every picture taken back, as the app ends.
    pub fn release_all(&mut self) -> Vec<u8> {
        self.commands(&[])
    }
}

/// A kitty graphics command, wrapped for tmux when the app runs inside it.
fn command(body: &str) -> Vec<u8> {
    let seq = format!("\x1b_G{body}\x1b\\");
    if std::env::var_os("TMUX").is_some_and(|t| !t.is_empty()) {
        format!("\x1bPtmux;{}\x1b\\", seq.replace('\x1b', "\x1b\x1b")).into_bytes()
    } else {
        seq.into_bytes()
    }
}

/// An image a note names, opened in Quick Look (or the desktop's viewer),
/// only from inside the notes.
pub fn open(store: &Store, note: &Path, path: &str) -> bool {
    match resolve(store.root(), note, path) {
        Some(file) => crate::platform::preview(&file).is_ok(),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notes::store::tests::{temp, write};
    use crate::notes::text::Text;

    #[test]
    fn an_image_pasted_is_saved_beside_the_note_and_written_under_the_item() {
        let (_dir, mut s) = temp("paste");
        let path = s.create("", Some("Bug report"));
        write(&path, "# Bug report\n\n- [ ] the button is cut off\n");
        let mut doc = Doc::new(Text::from_file(&fs::read_to_string(&path).unwrap()));
        doc.row = 2;
        let grab = |file: &Path| fs::write(file, b"\x89PNG").is_ok();
        let rel = paste(&mut doc, &path, &grab).unwrap();
        let pattern =
            regex::Regex::new(r"^\.attachments/Bug-report/image-\d{4}-\d{2}-\d{2}-\d{6}\.png$")
                .unwrap();
        assert!(
            pattern.is_match(&rel),
            "beside the note, in a folder the list leaves out, with no space in its path: {rel}"
        );
        let name = rel.rsplit('/').next().unwrap().trim_end_matches(".png");
        assert_eq!(
            doc.line(3),
            format!("  ![{name}]({rel})"),
            "on a line of its own, indented under the item"
        );
        assert!(s.root().join(&rel).exists());
        let span = render::inline(&doc.line(3), 0)
            .into_iter()
            .find(|sp| sp.image.is_some())
            .unwrap();
        assert_eq!(
            &doc.line(3)[span.s..span.e],
            name,
            "shown by its name, the markup hidden"
        );
        assert_eq!(
            resolve(s.root(), &path, &rel),
            Some(fs::canonicalize(s.root().join(&rel)).unwrap())
        );
        assert_eq!(
            resolve(s.root(), &path, "../../etc/passwd"),
            None,
            "never a file outside the notes"
        );
        assert_eq!(
            resolve(s.root(), &path, "data:image/png;base64,iVBORw0KGgo="),
            None,
            "nor a data: address"
        );
        let none = |_: &Path| false;
        assert_eq!(
            paste(&mut doc, &path, &none),
            Err("no image on the clipboard".into())
        );
    }

    #[test]
    fn a_picture_is_placeholder_cells_sized_to_the_window_and_sent_once() {
        let (_dir, mut s) = temp("draw");
        let note = s.create("", Some("Shots"));
        let dir = note.parent().unwrap().join(".attachments/Shots");
        fs::create_dir_all(&dir).unwrap();
        // A PNG header saying 1300 by 280 pixels, which is all drawing reads.
        let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
        png.extend(1300u32.to_be_bytes());
        png.extend(280u32.to_be_bytes());
        fs::write(dir.join("a.png"), &png).unwrap();
        fs::write(dir.join("b.png"), "not a png").unwrap();
        let mut images = Images {
            on: true,
            cell: (8, 16),
            ..Images::default()
        };
        let line = "![a](.attachments/Shots/a.png)";
        let kind = render::line_kind(line, false);
        let pics = images.pictures(s.root(), &note, line, kind, 100, 40);
        assert_eq!(pics.len(), 1);
        let p = &pics[0];
        assert!(
            p.cols > 0 && (p.cols as u32) < 1300u32.div_ceil(8),
            "as wide as the window has room for"
        );
        assert_eq!(
            p.rows as u32,
            (p.cols as u32 * 8 * 280).div_ceil(1300 * 16),
            "and as tall as its shape says"
        );
        assert!(
            images
                .pictures(
                    s.root(),
                    &note,
                    "![b](.attachments/Shots/b.png)",
                    Kind::Body,
                    100,
                    40
                )
                .is_empty(),
            "a file that isn't a picture isn't drawn"
        );
        assert!(
            images
                .pictures(s.root(), &note, line, Kind::Code, 100, 40)
                .is_empty(),
            "nor one in a code block"
        );
        let sent = String::from_utf8(images.commands(&pics)).unwrap();
        assert!(sent.contains("a=T,U=1,f=100,t=f"));
        assert!(
            images.commands(&pics).is_empty(),
            "a picture already sent isn't sent again"
        );
        let taken = String::from_utf8(images.commands(&[])).unwrap();
        assert!(
            taken.contains("a=d,d=I"),
            "one no longer shown is taken back"
        );
        assert!(Picture::cell(0, 0).starts_with(PLACEHOLDER));
        let mut off = Images {
            on: false,
            cell: (8, 16),
            ..Images::default()
        };
        assert!(
            off.pictures(s.root(), &note, line, kind, 100, 40)
                .is_empty(),
            "with the setting off, an image is its name"
        );
    }
}
