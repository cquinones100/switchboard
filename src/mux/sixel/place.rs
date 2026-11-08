//! Where a frame shows pictures: the cells holding kitty's placeholder
//! (U+10EEEE) after everything is drawn, the panes' text and whatever
//! boxes are over it, read as kitty reads them, and gathered into
//! rectangles of one picture each, so the server can draw each as sixel
//! (src/mux/sixel/mod.rs).
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
//! picture scrolled half off is a piece starting at the row it is cut at.
//!
//! Every placeholder cell is blanked in the frame (a space on its own
//! background), since a terminal without kitty's pictures draws the
//! character as a box. No piece is ever on the frame's last row: a sixel
//! reaching it would scroll the screen.

use ratatui::buffer::{Buffer, Cell};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier};

use crate::mux::PaneId;
use crate::notes::images::{MARKS, PLACEHOLDER};

/// A part of a picture a frame shows: which pane's image, where on the
/// screen, and the picture's column and row of its top left cell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Piece {
    pub pane: PaneId,
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

/// The pieces of pictures in each pane's area of `buf` (`panes`), every
/// placeholder there blanked; none at or below row `bottom`.
pub fn find(buf: &mut Buffer, panes: &[(PaneId, Rect)], bottom: u16) -> Vec<Piece> {
    let mut done: Vec<Piece> = Vec::new();
    for (pane, area) in panes {
        let area = area.intersection(buf.area);
        // The pieces that reached the row above, which a run may go on.
        let mut open: Vec<Piece> = Vec::new();
        for y in area.top()..area.bottom() {
            let mut next: Vec<Piece> = Vec::new();
            let mut left: Option<Mark> = None;
            let mut run: Option<Piece> = None;
            for x in area.left()..area.right() {
                let mark = match read(&buf[(x, y)], left) {
                    None => {
                        left = None;
                        end_run(&mut run, &mut open, &mut next);
                        continue;
                    }
                    Some(mark) => mark,
                };
                blank(&mut buf[(x, y)]);
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
                        pane: *pane,
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
    }
    done.sort_by_key(|p| (p.area.y, p.area.x));
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

    fn piece(id: u32, area: Rect, col: u16, row: u16) -> Piece {
        Piece {
            pane: PANE,
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
        let found = find(&mut buf, &[(PANE, whole)], 9);
        assert_eq!(found, vec![piece(0x01_02_03, Rect::new(2, 1, 5, 3), 0, 0)]);
        assert_eq!(buf[(2, 1)].symbol(), " ", "blanked");
        assert_eq!(buf[(2, 1)].fg, Color::Reset);
        assert_eq!(buf[(0, 0)].symbol(), "t", "the rest left alone");
    }

    #[test]
    fn a_box_over_a_picture_cuts_it_into_the_pieces_left_showing() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 10));
        draw(&mut buf, 42, (0, 0), 0, 6, 10);
        // A box over its middle: rows 2 and 3, columns 3 to 6.
        buf.set_string(3, 2, "╭──╮", Style::new());
        buf.set_string(3, 3, "╰──╯", Style::new());
        let whole = buf.area;
        let found = find(&mut buf, &[(PANE, whole)], 9);
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
        let found = find(&mut buf, &[(PANE, pane)], 9);
        assert_eq!(found, vec![piece(7, Rect::new(4, 2, 4, 3), 0, 3)]);
        // Cut at the bottom by the frame's last row: blanked, not shown.
        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 10));
        draw(&mut buf, 7, (0, 7), 0, 3, 2);
        let whole = buf.area;
        let found = find(&mut buf, &[(PANE, whole)], 9);
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
        // A placeholder outside the panes looked in stays as it is.
        buf[(0, 3)].set_symbol(&Picture::cell(0, 0)).set_style(fg);
        let pane = Rect::new(0, 0, 10, 3);
        let found = find(&mut buf, &[(PANE, pane)], 3);
        assert_eq!(
            found,
            vec![
                piece(9, Rect::new(0, 0, 3, 1), 0, 1),
                piece(0x0200_0009, Rect::new(5, 0, 2, 1), 0, 0),
            ]
        );
        assert!(buf[(0, 3)].symbol().starts_with(PLACEHOLDER));
    }
}
