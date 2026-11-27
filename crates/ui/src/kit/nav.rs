//! One list navigation: the keys that move a cursor through a list, the
//! same in every list, so `j` means one row down in mission control, a
//! menu, a box and the diff alike. `j` and Down (C-n), `k` and Up (C-p)
//! a row, `h` and Left, `l` and Right a column, `g` and Home to the top,
//! `G` and End to the bottom. A list decides what a step does there (a
//! row, a week, a section); a key `step` doesn't name is the list's own.

use crate::input::{Key, KeyCode, Mods};

/// Where a key moves a list's cursor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Move {
    Down,
    Up,
    Left,
    Right,
    Top,
    Bottom,
}

impl Move {
    /// The rows a step down or up moves by; None for the others.
    pub fn rows(self) -> Option<isize> {
        match self {
            Move::Down => Some(1),
            Move::Up => Some(-1),
            _ => None,
        }
    }

    /// How far a step up or down moves a cursor `count` rows a step, Top
    /// and Bottom past either end of any list, for the list to clamp;
    /// None for left and right.
    pub fn along(self, count: isize) -> Option<isize> {
        match self {
            Move::Top => Some(isize::MIN / 2),
            Move::Bottom => Some(isize::MAX / 2),
            _ => self.rows().map(|by| by * count),
        }
    }

    /// The columns a step left or right moves by; None for the others.
    pub fn cols(self) -> Option<isize> {
        match self {
            Move::Right => Some(1),
            Move::Left => Some(-1),
            _ => None,
        }
    }
}

/// The step `key` is in a list, if it is one.
pub fn step(key: Key) -> Option<Move> {
    let k = key.normalized();
    let plain = k.mods == Mods::NONE;
    let ctrl = k.mods == Mods::CTRL;
    Some(match k.code {
        KeyCode::Char('j') | KeyCode::Down if plain => Move::Down,
        KeyCode::Char('k') | KeyCode::Up if plain => Move::Up,
        KeyCode::Char('n') if ctrl => Move::Down,
        KeyCode::Char('p') if ctrl => Move::Up,
        KeyCode::Char('h') | KeyCode::Left if plain => Move::Left,
        KeyCode::Char('l') | KeyCode::Right if plain => Move::Right,
        KeyCode::Char('g') | KeyCode::Home if plain => Move::Top,
        KeyCode::Char('G') | KeyCode::End if plain => Move::Bottom,
        _ => return None,
    })
}

/// The row a key steps in a list whose search is being typed into: the
/// arrows and C-n C-p alone, the letters being the search's, and Home and
/// End its line's.
pub fn typing_rows(key: Key) -> Option<isize> {
    let typed = matches!(key.normalized().code, KeyCode::Char(c) if c != 'n' && c != 'p');
    let ends = matches!(key.code, KeyCode::Home | KeyCode::End);
    step(key).filter(|_| !typed && !ends).and_then(Move::rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_list_moves_on_the_same_keys() {
        let step_of = |code, mods| step(Key::new(code, mods));
        assert_eq!(step_of(KeyCode::Char('n'), Mods::CTRL), Some(Move::Down));
        assert_eq!(step_of(KeyCode::Up, Mods::NONE), Some(Move::Up));
        assert_eq!(step_of(KeyCode::Char('g'), Mods::SHIFT), Some(Move::Bottom));
        assert_eq!(step_of(KeyCode::Char('j'), Mods::ALT), None);
        assert_eq!(step_of(KeyCode::Char('J'), Mods::NONE), None);
        let typing = |code, mods| typing_rows(Key::new(code, mods));
        assert_eq!(typing(KeyCode::Char('j'), Mods::NONE), None);
        assert_eq!(typing(KeyCode::Char('n'), Mods::NONE), None);
        assert_eq!(typing(KeyCode::Char('p'), Mods::CTRL), Some(-1));
    }
}
