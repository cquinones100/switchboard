//! A picture made sixel, for a terminal that draws sixel but not kitty's
//! pictures (src/mux/sixel/mod.rs says when and where): its PNG read,
//! fitted into the cells its placement takes as kitty fits one (as large
//! as fits, its shape kept, centred), its colours brought down to a
//! palette of at most 256 (sixel's colour registers), and the part of it a
//! frame shows written as a DEC sixel string.
//!
//! The palette is the picture's own colours when it has 256 or fewer, as a
//! screenshot of text often does, else NeuQuant's (color_quant, the
//! quantiser the image crate's GIF encoder uses), and each pixel takes its
//! nearest colour with no dithering, which would speckle the edges of
//! text. A pixel less than half opaque is left clear: the string asks for
//! a clear background (its second parameter, 1), so the cell under it
//! shows through, as it does in a letterboxed picture's margins in kitty.
//!
//! The string is `ESC P 0;1;0 q`, raster attributes saying a 1:1 pixel
//! and the picture's size, the colour registers it uses (in percent, as
//! sixel gives RGB), then its bands of six rows: each colour's sixels in
//! the band run-length encoded (`!<n>`), a `$` between colours and a `-`
//! between bands. What is asked of it is a multiple of six rows tall
//! (`mod.rs`), so no band reaches past the cells it is drawn for: a
//! terminal scrolls once a band would pass its last row, and some count a
//! band's whole height whatever it draws.

use std::collections::HashMap;
use std::io::{BufReader, Write};
use std::path::Path;

use image::{ImageFormat, ImageReader, Limits, Rgba, RgbaImage, imageops};

/// The widest or tallest a picture is read, or its placement's box made,
/// in pixels.
pub const MOST: u32 = 16_384;

/// NeuQuant's sampling: 1 learns from every pixel, 30 from one in thirty;
/// a few keeps the colours of small text without taking long.
const SAMPLE: i32 = 3;

/// A picture at the size it is shown, each pixel a colour of its palette.
#[derive(Debug, PartialEq, Eq)]
pub struct Indexed {
    pub width: u32,
    pub height: u32,
    pub palette: Vec<[u8; 3]>,
    /// Each pixel's colour in the palette, row by row, or `clear`.
    pub pixels: Vec<u8>,
    /// The number that marks a clear pixel, when the picture has any.
    pub clear: Option<u8>,
}

/// A picture fitted into its placement: what every part of it shown is
/// encoded from.
#[derive(Debug, PartialEq, Eq)]
pub struct Fitted {
    pub picture: Indexed,
    /// Where the picture's top left is in its box, in pixels.
    pub at: (u32, u32),
    /// The box: the placement's cells, in pixels.
    pub size: (u32, u32),
}

impl Fitted {
    /// About what it takes in memory.
    pub fn bytes(&self) -> usize {
        self.picture.pixels.len() + 3 * self.picture.palette.len()
    }
}

/// A PNG file's pixels; anything else, or one larger than `MOST` a side,
/// is refused.
pub fn read_png(path: &Path) -> Result<RgbaImage, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut reader = ImageReader::with_format(BufReader::new(file), ImageFormat::Png);
    let mut limits = Limits::default();
    limits.max_image_width = Some(MOST);
    limits.max_image_height = Some(MOST);
    reader.limits(limits);
    let image = reader
        .decode()
        .map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(image.to_rgba8())
}

/// The cells a placement takes: its columns and rows as given, a missing
/// one from the picture's shape, both from its size in cells when neither
/// is given, as kitty sizes one.
pub fn cells_of(
    given: (Option<u16>, Option<u16>),
    image: (u32, u32),
    cell: (u32, u32),
) -> (u32, u32) {
    let (w, h) = (image.0.max(1) as u64, image.1.max(1) as u64);
    let (cw, ch) = (cell.0.max(1) as u64, cell.1.max(1) as u64);
    let (cols, rows) = match given {
        (Some(c), Some(r)) => (c as u64, r as u64),
        (Some(c), None) => (c as u64, (c as u64 * cw * h).div_ceil(w * ch)),
        (None, Some(r)) => ((r as u64 * ch * w).div_ceil(h * cw), r as u64),
        (None, None) => (w.div_ceil(cw), h.div_ceil(ch)),
    };
    let most = (MOST as u64).max(1);
    (cols.clamp(1, most) as u32, rows.clamp(1, most) as u32)
}

/// The size a picture of `image` pixels is drawn at in a box of `within`:
/// as large as fits, its shape kept.
pub fn fit(image: (u32, u32), within: (u32, u32)) -> (u32, u32) {
    let (w, h) = (image.0 as u64, image.1 as u64);
    let (bw, bh) = (within.0 as u64, within.1 as u64);
    if w == 0 || h == 0 || bw == 0 || bh == 0 {
        return (0, 0);
    }
    if w * bh >= h * bw {
        (bw as u32, ((h * bw + w / 2) / w).clamp(1, bh) as u32)
    } else {
        (((w * bh + h / 2) / h).clamp(1, bw) as u32, bh as u32)
    }
}

/// A picture fitted into a placement `cells` big, of cells `cell` pixels:
/// scaled (Lanczos, sharp for text) and centred, its colours a palette's.
pub fn prepare(image: &RgbaImage, cells: (u32, u32), cell: (u32, u32)) -> Result<Fitted, String> {
    let size = (
        cells.0.saturating_mul(cell.0.max(1)),
        cells.1.saturating_mul(cell.1.max(1)),
    );
    if size.0 > MOST || size.1 > MOST {
        return Err(format!(
            "a placement of {}x{} pixels is too large",
            size.0, size.1
        ));
    }
    let (w, h) = fit(image.dimensions(), size);
    if w == 0 || h == 0 {
        return Err("an empty picture".into());
    }
    let scaled;
    let shown = if (w, h) == image.dimensions() {
        image
    } else {
        scaled = imageops::resize(image, w, h, imageops::FilterType::Lanczos3);
        &scaled
    };
    Ok(Fitted {
        picture: quantize(shown),
        at: ((size.0 - w) / 2, (size.1 - h) / 2),
        size,
    })
}

fn solid(p: &Rgba<u8>) -> bool {
    p[3] >= 128
}

/// The picture's own colours when there are few enough, else a palette of
/// 256 NeuQuant learns from it (255 and a clear one when any pixel is
/// clear), each pixel its nearest.
pub fn quantize(image: &RgbaImage) -> Indexed {
    let (width, height) = image.dimensions();
    let clear = image.pixels().any(|p| !solid(p)).then_some(255u8);
    let room = if clear.is_some() { 255 } else { 256 };
    let mut own: HashMap<[u8; 3], u8> = HashMap::new();
    let mut palette: Vec<[u8; 3]> = Vec::new();
    let mut few = true;
    for p in image.pixels().filter(|p| solid(p)) {
        let rgb = [p[0], p[1], p[2]];
        if own.contains_key(&rgb) {
            continue;
        }
        if palette.len() == room {
            few = false;
            break;
        }
        own.insert(rgb, palette.len() as u8);
        palette.push(rgb);
    }
    if few {
        let pixels = image
            .pixels()
            .map(|p| match solid(p) {
                true => own[&[p[0], p[1], p[2]]],
                false => 255,
            })
            .collect();
        return Indexed {
            width,
            height,
            palette,
            pixels,
            clear,
        };
    }
    let samples: Vec<u8> = image
        .pixels()
        .filter(|p| solid(p))
        .flat_map(|p| [p[0], p[1], p[2], 255])
        .collect();
    let quant = color_quant::NeuQuant::new(SAMPLE, room, &samples);
    let palette = quant
        .color_map_rgb()
        .chunks(3)
        .map(|c| [c[0], c[1], c[2]])
        .collect();
    let mut nearest: HashMap<[u8; 3], u8> = HashMap::new();
    let pixels = image
        .pixels()
        .map(|p| {
            if !solid(p) {
                return 255;
            }
            *nearest
                .entry([p[0], p[1], p[2]])
                .or_insert_with(|| quant.index_of(&[p[0], p[1], p[2], 255]) as u8)
        })
        .collect();
    Indexed {
        width,
        height,
        palette,
        pixels,
        clear,
    }
}

/// A colour's channel as sixel gives it, in percent.
pub fn percent(channel: u8) -> u32 {
    (channel as u32 * 100 + 127) / 255
}

/// The sixel string that draws `crop` (x, y, width, height, in pixels of
/// the placement's box) of a fitted picture from the crop's top left: the
/// picture where it falls, clear elsewhere.
pub fn encode(fitted: &Fitted, crop: (u32, u32, u32, u32)) -> Vec<u8> {
    let (cx, cy, width, height) = crop;
    let pic = &fitted.picture;
    let (ax, ay) = fitted.at;
    // The crop's columns and rows the picture covers.
    let x0 = ax.saturating_sub(cx).min(width);
    let x1 = (ax + pic.width).saturating_sub(cx).min(width);
    let y0 = ay.saturating_sub(cy).min(height);
    let y1 = (ay + pic.height).saturating_sub(cy).min(height);
    let row = |y: u32| -> &[u8] {
        let at = ((cy + y - ay) * pic.width) as usize;
        &pic.pixels[at..at + pic.width as usize]
    };
    let column = |x: u32| (cx + x - ax) as usize;
    // A register for each colour the crop shows, numbered as they come.
    let mut register = [u16::MAX; 256];
    let mut used: Vec<u8> = Vec::new();
    for y in y0..y1 {
        let line = row(y);
        for x in x0..x1 {
            let p = line[column(x)];
            if Some(p) != pic.clear && register[p as usize] == u16::MAX {
                register[p as usize] = used.len() as u16;
                used.push(p);
            }
        }
    }
    let mut out = Vec::with_capacity(64 + (width * height / 4) as usize);
    let _ = write!(out, "\x1bP0;1;0q\"1;1;{width};{height}");
    for (n, p) in used.iter().enumerate() {
        let [r, g, b] = pic.palette.get(*p as usize).copied().unwrap_or_default();
        let _ = write!(out, "#{n};2;{};{};{}", percent(r), percent(g), percent(b));
    }
    let w = width as usize;
    let mut bits = vec![0u8; used.len() * w];
    let mut marked = vec![false; used.len()];
    let mut in_band: Vec<usize> = Vec::new();
    let bands = height.div_ceil(6);
    for band in 0..bands {
        for k in 0..6 {
            let y = band * 6 + k;
            if y < y0 || y >= y1 {
                continue;
            }
            let line = row(y);
            for x in x0..x1 {
                let p = line[column(x)];
                if Some(p) == pic.clear {
                    continue;
                }
                let reg = register[p as usize] as usize;
                bits[reg * w + x as usize] |= 1 << k;
                if !marked[reg] {
                    marked[reg] = true;
                    in_band.push(reg);
                }
            }
        }
        in_band.sort_unstable();
        for (i, &reg) in in_band.iter().enumerate() {
            if i > 0 {
                out.push(b'$');
            }
            let _ = write!(out, "#{reg}");
            let line = &mut bits[reg * w..(reg + 1) * w];
            let end = line.iter().rposition(|b| *b != 0).map_or(0, |e| e + 1);
            run_length(&mut out, &line[..end]);
            line[..end].fill(0);
            marked[reg] = false;
        }
        in_band.clear();
        if band + 1 < bands {
            out.push(b'-');
        }
    }
    out.extend_from_slice(b"\x1b\\");
    out
}

/// A colour's sixels in a band, a run of four or more of one as `!<n>`.
fn run_length(out: &mut Vec<u8>, sixels: &[u8]) {
    let mut i = 0;
    while i < sixels.len() {
        let s = sixels[i];
        let n = sixels[i..].iter().take_while(|b| **b == s).count();
        let ch = 0x3f + s;
        if n >= 4 {
            let _ = write!(out, "!{n}");
            out.push(ch);
        } else {
            out.extend(std::iter::repeat_n(ch, n));
        }
        i += n;
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// What a sixel string draws, read as a terminal reads it: its size
    /// from the raster attributes, and each pixel's colour (`None` clear)
    /// as the registers give it, back from percent.
    pub(crate) fn decode(s: &[u8]) -> (u32, u32, Vec<Option<[u8; 3]>>) {
        let body = s
            .strip_prefix(b"\x1bP0;1;0q\"")
            .and_then(|b| b.strip_suffix(b"\x1b\\"))
            .expect("a sixel string asking for a clear background");
        let mut i = 0;
        let number = |i: &mut usize| -> u32 {
            let start = *i;
            while *i < body.len() && body[*i].is_ascii_digit() {
                *i += 1;
            }
            std::str::from_utf8(&body[start..*i])
                .unwrap()
                .parse()
                .unwrap()
        };
        let mut raster = Vec::new();
        loop {
            raster.push(number(&mut i));
            if body.get(i) == Some(&b';') {
                i += 1;
            } else {
                break;
            }
        }
        assert_eq!(&raster[..2], &[1, 1], "a 1:1 pixel");
        let (width, height) = (raster[2], raster[3]);
        let mut pixels = vec![None; (width * height) as usize];
        let mut registers: HashMap<u32, [u8; 3]> = HashMap::new();
        let back = |p: u32| ((p * 255 + 50) / 100) as u8;
        let (mut x, mut band, mut color) = (0u32, 0u32, None);
        while i < body.len() {
            match body[i] {
                b'#' => {
                    i += 1;
                    let n = number(&mut i);
                    if body.get(i) == Some(&b';') {
                        i += 1;
                        assert_eq!(number(&mut i), 2, "RGB");
                        let mut rgb = [0u8; 3];
                        for c in &mut rgb {
                            assert_eq!(body[i], b';');
                            i += 1;
                            *c = back(number(&mut i));
                        }
                        registers.insert(n, rgb);
                    } else {
                        color = Some(registers[&n]);
                    }
                }
                b'$' => {
                    x = 0;
                    i += 1;
                }
                b'-' => {
                    x = 0;
                    band += 1;
                    i += 1;
                }
                b'!' => {
                    i += 1;
                    let n = number(&mut i);
                    let s = body[i] - 0x3f;
                    for _ in 0..n {
                        paint(&mut pixels, width, height, x, band, s, color);
                        x += 1;
                    }
                    i += 1;
                }
                c @ 0x3f..=0x7e => {
                    paint(&mut pixels, width, height, x, band, c - 0x3f, color);
                    x += 1;
                    i += 1;
                }
                other => panic!("{:?} in a sixel string", other as char),
            }
        }
        (width, height, pixels)
    }

    fn paint(
        pixels: &mut [Option<[u8; 3]>],
        width: u32,
        height: u32,
        x: u32,
        band: u32,
        sixel: u8,
        color: Option<[u8; 3]>,
    ) {
        for k in 0..6 {
            if sixel & (1 << k) != 0 {
                let y = band * 6 + k;
                assert!(x < width && y < height, "a pixel past the raster: {x},{y}");
                pixels[(y * width + x) as usize] = Some(color.expect("a colour chosen"));
            }
        }
    }

    /// A colour as it comes back from sixel's percent.
    fn through_percent([r, g, b]: [u8; 3]) -> [u8; 3] {
        let back = |c: u8| ((percent(c) * 255 + 50) / 100) as u8;
        [back(r), back(g), back(b)]
    }

    fn whole(image: &RgbaImage) -> Fitted {
        Fitted {
            picture: quantize(image),
            at: (0, 0),
            size: image.dimensions(),
        }
    }

    #[test]
    fn a_picture_comes_back_pixel_for_pixel_with_its_clear_ones_clear() {
        let mut image = RgbaImage::new(7, 13);
        for (x, y, p) in image.enumerate_pixels_mut() {
            *p = match (x + y) % 5 {
                0 => Rgba([255, 255, 255, 255]),
                1 => Rgba([30, 30, 30, 255]),
                2 => Rgba([3, 102, 214, 200]),
                3 => Rgba([x as u8 * 30, y as u8 * 19, 7, 255]),
                _ => Rgba([200, 0, 0, 40]),
            };
        }
        let fitted = whole(&image);
        assert_eq!(fitted.picture.clear, Some(255));
        let sixel = encode(&fitted, (0, 0, 7, 12));
        let (w, h, pixels) = decode(&sixel);
        assert_eq!((w, h), (7, 12), "the size asked, a whole number of bands");
        for y in 0..12 {
            for x in 0..7 {
                let p = image.get_pixel(x, y);
                let want = solid(p).then(|| through_percent([p[0], p[1], p[2]]));
                assert_eq!(pixels[(y * 7 + x) as usize], want, "{x},{y}");
            }
        }
    }

    #[test]
    fn bands_are_six_rows_and_runs_are_counted() {
        let red = RgbaImage::from_pixel(20, 12, Rgba([255, 0, 0, 255]));
        let sixel = String::from_utf8(encode(&whole(&red), (0, 0, 20, 12))).unwrap();
        assert_eq!(
            sixel, "\x1bP0;1;0q\"1;1;20;12#0;2;100;0;0#0!20~-#0!20~\x1b\\",
            "one register, two full bands, each a run"
        );
        // A run of three is written out, and trailing clear sixels dropped.
        let mut image = RgbaImage::from_pixel(5, 6, Rgba([0, 0, 0, 0]));
        for x in 0..3 {
            image.put_pixel(x, 0, Rgba([0, 0, 255, 255]));
        }
        let sixel = String::from_utf8(encode(&whole(&image), (0, 0, 5, 6))).unwrap();
        assert_eq!(sixel, "\x1bP0;1;0q\"1;1;5;6#0;2;0;0;100#0@@@\x1b\\");
    }

    #[test]
    fn a_picture_of_many_colours_takes_at_most_256_and_stays_close() {
        let mut image = RgbaImage::new(96, 96);
        for (x, y, p) in image.enumerate_pixels_mut() {
            *p = Rgba([
                (x * 255 / 95) as u8,
                (y * 255 / 95) as u8,
                ((x + y) * 255 / 190) as u8,
                255,
            ]);
        }
        let fitted = whole(&image);
        assert!(fitted.picture.palette.len() <= 256);
        assert_eq!(fitted.picture.clear, None);
        let (w, h, pixels) = decode(&encode(&fitted, (0, 0, 96, 96)));
        assert_eq!((w, h), (96, 96));
        let colours: std::collections::HashSet<[u8; 3]> = pixels
            .iter()
            .map(|p| p.expect("every pixel drawn"))
            .collect();
        assert!(colours.len() <= 256, "{} colours", colours.len());
        let worst = image
            .enumerate_pixels()
            .map(|(x, y, p)| {
                let got = pixels[(y * 96 + x) as usize].unwrap();
                (0..3)
                    .map(|c| (got[c] as i32 - p[c] as i32).abs())
                    .max()
                    .unwrap()
            })
            .max()
            .unwrap();
        assert!(worst <= 48, "each pixel near its own colour: {worst} off");
    }

    #[test]
    fn a_picture_is_fitted_into_its_cells_with_its_shape_kept() {
        assert_eq!(fit((1300, 280), (1000, 180)), (836, 180), "as tall as fits");
        assert_eq!(fit((200, 100), (100, 100)), (100, 50), "as wide as fits");
        assert_eq!(fit((0, 10), (10, 10)), (0, 0));
        assert_eq!(cells_of((Some(4), Some(2)), (99, 99), (10, 20)), (4, 2));
        assert_eq!(cells_of((Some(10), None), (100, 200), (10, 20)), (10, 10));
        assert_eq!(cells_of((None, None), (101, 41), (10, 20)), (11, 3));
        let wide = RgbaImage::from_pixel(1300, 280, Rgba([9, 9, 9, 255]));
        let fitted = prepare(&wide, (100, 9), (10, 20)).unwrap();
        assert_eq!(fitted.size, (1000, 180));
        assert_eq!((fitted.picture.width, fitted.picture.height), (836, 180));
        assert_eq!(fitted.at, (82, 0), "centred");
        assert!(prepare(&wide, (2000, 1), (10, 20)).is_err(), "too large");
    }

    #[test]
    fn a_crop_draws_its_part_of_the_picture_and_clear_past_it() {
        // Four quarters, each a colour, in a placement four cells by two
        // of ten by twenty pixels: just its size.
        let quarter = |x: u32, y: u32| match (x < 20, y < 20) {
            (true, true) => Rgba([255, 0, 0, 255]),
            (false, true) => Rgba([0, 255, 0, 255]),
            (true, false) => Rgba([0, 0, 255, 255]),
            (false, false) => Rgba([255, 255, 0, 255]),
        };
        let image = RgbaImage::from_fn(40, 40, quarter);
        let fitted = prepare(&image, (4, 2), (10, 20)).unwrap();
        assert_eq!((fitted.at, fitted.size), ((0, 0), (40, 40)));
        // The bottom row's right half: yellow alone, 18 rows of it.
        let (w, h, pixels) = decode(&encode(&fitted, (20, 20, 20, 18)));
        assert_eq!((w, h), (20, 18));
        assert!(pixels.iter().all(|p| *p == Some([255, 255, 0])));
        // Past the box's right edge: clear.
        let (_, _, pixels) = decode(&encode(&fitted, (30, 0, 20, 6)));
        assert!(pixels[..10].iter().all(|p| *p == Some([0, 255, 0])));
        assert!(pixels[10..20].iter().all(Option::is_none));
        // Scaled down, a solid colour stays itself.
        let red = RgbaImage::from_pixel(40, 40, Rgba([255, 0, 0, 255]));
        let small = prepare(&red, (2, 1), (10, 20)).unwrap();
        assert_eq!((small.picture.width, small.picture.height), (20, 20));
        assert_eq!(small.picture.palette, vec![[255, 0, 0]]);
    }

    #[test]
    fn only_a_png_is_read() {
        let dir = std::env::temp_dir().join(format!("sb-sixel-read-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let png = dir.join("a.png");
        RgbaImage::from_pixel(3, 2, Rgba([1, 2, 3, 255]))
            .save(&png)
            .unwrap();
        assert_eq!(read_png(&png).unwrap().dimensions(), (3, 2));
        let jpeg = dir.join("b.jpg");
        image::RgbImage::from_pixel(3, 2, image::Rgb([1, 2, 3]))
            .save(&jpeg)
            .unwrap();
        assert!(read_png(&jpeg).is_err(), "kitty's f=100 is PNG alone");
        assert!(read_png(&dir.join("missing.png")).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
