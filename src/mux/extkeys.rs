//! xterm's modifyOtherKeys as a pane's program sets it, read from its output
//! beside the emulator. tmux.conf's `extended-keys on` let a program in a
//! pane ask for keys the old forms can't tell apart (Ctrl-Shift-P from
//! Ctrl-P, Shift-Enter from Enter) and get them so, while a program that
//! asked for nothing got the old bytes; `input::encode_key` does the same
//! from the pane's modes. A program asks one of two ways: the kitty keyboard
//! protocol (`CSI > 1 u`), which alacritty_terminal keeps in its modes, or
//! modifyOtherKeys (`CSI > 4 ; <level> m`), which it reads and drops. So
//! that one is watched for here: `CSI > 4 ; 1 m` or `; 2 m` sets the level,
//! `CSI > 4 m`, `; 0 m` or a bare `CSI > m` sets it back, and so does a full
//! reset (`ESC c`), as in xterm. A sequence split between two reads is read
//! whole, and the scan skips to each ESC, so plain output costs a search.

const ESC: u8 = 0x1b;

/// The most parameter bytes a `CSI >` sequence is followed through; a
/// longer one is no modifyOtherKeys and is let go.
const MAX_PARAMS: usize = 16;

/// The level a pane's program has set, and where the scan is in its output.
#[derive(Debug, Default)]
pub struct ModifyOtherKeys {
    level: u8,
    state: State,
}

#[derive(Debug, Default)]
enum State {
    #[default]
    Ground,
    /// After an ESC.
    Esc,
    /// After `ESC [`.
    Csi,
    /// After `ESC [ >`, with the parameters so far.
    Params(Vec<u8>),
}

impl ModifyOtherKeys {
    /// The level set: 0 for none, 1 or 2.
    pub fn level(&self) -> u8 {
        self.level
    }

    /// Reads what the program wrote.
    pub fn feed(&mut self, bytes: &[u8]) {
        let mut rest = bytes;
        while let Some((&b, after)) = rest.split_first() {
            if let State::Ground = self.state {
                let Some(at) = rest.iter().position(|&b| b == ESC) else {
                    return;
                };
                rest = &rest[at + 1..];
                self.state = State::Esc;
                continue;
            }
            rest = after;
            self.state = match std::mem::take(&mut self.state) {
                State::Esc if b == b'[' => State::Csi,
                State::Esc if b == b'c' => {
                    self.level = 0;
                    State::Ground
                }
                State::Csi if b == b'>' => State::Params(Vec::new()),
                State::Params(mut params) if is_param(b) && params.len() < MAX_PARAMS => {
                    params.push(b);
                    State::Params(params)
                }
                State::Params(params) if b == b'm' => {
                    self.set(&params);
                    State::Ground
                }
                // An ESC breaks off what was begun and starts another.
                _ if b == ESC => State::Esc,
                _ => State::Ground,
            };
        }
    }

    /// `CSI > <params> m`: xterm's key modifier resources, of which the
    /// fourth is modifyOtherKeys; no parameters at all sets each back.
    fn set(&mut self, params: &[u8]) {
        if params.is_empty() {
            self.level = 0;
            return;
        }
        let mut fields = params.split(|&b| b == b';');
        if fields.next() != Some(b"4") {
            return;
        }
        self.level = match fields.next().unwrap_or_default() {
            b"" | b"0" => 0,
            b"1" => 1,
            b"2" => 2,
            _ => return,
        };
    }
}

fn is_param(b: u8) -> bool {
    b.is_ascii_digit() || b == b';' || b == b':'
}

#[cfg(test)]
mod tests {
    use super::*;

    fn level_after(chunks: &[&[u8]]) -> u8 {
        let mut keys = ModifyOtherKeys::default();
        for chunk in chunks {
            keys.feed(chunk);
        }
        keys.level()
    }

    #[test]
    fn the_level_is_what_the_program_set_last() {
        assert_eq!(level_after(&[b"hello"]), 0);
        assert_eq!(level_after(&[b"\x1b[>4;2m"]), 2);
        assert_eq!(level_after(&[b"text\x1b[>4;1mmore"]), 1);
        assert_eq!(level_after(&[b"\x1b[>4;2m", b"\x1b[>4;0m"]), 0);
        // Neovim's and Vim's ways of turning it off, and xterm's reset.
        assert_eq!(level_after(&[b"\x1b[>4;2m\x1b[>4m"]), 0);
        assert_eq!(level_after(&[b"\x1b[>4;2m\x1b[>4;m"]), 0);
        assert_eq!(level_after(&[b"\x1b[>4;2m\x1b[>m"]), 0);
        assert_eq!(level_after(&[b"\x1b[>4;2m\x1bc"]), 0);
    }

    #[test]
    fn a_sequence_split_between_reads_is_read_whole() {
        let whole = b"ab\x1b[>4;2mcd";
        for cut in 0..whole.len() {
            let (a, b) = whole.split_at(cut);
            assert_eq!(level_after(&[a, b]), 2, "cut at {cut}");
        }
    }

    #[test]
    fn other_sequences_are_not_it() {
        // Underline and faint; another resource; the kitty protocol's push.
        assert_eq!(level_after(&[b"\x1b[4;2m"]), 0);
        assert_eq!(level_after(&[b"\x1b[>1;2m"]), 0);
        assert_eq!(level_after(&[b"\x1b[>1u"]), 0);
        // A level xterm doesn't have changes nothing.
        assert_eq!(level_after(&[b"\x1b[>4;2m\x1b[>4;9m"]), 2);
        // Broken off by another ESC, which then begins a sequence of its own.
        assert_eq!(level_after(&[b"\x1b[>4\x1b[>4;1m"]), 1);
        assert_eq!(level_after(&[b"\x1b[>4;2\x18m"]), 0);
        // Too long to be one.
        assert_eq!(level_after(&[b"\x1b[>4;2;00000000000000000000m"]), 0);
    }
}
