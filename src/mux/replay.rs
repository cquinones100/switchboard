//! What a pane shows, and how its program has set its terminal up, as the
//! bytes that draw it again: fed to a new emulator of the same size, they
//! give the same screen and history cell for cell (each character with its
//! colours and attributes, wide characters, wrapped lines, links), the
//! alternate screen when a full-screen program has it up, the cursor where
//! it was and the pen it writes with, the modes the program set (the mouse,
//! bracketed paste, application keys, the kitty keyboard protocol's flags,
//! modifyOtherKeys, the cursor's shape), its title and the colours it set.
//!
//! This is how a server handing its panes to a new build
//! (src/mux/upgrade.rs) gives each the screen it had. The saved layout's
//! text is enough for a restart, which feeds it to a new shell; here the
//! program is the same one and goes on from the very cell it was at, so
//! Claude Code redrawing its prompt in place, or vim a line, lands where it
//! would have.
//!
//! What the emulator holds of a sequence or a character whose last bytes
//! haven't come yet (`unfinished`) is given after it, so the rest, which
//! the program writes once the new build reads, finishes it there.
//!
//! What isn't carried: a scroll region, tab stops, character sets, a
//! cursor the program saved, and the kitty keyboard flags of the screen
//! that isn't up. A full-screen program that leans on them draws itself
//! again once the pane is told its size after the handover.

use std::fmt::Write as _;

use alacritty_terminal::event::EventListener;
use alacritty_terminal::grid::{Dimensions, Grid};
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::cell::{Cell, Flags, Hyperlink};
use alacritty_terminal::term::{Term, TermMode};
use alacritty_terminal::vte::ansi::{Color, CursorShape, NamedColor, Rgb};

/// The attributes a cell's text is drawn with, as SGR sets them.
const ATTRIBUTES: Flags = Flags::BOLD
    .union(Flags::DIM)
    .union(Flags::ITALIC)
    .union(Flags::ALL_UNDERLINES)
    .union(Flags::INVERSE)
    .union(Flags::HIDDEN)
    .union(Flags::STRIKEOUT);

/// The bytes that draw `term` again, with the title the program set and the
/// modifyOtherKeys level it asked for (which the pane keeps beside the
/// emulator). `term` is left as it was: with the alternate screen up, the
/// screen beneath is read by swapping it in and back.
pub fn replay<L: EventListener>(term: &mut Term<L>, title: &str, other_keys: u8) -> String {
    let mut out = String::new();
    if term.mode().contains(TermMode::ALT_SCREEN) {
        let alternate = term.grid().clone();
        term.swap_alt();
        draw_grid(&mut out, term.grid(), true);
        place_cursor(&mut out, term.grid());
        term.swap_alt();
        *term.grid_mut() = alternate;
        out.push_str("\x1b[?1049h\x1b[H");
        draw_grid(&mut out, term.grid(), false);
    } else {
        draw_grid(&mut out, term.grid(), true);
    }
    place_cursor(&mut out, term.grid());
    modes(&mut out, term, other_keys);
    colours(&mut out, term);
    let title: String = title.chars().filter(|c| !c.is_control()).collect();
    if !title.is_empty() {
        let _ = write!(out, "\x1b]2;{title}\x1b\\");
    }
    let mut pen = Pen::default();
    pen.set(&mut out, &term.grid().cursor.template);
    out
}

/// The end of what a program wrote that is still unfinished: an escape
/// sequence whose final byte hasn't come (a CSI's, an OSC's BEL, a two-byte
/// escape's second), or a character whose last UTF-8 bytes haven't. The
/// emulator holds these, drawing nothing of them yet, so a new emulator
/// given the screen must be given them too, or it would take the rest for
/// text. `recent` is the last of what was written; a sequence that began
/// before it is lost.
pub fn unfinished(recent: &[u8]) -> &[u8] {
    let n = recent.len();
    // A character cut short: its lead byte, then fewer continuation bytes
    // than it says.
    let mut start = n;
    let mut at = n;
    while at > 0 && n - at < 3 && recent[at - 1] & 0xC0 == 0x80 {
        at -= 1;
    }
    if at > 0 {
        let lead = recent[at - 1];
        let wants = match lead {
            0xC0..=0xDF => 2,
            0xE0..=0xEF => 3,
            0xF0..=0xF7 => 4,
            _ => 1,
        };
        if wants > n - (at - 1) {
            start = at - 1;
        }
    }
    if let Some(esc) = recent.iter().rposition(|&b| b == 0x1b) {
        let rest = &recent[esc + 1..];
        let finished = match rest.first() {
            None => false,
            Some(b'[') => rest[1..].iter().any(|b| (0x40..=0x7e).contains(b)),
            // An OSC, DCS, APC or PM ends with BEL, or with an ST whose ESC
            // would be the last one.
            Some(b']' | b'P' | b'_' | b'^' | b'X') => rest[1..].contains(&0x07),
            Some(b) if (0x20..=0x2f).contains(b) => rest.len() > 1,
            Some(_) => true,
        };
        if !finished {
            start = start.min(esc);
        }
    }
    &recent[start..]
}

/// What text is written with, as last set.
#[derive(Default)]
struct Pen {
    sgr: Option<(Color, Color, Flags, Option<Color>)>,
    link: Option<Hyperlink>,
}

impl Pen {
    /// Sets the pen to `cell`'s, saying only what changed.
    fn set(&mut self, out: &mut String, cell: &Cell) {
        let sgr = (
            cell.fg,
            cell.bg,
            cell.flags & ATTRIBUTES,
            cell.underline_color(),
        );
        let plain = (
            Color::Named(NamedColor::Foreground),
            Color::Named(NamedColor::Background),
            Flags::empty(),
            None,
        );
        if self.sgr.unwrap_or(plain) != sgr {
            out.push_str("\x1b[0");
            let (fg, bg, flags, underline) = sgr;
            for (flag, code) in [
                (Flags::BOLD, "1"),
                (Flags::DIM, "2"),
                (Flags::ITALIC, "3"),
                (Flags::UNDERLINE, "4"),
                (Flags::DOUBLE_UNDERLINE, "4:2"),
                (Flags::UNDERCURL, "4:3"),
                (Flags::DOTTED_UNDERLINE, "4:4"),
                (Flags::DASHED_UNDERLINE, "4:5"),
                (Flags::INVERSE, "7"),
                (Flags::HIDDEN, "8"),
                (Flags::STRIKEOUT, "9"),
            ] {
                if flags.contains(flag) {
                    out.push(';');
                    out.push_str(code);
                }
            }
            colour(out, fg, 30, 90, 38);
            colour(out, bg, 40, 100, 48);
            if let Some(underline) = underline {
                colour(out, underline, 0, 0, 58);
            }
            out.push('m');
            self.sgr = Some(sgr);
        }
        let link = cell.hyperlink();
        if self.link != link {
            match &link {
                Some(link) => {
                    let _ = write!(out, "\x1b]8;id={};{}\x1b\\", link.id(), link.uri());
                }
                None => out.push_str("\x1b]8;;\x1b\\"),
            }
            self.link = link;
        }
    }

    /// Back to plain, before a line ends: a line scrolled in takes the
    /// pen's background, and a link mustn't run on to the next line.
    fn plain(&mut self, out: &mut String) {
        self.set(out, &Cell::default());
    }
}

/// An SGR colour: the sixteen named ones by their own codes (`base` for the
/// eight, `bright` for the bright eight), the palette's and any other by
/// `extended` (38, 48, 58); the default colours by nothing at all.
fn colour(out: &mut String, colour: Color, base: u8, bright: u8, extended: u8) {
    match colour {
        Color::Named(name) => {
            let n = name as usize;
            if n < 8 && base > 0 {
                let _ = write!(out, ";{}", base as usize + n);
            } else if n < 16 && bright > 0 {
                let _ = write!(out, ";{}", bright as usize + n - 8);
            } else if n < 16 {
                let _ = write!(out, ";{extended};5;{n}");
            }
        }
        Color::Indexed(i) => {
            let _ = write!(out, ";{extended};5;{i}");
        }
        Color::Spec(Rgb { r, g, b }) => {
            let _ = write!(out, ";{extended};2;{r};{g};{b}");
        }
    }
}

/// Whether a cell shows nothing: a blank on the default background, with
/// nothing under or through it, linking nowhere.
fn blank(cell: &Cell) -> bool {
    cell.c == ' '
        && cell.bg == Color::Named(NamedColor::Background)
        && !cell
            .flags
            .intersects(Flags::INVERSE | Flags::ALL_UNDERLINES | Flags::STRIKEOUT)
        && cell.zerowidth().is_none_or(<[char]>::is_empty)
        && cell.hyperlink().is_none()
}

/// A grid's lines, its history first when `history`, each written from its
/// first column: a wrapped line whole, so the next character wraps as it
/// did, and any other up to its last cell that shows something, then a new
/// line (which, at the bottom, scrolls the line before into the history),
/// but for the last.
fn draw_grid(out: &mut String, grid: &Grid<Cell>, history: bool) {
    let cols = grid.columns();
    let bottom = grid.screen_lines() as i32;
    let top = if history {
        -(grid.history_size() as i32)
    } else {
        0
    };
    let mut pen = Pen::default();
    for line in top..bottom {
        let row = &grid[Line(line)];
        let wrapped = cols > 0 && row[Column(cols - 1)].flags.contains(Flags::WRAPLINE);
        let end = if wrapped {
            cols
        } else {
            (0..cols)
                .rev()
                .find(|&c| !blank(&row[Column(c)]))
                .map_or(0, |c| c + 1)
        };
        for col in 0..end {
            let cell = &row[Column(col)];
            if cell
                .flags
                .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
            {
                continue;
            }
            pen.set(out, cell);
            if cell.c == '\t' && col + 1 < cols {
                // A tab keeps its character in the cell it was written at:
                // the cell is written blank with its colours, then the tab
                // over it, and the cursor put back after it.
                let _ = write!(out, " \x1b[D\t\x1b[{}G", col + 2);
                continue;
            }
            out.push(shown(cell.c));
            if let Some(marks) = cell.zerowidth() {
                out.extend(marks.iter().copied().filter(|c| !c.is_control()));
            }
        }
        if !wrapped && line + 1 < bottom {
            pen.plain(out);
            out.push_str("\r\n");
        }
    }
    pen.plain(out);
}

/// A cell's character as it can be written: a tab (which the emulator keeps
/// where a tab moved over) or anything else that would move the cursor
/// rather than fill the cell is a blank.
fn shown(c: char) -> char {
    if c.is_control() { ' ' } else { c }
}

/// The cursor where it is; one waiting to wrap at the end of a line is put
/// there by writing that line's last cell again.
fn place_cursor(out: &mut String, grid: &Grid<Cell>) {
    let cursor = &grid.cursor;
    let line = cursor.point.line.0.max(0) as usize + 1;
    let col = cursor.point.column.0 + 1;
    let _ = write!(out, "\x1b[{line};{col}H");
    if cursor.input_needs_wrap {
        let cell = &grid[cursor.point.line][cursor.point.column];
        if !cell.flags.intersects(
            Flags::WIDE_CHAR | Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER,
        ) {
            let mut pen = Pen::default();
            pen.set(out, cell);
            out.push(shown(cell.c));
            pen.plain(out);
        }
    }
}

/// The modes the program set that aren't the emulator's defaults.
fn modes<L: EventListener>(out: &mut String, term: &Term<L>, other_keys: u8) {
    let mode = *term.mode();
    let set = |out: &mut String, on: bool, code: &str| {
        if on {
            out.push_str(code);
        }
    };
    set(out, !mode.contains(TermMode::SHOW_CURSOR), "\x1b[?25l");
    set(out, mode.contains(TermMode::APP_CURSOR), "\x1b[?1h");
    set(out, mode.contains(TermMode::APP_KEYPAD), "\x1b=");
    set(
        out,
        mode.contains(TermMode::MOUSE_REPORT_CLICK),
        "\x1b[?1000h",
    );
    set(out, mode.contains(TermMode::MOUSE_DRAG), "\x1b[?1002h");
    set(out, mode.contains(TermMode::MOUSE_MOTION), "\x1b[?1003h");
    set(out, mode.contains(TermMode::UTF8_MOUSE), "\x1b[?1005h");
    set(out, mode.contains(TermMode::SGR_MOUSE), "\x1b[?1006h");
    set(out, mode.contains(TermMode::FOCUS_IN_OUT), "\x1b[?1004h");
    set(out, mode.contains(TermMode::BRACKETED_PASTE), "\x1b[?2004h");
    set(
        out,
        !mode.contains(TermMode::ALTERNATE_SCROLL),
        "\x1b[?1007l",
    );
    set(out, !mode.contains(TermMode::URGENCY_HINTS), "\x1b[?1042l");
    set(out, mode.contains(TermMode::INSERT), "\x1b[4h");
    set(out, mode.contains(TermMode::LINE_FEED_NEW_LINE), "\x1b[20h");
    // Last, since every line above was written with lines wrapping.
    set(out, !mode.contains(TermMode::LINE_WRAP), "\x1b[?7l");
    let kitty = super::pane::kitty_flags(mode);
    if kitty > 0 {
        let _ = write!(out, "\x1b[>{kitty}u");
    }
    if other_keys > 0 {
        let _ = write!(out, "\x1b[>4;{other_keys}m");
    }
    let style = term.cursor_style();
    let shape = match style.shape {
        CursorShape::Block | CursorShape::HollowBlock | CursorShape::Hidden => 1,
        CursorShape::Underline => 3,
        CursorShape::Beam => 5,
    };
    let code = shape + u8::from(!style.blinking);
    // A steady block is what a program that never set one has.
    if code != 2 {
        let _ = write!(out, "\x1b[{code} q");
    }
}

/// The colours the program set (OSC 4, 10, 11, 12).
fn colours<L: EventListener>(out: &mut String, term: &Term<L>) {
    let colours = term.colors();
    for i in 0..256 {
        if let Some(Rgb { r, g, b }) = colours[i] {
            let _ = write!(out, "\x1b]4;{i};rgb:{r:02x}/{g:02x}/{b:02x}\x1b\\");
        }
    }
    for (name, osc) in [
        (NamedColor::Foreground, 10),
        (NamedColor::Background, 11),
        (NamedColor::Cursor, 12),
    ] {
        if let Some(Rgb { r, g, b }) = colours[name] {
            let _ = write!(out, "\x1b]{osc};rgb:{r:02x}/{g:02x}/{b:02x}\x1b\\");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alacritty_terminal::event::VoidListener;
    use alacritty_terminal::term::Config;
    use alacritty_terminal::vte::ansi::Processor;

    struct Size(usize, usize);

    impl Dimensions for Size {
        fn total_lines(&self) -> usize {
            self.1
        }
        fn screen_lines(&self) -> usize {
            self.1
        }
        fn columns(&self) -> usize {
            self.0
        }
    }

    fn term(cols: usize, rows: usize) -> Term<VoidListener> {
        let config = Config {
            scrolling_history: 1000,
            kitty_keyboard: true,
            ..Config::default()
        };
        Term::new(config, &Size(cols, rows), VoidListener)
    }

    fn feed(term: &mut Term<VoidListener>, bytes: &[u8]) {
        Processor::<alacritty_terminal::vte::ansi::StdSyncHandler>::new().advance(term, bytes);
    }

    /// A cell as far as what is drawn: the character, its marks, colours,
    /// attributes, its place in a wide character or a wrapped line, its link.
    fn cell_of(cell: &Cell) -> String {
        format!(
            "{:?}{:?} {:?} {:?} {:?} {:?} {:?}",
            cell.c,
            cell.zerowidth(),
            cell.fg,
            cell.bg,
            cell.flags,
            cell.underline_color(),
            cell.hyperlink()
                .map(|l| (l.id().to_string(), l.uri().to_string())),
        )
    }

    fn grid_of(grid: &Grid<Cell>) -> Vec<Vec<String>> {
        let top = -(grid.history_size() as i32);
        (top..grid.screen_lines() as i32)
            .map(|l| {
                (0..grid.columns())
                    .map(|c| cell_of(&grid[Line(l)][Column(c)]))
                    .collect()
            })
            .collect()
    }

    /// Everything the replay carries, to compare two terminals by.
    fn state_of(term: &mut Term<VoidListener>) -> String {
        let mut state = format!(
            "{:?} {:?} {:?} {:?} {:?}\n",
            term.mode(),
            term.grid().cursor.point,
            term.grid().cursor.input_needs_wrap,
            cell_of(&term.grid().cursor.template),
            term.cursor_style(),
        );
        let colours = term.colors();
        state.push_str(&format!(
            "{:?} {:?}\n",
            colours[17],
            colours[NamedColor::Background]
        ));
        state.push_str(&format!("{:?}\n", grid_of(term.grid())));
        if term.mode().contains(TermMode::ALT_SCREEN) {
            let alternate = term.grid().clone();
            term.swap_alt();
            state.push_str(&format!(
                "beneath: {:?} {:?}\n",
                term.grid().cursor.point,
                grid_of(term.grid())
            ));
            term.swap_alt();
            *term.grid_mut() = alternate;
        }
        state
    }

    /// The first place two states differ, to say rather than both whole.
    fn same(a: &str, b: &str, what: &str) {
        if a == b {
            return;
        }
        let at = a
            .bytes()
            .zip(b.bytes())
            .position(|(x, y)| x != y)
            .unwrap_or(a.len().min(b.len()));
        let from = at.saturating_sub(300);
        let near = |s: &str| {
            s.get(from..(at + 300).min(s.len()))
                .unwrap_or("")
                .to_string()
        };
        panic!(
            "{what}: they differ at {at}:\n{}\n---\n{}",
            near(a),
            near(b)
        );
    }

    fn round_trip(term: &mut Term<VoidListener>, cols: usize, rows: usize) {
        let before = state_of(term);
        let bytes = replay(term, "a title", 0);
        same(&state_of(term), &before, "reading it leaves it as it was");
        let mut again = self::term(cols, rows);
        feed(&mut again, bytes.as_bytes());
        same(&state_of(&mut again), &before, "the replay");
    }

    /// Lines enough to scroll into the history, in every kind of text.
    fn busy_screen(term: &mut Term<VoidListener>) {
        for i in 0..30 {
            feed(term, format!("line {i}\r\n").as_bytes());
        }
        feed(
            term,
            "\x1b[1;31mbold red\x1b[0m plain \x1b[38;2;10;20;30mrgb\x1b[48;5;200m on 200\x1b[0m\r\n\
             \x1b[2;3;9mdim italic struck\x1b[0m \x1b[4:3;58;2;1;2;3mcurly\x1b[0m \x1b[7mreverse\x1b[0m\r\n\
             a line long enough to wrap round onto the next line of this narrow screen\r\n\
             wide: 日本語 and marks: e\u{301}\r\n\
             \x1b]8;;https://example.com\x1b\\a link\x1b]8;;\x1b\\ after it\r\n\
             \x1b[44m\x1b[K\x1b[0m\r\n\
             tab\there\r\n"
                .as_bytes(),
        );
        // A wide character that didn't fit at the end of a line.
        feed(
            term,
            "0123456789012345678901234567890123456789日\r\n".as_bytes(),
        );
    }

    #[test]
    fn a_screen_and_its_history_come_back_cell_for_cell() {
        let mut t = term(40, 10);
        busy_screen(&mut t);
        feed(
            &mut t,
            b"\x1b[3;5H\x1b[32;1m\x1b[?2004h\x1b[?1h\x1b=\x1b[?1002h\x1b[?1006h\x1b[?1004h\x1b[>1u\x1b[5 q\x1b]4;17;rgb:12/34/56\x1b\\\x1b]11;rgb:01/02/03\x1b\\",
        );
        assert!(t.grid().history_size() > 0);
        round_trip(&mut t, 40, 10);
    }

    #[test]
    fn a_full_screen_program_and_the_screen_beneath_come_back() {
        let mut t = term(40, 10);
        busy_screen(&mut t);
        feed(
            &mut t,
            b"\x1b[5;7H\x1b[?1049h\x1b[H\x1b[2J\x1b[2;3Hfull screen\x1b[7m reversed \x1b[0m\x1b[10;1Hstatus\x1b[?25l\x1b[?1003h",
        );
        round_trip(&mut t, 40, 10);
        // Leaving it again gives the screen beneath, its cursor put back.
        let bytes = replay(&mut t, "", 0);
        let mut again = term(40, 10);
        feed(&mut again, bytes.as_bytes());
        feed(&mut t, b"\x1b[?1049l");
        feed(&mut again, b"\x1b[?1049l");
        same(&state_of(&mut again), &state_of(&mut t), "after leaving it");
    }

    #[test]
    fn a_cursor_waiting_to_wrap_and_an_empty_screen() {
        let mut t = term(10, 4);
        round_trip(&mut t, 10, 4);
        feed(&mut t, b"\x1b[2;1H0123456789");
        assert!(t.grid().cursor.input_needs_wrap);
        round_trip(&mut t, 10, 4);
        // Lines that don't wrap, and the cursor's line kept as it was.
        feed(&mut t, b"\x1b[?7l\x1b[3;1Habc");
        round_trip(&mut t, 10, 4);
    }

    #[test]
    fn what_the_emulator_holds_unfinished_is_kept() {
        assert_eq!(unfinished(b"plain text"), b"");
        assert_eq!(unfinished(b"done\x1b[1m"), b"");
        assert_eq!(unfinished(b"text\x1b[38;5"), b"\x1b[38;5");
        assert_eq!(unfinished(b"text\x1b"), b"\x1b");
        assert_eq!(unfinished(b"\x1b]2;a ti"), b"\x1b]2;a ti");
        assert_eq!(unfinished(b"\x1b]2;title\x07after"), b"");
        assert_eq!(unfinished(b"\x1b]2;title\x1b\\"), b"");
        assert_eq!(unfinished(b"\x1b("), b"\x1b(");
        assert_eq!(unfinished(b"\x1b(B"), b"");
        let wide = "日".as_bytes();
        assert_eq!(unfinished(&[b"ab", &wide[..2]].concat()), &wide[..2]);
        assert_eq!(unfinished(&[b"ab", wide].concat()), b"");
        assert_eq!(unfinished(&wide[..1]), &wide[..1]);
        // A character cut short inside a sequence cut short: the sequence.
        let title = [b"x\x1b]2;", &wide[..2]].concat();
        assert_eq!(unfinished(&title), &title[1..]);
        // Given after the replay, the rest finishes it as it would have.
        let mut t = term(20, 3);
        feed(&mut t, b"\x1b[31");
        let mut again = term(20, 3);
        let mut bytes = replay(&mut t, "", 0).into_bytes();
        bytes.extend_from_slice(unfinished(b"\x1b[31"));
        feed(&mut again, &bytes);
        feed(&mut t, b"mred");
        feed(&mut again, b"mred");
        same(
            &state_of(&mut again),
            &state_of(&mut t),
            "a sequence finished after",
        );
    }

    #[test]
    fn modify_other_keys_and_the_title_are_said() {
        let mut t = term(10, 4);
        let bytes = replay(&mut t, "my\x07 title", 2);
        assert!(bytes.contains("\x1b[>4;2m"), "{bytes:?}");
        assert!(bytes.contains("\x1b]2;my title\x1b\\"), "{bytes:?}");
        let bytes = replay(&mut t, "", 0);
        assert!(!bytes.contains("\x1b]2;") && !bytes.contains("\x1b[>4;"));
    }
}
