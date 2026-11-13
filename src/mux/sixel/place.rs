//! Where a frame shows pictures: the cells holding kitty's placeholder
//! (U+10EEEE) after everything is drawn, the panes' text and whatever
//! boxes are over it, read as kitty reads them, and gathered into
//! rectangles of one picture each, so the server can draw each as sixel
//! (src/mux/sixel/mod.rs). A picture is a pane's (Notes', a view's), read
//! inside its pane, or one of the server's own (src/ui/images.rs), read
//! anywhere in the frame by the ids the frame's images have.
//!
//! A placeholder cell names its image by its foreground colour (24-bit, or
//! a 256-colour index), with a third combining mark for the id's highest
//! byte, and its row and column of the picture by its first two marks
//! (`notes::images::MARKS`); a mark left out is taken from the cell to its
//! left as kitty takes it. The cells are read inside each pane a picture
//! may come from, and each run of a row's cells of one picture, its
//! columns in order, is a piece, joined with the runs under it that go on
//! from its rows over the same columns. So a box over the middle of a
//! picture leaves a piece above it, one below and those beside it, and a
//! picture scrolled half off, under UX 2.0's header among them, is a piece
//! starting at the row it is cut at, which is the pane's first.
//!
//! A cell is read as a pane's only while it is still what its pane drew
//! there (`Drawn`, noted once the panes are drawn and before anything is
//! drawn over them): something drawn over a pane may copy a pane's cells,
//! placeholders and all, as UX 2.0's mission control cards show each tab's
//! last lines and a peek shows a tab's screen, and such a copy is no place
//! the pane's picture is. Any other cell naming one of the server's images
//! is that image's: the server writes its images' cells itself, where
//! they go, and copies none.
//!
//! Every placeholder cell in the frame is blanked (a space on its own
//! background), wherever it is, since a terminal without kitty's pictures
//! draws the character as a box: those of the panes, and those copied
//! anywhere else (a card, the header's rows, the sidebar). No piece is
//! ever on the frame's last row, which a pane reaches with UX 2.0's status
//! line gone: a sixel reaching it would scroll the screen, so a picture
//! there loses that row.

use std::collections::{HashMap, HashSet};

use ratatui::buffer::{Buffer, Cell};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier};

use crate::mux::PaneId;
use crate::notes::images::{MARKS, PLACEHOLDER};

/// A part of a picture a frame shows: which pane's image (`None`, the
/// server's own), where on the screen, and the picture's column and row of
/// its top left cell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Piece {
    pub pane: Option<PaneId>,
    pub id: u32,
    pub area: Rect,
    pub col: u16,
    pub row: u16,
}

/// A placeholder cell as read: its image, and its row and column of it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Mark {
    id: u32,
    row: u16,
    col: u16,
}

/// The number a combining mark stands for, by its place in `MARKS`.
fn number(c: char) -> Option<u16> {
    MARKS.binary_search(&(c as u32)).ok().map(|n| n as u16)
}

/// Whether a cell holds a placeholder.
fn is_placeholder(cell: &Cell) -> bool {
    cell.symbol().starts_with(PLACEHOLDER)
}

/// The placeholder cells the panes drew, by where they are, as they drew
/// them: taken once the panes are drawn and before anything is drawn over
/// them, so that afterwards a cell is read as a pane's only while it is
/// still the same.
#[derive(Debug, Default)]
pub struct Drawn {
    panes: Vec<(PaneId, Rect)>,
    cells: HashMap<(u16, u16), Cell>,
}

impl Drawn {
    /// What `panes` (each pane a picture may come from, and its area of
    /// `buf`) drew.
    pub fn of(buf: &Buffer, panes: Vec<(PaneId, Rect)>) -> Drawn {
        let mut cells = HashMap::new();
        for (_, area) in &panes {
            let area = area.intersection(buf.area);
            for y in area.top()..area.bottom() {
                for x in area.left()..area.right() {
                    let cell = &buf[(x, y)];
                    if is_placeholder(cell) {
                        cells.insert((x, y), cell.clone());
                    }
                }
            }
        }
        Drawn { panes, cells }
    }

    /// Whether the cell at `x`, `y` is still what its pane drew there.
    fn still(&self, x: u16, y: u16, cell: &Cell) -> bool {
        self.cells
            .get(&(x, y))
            .is_some_and(|c| c.symbol() == cell.symbol() && c.fg == cell.fg)
    }
}

/// A cell's placeholder, read with the cell to its left (`left`) for what
/// its marks leave out; `None` for a cell that isn't one, or whose colour
/// names no image.
fn read(cell: &Cell, left: Option<Mark>) -> Option<Option<Mark>> {
    let mut chars = cell.symbol().chars();
    if chars.next()? != PLACEHOLDER {
        return None;
    }
    let low = match cell.fg {
        Color::Rgb(r, g, b) => (r as u32) << 16 | (g as u32) << 8 | b as u32,
        Color::Indexed(n) => n as u32,
        _ => return Some(None),
    };
    let marks: Vec<u16> = chars.map_while(number).take(3).collect();
    // The cell to the left, when it is of the same image (its low bits;
    // the high byte may be what is inherited).
    let same = left.filter(|l| l.id & 0x00ff_ffff == low);
    let mark = match marks.as_slice() {
        [] => match same {
            Some(l) => Mark {
                id: l.id,
                row: l.row,
                col: l.col + 1,
            },
            None => Mark {
                id: low,
                row: 0,
                col: 0,
            },
        },
        [row] => match same.filter(|l| l.row == *row) {
            Some(l) => Mark {
                id: l.id,
                row: *row,
                col: l.col + 1,
            },
            None => Mark {
                id: low,
                row: *row,
                col: 0,
            },
        },
        [row, col] => {
            let high = same
                .filter(|l| l.row == *row && l.col + 1 == *col)
                .map_or(0, |l| l.id & 0xff00_0000);
            Mark {
                id: low | high,
                row: *row,
                col: *col,
            }
        }
        [row, col, high, ..] => Mark {
            id: low | (*high as u32 & 0xff) << 24,
            row: *row,
            col: *col,
        },
    };
    Some(Some(mark))
}

/// A cell as a blank on its own background.
fn blank(cell: &mut Cell) {
    cell.set_symbol(" ");
    cell.fg = Color::Reset;
    cell.modifier = Modifier::empty();
}

/// The pieces of pictures in `buf`, none at or below row `bottom`: in
/// each pane's area, from the cells still as the pane drew them (`drawn`),
/// and anywhere else, those of the server's own images, by their ids
/// (`server`); every placeholder in `buf`, wherever it is, blanked.
pub fn find(buf: &mut Buffer, drawn: &Drawn, server: &HashSet<u32>, bottom: u16) -> Vec<Piece> {
    let mut done: Vec<Piece> = Vec::new();
    for (pane, area) in &drawn.panes {
        let area = area.intersection(buf.area);
        done.extend(pieces_in(
            buf,
            area,
            bottom,
            Some(*pane),
            |x, y, cell, left| drawn.still(x, y, cell).then(|| read(cell, left)).flatten(),
        ));
    }
    if !server.is_empty() {
        done.extend(pieces_in(
            buf,
            buf.area,
            bottom,
            None,
            |x, y, cell, left| {
                let mark = read(cell, left)?;
                (!drawn.still(x, y, cell)).then(|| mark.filter(|m| server.contains(&m.id)))
            },
        ));
    }
    for cell in buf.content.iter_mut() {
        if is_placeholder(cell) {
            blank(cell);
        }
    }
    done.sort_by_key(|p| (p.area.y, p.area.x));
    done
}

/// The pieces in `area` of `buf`, none at or below row `bottom`, of the
/// pane `pane` (`None`, the server's): each run of a row's cells of one
/// picture, its columns in order, joined with the runs under it that go
/// on from its rows over the same columns. `mark_at` reads a cell with the
/// mark of the one to its left: `None` for one that is no placeholder here,
/// `Some(None)` for one naming no picture.
fn pieces_in(
    buf: &Buffer,
    area: Rect,
    bottom: u16,
    pane: Option<PaneId>,
    mark_at: impl Fn(u16, u16, &Cell, Option<Mark>) -> Option<Option<Mark>>,
) -> Vec<Piece> {
    let mut done: Vec<Piece> = Vec::new();
    // The pieces that reached the row above, which a run may go on.
    let mut open: Vec<Piece> = Vec::new();
    for y in area.top()..area.bottom() {
        let mut next: Vec<Piece> = Vec::new();
        let mut left: Option<Mark> = None;
        let mut run: Option<Piece> = None;
        for x in area.left()..area.right() {
            let mark = match mark_at(x, y, &buf[(x, y)], left) {
                None => {
                    left = None;
                    end_run(&mut run, &mut open, &mut next);
                    continue;
                }
                Some(mark) => mark,
            };
            left = mark;
            let Some(mark) = mark.filter(|_| y < bottom) else {
                end_run(&mut run, &mut open, &mut next);
                continue;
            };
            let goes_on = run.is_some_and(|r| {
                r.id == mark.id && r.row == mark.row && r.col + r.area.width == mark.col
            });
            if goes_on {
                if let Some(r) = &mut run {
                    r.area.width += 1;
                }
            } else {
                end_run(&mut run, &mut open, &mut next);
                run = Some(Piece {
                    pane,
                    id: mark.id,
                    area: Rect::new(x, y, 1, 1),
                    col: mark.col,
                    row: mark.row,
                });
            }
        }
        end_run(&mut run, &mut open, &mut next);
        done.append(&mut open);
        open = next;
    }
    done.append(&mut open);
    done
}

/// A row's run ended: joined to the piece above it that it goes on from,
/// else a piece of its own, among those reaching this row (`next`).
fn end_run(run: &mut Option<Piece>, open: &mut Vec<Piece>, next: &mut Vec<Piece>) {
    let Some(r) = run.take() else {
        return;
    };
    let above = open.iter().position(|p| {
        p.id == r.id
            && p.area.x == r.area.x
            && p.area.width == r.area.width
            && p.col == r.col
            && p.row + p.area.height == r.row
            && p.area.bottom() == r.area.y
    });
    match above {
        Some(i) => {
            let mut p = open.swap_remove(i);
            p.area.height += 1;
            next.push(p);
        }
        None => next.push(r),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notes::images::Picture;
    use ratatui::style::Style;

    const PANE: PaneId = PaneId(1);

    /// Draws a picture's placeholders, `rows` from `from_row` of it, `cols`
    /// wide, at `x`, `y`, as Notes writes them.
    fn draw(buf: &mut Buffer, id: u32, (x, y): (u16, u16), from_row: u16, rows: u16, cols: u16) {
        let fg = Color::Rgb((id >> 16) as u8, (id >> 8) as u8, id as u8);
        for r in 0..rows {
            for c in 0..cols {
                buf[(x + c, y + r)]
                    .set_symbol(&Picture::cell(from_row + r, c))
                    .set_style(Style::new().fg(fg));
            }
        }
    }

    /// The pieces in `buf`, its panes' cells as they are now being what
    /// the panes drew.
    fn find_now(buf: &mut Buffer, panes: &[(PaneId, Rect)], bottom: u16) -> Vec<Piece> {
        let drawn = Drawn::of(buf, panes.to_vec());
        find(buf, &drawn, &HashSet::new(), bottom)
    }

    fn piece(id: u32, area: Rect, col: u16, row: u16) -> Piece {
        Piece {
            pane: Some(PANE),
            id,
            area,
            col,
            row,
        }
    }

    #[test]
    fn a_picture_s_cells_are_one_piece_and_are_blanked() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 10));
        buf.set_string(0, 0, "text", Style::new());
        draw(&mut buf, 0x01_02_03, (2, 1), 0, 3, 5);
        let whole = buf.area;
        let found = find_now(&mut buf, &[(PANE, whole)], 9);
        assert_eq!(found, vec![piece(0x01_02_03, Rect::new(2, 1, 5, 3), 0, 0)]);
        assert_eq!(buf[(2, 1)].symbol(), " ", "blanked");
        assert_eq!(buf[(2, 1)].fg, Color::Reset);
        assert_eq!(buf[(0, 0)].symbol(), "t", "the rest left alone");
    }

    #[test]
    fn a_box_over_a_picture_cuts_it_into_the_pieces_left_showing() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 10));
        draw(&mut buf, 42, (0, 0), 0, 6, 10);
        let drawn = Drawn::of(&buf, vec![(PANE, buf.area)]);
        // A box over its middle: rows 2 and 3, columns 3 to 6.
        buf.set_string(3, 2, "╭──╮", Style::new());
        buf.set_string(3, 3, "╰──╯", Style::new());
        let found = find(&mut buf, &drawn, &HashSet::new(), 9);
        assert_eq!(
            found,
            vec![
                piece(42, Rect::new(0, 0, 10, 2), 0, 0),
                piece(42, Rect::new(0, 2, 3, 2), 0, 2),
                piece(42, Rect::new(7, 2, 3, 2), 7, 2),
                piece(42, Rect::new(0, 4, 10, 2), 0, 4),
            ]
        );
    }

    #[test]
    fn a_picture_scrolled_half_off_starts_at_the_row_it_is_cut_at() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 10));
        // Rows 3 to 5 of a six-row picture at the top of a pane below row 2.
        draw(&mut buf, 7, (4, 2), 3, 3, 4);
        let pane = Rect::new(0, 2, 20, 6);
        let found = find_now(&mut buf, &[(PANE, pane)], 9);
        assert_eq!(found, vec![piece(7, Rect::new(4, 2, 4, 3), 0, 3)]);
        // Cut at the bottom by the frame's last row: blanked, not shown.
        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 10));
        draw(&mut buf, 7, (0, 7), 0, 3, 2);
        let whole = buf.area;
        let found = find_now(&mut buf, &[(PANE, whole)], 9);
        assert_eq!(found, vec![piece(7, Rect::new(0, 7, 2, 2), 0, 0)]);
        assert_eq!(buf[(0, 9)].symbol(), " ");
    }

    #[test]
    fn marks_left_out_come_from_the_cell_to_the_left_and_outside_a_pane_is_not_looked_at() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 10, 4));
        let fg = Style::new().fg(Color::Rgb(0, 0, 9));
        // Row 1 of the picture marked on the first cell alone, then a
        // third mark (the id's high byte) on a cell of another picture.
        let row1 = format!("{PLACEHOLDER}{}", char::from_u32(MARKS[1]).unwrap());
        buf[(0, 0)].set_symbol(&row1).set_style(fg);
        buf[(1, 0)]
            .set_symbol(&PLACEHOLDER.to_string())
            .set_style(fg);
        buf[(2, 0)]
            .set_symbol(&PLACEHOLDER.to_string())
            .set_style(fg);
        let high = format!(
            "{}{}",
            Picture::cell(0, 0),
            char::from_u32(MARKS[2]).unwrap()
        );
        buf[(5, 0)].set_symbol(&high).set_style(fg);
        buf[(6, 0)].set_symbol(&Picture::cell(0, 1)).set_style(fg);
        // A placeholder outside the panes looked in is no piece, and is
        // blanked all the same.
        buf[(0, 3)].set_symbol(&Picture::cell(0, 0)).set_style(fg);
        let pane = Rect::new(0, 0, 10, 3);
        let found = find_now(&mut buf, &[(PANE, pane)], 3);
        assert_eq!(
            found,
            vec![
                piece(9, Rect::new(0, 0, 3, 1), 0, 1),
                piece(0x0200_0009, Rect::new(5, 0, 2, 1), 0, 0),
            ]
        );
        assert_eq!(buf[(0, 3)].symbol(), " ");
    }

    #[test]
    fn a_copy_of_a_pane_s_cells_drawn_over_it_is_blanked_and_drawn_nowhere() {
        // A picture in the pane, as the pane drew it.
        let mut buf = Buffer::empty(Rect::new(0, 0, 30, 12));
        let pane = Rect::new(0, 2, 30, 10);
        draw(&mut buf, 42, (0, 2), 0, 4, 6);
        let drawn = Drawn::of(&buf, vec![(PANE, pane)]);
        // Mission control's card over all of it but its top row, copying
        // the pane's lines, the picture's cells among them, a row lower
        // and two columns in; and the same copied into the rows above the
        // pane, where UX 2.0's header is.
        for y in 3..12 {
            buf.set_string(0, y, " ".repeat(30), Style::new());
        }
        draw(&mut buf, 42, (2, 4), 0, 4, 6);
        draw(&mut buf, 42, (10, 0), 0, 2, 6);
        // One of the pane's own cells drawn over in another colour (a
        // copy of a cell of the picture at its own place, a dimmed box).
        buf[(5, 2)].set_style(Style::new().fg(Color::Rgb(0, 0, 43)));
        let found = find(&mut buf, &drawn, &HashSet::new(), 11);
        assert_eq!(
            found,
            vec![piece(42, Rect::new(0, 2, 5, 1), 0, 0)],
            "the pane's own cells left showing, alone"
        );
        for y in 0..12 {
            for x in 0..30 {
                assert!(!is_placeholder(&buf[(x, y)]), "{x}, {y} blanked");
            }
        }
    }

    #[test]
    fn a_pane_reaching_the_last_row_has_its_picture_cut_above_it() {
        // UX 2.0: no status line, the pane down to the frame's last row.
        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 10));
        let pane = Rect::new(0, 4, 20, 6);
        draw(&mut buf, 7, (3, 7), 0, 3, 4);
        let found = find_now(&mut buf, &[(PANE, pane)], 9);
        assert_eq!(found, vec![piece(7, Rect::new(3, 7, 4, 2), 0, 0)]);
        assert_eq!(buf[(3, 9)].symbol(), " ", "its last row blank");
    }

    #[test]
    fn the_server_s_images_are_read_anywhere_by_their_ids_and_cut_by_what_covers_them() {
        // A pane's picture, as it drew it, and two of the server's images
        // drawn over the frame after it: one over the pane, part of it
        // covered by a box, and one outside any pane.
        let mut buf = Buffer::empty(Rect::new(0, 0, 30, 12));
        let pane = Rect::new(0, 0, 30, 6);
        draw(&mut buf, 42, (0, 0), 0, 2, 4);
        let drawn = Drawn::of(&buf, vec![(PANE, pane)]);
        let (meter, switch) = (0x80_1234, 0x80_5678);
        draw(&mut buf, meter, (10, 2), 0, 1, 10);
        buf.set_string(14, 2, "╭╮", Style::new());
        draw(&mut buf, switch, (2, 8), 0, 1, 4);
        // A placeholder naming an image the frame doesn't have.
        draw(&mut buf, 0x80_0001, (20, 9), 0, 1, 2);
        let server: HashSet<u32> = [meter, switch].into_iter().collect();
        let found = find(&mut buf, &drawn, &server, 11);
        let theirs = |id, area, col| Piece {
            pane: None,
            id,
            area,
            col,
            row: 0,
        };
        assert_eq!(
            found,
            vec![
                piece(42, Rect::new(0, 0, 4, 2), 0, 0),
                theirs(meter, Rect::new(10, 2, 4, 1), 0),
                theirs(meter, Rect::new(16, 2, 4, 1), 6),
                theirs(switch, Rect::new(2, 8, 4, 1), 0),
            ]
        );
        for y in 0..12 {
            for x in 0..30 {
                assert!(!is_placeholder(&buf[(x, y)]), "{x}, {y} blanked");
            }
        }
        // A pane's cell still as it drew it is the pane's, whatever its id.
        let mut buf = Buffer::empty(Rect::new(0, 0, 10, 2));
        draw(&mut buf, meter, (0, 0), 0, 1, 3);
        let drawn = Drawn::of(&buf, vec![(PANE, buf.area)]);
        let found = find(&mut buf, &drawn, &server, 2);
        assert_eq!(found, vec![piece(meter, Rect::new(0, 0, 3, 1), 0, 0)]);
    }
}
