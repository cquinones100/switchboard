//! Tall images, sliced for Slack (lib/slack-relay/slices.js). Slack shows
//! an image at a size set by its longest side, so a very tall one, a long
//! diff's screenshot, is shrunk until its width is a sliver and its text
//! unreadable, on a phone above all. A PNG more than `MAX_RATIO` times as
//! tall as it is wide is cut into slices about as tall as it is wide, even
//! in height and in order, and those are shared instead. It is read and
//! written with zlib alone (miniz_oxide): 8-bit RGB and RGBA PNGs without
//! interlacing, which is what Chrome and Playwright save. Anything else,
//! or one too big to decode in memory (`MAX_RAW_BYTES`), is shared as it
//! is.

pub const MAX_RATIO: u32 = 2;
pub const MAX_RAW_BYTES: usize = 512 * 1024 * 1024;
const SIGNATURE: [u8; 8] = [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];

/// A PNG's header and its compressed image data.
#[derive(Debug)]
pub struct Png {
    pub width: u32,
    pub height: u32,
    pub bit_depth: u8,
    pub color_type: u8,
    pub interlace: u8,
    pub idat: Vec<u8>,
}

fn be32(b: &[u8]) -> u32 {
    u32::from_be_bytes([b[0], b[1], b[2], b[3]])
}

/// The PNG in `bytes`, or `None` when it isn't one this reads.
pub fn read_png(bytes: &[u8]) -> Option<Png> {
    if bytes.len() < 33 || bytes[..8] != SIGNATURE {
        return None;
    }
    let mut header = None;
    let mut idat = Vec::new();
    let mut at = 8usize;
    while at + 8 <= bytes.len() {
        let length = be32(&bytes[at..]) as usize;
        let kind = &bytes[at + 4..at + 8];
        let data = bytes.get(at + 8..at + 8 + length)?;
        match kind {
            b"IHDR" if data.len() >= 13 => {
                header = Some((be32(data), be32(&data[4..]), data[8], data[9], data[12]))
            }
            b"IDAT" => idat.extend_from_slice(data),
            b"IEND" => break,
            _ => {}
        }
        at += 12 + length;
    }
    let (width, height, bit_depth, color_type, interlace) = header?;
    Some(Png {
        width,
        height,
        bit_depth,
        color_type,
        interlace,
        idat,
    })
}

/// CRC-32 as PNG's chunks use it.
pub fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for b in data {
        crc ^= *b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xedb8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

/// The PNG for `rows` (each a row's pixels, unfiltered), with the given
/// width and color type, its rows stored without filtering.
pub fn write_png(width: u32, color_type: u8, rows: &[Vec<u8>]) -> Vec<u8> {
    let chunk = |kind: &[u8], data: &[u8]| {
        let mut out = (data.len() as u32).to_be_bytes().to_vec();
        let mut body = kind.to_vec();
        body.extend_from_slice(data);
        out.extend_from_slice(&body);
        out.extend_from_slice(&crc32(&body).to_be_bytes());
        out
    };
    let mut ihdr = width.to_be_bytes().to_vec();
    ihdr.extend_from_slice(&(rows.len() as u32).to_be_bytes());
    ihdr.extend_from_slice(&[8, color_type, 0, 0, 0]);
    let mut raw = Vec::with_capacity(rows.iter().map(|r| r.len() + 1).sum());
    for row in rows {
        raw.push(0);
        raw.extend_from_slice(row);
    }
    let mut out = SIGNATURE.to_vec();
    out.extend(chunk(b"IHDR", &ihdr));
    out.extend(chunk(
        b"IDAT",
        &miniz_oxide::deflate::compress_to_vec_zlib(&raw, 6),
    ));
    out.extend(chunk(b"IEND", &[]));
    out
}

/// Undoes PNG's per-row filters (none, sub, up, average, Paeth) on `line`
/// in place, given the row above it.
fn unfilter(filter: u8, line: &mut [u8], above: &[u8], bpp: usize) {
    for i in 0..line.len() {
        let a = if i >= bpp { line[i - bpp] as i32 } else { 0 };
        let b = above[i] as i32;
        let c = if i >= bpp { above[i - bpp] as i32 } else { 0 };
        let add = match filter {
            1 => a,
            2 => b,
            3 => (a + b) >> 1,
            4 => {
                let p = a + b - c;
                let (pa, pb, pc) = ((p - a).abs(), (p - b).abs(), (p - c).abs());
                if pa <= pb && pa <= pc {
                    a
                } else if pb <= pc {
                    b
                } else {
                    c
                }
            }
            _ => 0,
        };
        line[i] = ((line[i] as i32 + add) & 255) as u8;
    }
}

/// The file as it is, or its slices, named `<name>-part<k>of<n>.png`.
pub fn sliced(filename: &str, content: Vec<u8>) -> Vec<(String, Vec<u8>)> {
    let Some(png) = read_png(&content) else {
        return vec![(filename.to_string(), content)];
    };
    let bpp = match (png.bit_depth, png.interlace, png.color_type) {
        (8, 0, 2) => 3,
        (8, 0, 6) => 4,
        _ => return vec![(filename.to_string(), content)],
    };
    if png.width == 0 || png.height <= png.width * MAX_RATIO {
        return vec![(filename.to_string(), content)];
    }
    let stride = png.width as usize * bpp;
    let height = png.height as usize;
    if (stride + 1) * height > MAX_RAW_BYTES {
        return vec![(filename.to_string(), content)];
    }
    let Ok(raw) = miniz_oxide::inflate::decompress_to_vec_zlib(&png.idat) else {
        return vec![(filename.to_string(), content)];
    };
    if raw.len() < (stride + 1) * height {
        return vec![(filename.to_string(), content)];
    }
    let parts = height.div_ceil(png.width as usize);
    let each = height.div_ceil(parts);
    let base = if filename.to_ascii_lowercase().ends_with(".png") {
        &filename[..filename.len() - 4]
    } else {
        filename
    };
    let mut slices: Vec<Vec<Vec<u8>>> = vec![Vec::new(); parts];
    let mut above = vec![0u8; stride];
    for y in 0..height {
        let start = y * (stride + 1);
        let mut line = raw[start + 1..start + 1 + stride].to_vec();
        unfilter(raw[start], &mut line, &above, bpp);
        slices[y / each].push(line.clone());
        above = line;
    }
    slices
        .iter()
        .enumerate()
        .map(|(i, rows)| {
            (
                format!("{base}-part{}of{parts}.png", i + 1),
                write_png(png.width, png.color_type, rows),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A PNG whose rows are stored with each of the filters in turn, so
    /// slicing has to undo them all.
    fn filtered_png(
        width: u32,
        height: u32,
        pixel: impl Fn(u32, u32) -> [u8; 3],
    ) -> (Vec<u8>, Vec<Vec<u8>>) {
        let rows: Vec<Vec<u8>> = (0..height)
            .map(|y| (0..width).flat_map(|x| pixel(x, y)).collect())
            .collect();
        let mut raw = Vec::new();
        let zero = vec![0u8; rows[0].len()];
        for (y, row) in rows.iter().enumerate() {
            let above = if y == 0 { &zero } else { &rows[y - 1] };
            let filter = (y % 5) as u8;
            raw.push(filter);
            for i in 0..row.len() {
                let a = if i >= 3 { row[i - 3] as i32 } else { 0 };
                let b = above[i] as i32;
                let c = if i >= 3 { above[i - 3] as i32 } else { 0 };
                let predicted = match filter {
                    1 => a,
                    2 => b,
                    3 => (a + b) >> 1,
                    4 => {
                        let p = a + b - c;
                        let (pa, pb, pc) = ((p - a).abs(), (p - b).abs(), (p - c).abs());
                        if pa <= pb && pa <= pc {
                            a
                        } else if pb <= pc {
                            b
                        } else {
                            c
                        }
                    }
                    _ => 0,
                };
                raw.push(((row[i] as i32 - predicted) & 255) as u8);
            }
        }
        let mut png = SIGNATURE.to_vec();
        let chunk = |kind: &[u8], data: &[u8]| {
            let mut out = (data.len() as u32).to_be_bytes().to_vec();
            let mut body = kind.to_vec();
            body.extend_from_slice(data);
            out.extend_from_slice(&body);
            out.extend_from_slice(&crc32(&body).to_be_bytes());
            out
        };
        let mut ihdr = width.to_be_bytes().to_vec();
        ihdr.extend_from_slice(&height.to_be_bytes());
        ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
        png.extend(chunk(b"IHDR", &ihdr));
        png.extend(chunk(
            b"IDAT",
            &miniz_oxide::deflate::compress_to_vec_zlib(&raw, 6),
        ));
        png.extend(chunk(b"IEND", &[]));
        (png, rows)
    }

    #[test]
    fn a_tall_image_is_sliced_about_as_tall_as_it_is_wide_its_pixels_kept() {
        let pixel = |x: u32, y: u32| [(x * 7 + y) as u8, (y * 3) as u8, (x ^ y) as u8];
        let (png, rows) = filtered_png(10, 35, pixel);
        let parts = sliced("long.png", png);
        assert_eq!(
            parts.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(),
            [
                "long-part1of4.png",
                "long-part2of4.png",
                "long-part3of4.png",
                "long-part4of4.png"
            ]
        );
        let mut back = Vec::new();
        for (_, bytes) in &parts {
            let png = read_png(bytes).unwrap();
            assert_eq!(png.width, 10);
            let raw = miniz_oxide::inflate::decompress_to_vec_zlib(&png.idat).unwrap();
            for y in 0..png.height as usize {
                let start = y * 31;
                assert_eq!(raw[start], 0);
                back.push(raw[start + 1..start + 31].to_vec());
            }
        }
        assert_eq!(back, rows, "every pixel, in order");
    }

    #[test]
    fn a_short_image_or_anything_else_is_shared_as_it_is() {
        let (png, _) = filtered_png(10, 15, |_, _| [1, 2, 3]);
        assert_eq!(sliced("a.png", png.clone()), [("a.png".to_string(), png)]);
        assert_eq!(sliced("b.jpg", b"not a png".to_vec()).len(), 1);
        assert_eq!(crc32(b"IEND"), 0xae42_6082);
    }
}
