//! One list navigation, the same keys in every list: `j` `k`, the arrows and
//! C-n C-p a row, `h` `l` a column, `g` `G` Home End the ends; the list
//! decides what a step is (a row, a week, a section).

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

    /// How far a step moves a cursor `count` rows a step, Top and Bottom
    /// past either end for the list to clamp; None for left and right.
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

/// The row a key steps in a list whose search is typed into: the arrows
/// and C-n C-p alone, the letters, Home and End being the search's.
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
        let key = |c, mods| Key::new(KeyCode::Char(c), mods);
        assert_eq!(step(key('n', Mods::CTRL)), Some(Move::Down));
        assert_eq!(step(key('g', Mods::SHIFT)), Some(Move::Bottom));
        assert_eq!(step(key('j', Mods::ALT)), None);
        assert_eq!(typing_rows(key('j', Mods::NONE)), None);
        assert_eq!(typing_rows(key('p', Mods::CTRL)), Some(-1));
    }
}
