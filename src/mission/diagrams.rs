//! The Mermaid diagrams of the description beside mission control's PR and
//! ticket boards, drawn as pictures as the PR and ticket views draw them
//! (`views::pictures`), where the board drew each as text boxes cut at the
//! pane's edge. New with the Rust version: the tmux version's boards had no
//! description beside them.
//!
//! The server draws mission control on its own task, where nothing reads a
//! file or starts Chrome, so the diagrams are asked for off it
//! (`mission::serve`) and kept here once had: what is known of each, by the
//! theme it was drawn in and its key in the layout (`Wanted::key`), so one
//! asked for or failed isn't asked for again, and the ready ones' bytes
//! held to `HELD`, the oldest going first. A snapshot takes the ready ones
//! (`ready`), and the board lays the description out with them
//! (`render::about`'s `Images`); until one is ready, and on a client that
//! draws no images, its text drawing stays.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use image::RgbaImage;

use crate::views::markdown::{Markdown, Wanted};

/// The most memory the ready diagrams hold together.
const HELD: usize = 64 * 1024 * 1024;

/// The widest cell, in pixels, a diagram is kept for: more than any
/// terminal's (a Retina Ghostty's is about 18, Windows Terminal's sixel's
/// 10), so the one copy every terminal attached draws from, each scaling
/// it to its own cells, is sharp in all of them.
pub const WIDEST_CELL: u32 = 24;

/// A diagram ready to draw: its picture, and a key made of its pixels once
/// (`ui::images::picture_key`), so a draw hashes nothing.
#[derive(Clone, Debug)]
pub struct Ready {
    pub image: Arc<RgbaImage>,
    pub content: u64,
}

#[derive(Debug)]
enum Known {
    Asked,
    Failed,
    Ready(Ready),
}

/// What the server knows of the diagrams it has been asked to draw.
#[derive(Debug, Default)]
pub struct Diagrams {
    known: HashMap<(bool, String), Known>,
    /// The ready ones, oldest first.
    order: VecDeque<(bool, String)>,
    held: usize,
}

/// The Mermaid diagrams a description holds, by their source, each once.
pub fn sources(description: &str, place: &str) -> Vec<String> {
    Markdown::parse(description, place)
        .wanted()
        .into_iter()
        .filter_map(|w| match w {
            Wanted::Diagram(source) => Some(source),
            _ => None,
        })
        .collect()
}

/// A diagram made no wider than `width` pixels, its shape kept: the side
/// pane's width at the widest cell (`WIDEST_CELL`), so the server holds no
/// more of it than a terminal shows.
pub fn fitted(img: RgbaImage, width: u32) -> RgbaImage {
    let (w, h) = img.dimensions();
    let width = width.max(1);
    if w <= width {
        return img;
    }
    let scaled_h = ((h as f64 * width as f64 / w as f64) as u32).max(1);
    image::imageops::resize(&img, width, scaled_h, image::imageops::FilterType::Triangle)
}

fn key(source: &str) -> String {
    Wanted::Diagram(source.to_string()).key()
}

impl Diagrams {
    /// Those of `sources` nothing is known of yet in this theme, now taken
    /// as asked for: the ones to draw.
    pub fn ask(&mut self, sources: Vec<String>, light: bool) -> Vec<String> {
        sources
            .into_iter()
            .filter(|source| {
                let at = (light, key(source));
                let is_new = !self.known.contains_key(&at);
                if is_new {
                    self.known.insert(at, Known::Asked);
                }
                is_new
            })
            .collect()
    }

    /// A diagram drawn, or one that couldn't be, which stays its text.
    /// Whether one became ready.
    pub fn got(&mut self, source: &str, light: bool, image: Option<RgbaImage>) -> bool {
        let at = (light, key(source));
        let Some(image) = image.filter(|i| i.as_raw().len() <= HELD) else {
            self.known.insert(at, Known::Failed);
            return false;
        };
        self.held += image.as_raw().len();
        let ready = Ready {
            content: crate::ui::images::picture_key(&image),
            image: Arc::new(image),
        };
        if let Some(Known::Ready(old)) = self.known.insert(at.clone(), Known::Ready(ready)) {
            self.held -= old.image.as_raw().len();
            self.order.retain(|o| *o != at);
        }
        self.order.push_back(at);
        // The oldest forgotten past the limit, to be asked for again (from
        // the image cache) when a description shows one.
        while self.held > HELD && self.order.len() > 1 {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            if let Some(Known::Ready(old)) = self.known.remove(&oldest) {
                self.held -= old.image.as_raw().len();
            }
        }
        true
    }

    /// The diagrams ready in this theme, by their key in the layout.
    pub fn ready(&self, light: bool) -> HashMap<String, Ready> {
        self.known
            .iter()
            .filter_map(|((theme, key), known)| match known {
                Known::Ready(ready) if *theme == light => Some((key.clone(), ready.clone())),
                _ => None,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BODY: &str = "Why\n\n```mermaid\nflowchart LR\n  A --> B\n```\n\n```mermaid\nflowchart LR\n  A --> B\n```\n\n```mermaid\nsequenceDiagram\n  A->>B: hi\n```\n";

    #[test]
    fn a_description_s_diagrams_are_found_once_each() {
        let found = sources(BODY, "the PR");
        assert_eq!(found.len(), 2, "{found:?}");
        assert!(found[0].contains("flowchart") && found[1].contains("sequenceDiagram"));
        assert!(sources("No diagram here.", "the PR").is_empty());
    }

    #[test]
    fn a_diagram_is_asked_for_once_in_a_theme_and_ready_once_drawn() {
        let mut d = Diagrams::default();
        let found = sources(BODY, "the PR");
        assert_eq!(d.ask(found.clone(), false).len(), 2);
        assert!(d.ask(found.clone(), false).is_empty(), "asked for already");
        assert_eq!(d.ask(found.clone(), true).len(), 2, "the light theme's");
        assert!(d.ready(false).is_empty());

        assert!(d.got(&found[0], false, Some(RgbaImage::new(40, 20))));
        assert!(!d.got(&found[1], false, None), "one Mermaid refused");
        let ready = d.ready(false);
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[&key(&found[0])].image.dimensions(), (40, 20));
        assert!(d.ready(true).is_empty(), "drawn for the dark theme alone");
        assert!(
            d.ask(found, false).is_empty(),
            "a failed one isn't asked again"
        );
    }

    #[test]
    fn the_oldest_diagrams_go_past_the_limit_and_are_asked_for_again() {
        let mut d = Diagrams::default();
        let side = 2048;
        let each = (side * side * 4) as usize;
        let n = HELD / each + 1;
        let names: Vec<String> = (0..n)
            .map(|i| format!("flowchart LR\n  A --> B{i}"))
            .collect();
        d.ask(names.clone(), false);
        for name in &names {
            d.got(name, false, Some(RgbaImage::new(side, side)));
        }
        assert!(d.held <= HELD);
        assert_eq!(d.ready(false).len(), n - 1);
        assert_eq!(d.ask(names, false).len(), 1, "the first, forgotten");
    }

    #[test]
    fn a_wide_diagram_is_kept_at_the_pane_s_width_its_shape_kept() {
        assert_eq!(
            fitted(RgbaImage::new(1600, 800), 400).dimensions(),
            (400, 200)
        );
        assert_eq!(
            fitted(RgbaImage::new(300, 200), 400).dimensions(),
            (300, 200)
        );
    }
}
