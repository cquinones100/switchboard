//! A picture shown full size over the pane that asked for it (`switchboard
//! picture <file>`, from the notes app's Enter on a picture), in place of
//! Quick Look: Images' picture atom (src/ui/images.rs) as large as the pane,
//! less a row of key hints under it, reaching the terminal as any atom
//! does (src/mux/graphics.rs): sent inline by kitty's protocol, or drawn
//! as sixel over its cells with Settings' "Image protocol" on sixel. Esc, q
//! or Enter go back, as does a click; every other key is kept from the
//! pane while it is up.
//!
//! The server keeps `area` on the pane's place on each draw
//! (`Server::pictures_before_draw`), so a resize or a split moves it with
//! the pane, and closes it when the pane goes or leaves the screen. The
//! picture is decoded and made no larger than `LARGEST` off the server's
//! task, and its key made once, so a draw neither reads, hashes nor copies
//! its pixels.

use std::path::Path;
use std::sync::Arc;

use image::{ImageDecoder, RgbaImage};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};

use super::draw::{self, Gesture};
use super::{Look, Outcome, Overlay, Step};
use super::{images, kit};
use crate::mux::PaneId;
use crate::mux::compose::Frame;
use crate::mux::input::{Key, KeyCode, Mouse};

/// The widest or tallest a picture is kept, in pixels: more than a pane
/// on a large screen draws, so nothing shows softer, and little enough that
/// rastering it at a new size stays quick.
pub const LARGEST: u32 = 3200;
/// The largest file read as a picture.
pub const LARGEST_FILE: u64 = 64 * 1024 * 1024;
/// The most memory a picture may take once decoded, checked before any is
/// taken, since a small file can say it is enormous.
pub const LARGEST_DECODED: u64 = 512 * 1024 * 1024;

/// A picture file decoded, no larger than `LARGEST` either way.
pub fn decode(file: &Path) -> Result<RgbaImage, String> {
    let size = std::fs::metadata(file)
        .map_err(|e| format!("Couldn't read {}: {e}", file.display()))?
        .len();
    if size > LARGEST_FILE {
        return Err(format!("{} is too large to show", file.display()));
    }
    let unreadable = |_| format!("{} isn't a picture this can show", file.display());
    let mut decoder = image::ImageReader::open(file)
        .and_then(|r| r.with_guessed_format())
        .map_err(|e| format!("Couldn't read {}: {e}", file.display()))?
        .into_decoder()
        .map_err(unreadable)?;
    // A photo stored on its side is turned as its EXIF says, as Quick Look
    // turns it.
    if decoder.total_bytes() > LARGEST_DECODED {
        return Err(format!("{} is too large to show", file.display()));
    }
    let orientation = decoder.orientation().ok();
    let mut img = image::DynamicImage::from_decoder(decoder).map_err(unreadable)?;
    if let Some(o) = orientation {
        img.apply_orientation(o);
    }
    let img = if img.width() > LARGEST || img.height() > LARGEST {
        img.resize(LARGEST, LARGEST, image::imageops::FilterType::Triangle)
    } else {
        img
    };
    Ok(img.to_rgba8())
}

pub struct Picture {
    pub pane: PaneId,
    /// The pane's place on the screen, as the server last found it.
    pub area: Option<Rect>,
    title: String,
    img: Arc<RgbaImage>,
    key: u64,
}

impl Picture {
    pub fn new(pane: PaneId, title: &str, img: RgbaImage) -> Picture {
        Picture {
            pane,
            area: None,
            title: title.to_string(),
            key: images::picture_key(&img),
            img: Arc::new(img),
        }
    }

    pub fn title(&self) -> &str {
        &self.title
    }
}

/// The row of key hints under the picture: its name, then the key in the
/// accent color and what it does dim.
fn hints(title: &str, look: &Look) -> Line<'static> {
    let mut spans = vec![
        Span::styled(format!(" {title}"), look.text()),
        Span::raw("  "),
    ];
    spans.extend(kit::hints(&[("Esc", "back")], &look.theme));
    Line::from(spans)
}

impl Overlay for Picture {
    fn kind(&self) -> &'static str {
        "picture"
    }

    fn draw(&mut self, _buf: &mut Buffer, _look: &Look) {}

    fn draw_frame(&mut self, frame: &mut Frame, look: &Look) {
        let Some(area) = self.area.map(|a| a.intersection(frame.buf.area)) else {
            return;
        };
        if area.height < 2 || area.width == 0 {
            return;
        }
        for y in area.top()..area.bottom() {
            for x in area.left()..area.right() {
                frame.buf[(x, y)].reset();
            }
        }
        frame.links.clear_area(area);
        let shown = Rect {
            height: area.height - 1,
            ..area
        };
        if frame.graphics.is_some() {
            images::picture_shared(
                frame,
                &format!("picture:{}", self.pane),
                shown,
                &self.title,
                &self.img,
                self.key,
            );
        } else {
            let say = Line::from(Span::styled(
                format!(" {}: this terminal doesn't draw pictures", self.title),
                look.dim(),
            ));
            draw::put_line(&mut frame.buf, area.x, area.y, area.width, &say);
        }
        draw::put_line(
            &mut frame.buf,
            area.x,
            area.bottom() - 1,
            area.width,
            &hints(&self.title, look),
        );
    }

    fn key(&mut self, key: Key, _look: &Look) -> Step {
        match key.code {
            KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') => Step::Close(Outcome::Dismissed),
            _ => Step::Same,
        }
    }

    fn mouse(&mut self, mouse: Mouse, _look: &Look) -> Step {
        match draw::gesture(&mouse) {
            Gesture::Click => Step::Close(Outcome::Dismissed),
            _ => Step::Same,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mux::input::Mods;

    fn picture() -> Picture {
        Picture::new(PaneId(3), "shot.png", RgbaImage::new(40, 20))
    }

    #[test]
    fn it_fills_the_pane_with_the_picture_and_its_keys_under_it() {
        let mut p = picture();
        p.area = Some(Rect::new(10, 2, 30, 12));
        let mut frame = Frame::new(60, 20);
        frame.graphics = Some(crate::mux::graphics::Graphics::kitty((8, 16)));
        p.draw_frame(&mut frame, &Look::default());
        assert_eq!(frame.images.len(), 1, "one image");
        let placed = &frame.images[0];
        assert_eq!(
            (placed.cols, placed.rows),
            (30, 11),
            "the pane less the hints row"
        );
        let hints: String = (10..40)
            .map(|x| frame.buf[(x, 13)].symbol().to_string())
            .collect();
        assert!(hints.contains("shot.png  Esc back"), "{hints}");
        assert_eq!(frame.buf[(9, 5)].symbol(), " ", "nothing left of the pane");
    }

    #[test]
    fn without_graphics_it_says_so_rather_than_drawing() {
        let mut p = picture();
        p.area = Some(Rect::new(0, 0, 60, 10));
        let mut frame = Frame::new(60, 10);
        p.draw_frame(&mut frame, &Look::default());
        assert!(frame.images.is_empty());
        let top: String = (0..60)
            .map(|x| frame.buf[(x, 0)].symbol().to_string())
            .collect();
        assert!(top.contains("doesn't draw pictures"), "{top}");
    }

    #[test]
    fn esc_goes_back_and_other_keys_stay_with_it() {
        let mut p = picture();
        let look = Look::default();
        assert_eq!(
            p.key(Key::new(KeyCode::Esc, Mods::NONE), &look),
            Step::Close(Outcome::Dismissed)
        );
        assert_eq!(
            p.key(Key::new(KeyCode::Char('j'), Mods::NONE), &look),
            Step::Same
        );
    }

    #[test]
    fn a_picture_is_decoded_no_larger_than_the_largest_kept() {
        let dir = std::env::temp_dir().join(format!(
            "sb-picture-{}-{}",
            std::process::id(),
            crate::util::now_ms()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("wide.png");
        RgbaImage::new(LARGEST * 2, 10).save(&file).unwrap();
        let img = decode(&file).unwrap();
        assert_eq!(img.width(), LARGEST);
        let text = dir.join("note.md");
        std::fs::write(&text, "# not a picture").unwrap();
        assert!(decode(&text).is_err());
        // A small file saying it is far larger than any screen.
        let huge = dir.join("huge.bmp");
        let mut bmp = b"BM".to_vec();
        bmp.extend(70u32.to_le_bytes());
        bmp.extend([0; 4]);
        bmp.extend(54u32.to_le_bytes());
        bmp.extend(40u32.to_le_bytes());
        bmp.extend(60000i32.to_le_bytes());
        bmp.extend(60000i32.to_le_bytes());
        bmp.extend(1u16.to_le_bytes());
        bmp.extend(24u16.to_le_bytes());
        bmp.extend([0; 24]);
        std::fs::write(&huge, bmp).unwrap();
        assert!(
            decode(&huge).unwrap_err().contains("too large"),
            "refused before taking the memory"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
