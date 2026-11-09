//! Images the server draws itself, with the kitty graphics protocol: the
//! image atoms of the UX 2.0 canvas's board 0 (an avatar's photo, a
//! picture, a meter with a tick, a series as a chart or a sparkline, and
//! the soft shadow under a box), and board 6's lane and chart over time (a
//! tab's history), rastered here in Rust at the size of the
//! terminal's cell (src/mux/terminal.rs) and sent to each client that can
//! show them.
//!
//! An atom is placed with the protocol's Unicode placeholders, as a note's
//! picture is (src/notes/images.rs): its cells in the frame are U+10EEEE
//! marked with their row and column and colored with the image's id, so
//! they ride in the frame as text, move with their cells, and a box drawn
//! over them covers them as it covers text. The shadow alone is a direct
//! placement under the text (`z=-1`), since it must sit over the cells'
//! backgrounds and under a box's words, which a placeholder can't.
//!
//! Each client keeps what it was sent (`ImageCache`, in its `Screen`), by
//! a stable name the caller gives, with a key made of what the image is
//! drawn from: an image whose key is unchanged isn't sent again, one that
//! changed replaces the old under the same id, and one no longer placed is
//! deleted, so the terminal never collects images. Anything that may have
//! cleared the terminal (an attach, a resize, `Screen::invalidate`, a new
//! build after `switchboard upgrade`, whose screen starts empty) sends them
//! all again.
//!
//! The image's data is always sent inline (`t=d`), never a file's path: a
//! graphics command naming a file has the terminal read any file of yours,
//! which is why only the notes pane may pass its own (src/mux/passthrough.rs).
//!
//! Each atom has a text form drawn in the same cells where the terminal
//! doesn't speak the protocol (`Frame::graphics` is `None`: not Ghostty or
//! kitty, or the `server_images` setting off), and the atom's function
//! picks the form, so no part checks the terminal itself. An avatar is its
//! text form, the initial on the person's color, unless it has a photo,
//! since the binary carries no font to raster an initial with.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use image::RgbaImage;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use tiny_skia::{
    FillRule, Mask, Paint, PathBuilder, Pixmap, PixmapPaint, Rect as SkRect, Transform,
};

use crate::mux::compose::Frame;
use crate::util;

/// kitty's gen/rowcolumn-diacritics.txt: the nth mark means row or column n.
pub const MARKS: &[u32] = &[
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

/// The ids the server's images take: those with the top of their 24 bits
/// set, so they never meet a note's (`notes::images`, which clears it).
pub const SERVER_IDS: u32 = 0x80_0000;

/// The placeholder cell at a row and column of an image.
pub fn placeholder(row: u16, col: u16) -> String {
    let mark = |n: u16| {
        MARKS
            .get(n as usize)
            .and_then(|&m| char::from_u32(m))
            .unwrap_or('\u{0305}')
    };
    format!("{PLACEHOLDER}{}{}", mark(row), mark(col))
}

/// The color a placeholder carries: the image's 24-bit id.
pub fn id_color(id: u32) -> Color {
    Color::Rgb((id >> 16) as u8, (id >> 8 & 0xff) as u8, (id & 0xff) as u8)
}

/// Whether a terminal, by its `TERM` or `TERM_PROGRAM`, speaks kitty's
/// graphics protocol with placeholders: Ghostty and kitty do.
pub fn speaks_kitty(term: &str) -> bool {
    let term = term.to_lowercase();
    term.contains("ghostty") || term.contains("kitty")
}

/// The id of one of the server's images, from the stable name its caller
/// gives it (`"usage/chart"`): FNV-1a over the name, in the server's range.
pub fn id_of(name: &str) -> u32 {
    let mut h: u32 = 2_166_136_261;
    for b in name.bytes() {
        h = (h ^ b as u32).wrapping_mul(16_777_619);
    }
    SERVER_IDS | (h & (SERVER_IDS - 1))
}

// ── Colors ──────────────────────────────────────────────────────────────────

/// A color as the pixels it is: an indexed one by xterm's 256-color
/// palette, the named ANSI ones by xterm's defaults.
pub fn rgb(color: Color) -> [u8; 3] {
    const ANSI: [[u8; 3]; 16] = [
        [0, 0, 0],
        [205, 0, 0],
        [0, 205, 0],
        [205, 205, 0],
        [0, 0, 238],
        [205, 0, 205],
        [0, 205, 205],
        [229, 229, 229],
        [127, 127, 127],
        [255, 0, 0],
        [0, 255, 0],
        [255, 255, 0],
        [92, 92, 255],
        [255, 0, 255],
        [0, 255, 255],
        [255, 255, 255],
    ];
    let indexed = |n: u8| -> [u8; 3] {
        match n {
            0..=15 => ANSI[n as usize],
            16..=231 => {
                let n = n - 16;
                let level = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
                [level(n / 36), level(n / 6 % 6), level(n % 6)]
            }
            _ => {
                let v = 8 + (n - 232) * 10;
                [v, v, v]
            }
        }
    };
    match color {
        Color::Rgb(r, g, b) => [r, g, b],
        Color::Indexed(n) => indexed(n),
        Color::Black => ANSI[0],
        Color::Red => ANSI[1],
        Color::Green => ANSI[2],
        Color::Yellow => ANSI[3],
        Color::Blue => ANSI[4],
        Color::Magenta => ANSI[5],
        Color::Cyan => ANSI[6],
        Color::Gray => ANSI[7],
        Color::DarkGray => ANSI[8],
        Color::LightRed => ANSI[9],
        Color::LightGreen => ANSI[10],
        Color::LightYellow => ANSI[11],
        Color::LightBlue => ANSI[12],
        Color::LightMagenta => ANSI[13],
        Color::LightCyan => ANSI[14],
        Color::White | Color::Reset => ANSI[15],
    }
}

/// The tones the atoms are drawn in, from the boxes' theme (light or dark,
/// src/ui/mod.rs).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Tones {
    /// A meter's track, a picture's border.
    pub rule: Color,
    /// Under a picture.
    pub sunken: Color,
    /// A meter's tick.
    pub light: Color,
    /// A picture's words in its text form.
    pub secondary: Color,
    /// A series' value past where it should be.
    pub failed: Color,
}

impl Tones {
    pub fn now() -> Tones {
        let theme = if crate::status::theme::light() {
            super::Theme::LIGHT
        } else {
            super::Theme::DARK
        };
        Tones {
            rule: theme.faint,
            sunken: theme.panel,
            light: theme.bright,
            secondary: theme.dim,
            failed: crate::status::theme::RED,
        }
    }
}

// ── Rastering ───────────────────────────────────────────────────────────────

/// A cell's width and height in pixels.
pub type Cell = (u16, u16);

fn paint(color: [u8; 3], alpha: f32) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color_rgba8(
        color[0],
        color[1],
        color[2],
        (alpha.clamp(0.0, 1.0) * 255.0) as u8,
    );
    p.anti_alias = true;
    p
}

fn round_rect(x: f32, y: f32, w: f32, h: f32, r: f32) -> Option<tiny_skia::Path> {
    if w <= 0.0 || h <= 0.0 {
        return None;
    }
    let r = r.min(w / 2.0).min(h / 2.0).max(0.0);
    // The arcs as cubic curves, k being a quarter circle's control length.
    let k = r * 0.552_284_8;
    let mut pb = PathBuilder::new();
    pb.move_to(x + r, y);
    pb.line_to(x + w - r, y);
    pb.cubic_to(x + w - r + k, y, x + w, y + r - k, x + w, y + r);
    pb.line_to(x + w, y + h - r);
    pb.cubic_to(x + w, y + h - r + k, x + w - r + k, y + h, x + w - r, y + h);
    pb.line_to(x + r, y + h);
    pb.cubic_to(x + r - k, y + h, x, y + h - r + k, x, y + h - r);
    pb.line_to(x, y + r);
    pb.cubic_to(x, y + r - k, x + r - k, y, x + r, y);
    pb.close();
    pb.finish()
}

fn fill(pm: &mut Pixmap, path: Option<tiny_skia::Path>, color: [u8; 3], alpha: f32) {
    if let Some(path) = path {
        pm.fill_path(
            &path,
            &paint(color, alpha),
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
}

fn canvas(w: u32, h: u32) -> Pixmap {
    Pixmap::new(w.max(1), h.max(1)).expect("a size of at least one pixel")
}

/// An image scaled to fit `w`×`h` (`cover`: to fill it, cropped), as a
/// pixmap, with where it goes inside.
fn scaled(img: &RgbaImage, w: u32, h: u32, cover: bool) -> (Pixmap, i32, i32) {
    let (iw, ih) = (img.width().max(1) as f32, img.height().max(1) as f32);
    let fit = (w as f32 / iw).min(h as f32 / ih);
    let fill = (w as f32 / iw).max(h as f32 / ih);
    let s = if cover { fill } else { fit };
    let (sw, sh) = (((iw * s) as u32).max(1), ((ih * s) as u32).max(1));
    let small = image::imageops::resize(img, sw, sh, image::imageops::FilterType::Triangle);
    let mut pm = canvas(sw, sh);
    for (px, out) in small.pixels().zip(pm.pixels_mut()) {
        let [r, g, b, a] = px.0;
        *out = tiny_skia::ColorU8::from_rgba(r, g, b, a).premultiply();
    }
    let x = (w as i32 - sw as i32) / 2;
    let y = (h as i32 - sh as i32) / 2;
    (pm, x, y)
}

/// A photo cropped to a circle the cells wide (or tall, the less).
pub fn raster_avatar(photo: &RgbaImage, cols: u16, rows: u16, cell: Cell) -> Pixmap {
    let (w, h) = (cols as u32 * cell.0 as u32, rows as u32 * cell.1 as u32);
    let d = w.min(h);
    let mut pm = canvas(w, h);
    let (x0, y0) = ((w - d) as f32 / 2.0, (h - d) as f32 / 2.0);
    let mut mask = Mask::new(w.max(1), h.max(1)).expect("a size of at least one pixel");
    if let Some(circle) =
        PathBuilder::from_circle(x0 + d as f32 / 2.0, y0 + d as f32 / 2.0, d as f32 / 2.0)
    {
        mask.fill_path(&circle, FillRule::Winding, true, Transform::identity());
    }
    let (img, x, y) = scaled(photo, d, d, true);
    pm.draw_pixmap(
        x + x0 as i32,
        y + y0 as i32,
        img.as_ref(),
        &PixmapPaint::default(),
        Transform::identity(),
        Some(&mask),
    );
    pm
}

/// A picture scaled to fit, on the sunken surface inside a 1 px border in
/// the rule's color with a 6 px corner.
pub fn raster_picture(img: &RgbaImage, cols: u16, rows: u16, cell: Cell, tones: &Tones) -> Pixmap {
    let (w, h) = (cols as u32 * cell.0 as u32, rows as u32 * cell.1 as u32);
    let mut pm = canvas(w, h);
    let (wf, hf) = (w as f32, h as f32);
    fill(
        &mut pm,
        round_rect(0.0, 0.0, wf, hf, 6.0),
        rgb(tones.rule),
        1.0,
    );
    fill(
        &mut pm,
        round_rect(1.0, 1.0, wf - 2.0, hf - 2.0, 5.0),
        rgb(tones.sunken),
        1.0,
    );
    let pad = 4;
    let (iw, ih) = (w.saturating_sub(2 * pad), h.saturating_sub(2 * pad));
    if iw > 0 && ih > 0 {
        let mut mask = Mask::new(w.max(1), h.max(1)).expect("a size of at least one pixel");
        if let Some(inside) = round_rect(1.0, 1.0, wf - 2.0, hf - 2.0, 5.0) {
            mask.fill_path(&inside, FillRule::Winding, true, Transform::identity());
        }
        let (scaled, x, y) = scaled(img, iw, ih, false);
        pm.draw_pixmap(
            x + pad as i32,
            y + pad as i32,
            scaled.as_ref(),
            &PixmapPaint::default(),
            Transform::identity(),
            Some(&mask),
        );
    }
    pm
}

/// A meter one row tall: a track in the rule's color and a fill to `frac`,
/// each 0.3 of the row tall and centered with a 3 px corner, and with
/// `tick` a light 2 px mark at that fraction, 0.8 of the row tall.
pub fn raster_meter(
    cols: u16,
    frac: f32,
    color: Color,
    tick: Option<f32>,
    cell: Cell,
    tones: &Tones,
) -> Pixmap {
    let (w, h) = (cols as f32 * cell.0 as f32, cell.1 as f32);
    let mut pm = canvas(w as u32, h as u32);
    let bar = (h * 0.3).max(2.0);
    let y = (h - bar) / 2.0;
    fill(
        &mut pm,
        round_rect(0.0, y, w, bar, 3.0),
        rgb(tones.rule),
        1.0,
    );
    let fw = w * frac.clamp(0.0, 1.0);
    fill(&mut pm, round_rect(0.0, y, fw, bar, 3.0), rgb(color), 1.0);
    if let Some(t) = tick {
        let th = h * 0.8;
        let tx = (w * t.clamp(0.0, 1.0) - 1.0).clamp(0.0, (w - 2.0).max(0.0));
        if let Some(r) = SkRect::from_xywh(tx, (h - th) / 2.0, 2.0, th) {
            pm.fill_rect(
                r,
                &paint(rgb(tones.light), 1.0),
                Transform::identity(),
                None,
            );
        }
    }
    pm
}

/// One value of a series: its parts, stacked from the bottom, each with
/// its color (one part for a plain value).
pub type Bar = Vec<(f32, Color)>;

/// A series as bars at pixel resolution, each 76% of its slot with a
/// 1.5 px corner, stacked when a value has parts; what of a value is past
/// `over` is in the failed tone. One row tall it is a sparkline.
pub fn raster_series(
    cols: u16,
    rows: u16,
    bars: &[Bar],
    max: f32,
    over: Option<f32>,
    cell: Cell,
    tones: &Tones,
) -> Pixmap {
    let (w, h) = (cols as f32 * cell.0 as f32, rows as f32 * cell.1 as f32);
    let mut pm = canvas(w as u32, h as u32);
    if bars.is_empty() || max <= 0.0 {
        return pm;
    }
    let slot = w / bars.len() as f32;
    let bw = (slot * 0.76).max(1.0);
    let px = |v: f32| (v / max).clamp(0.0, 1.0) * h;
    for (i, bar) in bars.iter().enumerate() {
        let x = i as f32 * slot + (slot - bw) / 2.0;
        let mut below = 0.0f32;
        for &(v, color) in bar {
            let v = v.max(0.0);
            let (from, to) = (below, below + v);
            let mut part = |a: f32, b: f32, c: Color| {
                let (y0, y1) = (h - px(b), h - px(a));
                fill(&mut pm, round_rect(x, y0, bw, y1 - y0, 1.5), rgb(c), 1.0);
            };
            match over {
                Some(o) if to > o => {
                    if from < o {
                        part(from, o, color);
                    }
                    part(from.max(o), to, tones.failed);
                }
                _ => part(from, to, color),
            }
            below = to;
        }
    }
    pm
}

/// How far a box's shadow reaches past it on every side, in pixels.
pub const SHADOW_REACH: u32 = 34;

/// The soft shadow under a box `cols`×`rows` cells: the box's rectangle
/// with a 7 px corner, blurred about 34 px and 14 px down at 60% black,
/// with a tighter one (3 px down, 8 px blur, 45%) over it; larger than the
/// box by `SHADOW_REACH` on every side, and clear under the box's own
/// cells, since it is placed over the backgrounds and would darken the
/// box's surface.
pub fn raster_shadow(cols: u16, rows: u16, cell: Cell) -> Pixmap {
    let (bw, bh) = (cols as u32 * cell.0 as u32, rows as u32 * cell.1 as u32);
    let pad = SHADOW_REACH;
    let (w, h) = (bw + 2 * pad, bh + 2 * pad);
    let layer = |dy: f32, blur: f32, alpha: f32| -> Vec<f32> {
        let mut mask = Mask::new(w, h).expect("a size of at least one pixel");
        if let Some(r) = round_rect(pad as f32, pad as f32 + dy, bw as f32, bh as f32, 7.0) {
            mask.fill_path(&r, FillRule::Winding, true, Transform::identity());
        }
        let mut a: Vec<f32> = mask
            .data()
            .iter()
            .map(|&v| v as f32 / 255.0 * alpha)
            .collect();
        blur_alpha(&mut a, w as usize, h as usize, blur / 2.0);
        a
    };
    let wide = layer(14.0, 34.0, 0.6);
    let tight = layer(3.0, 8.0, 0.45);
    let mut pm = canvas(w, h);
    let under_box = |i: usize| {
        let (x, y) = ((i % w as usize) as u32, (i / w as usize) as u32);
        (pad..pad + bw).contains(&x) && (pad..pad + bh).contains(&y)
    };
    for (i, ((out, a), b)) in pm.pixels_mut().iter_mut().zip(wide).zip(tight).enumerate() {
        if under_box(i) {
            continue;
        }
        let alpha = a + b * (1.0 - a);
        *out = tiny_skia::ColorU8::from_rgba(0, 0, 0, (alpha.clamp(0.0, 1.0) * 255.0) as u8)
            .premultiply();
    }
    pm
}

/// A Gaussian blur of `sigma` pixels, as three box blurs each way.
fn blur_alpha(a: &mut [f32], w: usize, h: usize, sigma: f32) {
    if sigma < 0.5 || w == 0 || h == 0 {
        return;
    }
    let r = (((12.0 * sigma * sigma / 3.0) + 1.0).sqrt() - 1.0) / 2.0;
    let r = r.round().max(1.0) as usize;
    let mut tmp = vec![0.0f32; a.len()];
    for _ in 0..3 {
        box_pass(a, &mut tmp, w, h, r, true);
        box_pass(&tmp, a, w, h, r, false);
    }
}

fn box_pass(src: &[f32], dst: &mut [f32], w: usize, h: usize, r: usize, across: bool) {
    let (lines, len) = if across { (h, w) } else { (w, h) };
    let at = |line: usize, i: usize| if across { line * w + i } else { i * w + line };
    let span = (2 * r + 1) as f32;
    for line in 0..lines {
        let get = |i: isize| -> f32 {
            if i < 0 || i as usize >= len {
                0.0
            } else {
                src[at(line, i as usize)]
            }
        };
        let mut sum: f32 = (-(r as isize)..=r as isize).map(get).sum();
        for i in 0..len {
            dst[at(line, i)] = sum / span;
            sum += get(i as isize + r as isize + 1) - get(i as isize - r as isize);
        }
    }
}

/// A pixmap as a PNG.
pub fn png(pm: &Pixmap) -> Vec<u8> {
    use image::ImageEncoder;
    let mut data = Vec::with_capacity(pm.data().len());
    for p in pm.pixels() {
        let c = p.demultiply();
        data.extend_from_slice(&[c.red(), c.green(), c.blue(), c.alpha()]);
    }
    let mut out = Vec::new();
    let _ = image::codecs::png::PngEncoder::new(&mut out).write_image(
        &data,
        pm.width(),
        pm.height(),
        image::ExtendedColorType::Rgba8,
    );
    out
}

// ── Placing ─────────────────────────────────────────────────────────────────

/// Where an image goes on the screen: by placeholders in the frame's
/// cells, or placed directly under the text at a cell with an offset in
/// pixels (the shadow).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Spot {
    Placeholders,
    Under { x: u16, y: u16, dx: u16, dy: u16 },
}

/// An image a frame places: its id, its size in cells, where it goes, a
/// key made of everything it is drawn from, and how to draw it, which
/// runs only when the client hasn't the image with that key already.
pub struct Placed {
    pub id: u32,
    pub cols: u16,
    pub rows: u16,
    pub spot: Spot,
    pub key: u64,
    pub draw: Box<dyn Fn() -> Pixmap>,
}

impl std::fmt::Debug for Placed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Placed")
            .field("id", &self.id)
            .field("cols", &self.cols)
            .field("rows", &self.rows)
            .field("spot", &self.spot)
            .finish()
    }
}

fn key_of(parts: impl Hash) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    parts.hash(&mut h);
    h.finish()
}

/// Writes an image's placeholder cells into the frame at `area`'s corner.
fn put_placeholders(frame: &mut Frame, id: u32, area: Rect) {
    let area = area.intersection(frame.buf.area);
    let style = Style::new().fg(id_color(id));
    for r in 0..area.height {
        for c in 0..area.width {
            frame.buf[(area.x + c, area.y + r)]
                .set_symbol(&placeholder(r, c))
                .set_style(style);
        }
    }
    frame.links.clear_area(area);
}

/// Places an image by placeholders, if the area fits the marks.
fn place(frame: &mut Frame, name: &str, area: Rect, key: u64, draw: Box<dyn Fn() -> Pixmap>) {
    let id = id_of(name);
    let (cols, rows) = (area.width, area.height);
    put_placeholders(frame, id, area);
    frame.images.push(Placed {
        id,
        cols,
        rows,
        spot: Spot::Placeholders,
        key,
        draw,
    });
}

/// Whether an area can be an image: inside the frame and no wider or taller
/// than the marks can number.
fn placeable(frame: &Frame, area: Rect) -> bool {
    area.width > 0
        && area.height > 0
        && area.intersection(frame.buf.area) == area
        && (area.width as usize) <= MARKS.len()
        && (area.height as usize) <= MARKS.len()
}

fn put_text(frame: &mut Frame, x: u16, y: u16, text: &str, style: Style, room: u16) {
    if y >= frame.buf.area.bottom() {
        return;
    }
    let mut x = x;
    let end = (x + room).min(frame.buf.area.right());
    for ch in text.chars() {
        let w = super::draw::width(ch.encode_utf8(&mut [0; 4])) as u16;
        if x + w > end {
            break;
        }
        frame.buf[(x, y)]
            .set_symbol(ch.encode_utf8(&mut [0; 4]))
            .set_style(style);
        x += w.max(1);
    }
}

// ── The atoms ───────────────────────────────────────────────────────────────

/// Someone's avatar in `area`: their photo cropped to a circle where the
/// terminal draws images and there is a photo, else their initial on their
/// color.
pub fn avatar(
    frame: &mut Frame,
    name: &str,
    area: Rect,
    who: &str,
    color: Color,
    photo: Option<&RgbaImage>,
) {
    let area = area.intersection(frame.buf.area);
    if let (Some(cell), Some(photo)) = (frame.graphics, photo)
        && placeable(frame, area)
    {
        let key = key_of((
            "avatar",
            area.width,
            area.height,
            cell,
            photo.dimensions(),
            photo.as_raw(),
        ));
        let photo = photo.clone();
        let (cols, rows) = (area.width, area.height);
        place(
            frame,
            name,
            area,
            key,
            Box::new(move || raster_avatar(&photo, cols, rows, cell)),
        );
        return;
    }
    let initial: String = who
        .chars()
        .find(|c| c.is_alphanumeric())
        .map(|c| c.to_uppercase().collect())
        .unwrap_or_else(|| "?".into());
    let [r, g, b] = rgb(color);
    let lum = 0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32;
    let fg = if lum > 140.0 {
        Color::Indexed(232)
    } else {
        Color::Indexed(255)
    };
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            frame.buf[(x, y)]
                .set_symbol(" ")
                .set_style(Style::new().bg(color));
        }
    }
    frame.links.clear_area(area);
    let mid = area.y + area.height.saturating_sub(1) / 2;
    let x = area.x + area.width.saturating_sub(1) / 2;
    put_text(
        frame,
        x,
        mid,
        &initial,
        Style::new().fg(fg).bg(color).bold(),
        area.right() - x,
    );
}

/// A picture in `area`, scaled to fit inside its border; as text, its name
/// and that Enter shows it full size.
pub fn picture(frame: &mut Frame, name: &str, area: Rect, title: &str, img: &RgbaImage) {
    let content = picture_key(img);
    picture_shared(
        frame,
        name,
        area,
        title,
        &std::sync::Arc::new(img.clone()),
        content,
    );
}

/// A key made of a picture's pixels, for `picture_shared`.
pub fn picture_key(img: &RgbaImage) -> u64 {
    key_of((img.dimensions(), img.as_raw()))
}

/// `picture`, for a picture drawn on every frame while it is up (one shown
/// full size): `content` is a key the caller made once from the picture,
/// so a draw neither hashes nor copies its pixels.
pub fn picture_shared(
    frame: &mut Frame,
    name: &str,
    area: Rect,
    title: &str,
    img: &std::sync::Arc<RgbaImage>,
    content: u64,
) {
    let area = area.intersection(frame.buf.area);
    let tones = Tones::now();
    if let Some(cell) = frame.graphics
        && placeable(frame, area)
    {
        let key = key_of(("picture", area.width, area.height, cell, tones, content));
        let img = img.clone();
        let (cols, rows) = (area.width, area.height);
        place(
            frame,
            name,
            area,
            key,
            Box::new(move || raster_picture(&img, cols, rows, cell, &tones)),
        );
        return;
    }
    if area.height == 0 {
        return;
    }
    frame.links.clear_area(area);
    let style = Style::new().fg(tones.secondary);
    put_text(frame, area.x, area.y, title, style, area.width);
    if area.height > 1 {
        put_text(
            frame,
            area.x,
            area.y + 1,
            "Enter shows it full size",
            style,
            area.width,
        );
    }
}

/// A meter one row tall across `area`, filled to `frac` in `color`, with a
/// tick at `tick`; as text, `━` in the fill's color and the rule's, the
/// tick a `│`.
pub fn meter(
    frame: &mut Frame,
    name: &str,
    area: Rect,
    frac: f32,
    color: Color,
    tick: Option<f32>,
) {
    let area = Rect {
        height: area.height.min(1),
        ..area
    }
    .intersection(frame.buf.area);
    let tones = Tones::now();
    if let Some(cell) = frame.graphics
        && placeable(frame, area)
    {
        let key = key_of((
            "meter",
            area.width,
            cell,
            tones,
            frac.to_bits(),
            color,
            tick.map(f32::to_bits),
        ));
        let cols = area.width;
        place(
            frame,
            name,
            area,
            key,
            Box::new(move || raster_meter(cols, frac, color, tick, cell, &tones)),
        );
        return;
    }
    if area.height == 0 {
        return;
    }
    frame.links.clear_area(area);
    let w = area.width as f32;
    let filled = (w * frac.clamp(0.0, 1.0)).round() as u16;
    let tick_at = tick.map(|t| ((w * t.clamp(0.0, 1.0)) as u16).min(area.width.saturating_sub(1)));
    for i in 0..area.width {
        let cell = &mut frame.buf[(area.x + i, area.y)];
        if tick_at == Some(i) {
            cell.set_symbol("│").set_style(Style::new().fg(tones.light));
        } else {
            let fg = if i < filled { color } else { tones.rule };
            cell.set_symbol("━").set_style(Style::new().fg(fg));
        }
    }
}

const STEPS: [&str; 8] = ["▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"];

/// A series in `area`, a bar per value scaled to `max`, stacked where a
/// value has parts, what is past `over` in the failed tone; one row tall
/// it is a sparkline. As text, `▁`…`█` per value, by its total, in its top
/// part's color, one value a column.
pub fn series(
    frame: &mut Frame,
    name: &str,
    area: Rect,
    bars: &[Bar],
    max: f32,
    over: Option<f32>,
) {
    let area = area.intersection(frame.buf.area);
    let tones = Tones::now();
    if let Some(cell) = frame.graphics
        && placeable(frame, area)
    {
        let shape: Vec<Vec<(u32, Color)>> = bars
            .iter()
            .map(|b| b.iter().map(|&(v, c)| (v.to_bits(), c)).collect())
            .collect();
        let key = key_of((
            "series",
            area.width,
            area.height,
            cell,
            tones,
            shape,
            max.to_bits(),
            over.map(f32::to_bits),
        ));
        let bars = bars.to_vec();
        let (cols, rows) = (area.width, area.height);
        place(
            frame,
            name,
            area,
            key,
            Box::new(move || raster_series(cols, rows, &bars, max, over, cell, &tones)),
        );
        return;
    }
    if area.height == 0 || max <= 0.0 {
        return;
    }
    frame.links.clear_area(area);
    let shown = &bars[bars.len().saturating_sub(area.width as usize)..];
    let eighths = area.height as f32 * 8.0;
    for (i, bar) in shown.iter().enumerate() {
        let total: f32 = bar.iter().map(|(v, _)| v.max(0.0)).sum();
        let color = match over {
            Some(o) if total > o => tones.failed,
            _ => bar.last().map(|&(_, c)| c).unwrap_or(tones.rule),
        };
        let mut left = ((total / max).clamp(0.0, 1.0) * eighths).round() as u16;
        if total > 0.0 {
            left = left.max(1);
        }
        let x = area.x + i as u16;
        for r in (0..area.height).rev() {
            let y = area.y + r;
            let here = left.min(8);
            left -= here;
            let symbol = if here == 0 {
                " "
            } else {
                STEPS[here as usize - 1]
            };
            frame.buf[(x, y)]
                .set_symbol(symbol)
                .set_style(Style::new().fg(color));
        }
    }
}

/// The shadow under a box at `area`: an image placed under the text,
/// reaching past the box; as text, half blocks down its right and along
/// its bottom in a near-black shade.
pub fn shadow(frame: &mut Frame, name: &str, area: Rect) {
    if let Some(cell) = frame.graphics
        && area.width > 0
        && area.height > 0
    {
        let (cw, ch) = (cell.0.max(1) as i64, cell.1.max(1) as i64);
        let reach = SHADOW_REACH as i64;
        let (px, py) = (area.x as i64 * cw - reach, area.y as i64 * ch - reach);
        // An image can't start left of or above the screen: cropped there.
        let (crop_x, crop_y) = ((-px).max(0) as u32, (-py).max(0) as u32);
        let (px, py) = (px.max(0), py.max(0));
        let spot = Spot::Under {
            x: (px / cw) as u16,
            y: (py / ch) as u16,
            dx: (px % cw) as u16,
            dy: (py % ch) as u16,
        };
        let (cols, rows) = (area.width, area.height);
        let key = key_of(("shadow", cols, rows, cell, spot, crop_x, crop_y));
        frame.images.push(Placed {
            id: id_of(name),
            cols: 0,
            rows: 0,
            spot,
            key,
            draw: Box::new(move || {
                let full = raster_shadow(cols, rows, cell);
                if crop_x == 0 && crop_y == 0 {
                    return full;
                }
                let (w, h) = (
                    full.width().saturating_sub(crop_x).max(1),
                    full.height().saturating_sub(crop_y).max(1),
                );
                let mut out = canvas(w, h);
                out.draw_pixmap(
                    -(crop_x as i32),
                    -(crop_y as i32),
                    full.as_ref(),
                    &PixmapPaint::default(),
                    Transform::identity(),
                    None,
                );
                out
            }),
        });
        return;
    }
    shadow_cells(&mut frame.buf, area);
}

/// The shadow under a box as text: half blocks down its right and along its
/// bottom in a near-black shade, for a terminal that draws no images, and
/// for the apps in panes, which draw into a buffer of their own.
pub fn shadow_cells(buf: &mut Buffer, area: Rect) {
    let screen = buf.area;
    let shade = Style::new().fg(Color::Indexed(233));
    let right = area.right();
    if right < screen.right() {
        for y in area.top() + 1..area.bottom().min(screen.bottom()) + 1 {
            if y < screen.bottom() {
                buf[(right, y)].set_symbol("▌").set_style(shade);
            }
        }
    }
    let below = area.bottom();
    if below < screen.bottom() {
        for x in area.left() + 1..right.min(screen.right()) {
            buf[(x, below)].set_symbol("▀").set_style(shade);
        }
    }
}

// ── A tab's history (board 6) ───────────────────────────────────────────────

/// A vertical line across a lane or a chart, at a fraction of its width: the
/// moment a tab's history shows, or a compaction (dashed).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rule {
    pub at: f32,
    pub color: Color,
    pub dashed: bool,
}

impl Hash for Rule {
    fn hash<H: Hasher>(&self, h: &mut H) {
        self.at.to_bits().hash(h);
        self.color.hash(h);
        self.dashed.hash(h);
    }
}

/// A part of a lane: from and to, as fractions of its width, and its color.
pub type Segment = (f32, f32, Color);

fn draw_rules(pm: &mut Pixmap, rules: &[Rule], w: f32, h: f32) {
    for r in rules {
        let x = (w * r.at.clamp(0.0, 1.0) - 1.0).clamp(0.0, (w - 2.0).max(0.0));
        let dash = if r.dashed { 3.0 } else { h };
        let mut y = 0.0;
        while y < h {
            if let Some(rect) = SkRect::from_xywh(x, y, 2.0, dash.min(h - y)) {
                pm.fill_rect(rect, &paint(rgb(r.color), 1.0), Transform::identity(), None);
            }
            y += dash * 2.0;
        }
    }
}

/// A lane one row tall: each segment a bar 0.6 of the row tall with a 4 px
/// corner and a pixel's gap at each end, and the rules over them.
pub fn raster_lane(cols: u16, segments: &[Segment], rules: &[Rule], cell: Cell) -> Pixmap {
    let (w, h) = (cols as f32 * cell.0 as f32, cell.1 as f32);
    let mut pm = canvas(w as u32, h as u32);
    let bar = h * 0.6;
    for &(from, to, color) in segments {
        let (x0, x1) = (w * from.clamp(0.0, 1.0), w * to.clamp(0.0, 1.0));
        fill(
            &mut pm,
            round_rect(
                x0 + 1.0,
                (h - bar) / 2.0,
                (x1 - x0 - 2.0).max(1.0),
                bar,
                4.0,
            ),
            rgb(color),
            1.0,
        );
    }
    draw_rules(&mut pm, rules, w, h);
    pm
}

/// A quantity over time as a filled area: `values`, each a fraction of the
/// chart's height (`None` where nothing is known), evenly across its width,
/// in `color`, and the rules over it.
pub fn raster_chart(
    cols: u16,
    rows: u16,
    values: &[Option<f32>],
    color: Color,
    rules: &[Rule],
    cell: Cell,
) -> Pixmap {
    let (w, h) = (cols as f32 * cell.0 as f32, rows as f32 * cell.1 as f32);
    let mut pm = canvas(w as u32, h as u32);
    if !values.is_empty() {
        let slot = w / values.len() as f32;
        for (i, v) in values.iter().enumerate() {
            let Some(v) = v else { continue };
            let bh = v.clamp(0.0, 1.0) * h;
            if let Some(rect) = SkRect::from_xywh(i as f32 * slot, h - bh, slot.max(1.0), bh) {
                pm.fill_rect(rect, &paint(rgb(color), 0.7), Transform::identity(), None);
            }
        }
    }
    draw_rules(&mut pm, rules, w, h);
    pm
}

/// The column of an area a fraction of its width falls in.
fn column_of(at: f32, cols: u16) -> u16 {
    ((at.clamp(0.0, 1.0) * cols as f32) as u16).min(cols.saturating_sub(1))
}

/// A lane one row tall across `area`: its segments as bars, its rules as
/// lines. As text, `━` in each segment's color, a rule a `│`.
pub fn lane(frame: &mut Frame, name: &str, area: Rect, segments: &[Segment], rules: &[Rule]) {
    let area = Rect {
        height: area.height.min(1),
        ..area
    }
    .intersection(frame.buf.area);
    if let Some(cell) = frame.graphics
        && placeable(frame, area)
    {
        let shape: Vec<(u32, u32, Color)> = segments
            .iter()
            .map(|&(a, b, c)| (a.to_bits(), b.to_bits(), c))
            .collect();
        let key = key_of(("lane", area.width, cell, shape, rules));
        let (segments, rules, cols) = (segments.to_vec(), rules.to_vec(), area.width);
        place(
            frame,
            name,
            area,
            key,
            Box::new(move || raster_lane(cols, &segments, &rules, cell)),
        );
        return;
    }
    if area.height == 0 || area.width == 0 {
        return;
    }
    frame.links.clear_area(area);
    for i in 0..area.width {
        let mid = (i as f32 + 0.5) / area.width as f32;
        let cell = &mut frame.buf[(area.x + i, area.y)];
        match segments.iter().find(|(a, b, _)| *a <= mid && mid < *b) {
            Some(&(_, _, color)) => cell.set_symbol("━").set_style(Style::new().fg(color)),
            None => cell.set_symbol(" ").set_style(Style::new()),
        };
    }
    for r in rules {
        let x = area.x + column_of(r.at, area.width);
        frame.buf[(x, area.y)]
            .set_symbol("│")
            .set_style(Style::new().fg(r.color));
    }
}

/// A chart in `area` of `values` (fractions of its height, `None` where
/// nothing is known), filled in `color`, with its rules. As text, `▁`…`█`
/// per column by the most of its values; a dashed rule a `┆` where the
/// column is empty, a solid one a `│`.
pub fn chart(
    frame: &mut Frame,
    name: &str,
    area: Rect,
    values: &[Option<f32>],
    color: Color,
    rules: &[Rule],
) {
    let area = area.intersection(frame.buf.area);
    if let Some(cell) = frame.graphics
        && placeable(frame, area)
    {
        let shape: Vec<Option<u32>> = values.iter().map(|v| v.map(f32::to_bits)).collect();
        let key = key_of(("chart", area.width, area.height, cell, shape, color, rules));
        let (values, rules) = (values.to_vec(), rules.to_vec());
        let (cols, rows) = (area.width, area.height);
        place(
            frame,
            name,
            area,
            key,
            Box::new(move || raster_chart(cols, rows, &values, color, &rules, cell)),
        );
        return;
    }
    if area.height == 0 || area.width == 0 {
        return;
    }
    frame.links.clear_area(area);
    let per = values.len() as f32 / area.width as f32;
    let eighths = area.height as f32 * 8.0;
    for i in 0..area.width {
        let from = ((i as f32 * per) as usize).min(values.len());
        let to = (((i + 1) as f32 * per) as usize).clamp(from, values.len());
        let most = values[from..to]
            .iter()
            .flatten()
            .fold(None, |m: Option<f32>, v| Some(m.map_or(*v, |m| m.max(*v))));
        let mut left = most.map_or(0, |v| ((v.clamp(0.0, 1.0) * eighths).round() as u16).max(1));
        for r in (0..area.height).rev() {
            let here = left.min(8);
            left -= here;
            let symbol = if here == 0 {
                " "
            } else {
                STEPS[here as usize - 1]
            };
            frame.buf[(area.x + i, area.y + r)]
                .set_symbol(symbol)
                .set_style(Style::new().fg(color));
        }
    }
    for rule in rules {
        let x = area.x + column_of(rule.at, area.width);
        let symbol = if rule.dashed { "┆" } else { "│" };
        for r in 0..area.height {
            let cell = &mut frame.buf[(x, area.y + r)];
            if cell.symbol() == " " || !rule.dashed {
                cell.set_symbol(symbol)
                    .set_style(Style::new().fg(rule.color));
            }
        }
    }
}

// ── Sending ─────────────────────────────────────────────────────────────────

/// One kitty graphics command carrying `data` base64'd in chunks of 4096
/// bytes, `keys` on the first; with no data, the keys alone.
fn command(keys: &str, data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    if data.is_empty() {
        out.extend_from_slice(format!("\x1b_G{keys}\x1b\\").as_bytes());
        return out;
    }
    let encoded = util::base64(data);
    let chunks: Vec<&[u8]> = encoded.as_bytes().chunks(4096).collect();
    for (i, chunk) in chunks.iter().enumerate() {
        let more = u8::from(i + 1 < chunks.len());
        out.extend_from_slice(b"\x1b_G");
        if i == 0 {
            out.extend_from_slice(format!("{keys},m={more};").as_bytes());
        } else {
            out.extend_from_slice(format!("m={more};").as_bytes());
        }
        out.extend_from_slice(chunk);
        out.extend_from_slice(b"\x1b\\");
    }
    out
}

fn delete(id: u32) -> Vec<u8> {
    command(&format!("a=d,d=I,i={id},q=2"), &[])
}

/// The images one client's terminal holds, by id, each with its key.
#[derive(Default, Debug)]
pub struct ImageCache {
    sent: HashMap<u32, u64>,
}

impl ImageCache {
    /// The commands that bring the terminal from what it holds to what the
    /// frame places: an id new, or placed with another key, is sent (the
    /// old under it deleted first), and one no longer placed is deleted.
    pub fn commands(&mut self, placed: &[Placed]) -> Vec<u8> {
        let mut out = Vec::new();
        let now: HashMap<u32, &Placed> = placed.iter().map(|p| (p.id, p)).collect();
        let mut gone: Vec<u32> = self
            .sent
            .keys()
            .filter(|id| !now.contains_key(id))
            .copied()
            .collect();
        gone.sort_unstable();
        for id in gone {
            out.extend(delete(id));
            self.sent.remove(&id);
        }
        let mut done = std::collections::HashSet::new();
        for p in placed {
            if !done.insert(p.id) || self.sent.get(&p.id) == Some(&p.key) {
                continue;
            }
            if self.sent.contains_key(&p.id) {
                out.extend(delete(p.id));
            }
            let data = png(&(p.draw)());
            match p.spot {
                Spot::Placeholders => out.extend(command(
                    &format!("a=T,U=1,f=100,t=d,q=2,i={},c={},r={}", p.id, p.cols, p.rows),
                    &data,
                )),
                Spot::Under { x, y, dx, dy } => {
                    out.extend_from_slice(format!("\x1b7\x1b[{};{}H", y + 1, x + 1).as_bytes());
                    out.extend(command(
                        &format!("a=T,f=100,t=d,q=2,i={},X={dx},Y={dy},z=-1,C=1", p.id),
                        &data,
                    ));
                    out.extend_from_slice(b"\x1b8");
                }
            }
            self.sent.insert(p.id, p.key);
        }
        out
    }

    /// Every image deleted, and nothing taken as held, so the next frame
    /// sends them all again (the terminal may have lost them).
    pub fn forget(&mut self) -> Vec<u8> {
        let mut ids: Vec<u32> = self.sent.drain().map(|(id, _)| id).collect();
        ids.sort_unstable();
        ids.into_iter().flat_map(delete).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CELL: Cell = (8, 16);

    fn graphics_frame(cols: u16, rows: u16) -> Frame {
        let mut f = Frame::new(cols, rows);
        f.graphics = Some(CELL);
        f
    }

    fn text(out: &[u8]) -> String {
        String::from_utf8_lossy(out).into_owned()
    }

    fn red() -> Color {
        Color::Indexed(196)
    }

    #[test]
    fn a_kitty_or_ghostty_terminal_speaks_the_protocol_and_others_do_not() {
        assert!(speaks_kitty("xterm-ghostty"));
        assert!(speaks_kitty("xterm-kitty"));
        assert!(!speaks_kitty("xterm-256color"));
        assert!(!speaks_kitty(""));
    }

    #[test]
    fn each_atom_rasters_at_its_cells_times_the_cell_and_the_same_input_gives_the_same_bytes() {
        let tones = Tones::now();
        let photo = RgbaImage::from_pixel(10, 10, image::Rgba([10, 200, 30, 255]));
        let cases = [
            (raster_avatar(&photo, 2, 1, CELL), (16, 16)),
            (raster_picture(&photo, 6, 3, CELL, &tones), (48, 48)),
            (
                raster_meter(10, 0.5, red(), Some(0.7), CELL, &tones),
                (80, 16),
            ),
            (
                raster_series(
                    4,
                    2,
                    &[
                        vec![(1.0, red())],
                        vec![(2.0, red()), (1.0, Color::Indexed(41))],
                    ],
                    3.0,
                    Some(2.5),
                    CELL,
                    &tones,
                ),
                (32, 32),
            ),
            (
                raster_shadow(4, 2, CELL),
                (32 + 2 * SHADOW_REACH, 32 + 2 * SHADOW_REACH),
            ),
        ];
        for (pm, (w, h)) in &cases {
            assert_eq!((pm.width(), pm.height()), (*w, *h));
        }
        assert_eq!(
            png(&raster_meter(10, 0.5, red(), Some(0.7), CELL, &tones)),
            png(&raster_meter(10, 0.5, red(), Some(0.7), CELL, &tones))
        );
        // The meter's fill is red at its left, and the track the rule's.
        let m = raster_meter(10, 0.5, red(), None, CELL, &tones);
        let mid = m.pixel(10, 8).unwrap().demultiply();
        assert_eq!([mid.red(), mid.green(), mid.blue()], rgb(red()));
        let shadow = raster_shadow(4, 2, CELL);
        let below = shadow.pixel(SHADOW_REACH + 16, SHADOW_REACH + 2 * CELL.1 as u32 + 4);
        let edge = shadow.pixel(0, 0).unwrap();
        assert!(below.unwrap().alpha() > edge.alpha());
        // Clear under the box, whose surface it would darken.
        let under = shadow.pixel(SHADOW_REACH + 16, SHADOW_REACH + 16).unwrap();
        assert_eq!(under.alpha(), 0);
    }

    #[test]
    fn an_atom_places_placeholders_colored_with_its_id_in_the_server_s_range() {
        let mut f = graphics_frame(20, 3);
        meter(
            &mut f,
            "usage/meter",
            Rect::new(2, 1, 10, 1),
            0.4,
            red(),
            None,
        );
        assert_eq!(f.images.len(), 1);
        let id = f.images[0].id;
        assert_eq!(id, id_of("usage/meter"));
        assert!(id & SERVER_IDS != 0);
        let c = &f.buf[(2, 1)];
        assert!(c.symbol().starts_with(PLACEHOLDER));
        assert_eq!(c.fg, id_color(id));
        assert_eq!(f.buf[(11, 1)].symbol(), placeholder(0, 9));
        assert_eq!(f.buf[(12, 1)].symbol(), " ");
    }

    #[test]
    fn the_cache_sends_once_replaces_a_change_and_deletes_what_goes() {
        let mut cache = ImageCache::default();
        let mut f = graphics_frame(20, 3);
        meter(&mut f, "m", Rect::new(0, 0, 10, 1), 0.4, red(), None);
        let id = id_of("m");
        let first = text(&cache.commands(&f.images));
        assert!(first.starts_with(&format!("\x1b_Ga=T,U=1,f=100,t=d,q=2,i={id},c=10,r=1,m=")));
        assert_eq!(first.matches("a=T").count(), 1);

        let mut same = graphics_frame(20, 3);
        meter(&mut same, "m", Rect::new(0, 0, 10, 1), 0.4, red(), None);
        assert!(cache.commands(&same.images).is_empty());

        let mut changed = graphics_frame(20, 3);
        meter(&mut changed, "m", Rect::new(0, 0, 10, 1), 0.9, red(), None);
        let again = text(&cache.commands(&changed.images));
        assert!(again.starts_with(&format!("\x1b_Ga=d,d=I,i={id},q=2\x1b\\\x1b_Ga=T")));

        let empty = graphics_frame(20, 3);
        assert_eq!(
            text(&cache.commands(&empty.images)),
            format!("\x1b_Ga=d,d=I,i={id},q=2\x1b\\")
        );
        assert!(cache.commands(&empty.images).is_empty());

        cache.commands(&f.images);
        assert!(!cache.forget().is_empty());
        assert!(cache.commands(&f.images).windows(3).any(|w| w == b"a=T"));
    }

    #[test]
    fn a_large_image_is_sent_in_chunks_and_every_command_carries_its_data_inline() {
        let mut cache = ImageCache::default();
        let mut f = graphics_frame(60, 20);
        let noise = RgbaImage::from_fn(200, 200, |x, y| {
            image::Rgba([
                (x * 7 % 256) as u8,
                (y * 13 % 256) as u8,
                ((x ^ y) % 256) as u8,
                255,
            ])
        });
        picture(&mut f, "p", Rect::new(0, 0, 40, 15), "shot.png", &noise);
        shadow(&mut f, "box", Rect::new(5, 5, 10, 4));
        let out = text(&cache.commands(&f.images));
        let commands: Vec<&str> = out.split("\x1b_G").skip(1).collect();
        assert!(commands.len() > 2);
        assert!(out.contains(",m=1;"));
        assert!(commands.last().unwrap().starts_with("m=0;"));
        for c in &commands {
            let keys = c.split(';').next().unwrap();
            assert!(!keys.contains("t=f") && !keys.contains("t=t") && !keys.contains("t=s"));
        }
        assert!(out.contains("z=-1,C=1"));
    }

    #[test]
    fn ids_never_meet_a_note_s() {
        for name in ["a", "usage/chart", "avatar/Ana/2x1", ""] {
            assert!(id_of(name) >= SERVER_IDS && id_of(name) < 0x100_0000);
        }
        assert!(crate::notes::images::id_of("x") < SERVER_IDS);
    }

    #[test]
    fn with_graphics_off_each_atom_draws_its_text_form() {
        let mut f = Frame::new(20, 6);
        let photo = RgbaImage::new(4, 4);
        avatar(
            &mut f,
            "a",
            Rect::new(0, 0, 2, 1),
            "ana",
            red(),
            Some(&photo),
        );
        assert_eq!(f.buf[(0, 0)].symbol(), "A");
        assert_eq!(f.buf[(0, 0)].bg, red());
        meter(&mut f, "m", Rect::new(0, 1, 10, 1), 0.5, red(), Some(0.8));
        assert_eq!(f.buf[(0, 1)].symbol(), "━");
        assert_eq!(f.buf[(0, 1)].fg, red());
        assert_eq!(f.buf[(8, 1)].symbol(), "│");
        assert_ne!(f.buf[(6, 1)].fg, red());
        series(
            &mut f,
            "s",
            Rect::new(0, 2, 4, 1),
            &[vec![(0.0, red())], vec![(4.0, red())], vec![(8.0, red())]],
            8.0,
            None,
        );
        assert_eq!(f.buf[(1, 2)].symbol(), "▄");
        assert_eq!(f.buf[(2, 2)].symbol(), "█");
        picture(&mut f, "p", Rect::new(0, 3, 20, 2), "shot.png", &photo);
        assert_eq!(f.buf[(0, 3)].symbol(), "s");
        shadow(&mut f, "b", Rect::new(10, 0, 4, 2));
        assert_eq!(f.buf[(14, 1)].symbol(), "▌");
        assert_eq!(f.buf[(11, 2)].symbol(), "▀");
        assert!(f.images.is_empty());
    }

    #[test]
    fn an_avatar_without_a_photo_is_its_initial_even_with_graphics() {
        let mut f = graphics_frame(4, 1);
        avatar(&mut f, "a", Rect::new(0, 0, 2, 1), "Bo", red(), None);
        assert!(f.images.is_empty());
        assert_eq!(f.buf[(0, 0)].symbol(), "B");
    }
}
