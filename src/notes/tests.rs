//! Notes and the workspace together: what the app draws, as the server
//! reads it. The app's own tests are its crate's (crates/notes).

use std::path::PathBuf;
use std::sync::mpsc::channel;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::app::{App, Options};
use super::store::Store;
use crate::mux::PaneId;
use crate::mux::sixel::place;

const COLS: u16 = 120;
const ROWS: u16 = 30;

/// A notes directory of its own for a test, removed when it ends.
struct Dir(PathBuf);

impl Dir {
    fn new() -> Dir {
        let path =
            std::env::temp_dir().join(format!("notes-workspace-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Dir(path)
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

// UX 2.0's chips and a note's pictures, as the workspace reads the screen
// to draw the pictures as sixel (src/mux/sixel/place.rs): a picture's rows
// follow the chip row under its item, in order, from its first, as the
// note scrolls, and one cut by the bottom of the screen shows its first.
#[test]
fn a_picture_under_an_item_with_chips_scrolls_with_them_in_one_piece() {
    let dir = Dir::new();
    let mut store = Store::at(&dir.0);
    store.state = dir.0.join(".state-for-test");
    // Pictures of 80 by 48 pixels: ten cells of 8 by 16 wide, three tall.
    for name in ["a.png", "b.png"] {
        image::RgbaImage::from_pixel(80, 48, image::Rgba([0, 0, 255, 255]))
            .save(dir.0.join(name))
            .unwrap();
    }
    let path = store.create("", Some("Plan"));
    // Lines enough that the second picture is cut by the bottom of the
    // note, over UX 2.0's "Add an item" row.
    let mut text = String::from("# Plan\n\n");
    for n in 0..17 {
        text.push_str(&format!("line {n}\n"));
    }
    // Long enough to be cut before its chip while the cursor is elsewhere,
    // its picture still under it.
    text.push_str(&format!(
        "- [ ] Pick a name, needs a choice {}![name](a.png)\n",
        "word ".repeat(30)
    ));
    text.push_str("- [ ] Draw the logo, needs a choice\n  ![logo](b.png)\n");
    for n in 17..50 {
        text.push_str(&format!("line {n}\n"));
    }
    std::fs::write(&path, &text).unwrap();
    let opts = Options {
        images: true,
        list: false,
        ..Options::default()
    };
    let (tx, _rx) = channel();
    let mut app = App::new(store, opts, COLS, ROWS, tx);
    app.images.cell = (8, 16);
    app.start();
    // The screen's rows, and the pieces the workspace finds in it.
    let draw = |app: &mut App| {
        let mut buf = Buffer::empty(Rect::new(0, 0, COLS, ROWS));
        app.draw(&mut buf);
        let rows: Vec<String> = (0..buf.area.height)
            .map(|y| {
                (0..buf.area.width)
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect::<String>()
            })
            .collect();
        let drawn = place::Drawn::of(&buf, vec![(PaneId(1), buf.area)]);
        let bottom = buf.area.height;
        let pieces = place::find(&mut buf, &drawn, &Default::default(), bottom);
        (rows, pieces)
    };
    // Each piece a picture from its first row, ten cells wide, under its
    // item, its chips at its right, or under the line naming it that is
    // under the item, as far up as the note shows (the toolbar on the top
    // row).
    let check = |rows: &[String], pieces: &[place::Piece]| {
        let screen = rows.join("\n");
        for p in pieces {
            assert_eq!((p.row, p.col, p.area.width), (0, 0, 10), "{p:?}\n{screen}");
            let y = p.area.y as usize;
            let above: &[&str] = if rows[y - 1].contains("Pick a name") {
                &["Pick a name"]
            } else {
                &["logo", "Draw the logo"]
            };
            for (n, want) in above.iter().enumerate() {
                let Some(at) = y.checked_sub(n + 1).filter(|at| *at >= 1) else {
                    break;
                };
                assert!(rows[at].contains(want), "{want} at {at}: {p:?}\n{screen}");
            }
        }
    };
    // From the top: the first picture whole, the second cut by the
    // bottom of the screen after its first two rows.
    let (rows, pieces) = draw(&mut app);
    check(&rows, &pieces);
    let heights: Vec<u16> = pieces.iter().map(|p| p.area.height).collect();
    assert_eq!(heights, vec![3, 2], "{pieces:?}\n{}", rows.join("\n"));
    assert_eq!(pieces[1].area.bottom(), app.note_area().bottom());
    // The cursor down a line at a time, the note scrolling up under it: the
    // pictures and their items go up together, whole, and each leaves
    // the screen with its line.
    let mut whole = 0;
    for row in 20..50 {
        app.editor.doc.row = row;
        let (rows, pieces) = draw(&mut app);
        check(&rows, &pieces);
        whole += pieces.iter().filter(|p| p.area.height == 3).count();
    }
    assert!(whole > 0, "the pictures seen whole as the note scrolled");
}
