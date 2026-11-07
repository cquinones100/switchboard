//! What one pane may pass through the multiplexer that another may not:
//! kitty's graphics commands out to your terminal, and easy mode's Cmd keys
//! in from it. tmux gave the notes app both, bin/notes setting its pane's
//! `allow-passthrough` for the pictures in a note, and tmux.conf binding
//! the Cmd keys to reach only a pane Notes marked `@notes_cmd_keys`.
//!
//! The pictures (src/notes/images.rs) are given to the terminal as kitty
//! graphics commands, `ESC _ G <keys> ; <data> ESC \`, each laid over cells
//! of the note that hold the placeholder character U+10EEEE, marked with
//! its row and column and coloured with the image's id. The emulator keeps
//! those cells as any text (the marks as the character's combining marks),
//! so they reach the terminal with the frame; the commands it drops, as a
//! terminal drops a string it doesn't know. So the server picks them out of
//! the output of the pane allowed to send them (`Scanner`), whole however
//! the reads split them, and sends each on to the clients showing that
//! pane's session. Only the notes app's pane (`@notes_pane`) or one marked
//! `@allow_passthrough` (as Notes marks its own, however it was started) is
//! allowed: a command can have the terminal read any file of yours, so
//! never any program's.
//!
//! Ghostty sends Cmd-S, Cmd-Z, Cmd-Shift-Z and Cmd-X, with the keybinds
//! docs/guide.md gives (the tmux version's Ghostty config set them), as
//! Ctrl+Alt+Shift keys (CSI u), which no binding takes:
//! they reach the pane only while it is marked `@notes_cmd_keys`, as they
//! came, and are dropped anywhere else, so no other program reads them as
//! Ctrl keys.

use std::collections::BTreeMap;

use super::TabId;
use super::input::Key;
use super::proto::ServerMsg;
use super::server::{ClientId, Server};

/// The longest command kept while it comes; a longer one is dropped whole.
/// kitty's own chunks are 4096 bytes of data.
const MAX: usize = 1 << 20;

const ESC: u8 = 0x1b;

/// The Cmd keys as Ghostty sends them, in tmux's notation (tmux.conf's
/// "Cmd keys for Notes' easy mode").
const CMD_KEYS: [&str; 4] = ["C-M-S-s", "C-M-S-z", "C-M-S-y", "C-M-S-x"];

/// Whether a pane's options let it send graphics commands to the terminal.
pub fn graphics_allowed(opts: &BTreeMap<String, String>) -> bool {
    ["@notes_pane", "@allow_passthrough"]
        .iter()
        .any(|name| opts.get(*name).is_some_and(|v| is_on(v)))
}

/// Whether a pane takes easy mode's Cmd keys.
pub fn takes_cmd_keys(opts: &BTreeMap<String, String>) -> bool {
    opts.get("@notes_cmd_keys").is_some_and(|v| is_on(v))
}

fn is_on(value: &str) -> bool {
    !matches!(value, "" | "0" | "off")
}

/// Whether a key is one of the Cmd keys.
pub fn is_cmd_key(key: &Key) -> bool {
    CMD_KEYS
        .iter()
        .any(|k| Key::parse(k).is_some_and(|k| k == *key))
}

/// Where a scan is in the output.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum State {
    #[default]
    Ground,
    /// After an ESC.
    Esc,
    /// After `ESC _`: an APC string, kitty's if `G` comes next.
    Apc,
    /// In a graphics command.
    Body,
    /// An ESC in one: its end, if `\` comes next.
    BodyEsc,
}

/// Picks kitty's graphics commands out of a program's output as it comes,
/// a command split between reads kept until its end comes.
#[derive(Debug, Default)]
pub struct Scanner {
    state: State,
    command: Vec<u8>,
    too_long: bool,
}

impl Scanner {
    /// The commands that ended in `bytes`, each whole, `ESC _ G` to `ESC \`.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<Vec<u8>> {
        let mut found = Vec::new();
        for &b in bytes {
            self.byte(b, &mut found);
        }
        found
    }

    fn byte(&mut self, b: u8, found: &mut Vec<Vec<u8>>) {
        match self.state {
            State::Ground => {
                if b == ESC {
                    self.state = State::Esc;
                }
            }
            State::Esc => {
                self.state = match b {
                    b'_' => State::Apc,
                    ESC => State::Esc,
                    _ => State::Ground,
                }
            }
            State::Apc => {
                if b == b'G' {
                    self.state = State::Body;
                    self.command.clear();
                    self.command.extend_from_slice(b"\x1b_G");
                    self.too_long = false;
                } else {
                    // Another kind of APC, which goes by as text would.
                    self.state = if b == ESC { State::Esc } else { State::Ground };
                }
            }
            State::Body => match b {
                ESC => self.state = State::BodyEsc,
                // CAN and SUB cut a string short, as a terminal reads them.
                0x18 | 0x1a => {
                    self.state = State::Ground;
                    self.command.clear();
                }
                _ => self.push(b),
            },
            State::BodyEsc => {
                if b == b'\\' {
                    self.push(ESC);
                    self.push(b'\\');
                    let command = std::mem::take(&mut self.command);
                    if !self.too_long {
                        found.push(command);
                    }
                    self.state = State::Ground;
                } else {
                    // An ESC that isn't the end ends it unfinished, and
                    // begins whatever comes next.
                    self.command.clear();
                    self.state = State::Esc;
                    self.byte(b, found);
                }
            }
        }
    }

    fn push(&mut self, b: u8) {
        if self.command.len() < MAX {
            self.command.push(b);
        } else {
            self.too_long = true;
        }
    }
}

impl Server {
    /// Sends graphics commands from a pane in `tab` to every client showing
    /// its session, so the picture is there whichever tab the client shows
    /// when its placeholders are drawn.
    pub(super) fn pass_graphics(&mut self, tab: TabId, commands: Vec<Vec<u8>>) {
        if commands.is_empty() {
            return;
        }
        let Some(session) = self.session_of_tab(tab).map(|s| s.name.clone()) else {
            return;
        };
        let bytes: Vec<u8> = commands.concat();
        for c in self.clients.values().filter(|c| c.session == session) {
            let _ = c.tx.send(ServerMsg::Output(bytes.clone()));
        }
    }

    /// A Cmd key pressed in a client: to its current pane, as it came, when
    /// that pane takes them, else nowhere.
    pub(super) fn cmd_key(&mut self, client: ClientId, raw: &[u8]) {
        let Some(session) = self.clients.get(&client).map(|c| c.session.clone()) else {
            return;
        };
        if self.sidebar.focused(&session) {
            return;
        }
        let Some(pane) = self.current_pane(client) else {
            return;
        };
        if let Some(slot) = self.panes.get_mut(&pane)
            && slot.copy.is_none()
            && takes_cmd_keys(&slot.opts)
        {
            let _ = slot.pane.write(raw);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn commands_are_picked_out_whole_however_the_reads_split_them() {
        let mut s = Scanner::default();
        let out = b"text\x1b[1mbold\x1b_Ga=T,q=2;QUFB\x1b\\more";
        assert_eq!(s.feed(out), vec![b"\x1b_Ga=T,q=2;QUFB\x1b\\".to_vec()]);
        // Split anywhere, even between ESC and its backslash.
        let whole = b"a\x1b_Gi=1;AA\x1b\\b\x1b_Gi=2;BB\x1b\\";
        for at in 0..whole.len() {
            let mut s = Scanner::default();
            let mut found = s.feed(&whole[..at]);
            found.extend(s.feed(&whole[at..]));
            assert_eq!(
                found,
                vec![
                    b"\x1b_Gi=1;AA\x1b\\".to_vec(),
                    b"\x1b_Gi=2;BB\x1b\\".to_vec()
                ],
                "split at {at}"
            );
        }
    }

    #[test]
    fn other_strings_and_unfinished_commands_are_not_passed() {
        let mut s = Scanner::default();
        // Another APC, an OSC, and a command an ESC cut short, after which
        // the next one is still found.
        let out = b"\x1b_Xnope\x1b\\\x1b]0;title\x07\x1b_Gcut\x1b[0m\x1b_Gok\x1b\\";
        assert_eq!(s.feed(out), vec![b"\x1b_Gok\x1b\\".to_vec()]);
        assert_eq!(s.feed(b"\x1b_Gcan\x18\x1b\\"), Vec::<Vec<u8>>::new());
        // One too long is dropped whole, and the scan carries on after it.
        let mut long = b"\x1b_G".to_vec();
        long.extend(std::iter::repeat_n(b'A', MAX + 10));
        long.extend_from_slice(b"\x1b\\\x1b_Gnext\x1b\\");
        assert_eq!(s.feed(&long), vec![b"\x1b_Gnext\x1b\\".to_vec()]);
    }

    #[test]
    fn only_the_notes_pane_or_one_marked_may_send_them() {
        assert!(graphics_allowed(&opts(&[("@notes_pane", "1")])));
        assert!(graphics_allowed(&opts(&[("@allow_passthrough", "on")])));
        assert!(!graphics_allowed(&opts(&[("@allow_passthrough", "0")])));
        assert!(!graphics_allowed(&opts(&[("@notes_cmd_keys", "1")])));
        assert!(!graphics_allowed(&BTreeMap::new()));
        assert!(takes_cmd_keys(&opts(&[("@notes_cmd_keys", "1")])));
        assert!(!takes_cmd_keys(&opts(&[("@notes_pane", "1")])));
    }

    #[test]
    fn the_cmd_keys_are_ghosttys_ctrl_alt_shift_ones() {
        let parse = |n: &str| Key::parse(n).unwrap();
        assert!(is_cmd_key(&parse("C-M-S-s")));
        assert!(is_cmd_key(&parse("C-M-S-y")));
        assert!(!is_cmd_key(&parse("C-M-s")));
        assert!(!is_cmd_key(&parse("C-M-S-a")));
        assert!(!is_cmd_key(&parse("M-s")));
    }
}
