//! Images the server draws itself: the image atoms of the UX 2.0
//! canvas's board 0 (an avatar's photo, a picture, a meter with a tick, a
//! series as a chart or a sparkline, a setting's on/off switch, the
//! keyboard as a heatmap of the keys pressed, and the soft shadow under a
//! box), and board 6's lane and chart over time (a tab's history), each
//! described into a frame here and rastered here in Rust at the frame's
//! cell, and turned into what reaches each client's terminal by
//! src/mux/graphics.rs, which alone decides how: kitty's graphics
//! protocol, sixel (src/mux/sixel), or the atom's text form.
//!
//! An atom is placed with kitty's Unicode placeholders, as a note's
//! picture is (src/notes/images.rs): its cells in the frame are U+10EEEE
//! marked with their row and column and colored with the image's id, so
//! they ride in the frame as text, move with their cells, and a box drawn
//! over them covers them as it covers text. A kitty terminal draws the
//! image on them; with sixel the server reads them back once everything
//! is drawn, and draws as sixel the parts of the image whose cells still
//! show, which is how a sixel is cut and covered as the text is. The
//! shadow alone is a direct placement under the text (`Spot::Under`),
//! since it must sit over the cells' backgrounds and under a box's words,
//! which a placeholder can't; where nothing can sit under the text (sixel,
//! `Graphics::under_text`) it is its text form.
//!
//! Each atom adds itself to the frame's `images` as a `Placed`: its id,
//! from the stable name its caller gives, a key made of everything it is
//! drawn from, and how to raster it (`Draw`), which runs only when a
//! terminal hasn't the image with that key already, off the server's task
//! where it is large enough to take a while.
//!
//! Each atom has a text form drawn in the same cells where the frame
//! carries no `Graphics` (a terminal that speaks neither protocol, or the
//! `server_images` setting off), and the atom's function picks the form,
//! so no part checks the terminal itself. An avatar is its text form, the
//! initial on the person's color, unless it has a photo, since the binary
//! carries no font to raster an initial with.

use std::hash::{Hash, Hasher};
use std::sync::Arc;

use image::RgbaImage;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use tiny_skia::{
    FillRule, Mask, Paint, PathBuilder, Pixmap, PixmapPaint, Rect as SkRect, Transform,
};

use crate::mux::compose::Frame;

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
    /// A switch's track while on.
    pub done: Color,
}

impl Tones {
    pub fn now() -> Tones {
        let theme = crate::status::theme::theme();
        Tones {
            rule: theme.faint,
            sunken: theme.panel,
            light: theme.bright,
            secondary: theme.dim,
            failed: crate::status::theme::RED,
            done: super::kit::Tone::Done.fg(&theme),
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

/// A closed tab's last screen (src/usage/shots.rs) as board 18's
/// thumbnail: on the sunken surface inside a 1 px border with a 6 px
/// corner, each run of its lines a rounded bar of its color as wide as it
/// was, its lines scaled to fit the cells, the last ones kept when they
/// don't all fit.
pub fn raster_screen(
    shot: &crate::usage::shots::Shot,
    cols: u16,
    rows: u16,
    cell: Cell,
    tones: &Tones,
) -> Pixmap {
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
    let pad = 6.0;
    let (iw, ih) = (wf - 2.0 * pad, hf - 2.0 * pad);
    if iw <= 0.0 || ih <= 0.0 || shot.lines.is_empty() {
        return pm;
    }
    // A line no shorter than 3 px, so a bar reads; the last lines kept.
    let line_h = (ih / shot.lines.len() as f32).max(3.0);
    let shown = ((ih / line_h) as usize).min(shot.lines.len());
    let from = shot.lines.len() - shown;
    let col_w = iw / f32::from(shot.cols.max(1));
    for (n, line) in shot.lines[from..].iter().enumerate() {
        let y = pad + n as f32 * line_h + line_h * 0.2;
        let bar_h = line_h * 0.6;
        let mut x = 0u32;
        for (run, color) in &line.runs {
            if let Some(c) = color {
                let bx = pad + x as f32 * col_w;
                let bw = (*run as f32 * col_w).min(pad + iw - bx);
                fill(&mut pm, round_rect(bx, y, bw, bar_h, bar_h / 2.0), *c, 0.85);
            }
            x += u32::from(*run);
        }
    }
    pm
}

/// A closed tab's last screen in `area`, as `raster_screen` draws it; as
/// text, its last lines, dim.
pub fn screen(
    frame: &mut Frame,
    name: &str,
    area: Rect,
    shot: &std::sync::Arc<crate::usage::shots::Shot>,
) {
    let area = area.intersection(frame.buf.area);
    let tones = Tones::now();
    if let Some(cell) = frame.graphics.map(|g| g.cell)
        && placeable(frame, area)
    {
        let key = key_of((
            "screen",
            area.width,
            area.height,
            cell,
            tones,
            shot.as_ref(),
        ));
        let shot = shot.clone();
        let (cols, rows) = (area.width, area.height);
        place(
            frame,
            name,
            area,
            key,
            false,
            Arc::new(move || raster_screen(&shot, cols, rows, cell, &tones)),
        );
        return;
    }
    frame.links.clear_area(area);
    let style = Style::new().fg(tones.secondary);
    for (row, text) in shot.last_text(area.height as usize).iter().enumerate() {
        put_text(frame, area.x, area.y + row as u16, text, style, area.width);
    }
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

/// One rounded end of kit's objects, one cell: the half circle U+E0B6
/// (`left`, round on its left and flat on its right, against the object)
/// or U+E0B4 in `color`, the rest of the cell clear, for a terminal that
/// draws neither the glyphs nor a font's (src/mux/graphics.rs's
/// `Ends::Pictures`).
pub fn raster_end(left: bool, color: [u8; 3], cell: Cell) -> Pixmap {
    let (w, h) = (cell.0.max(1) as f32, cell.1.max(1) as f32);
    let mut pm = canvas(w as u32, h as u32);
    let x = if left { 0.0 } else { -w };
    if let Some(oval) = SkRect::from_xywh(x, 0.0, 2.0 * w, h) {
        fill(&mut pm, PathBuilder::from_oval(oval), color, 1.0);
    }
    pm
}

/// How many columns a switch takes.
pub const SWITCH_COLS: u16 = 4;

/// A switch one row tall and `SWITCH_COLS` wide: a track 3.6 cells wide and
/// 0.64 of the row tall with a full radius, green when on and the rule's
/// color when off, and its knob a circle 0.48 of the row, dark at the
/// right when on, light at the left when off.
pub fn raster_switch(on: bool, cell: Cell, tones: &Tones) -> Pixmap {
    let (cw, h) = (cell.0 as f32, cell.1 as f32);
    let w = SWITCH_COLS as f32 * cw;
    let mut pm = canvas(w as u32, h as u32);
    let (tw, th) = (cw * 3.6, h * 0.64);
    let (tx, ty) = ((w - tw) / 2.0, (h - th) / 2.0);
    let track = if on { tones.done } else { tones.rule };
    fill(
        &mut pm,
        round_rect(tx, ty, tw, th, th / 2.0),
        rgb(track),
        1.0,
    );
    let d = h * 0.48;
    let inset = (th - d) / 2.0;
    let kx = if on { tx + tw - inset - d } else { tx + inset };
    let knob = if on {
        Color::Indexed(234)
    } else {
        Color::Indexed(255)
    };
    fill(
        &mut pm,
        round_rect(kx, (h - d) / 2.0, d, d, d / 2.0),
        rgb(knob),
        1.0,
    );
    pm
}

/// One value of a series: its parts, stacked from the bottom, each with
/// its color (one part for a plain value).
pub type Bar = Vec<(f32, Color)>;

/// What a series draws besides its bars: the part of every bar at
/// `outline` (a stacked bar's nth part, from the bottom) outlined in the
/// light tone, a dashed line across at each of `grid`'s heights (in the
/// values' units), and a line down its right edge in `now`'s color, where
/// the last bar is the time it is now.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Marks {
    pub outline: Option<usize>,
    pub grid: Vec<f32>,
    pub now: Option<Color>,
}

/// A series as bars at pixel resolution, each 76% of its slot with a
/// 1.5 px corner, stacked when a value has parts; what of a value is past
/// `over` is in the failed tone; with its `marks` over and under them.
/// One row tall it is a sparkline.
#[allow(clippy::too_many_arguments)]
pub fn raster_series(
    cols: u16,
    rows: u16,
    bars: &[Bar],
    max: f32,
    over: Option<f32>,
    marks: &Marks,
    cell: Cell,
    tones: &Tones,
) -> Pixmap {
    let (w, h) = (cols as f32 * cell.0 as f32, rows as f32 * cell.1 as f32);
    let mut pm = canvas(w as u32, h as u32);
    if max > 0.0 {
        for &at in &marks.grid {
            let y = (h - (at / max).clamp(0.0, 1.0) * h).clamp(0.0, h - 1.0);
            let mut x = 0.0;
            while x < w {
                if let Some(r) = SkRect::from_xywh(x, y, 3.0f32.min(w - x), 1.0) {
                    pm.fill_rect(r, &paint(rgb(tones.rule), 1.0), Transform::identity(), None);
                }
                x += 6.0;
            }
        }
    }
    if let Some(color) = marks.now
        && let Some(r) = SkRect::from_xywh((w - 2.0).max(0.0), 0.0, 2.0f32.min(w), h)
    {
        pm.fill_rect(r, &paint(rgb(color), 1.0), Transform::identity(), None);
    }
    if bars.is_empty() || max <= 0.0 {
        return pm;
    }
    let slot = w / bars.len() as f32;
    let bw = (slot * 0.76).max(1.0);
    let px = |v: f32| (v / max).clamp(0.0, 1.0) * h;
    for (i, bar) in bars.iter().enumerate() {
        let x = i as f32 * slot + (slot - bw) / 2.0;
        let mut below = 0.0f32;
        for (at, &(v, color)) in bar.iter().enumerate() {
            let v = v.max(0.0);
            if marks.outline == Some(at) && v > 0.0 {
                let (y0, y1) = (h - px(below + v), h - px(below));
                let ring = round_rect(x - 1.5, y0 - 1.5, bw + 3.0, y1 - y0 + 3.0, 2.5);
                fill(&mut pm, ring, rgb(tones.light), 1.0);
            }
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

/// A pixmap's pixels, no longer premultiplied.
pub fn rgba(pm: &Pixmap) -> RgbaImage {
    let mut data = Vec::with_capacity(pm.data().len());
    for p in pm.pixels() {
        let c = p.demultiply();
        data.extend_from_slice(&[c.red(), c.green(), c.blue(), c.alpha()]);
    }
    RgbaImage::from_raw(pm.width(), pm.height(), data).expect("four bytes a pixel")
}

/// A pixmap as a PNG.
pub fn png(pm: &Pixmap) -> Vec<u8> {
    use image::ImageEncoder;
    let pixels = rgba(pm);
    let mut out = Vec::new();
    let _ = image::codecs::png::PngEncoder::new(&mut out).write_image(
        pixels.as_raw(),
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

/// How an image is rastered: shared, and sent off the server's task to
/// run where it is large (src/mux/sixel).
pub type Draw = Arc<dyn Fn() -> Pixmap + Send + Sync>;

/// An image a frame places: its id, its size in cells, where it goes, a
/// key made of everything it is drawn from, whether it is drawn from a
/// picture of its own (a photo, a screenshot), and how to draw it, which
/// runs only when the client hasn't the image with that key already.
pub struct Placed {
    pub id: u32,
    pub cols: u16,
    pub rows: u16,
    pub spot: Spot,
    pub key: u64,
    /// Drawn from a picture of its own, of more colours than a palette
    /// holds as they are, so with sixel its palette is learned off the
    /// server's task (src/mux/sixel); a drawing of a few colours (a meter,
    /// a switch, a chart) is drawn as the frame is.
    pub photo: bool,
    pub draw: Draw,
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
fn put_placeholders(frame: &mut Frame, id: u32, area: Rect, from_row: u16) {
    let area = area.intersection(frame.buf.area);
    let style = Style::new().fg(id_color(id));
    for r in 0..area.height {
        for c in 0..area.width {
            frame.buf[(area.x + c, area.y + r)]
                .set_symbol(&placeholder(from_row + r, c))
                .set_style(style);
        }
    }
    frame.links.clear_area(area);
}

/// Places an image by placeholders, if the area fits the marks.
fn place(frame: &mut Frame, name: &str, area: Rect, key: u64, photo: bool, draw: Draw) {
    let id = id_of(name);
    let (cols, rows) = (area.width, area.height);
    put_placeholders(frame, id, area, 0);
    frame.images.push(Placed {
        id,
        cols,
        rows,
        spot: Spot::Placeholders,
        key,
        photo,
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
/// color in the first row's two columns (`kit::avatar`'s text form).
pub fn avatar(
    frame: &mut Frame,
    name: &str,
    area: Rect,
    who: &str,
    color: Color,
    photo: Option<&RgbaImage>,
) {
    let area = area.intersection(frame.buf.area);
    if let (Some(cell), Some(photo)) = (frame.graphics.map(|g| g.cell), photo)
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
            true,
            Arc::new(move || raster_avatar(&photo, cols, rows, cell)),
        );
        return;
    }
    // Without the photo, the kit's text form (board 0d): the initial on
    // the color in the first row's two columns, the rest left as it is.
    let mut x = area.x;
    for span in crate::ui::kit::initial(who, color) {
        let room = area.right().saturating_sub(x);
        put_text(frame, x, area.y, &span.content, span.style, room);
        x += crate::ui::draw::width(&span.content) as u16;
    }
    frame.links.clear_area(Rect::new(
        area.x,
        area.y,
        area.width.min(2),
        area.height.min(1),
    ));
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
    if let Some(cell) = frame.graphics.map(|g| g.cell)
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
            true,
            Arc::new(move || raster_picture(&img, cols, rows, cell, &tones)),
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

/// A picture `rows` rows tall of which `area` shows the rows from
/// `from_row` down (one cut at the top or the bottom of the pane it is laid
/// out in, or both): drawn whole, `area`'s width by `rows` cells, as
/// `picture` draws one, under the id its layout gave it, with only `area`'s
/// cells carrying its placeholders, those of the rows that show. Nothing where
/// the terminal draws no images, since what lays it out lays out its text
/// there instead.
pub fn picture_cut(
    frame: &mut Frame,
    id: u32,
    area: Rect,
    from_row: u16,
    rows: u16,
    img: &std::sync::Arc<RgbaImage>,
    content: u64,
) {
    let Some(cell) = frame.graphics.map(|g| g.cell) else {
        return;
    };
    let cols = area.width;
    let shown = area.intersection(frame.buf.area);
    let starts_on_the_screen = (shown.x, shown.y) == (area.x, area.y);
    let fits_the_marks = rows >= from_row + area.height
        && (rows as usize) <= MARKS.len()
        && (cols as usize) <= MARKS.len();
    if shown.is_empty() || !starts_on_the_screen || !fits_the_marks {
        return;
    }
    let area = shown;
    let tones = Tones::now();
    let img = img.clone();
    put_placeholders(frame, id, area, from_row);
    frame.images.push(Placed {
        id,
        cols,
        rows,
        spot: Spot::Placeholders,
        key: key_of(("picture", cols, rows, cell, tones, content)),
        photo: true,
        draw: Arc::new(move || raster_picture(&img, cols, rows, cell, &tones)),
    });
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
    if let Some(cell) = frame.graphics.map(|g| g.cell)
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
            false,
            Arc::new(move || raster_meter(cols, frac, color, tick, cell, &tones)),
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

/// A switch at `area`'s left, on or off (`raster_switch`); as text,
/// ` ━●` green when on and `○━ ` dim when off, `SWITCH_COLS` wide.
pub fn switch(frame: &mut Frame, name: &str, area: Rect, on: bool) {
    let area = Rect {
        width: area.width.min(SWITCH_COLS),
        height: area.height.min(1),
        ..area
    }
    .intersection(frame.buf.area);
    let tones = Tones::now();
    if let Some(cell) = frame.graphics.map(|g| g.cell)
        && area.width == SWITCH_COLS
        && placeable(frame, area)
    {
        let key = key_of(("switch", cell, tones, on));
        place(
            frame,
            name,
            area,
            key,
            false,
            Arc::new(move || raster_switch(on, cell, &tones)),
        );
        return;
    }
    if area.height == 0 {
        return;
    }
    frame.links.clear_area(area);
    let (text, color) = switch_text(on, &tones);
    put_text(
        frame,
        area.x,
        area.y,
        text,
        Style::new().fg(color),
        area.width,
    );
}

/// A switch's text form and its color.
pub fn switch_text(on: bool, tones: &Tones) -> (&'static str, Color) {
    if on {
        (" ━●", tones.done)
    } else {
        ("○━ ", tones.secondary)
    }
}

const STEPS: [&str; 8] = ["▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"];

/// A series in `area`, a bar per value scaled to `max`, stacked where a
/// value has parts, what is past `over` in the failed tone, with its
/// `marks`; one row tall it is a sparkline. As text, `▁`…`█` per value,
/// by its total: one row tall in its top part's color, taller each cell in
/// the color of the part at its height (the outlined part's bold), a value
/// a column, or with fewer values than columns the width shared out
/// among them, so the bars reach the right edge, the grid a dashed line where no bar reaches.
pub fn series(
    frame: &mut Frame,
    name: &str,
    area: Rect,
    bars: &[Bar],
    max: f32,
    over: Option<f32>,
    marks: &Marks,
) {
    let area = area.intersection(frame.buf.area);
    let tones = Tones::now();
    if let Some(cell) = frame.graphics.map(|g| g.cell)
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
            marks.outline,
            marks.grid.iter().map(|g| g.to_bits()).collect::<Vec<_>>(),
            marks.now,
        ));
        let bars = bars.to_vec();
        let marks = marks.clone();
        let (cols, rows) = (area.width, area.height);
        place(
            frame,
            name,
            area,
            key,
            false,
            Arc::new(move || raster_series(cols, rows, &bars, max, over, &marks, cell, &tones)),
        );
        return;
    }
    if area.height == 0 || max <= 0.0 {
        return;
    }
    frame.links.clear_area(area);
    let shown = &bars[bars.len().saturating_sub(area.width as usize)..];
    if shown.is_empty() {
        return;
    }
    // Bar i takes the columns from i·w/n up to (i+1)·w/n, so they fill the
    // width; `shown` is never wider than it, so each gets one at least.
    let (w, n) = (area.width as usize, shown.len());
    let from = |i: usize| i * w / n;
    let eighths = area.height as f32 * 8.0;
    let grid_rows: Vec<u16> = marks
        .grid
        .iter()
        .map(|&g| {
            let from_top = ((1.0 - (g / max).clamp(0.0, 1.0)) * area.height as f32) as u16;
            from_top.min(area.height - 1)
        })
        .collect();
    for (i, bar) in shown.iter().enumerate() {
        let total: f32 = bar.iter().map(|(v, _)| v.max(0.0)).sum();
        let color = match over {
            Some(o) if total > o => tones.failed,
            _ => bar
                .iter()
                .rev()
                .find(|(v, _)| *v > 0.0)
                .or(bar.last())
                .map(|&(_, c)| c)
                .unwrap_or(tones.rule),
        };
        let outlined = marks
            .outline
            .and_then(|at| bar.get(at))
            .is_some_and(|(v, _)| *v > 0.0);
        let style = if outlined {
            Style::new()
                .fg(color)
                .add_modifier(ratatui::style::Modifier::BOLD)
        } else {
            Style::new().fg(color)
        };
        let mut left = ((total / max).clamp(0.0, 1.0) * eighths).round() as u16;
        if total > 0.0 {
            left = left.max(1);
        }
        // Each cell in the color of the part at the top of what it fills,
        // so a stacked bar keeps its parts as text too, the outlined part's
        // cells bold.
        let style_at = |eighth: u16| -> Style {
            if area.height == 1 {
                return style;
            }
            let value = (eighth as f32 + 0.5) / eighths * max;
            let mut below = 0.0f32;
            for (at, &(v, c)) in bar.iter().enumerate() {
                below += v.max(0.0);
                if v > 0.0 && value <= below {
                    let c = match over {
                        Some(o) if value > o => tones.failed,
                        _ => c,
                    };
                    let s = Style::new().fg(c);
                    return if marks.outline == Some(at) {
                        s.add_modifier(ratatui::style::Modifier::BOLD)
                    } else {
                        s
                    };
                }
            }
            style
        };
        let mut column = Vec::new();
        for (k, r) in (0..area.height).rev().enumerate() {
            let here = left.min(8);
            left -= here;
            let top = (k as u16 * 8 + here).saturating_sub(1);
            column.push((r, here, style_at(top)));
        }
        for dx in 0..from(i + 1) - from(i) {
            let x = area.x as usize + from(i) + dx;
            if x >= area.right() as usize {
                break;
            }
            // The "now" edge down the last column, where no bar reaches.
            let now_here = marks
                .now
                .filter(|_| x + 1 == area.right() as usize && area.height > 1);
            for &(r, here, style) in &column {
                let y = area.y + r;
                let cell = &mut frame.buf[(x as u16, y)];
                if here == 0
                    && let Some(red) = now_here
                {
                    cell.set_symbol("│").set_style(Style::new().fg(red));
                } else if here == 0 {
                    if grid_rows.contains(&r) {
                        cell.set_symbol("┄").set_style(Style::new().fg(tones.rule));
                    } else {
                        cell.set_symbol(" ").set_style(Style::new());
                    }
                } else {
                    cell.set_symbol(STEPS[here as usize - 1]).set_style(style);
                }
            }
        }
    }
}

/// The shadow under a box at `area`: an image placed under the text,
/// reaching past the box, where the frame's pictures can sit under its
/// text (kitty's); as text, half blocks down its right and along its
/// bottom in a near-black shade, which is also what sixel shows, since a
/// sixel is drawn over its cells and would hide what it should lie under.
pub fn shadow(frame: &mut Frame, name: &str, area: Rect) {
    if let Some(cell) = frame.graphics.filter(|g| g.under_text()).map(|g| g.cell)
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
            photo: false,
            draw: Arc::new(move || {
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
/// bottom in a near-black shade, meeting at the corner in a quadrant (the
/// right edge's left half and the bottom's top half), for a terminal that
/// draws no images, and for the apps in panes, which draw into a buffer of
/// their own.
pub fn shadow_cells(buf: &mut Buffer, area: Rect) {
    let screen = buf.area;
    let shade = Style::new().fg(Color::Indexed(233));
    let right = area.right();
    let below = area.bottom();
    if right < screen.right() {
        for y in area.top() + 1..below.min(screen.bottom()) {
            buf[(right, y)].set_symbol("▌").set_style(shade);
        }
    }
    if below < screen.bottom() {
        for x in area.left() + 1..right.min(screen.right()) {
            buf[(x, below)].set_symbol("▀").set_style(shade);
        }
        // The corner a quadrant, so it ends where both edges do.
        if right < screen.right() {
            buf[(right, below)].set_symbol("▘").set_style(shade);
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
    if let Some(cell) = frame.graphics.map(|g| g.cell)
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
            false,
            Arc::new(move || raster_lane(cols, &segments, &rules, cell)),
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
    if let Some(cell) = frame.graphics.map(|g| g.cell)
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
            false,
            Arc::new(move || raster_chart(cols, rows, &values, color, &rules, cell)),
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

// ── The keyboard ────────────────────────────────────────────────────────────

/// The keyboard's rows, as a keyboard has them.
pub const KEY_ROWS: [&str; 4] = ["1234567890", "QWERTYUIOP", "ASDFGHJKL", "ZXCVBNM"];
/// How far in each row starts, in cells, staggered as a keyboard's are.
const KEY_STAGGER: [u16; 4] = [0, 2, 3, 5];
/// A key's cells across and down.
pub const KEY_W: u16 = 5;
pub const KEY_H: u16 = 2;

/// How often each key was pressed, by its letter or digit.
pub type Heat = std::collections::BTreeMap<char, u64>;
/// The keys drawn with an edge.
pub type Outlined = std::collections::BTreeSet<char>;

/// The keyboard's size in cells.
pub fn keyboard_size() -> (u16, u16) {
    let cols = KEY_ROWS
        .iter()
        .zip(KEY_STAGGER)
        .map(|(row, at)| at + row.len() as u16 * KEY_W)
        .max()
        .unwrap_or(0);
    (cols, KEY_ROWS.len() as u16 * KEY_H)
}

/// The key at a cell of the keyboard, counted from its corner.
pub fn key_at(col: u16, row: u16) -> Option<char> {
    let r = (row / KEY_H) as usize;
    let keys = KEY_ROWS.get(r)?;
    let from = col.checked_sub(KEY_STAGGER[r])?;
    keys.chars().nth((from / KEY_W) as usize)
}

/// Each key with the cell its top left is at.
fn key_cells() -> impl Iterator<Item = (char, u16, u16)> {
    KEY_ROWS.iter().enumerate().flat_map(|(r, keys)| {
        keys.chars()
            .enumerate()
            .map(move |(i, c)| (c, KEY_STAGGER[r] + i as u16 * KEY_W, r as u16 * KEY_H))
    })
}

/// A key's share of the most pressed, 0 to 1.
fn share(heat: &Heat, c: char) -> f32 {
    let most = heat.values().copied().max().unwrap_or(0);
    if most == 0 {
        return 0.0;
    }
    heat.get(&c).copied().unwrap_or(0) as f32 / most as f32
}

/// A 5 by 7 picture of each letter and digit, a row a byte, its five low
/// bits the pixels left to right: the binary carries no font to raster the
/// keys' letters with.
fn glyph(c: char) -> [u8; 7] {
    match c {
        'A' => [0x0E, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11],
        'B' => [0x1E, 0x11, 0x11, 0x1E, 0x11, 0x11, 0x1E],
        'C' => [0x0E, 0x11, 0x10, 0x10, 0x10, 0x11, 0x0E],
        'D' => [0x1E, 0x11, 0x11, 0x11, 0x11, 0x11, 0x1E],
        'E' => [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x1F],
        'F' => [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x10],
        'G' => [0x0E, 0x11, 0x10, 0x17, 0x11, 0x11, 0x0F],
        'H' => [0x11, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11],
        'I' => [0x0E, 0x04, 0x04, 0x04, 0x04, 0x04, 0x0E],
        'J' => [0x07, 0x02, 0x02, 0x02, 0x02, 0x12, 0x0C],
        'K' => [0x11, 0x12, 0x14, 0x18, 0x14, 0x12, 0x11],
        'L' => [0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x1F],
        'M' => [0x11, 0x1B, 0x15, 0x15, 0x11, 0x11, 0x11],
        'N' => [0x11, 0x11, 0x19, 0x15, 0x13, 0x11, 0x11],
        'O' => [0x0E, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E],
        'P' => [0x1E, 0x11, 0x11, 0x1E, 0x10, 0x10, 0x10],
        'Q' => [0x0E, 0x11, 0x11, 0x11, 0x15, 0x12, 0x0D],
        'R' => [0x1E, 0x11, 0x11, 0x1E, 0x14, 0x12, 0x11],
        'S' => [0x0F, 0x10, 0x10, 0x0E, 0x01, 0x01, 0x1E],
        'T' => [0x1F, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04],
        'U' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E],
        'V' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x0A, 0x04],
        'W' => [0x11, 0x11, 0x11, 0x15, 0x15, 0x15, 0x0A],
        'X' => [0x11, 0x11, 0x0A, 0x04, 0x0A, 0x11, 0x11],
        'Y' => [0x11, 0x11, 0x11, 0x0A, 0x04, 0x04, 0x04],
        'Z' => [0x1F, 0x01, 0x02, 0x04, 0x08, 0x10, 0x1F],
        '0' => [0x0E, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0E],
        '1' => [0x04, 0x0C, 0x04, 0x04, 0x04, 0x04, 0x0E],
        '2' => [0x0E, 0x11, 0x01, 0x02, 0x04, 0x08, 0x1F],
        '3' => [0x1F, 0x02, 0x04, 0x02, 0x01, 0x11, 0x0E],
        '4' => [0x02, 0x06, 0x0A, 0x12, 0x1F, 0x02, 0x02],
        '5' => [0x1F, 0x10, 0x1E, 0x01, 0x01, 0x11, 0x0E],
        '6' => [0x06, 0x08, 0x10, 0x1E, 0x11, 0x11, 0x0E],
        '7' => [0x1F, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08],
        '8' => [0x0E, 0x11, 0x11, 0x0E, 0x11, 0x11, 0x0E],
        '9' => [0x0E, 0x11, 0x11, 0x0F, 0x01, 0x02, 0x0C],
        _ => [0; 7],
    }
}

/// The keyboard as a heatmap: a rounded key per letter and digit, staggered
/// as a keyboard's rows are, filled in `accent` at an alpha from 0.18 to 1
/// by its share of the most pressed (an unpressed key on the sunken
/// surface), its letter centered, dark on a key past half and dim below,
/// and the outlined keys with a 1.5 px edge in the light tone around them,
/// as a series outlines its part.
pub fn raster_keyboard(
    heat: &Heat,
    outlined: &Outlined,
    accent: Color,
    cell: Cell,
    tones: &Tones,
) -> Pixmap {
    let (cols, rows) = keyboard_size();
    let (cw, ch) = (cell.0 as f32, cell.1 as f32);
    let mut pm = canvas((cols as f32 * cw) as u32, (rows as f32 * ch) as u32);
    let dark = rgb(Color::Indexed(232));
    for (c, x, y) in key_cells() {
        let (kx, ky) = (x as f32 * cw + 3.0, y as f32 * ch + 3.0);
        let (kw, kh) = (KEY_W as f32 * cw - 6.0, KEY_H as f32 * ch - 6.0);
        if outlined.contains(&c) {
            fill(
                &mut pm,
                round_rect(kx - 1.5, ky - 1.5, kw + 3.0, kh + 3.0, 5.5),
                rgb(tones.light),
                1.0,
            );
        }
        let s = share(heat, c);
        fill(
            &mut pm,
            round_rect(kx, ky, kw, kh, 4.0),
            rgb(tones.sunken),
            1.0,
        );
        if s > 0.0 {
            fill(
                &mut pm,
                round_rect(kx, ky, kw, kh, 4.0),
                rgb(accent),
                0.18 + 0.82 * s,
            );
        }
        let ink = if s > 0.5 { dark } else { rgb(tones.secondary) };
        let px = ((kh * 0.5 / 7.0).min(kw * 0.5 / 5.0)).floor().max(1.0);
        let (gx, gy) = (kx + (kw - 5.0 * px) / 2.0, ky + (kh - 7.0 * px) / 2.0);
        for (r, bits) in glyph(c).iter().enumerate() {
            for b in 0..5 {
                if bits & (0x10 >> b) != 0
                    && let Some(rect) =
                        SkRect::from_xywh(gx + b as f32 * px, gy + r as f32 * px, px, px)
                {
                    pm.fill_rect(rect, &paint(ink, 1.0), Transform::identity(), None);
                }
            }
        }
    }
    pm
}

/// The keyboard as a heatmap at `area`'s corner (`keyboard_size` cells),
/// `heat` how often each key was pressed and `outlined` the keys with an
/// edge; as text, each key's letter centered on its first row, in `accent`
/// at three steps by its share (dim, plain, bold) and in the rule's tone
/// when unpressed, the outlined ones bold and underlined.
pub fn keyboard(
    frame: &mut Frame,
    name: &str,
    area: Rect,
    heat: &Heat,
    outlined: &Outlined,
    accent: Color,
) {
    let (cols, rows) = keyboard_size();
    let area = Rect {
        width: cols,
        height: rows,
        ..area
    };
    let tones = Tones::now();
    if let Some(cell) = frame.graphics.map(|g| g.cell)
        && placeable(frame, area)
    {
        let key = key_of(("keyboard", cell, tones, heat, outlined, accent));
        let (heat, outlined) = (heat.clone(), outlined.clone());
        place(
            frame,
            name,
            area,
            key,
            false,
            Arc::new(move || raster_keyboard(&heat, &outlined, accent, cell, &tones)),
        );
        return;
    }
    let area = area.intersection(frame.buf.area);
    if area.is_empty() {
        return;
    }
    frame.links.clear_area(area);
    for (c, x, y) in key_cells() {
        let s = share(heat, c);
        let mut style = if s <= 0.0 {
            Style::new().fg(tones.rule)
        } else if s < 1.0 / 3.0 {
            Style::new()
                .fg(accent)
                .add_modifier(ratatui::style::Modifier::DIM)
        } else if s < 2.0 / 3.0 {
            Style::new().fg(accent)
        } else {
            Style::new()
                .fg(accent)
                .add_modifier(ratatui::style::Modifier::BOLD)
        };
        if outlined.contains(&c) {
            style = style
                .add_modifier(ratatui::style::Modifier::BOLD)
                .add_modifier(ratatui::style::Modifier::UNDERLINED);
        }
        let text = format!("[{c}]");
        put_text(
            frame,
            area.x + x + 1,
            area.y + y,
            &text,
            style,
            area.width.saturating_sub(x + 1),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_last_screen_is_rastered_as_bars_of_its_colors() {
        use crate::usage::shots::{Shot, ShotLine};
        let shot = Shot {
            cols: 10,
            lines: vec![ShotLine {
                runs: vec![(5, Some([255, 0, 0])), (5, None)],
                text: String::new(),
            }],
        };
        let tones = Tones {
            rule: Color::Rgb(1, 1, 1),
            sunken: Color::Rgb(0, 0, 0),
            light: Color::White,
            secondary: Color::Gray,
            failed: Color::Red,
            done: Color::Green,
        };
        let pm = raster_screen(&shot, 10, 4, (10, 20), &tones);
        let at = |x: u32, y: u32| pm.pixel(x, y).unwrap().demultiply();
        let mid = 40;
        let red = at(25, mid);
        assert!(red.red() > 180 && red.green() < 60, "the run's own color");
        let gap = at(80, mid);
        assert!(gap.red() < 20, "a gap is the surface");
    }

    const CELL: Cell = (8, 16);

    fn graphics_frame(cols: u16, rows: u16) -> Frame {
        let mut f = Frame::new(cols, rows);
        f.graphics = Some(crate::mux::graphics::Graphics::kitty(CELL));
        f
    }

    #[test]
    fn a_text_shadow_meets_at_its_corner_and_stops_at_the_screen() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 10, 8));
        let area = Rect::new(2, 1, 4, 3);
        shadow_cells(&mut buf, area);
        for y in 2..4 {
            assert_eq!(buf[(6, y)].symbol(), "▌", "row {y}");
        }
        assert_eq!(buf[(6, 1)].symbol(), " ");
        for x in 3..6 {
            assert_eq!(buf[(x, 4)].symbol(), "▀", "column {x}");
        }
        assert_eq!(buf[(2, 4)].symbol(), " ");
        assert_eq!(buf[(6, 4)].symbol(), "▘");
        assert_eq!(buf[(7, 4)].symbol(), " ");
        assert_eq!(buf[(6, 5)].symbol(), " ");

        // At the screen's right and bottom edges, nothing past them.
        let mut buf = Buffer::empty(Rect::new(0, 0, 6, 4));
        shadow_cells(&mut buf, area);
        assert!(
            buf.content.iter().all(|c| c.symbol() == " "),
            "{:?}",
            buf.content
        );
        let mut buf = Buffer::empty(Rect::new(0, 0, 10, 4));
        shadow_cells(&mut buf, area);
        assert_eq!(buf[(6, 3)].symbol(), "▌");
        assert!((0..10).all(|x| buf[(x, 3)].symbol() != "▀"));
    }

    #[test]
    fn a_cut_picture_is_drawn_whole_with_placeholders_on_the_rows_that_show() {
        let img = std::sync::Arc::new(RgbaImage::new(40, 80));
        let content = picture_key(&img);
        let id = id_of("mission/diagram/x");
        let mut f = graphics_frame(20, 6);
        picture_cut(&mut f, id, Rect::new(2, 1, 10, 3), 0, 8, &img, content);
        let [placed] = &f.images[..] else {
            panic!("one image: {:?}", f.images);
        };
        assert_eq!((placed.id, placed.cols, placed.rows), (id, 10, 8));
        let marked = |f: &Frame, x: u16, y: u16| f.buf[(x, y)].symbol().starts_with(PLACEHOLDER);
        assert!(marked(&f, 2, 1) && marked(&f, 11, 3));
        assert_eq!(f.buf[(11, 3)].symbol(), placeholder(2, 9));
        assert!(!marked(&f, 2, 4) && !marked(&f, 12, 1) && !marked(&f, 2, 0));

        // One reaching past the screen's bottom keeps the rows on it.
        let mut f = graphics_frame(20, 6);
        picture_cut(&mut f, id, Rect::new(2, 4, 10, 5), 0, 8, &img, content);
        assert_eq!(f.images.len(), 1);
        assert!(marked(&f, 2, 5));

        // One cut at the top has the placeholders of its rows that show.
        let mut f = graphics_frame(20, 6);
        picture_cut(&mut f, id, Rect::new(2, 0, 10, 3), 5, 8, &img, content);
        assert_eq!(f.images.len(), 1);
        assert_eq!(f.buf[(2, 0)].symbol(), placeholder(5, 0));
        assert_eq!(f.buf[(11, 2)].symbol(), placeholder(7, 9));

        // Where the terminal draws no images, nothing: its text was laid out.
        let mut f = Frame::new(20, 6);
        picture_cut(&mut f, id, Rect::new(2, 1, 10, 3), 0, 8, &img, content);
        assert!(f.images.is_empty() && !marked(&f, 2, 1));
    }

    fn red() -> Color {
        Color::Indexed(196)
    }

    #[test]
    fn a_rounded_end_is_its_half_circle_against_the_object() {
        let red = [220, 50, 50];
        let alpha = |pm: &Pixmap, x: u32, y: u32| pm.pixel(x, y).unwrap().alpha();
        let left = raster_end(true, red, (10, 20));
        assert_eq!((left.width(), left.height()), (10, 20));
        assert_eq!(alpha(&left, 9, 10), 255, "solid against the object");
        assert_eq!(alpha(&left, 0, 0), 0, "clear at its far corner");
        assert_eq!(alpha(&left, 1, 10), 255, "round out to its edge");
        let right = raster_end(false, red, (10, 20));
        assert_eq!(alpha(&right, 0, 10), 255);
        assert_eq!(alpha(&right, 9, 0), 0);
        let c = right.pixel(0, 10).unwrap().demultiply();
        assert_eq!([c.red(), c.green(), c.blue()], red);
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
                    &Marks {
                        outline: Some(1),
                        grid: vec![1.5],
                        now: Some(red()),
                    },
                    CELL,
                    &tones,
                ),
                (32, 32),
            ),
            (
                raster_shadow(4, 2, CELL),
                (32 + 2 * SHADOW_REACH, 32 + 2 * SHADOW_REACH),
            ),
            (raster_switch(true, CELL, &tones), (32, 16)),
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
    fn a_switch_differs_on_and_off_and_reads_as_text_without_images() {
        let tones = Tones::now();
        let (on, off) = (
            raster_switch(true, CELL, &tones),
            raster_switch(false, CELL, &tones),
        );
        assert_ne!(png(&on), png(&off));
        // The track is green at its middle while on, the rule's off.
        let mid = |pm: &Pixmap| {
            let p = pm.pixel(16, 3).unwrap().demultiply();
            [p.red(), p.green(), p.blue()]
        };
        assert_eq!(mid(&on), rgb(tones.done));
        assert_eq!(mid(&off), rgb(tones.rule));

        let mut f = Frame::new(10, 1);
        switch(&mut f, "s", Rect::new(1, 0, 4, 1), true);
        let row: String = (1..4).map(|x| f.buf[(x, 0)].symbol().to_string()).collect();
        assert_eq!(row, " ━●");
        assert_eq!(f.buf[(3, 0)].fg, tones.done);
        let mut f = Frame::new(10, 1);
        switch(&mut f, "s", Rect::new(1, 0, 4, 1), false);
        let row: String = (1..4).map(|x| f.buf[(x, 0)].symbol().to_string()).collect();
        assert_eq!(row, "○━ ");

        let mut f = graphics_frame(10, 1);
        switch(&mut f, "s", Rect::new(1, 0, 4, 1), true);
        assert_eq!(f.images.len(), 1);
        assert_eq!(f.buf[(4, 0)].symbol(), placeholder(0, 3));
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
            &Marks::default(),
        );
        assert_eq!(f.buf[(1, 2)].symbol(), "▄");
        assert_eq!(f.buf[(2, 2)].symbol(), "█");
        // Two rows tall, a bar of two parts keeps each part's color.
        let green = Color::Indexed(41);
        series(
            &mut f,
            "t",
            Rect::new(5, 0, 1, 2),
            &[vec![(1.0, red()), (1.0, green)]],
            2.0,
            None,
            &Marks {
                outline: Some(1),
                ..Marks::default()
            },
        );
        assert_eq!(f.buf[(5, 1)].fg, red());
        assert_eq!(f.buf[(5, 0)].fg, green);
        assert!(
            f.buf[(5, 0)]
                .modifier
                .contains(ratatui::style::Modifier::BOLD)
        );
        // The "now" edge down the last column, above its bar.
        series(
            &mut f,
            "n",
            Rect::new(6, 0, 1, 2),
            &[vec![(1.0, red())]],
            2.0,
            None,
            &Marks {
                now: Some(green),
                ..Marks::default()
            },
        );
        assert_eq!((f.buf[(6, 0)].symbol(), f.buf[(6, 0)].fg), ("│", green));
        assert_eq!(f.buf[(6, 1)].symbol(), "█");
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

    #[test]
    fn the_keyboard_rasters_at_its_cells_and_outlines_the_given_keys() {
        let tones = Tones::now();
        let heat: Heat = [('J', 10), ('K', 2)].into_iter().collect();
        let outlined: Outlined = ['Q'].into_iter().collect();
        let (cols, rows) = keyboard_size();
        assert_eq!((cols, rows), (52, 8));
        let pm = raster_keyboard(&heat, &outlined, red(), CELL, &tones);
        assert_eq!(
            (pm.width(), pm.height()),
            (cols as u32 * CELL.0 as u32, rows as u32 * CELL.1 as u32)
        );
        let at = |pm: &Pixmap, x: u32, y: u32| pm.pixel(x, y).unwrap().demultiply();
        // Q is the first key of the second row, two cells in: its edge, a
        // pixel outside the key, is lit, and W's isn't.
        let (qx, qy) = (2 * CELL.0 as u32 + 1, 2 * CELL.1 as u32 + 10);
        assert!(at(&pm, qx, qy).alpha() > 0);
        let wx = qx + KEY_W as u32 * CELL.0 as u32;
        assert_eq!(at(&pm, wx, qy).alpha(), 0);
        assert_eq!(
            raster_keyboard(&heat, &outlined, red(), CELL, &tones).data(),
            pm.data()
        );
        assert_eq!(key_at(0, 0), Some('1'));
        assert_eq!(key_at(2, 2), Some('Q'));
        assert_eq!(key_at(1, 2), None);
        assert_eq!(key_at(5 + 6 * 5, 7), Some('M'));
    }

    #[test]
    fn the_keyboards_text_shades_three_steps_and_marks_the_outlined() {
        use ratatui::style::Modifier;
        let heat: Heat = [('J', 9), ('K', 5), ('L', 1)].into_iter().collect();
        let outlined: Outlined = ['A'].into_iter().collect();
        let mut f = Frame::new(60, 10);
        keyboard(&mut f, "k", Rect::new(0, 0, 52, 8), &heat, &outlined, red());
        assert!(f.images.is_empty());
        let cell_of = |c: char| {
            let (_, x, y) = key_cells().find(|(k, _, _)| *k == c).unwrap();
            f.buf[(x + 2, y)].clone()
        };
        let j = cell_of('J');
        assert_eq!(j.symbol(), "J");
        assert_eq!(j.fg, red());
        assert!(j.modifier.contains(Modifier::BOLD));
        let k = cell_of('K');
        assert!(!k.modifier.contains(Modifier::BOLD) && !k.modifier.contains(Modifier::DIM));
        assert!(cell_of('L').modifier.contains(Modifier::DIM));
        assert_eq!(cell_of('Z').fg, Tones::now().rule);
        let a = cell_of('A');
        assert!(a.modifier.contains(Modifier::UNDERLINED));
    }
}
