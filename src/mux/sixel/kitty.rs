//! What a pane's program has given the terminal through kitty's graphics
//! protocol, read from the commands `passthrough::Scanner` picks out of the
//! output of a pane allowed to send them: each image's id, the PNG file it
//! names, and the cells its virtual placement takes, so the server can
//! draw the picture itself as sixel where the program wrote its
//! placeholders (src/mux/sixel/mod.rs).
//!
//! What is read is what Notes and the image view send (and kitty's rules
//! for the rest of it): a transmission of a PNG by its path (`t=f`,
//! `f=100`) as `a=t`, or as `a=T` with a virtual placement (`U=1`, its
//! columns `c` and rows `r`), a later `a=p` placing one sent before, and a
//! deletion by id (`d=i` or `d=I`) or a range of ids (`d=r`, `d=R`), the
//! only ones kitty applies to a virtual placement. Anything else (the
//! image's data in the command itself, a placement at the cursor, a
//! query) is passed over: its placeholders draw nothing, as they wouldn't
//! in kitty without a virtual placement.

use std::collections::HashMap;
use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// The most images one pane keeps; past it, the one given longest ago goes.
const MOST: usize = 512;

/// One graphics command, as far as it is read here.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Command {
    /// `a`: `t` transmit, `T` transmit and place, `p` place, `d` delete.
    pub action: u8,
    pub id: Option<u32>,
    /// `t`: where the data is; `f` for a file.
    pub medium: u8,
    /// `f`: the data's format; 100 for PNG.
    pub format: u32,
    pub cols: Option<u16>,
    pub rows: Option<u16>,
    /// `U=1`: a virtual placement, shown where placeholders are.
    pub virtual_placement: bool,
    /// `d`: what a deletion takes.
    pub delete: u8,
    /// `m=1`: more of the data follows in another command.
    pub more: bool,
    /// `x` and `y`: the ids a deletion by range takes.
    pub x: Option<u32>,
    pub y: Option<u32>,
    pub payload: Vec<u8>,
}

/// A command, `ESC _ G <keys> ; <payload> ESC \`, or `None` for one that
/// isn't whole.
pub fn parse(command: &[u8]) -> Option<Command> {
    let body = command.strip_prefix(b"\x1b_G")?.strip_suffix(b"\x1b\\")?;
    let (keys, payload) = match body.iter().position(|b| *b == b';') {
        Some(at) => (&body[..at], &body[at + 1..]),
        None => (body, &[][..]),
    };
    let mut c = Command {
        action: b't',
        id: None,
        medium: b'd',
        format: 32,
        cols: None,
        rows: None,
        virtual_placement: false,
        delete: b'a',
        more: false,
        x: None,
        y: None,
        payload: payload.to_vec(),
    };
    for pair in keys.split(|b| *b == b',') {
        let Some(eq) = pair.iter().position(|b| *b == b'=') else {
            continue;
        };
        let (key, value) = (&pair[..eq], &pair[eq + 1..]);
        let number = || {
            std::str::from_utf8(value)
                .ok()
                .and_then(|v| v.parse::<u32>().ok())
        };
        let letter = || (value.len() == 1).then(|| value[0]);
        match key {
            b"a" => c.action = letter()?,
            b"i" => c.id = number(),
            b"t" => c.medium = letter()?,
            b"f" => c.format = number()?,
            b"c" => c.cols = number().map(|n| n.min(u16::MAX as u32) as u16),
            b"r" => c.rows = number().map(|n| n.min(u16::MAX as u32) as u16),
            b"U" => c.virtual_placement = number() == Some(1),
            b"d" => c.delete = letter()?,
            b"m" => c.more = number() == Some(1),
            b"x" => c.x = number(),
            b"y" => c.y = number(),
            _ => {}
        }
    }
    Some(c)
}

/// What the server knows of a picture's file once looked at: its time and
/// length, which name this version of it, or that it isn't a file.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum Look {
    #[default]
    Unknown,
    Missing,
    File {
        modified: u128,
        len: u64,
    },
}

/// One image a program gave the terminal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Image {
    pub path: PathBuf,
    /// Placed for its placeholders, in this many columns and rows (kitty's
    /// `c` and `r`, either of which may be left to the picture's shape).
    pub placed: bool,
    pub cols: Option<u16>,
    pub rows: Option<u16>,
    /// Which transmission this is, the server's count: a file looked at is
    /// for the one that asked.
    pub serial: u64,
    pub look: Look,
}

/// An image as a handover carries it to a new build (src/mux/upgrade.rs).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Handed {
    pub id: u32,
    pub path: String,
    #[serde(default)]
    pub placed: bool,
    #[serde(default)]
    pub cols: Option<u16>,
    #[serde(default)]
    pub rows: Option<u16>,
}

/// One pane's images, by id.
#[derive(Clone, Debug, Default)]
pub struct Images(HashMap<u32, Image>);

impl Images {
    pub fn get(&self, id: u32) -> Option<&Image> {
        self.0.get(&id)
    }

    pub fn get_mut(&mut self, id: u32) -> Option<&mut Image> {
        self.0.get_mut(&id)
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Takes a command in, `serial` numbering a transmission; gives whether
    /// anything shown may have changed.
    pub fn apply(&mut self, c: &Command, serial: u64) -> bool {
        match c.action {
            b't' | b'T' => {
                let Some(id) = c.id.filter(|id| *id != 0) else {
                    return false;
                };
                if c.medium != b'f' || c.format != 100 || c.more {
                    return false;
                }
                let Some(path) = crate::util::unbase64(&c.payload)
                    .map(|bytes| PathBuf::from(OsString::from_vec(bytes)))
                    .filter(|p| p.is_absolute())
                else {
                    return false;
                };
                let placed = c.action == b'T' && c.virtual_placement;
                self.0.insert(
                    id,
                    Image {
                        path,
                        placed,
                        cols: if placed { c.cols } else { None },
                        rows: if placed { c.rows } else { None },
                        serial,
                        look: Look::Unknown,
                    },
                );
                if self.0.len() > MOST
                    && let Some(oldest) =
                        self.0.iter().min_by_key(|(_, i)| i.serial).map(|(k, _)| *k)
                {
                    self.0.remove(&oldest);
                }
                true
            }
            b'p' if c.virtual_placement => match c.id.and_then(|id| self.0.get_mut(&id)) {
                Some(image) => {
                    image.placed = true;
                    image.cols = c.cols;
                    image.rows = c.rows;
                    true
                }
                None => false,
            },
            b'd' => match (c.delete, c.id, c.x, c.y) {
                (b'i' | b'I', Some(id), _, _) => self.0.remove(&id).is_some(),
                (b'r' | b'R', _, Some(from), Some(to)) => {
                    let before = self.0.len();
                    self.0.retain(|id, _| *id < from || *id > to);
                    self.0.len() != before
                }
                _ => false,
            },
            _ => false,
        }
    }

    /// For a handover: each image whose path can be written as text.
    pub fn hand_over(&self) -> Vec<Handed> {
        let mut out: Vec<(u64, Handed)> = self
            .0
            .iter()
            .filter_map(|(id, i)| {
                Some((
                    i.serial,
                    Handed {
                        id: *id,
                        path: i.path.to_str()?.to_string(),
                        placed: i.placed,
                        cols: i.cols,
                        rows: i.rows,
                    },
                ))
            })
            .collect();
        out.sort_by_key(|(serial, _)| *serial);
        out.into_iter().map(|(_, h)| h).collect()
    }

    /// The images a handover carried, numbered from `serial` on in the
    /// order they were given; gives the next number.
    pub fn take_up(&mut self, handed: Vec<Handed>, mut serial: u64) -> u64 {
        for h in handed {
            let path = PathBuf::from(h.path);
            if h.id == 0 || !path.is_absolute() {
                continue;
            }
            serial += 1;
            self.0.insert(
                h.id,
                Image {
                    path,
                    placed: h.placed,
                    cols: h.cols,
                    rows: h.rows,
                    serial,
                    look: Look::Unknown,
                },
            );
        }
        serial
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util;

    fn transmit(keys: &str, path: &str) -> Vec<u8> {
        format!("\x1b_G{keys};{}\x1b\\", util::base64(path.as_bytes())).into_bytes()
    }

    #[test]
    fn a_command_s_keys_and_path_are_read() {
        let c = parse(&transmit(
            "a=T,U=1,f=100,t=f,q=2,i=4242,c=30,r=7",
            "/tmp/n/a.png",
        ))
        .unwrap();
        assert_eq!(
            (c.action, c.id, c.medium, c.format, c.cols, c.rows),
            (b'T', Some(4242), b'f', 100, Some(30), Some(7))
        );
        assert!(c.virtual_placement && !c.more);
        assert_eq!(util::unbase64(&c.payload).unwrap(), b"/tmp/n/a.png");
        let d = parse(b"\x1b_Ga=d,d=I,i=9,q=2\x1b\\").unwrap();
        assert_eq!((d.action, d.delete, d.id), (b'd', b'I', Some(9)));
        assert!(d.payload.is_empty());
        // kitty's defaults: transmit, direct data, RGBA.
        let bare = parse(b"\x1b_Gi=1;AAAA\x1b\\").unwrap();
        assert_eq!((bare.action, bare.medium, bare.format), (b't', b'd', 32));
        assert_eq!(parse(b"\x1b_Ga=T"), None, "not whole");
    }

    #[test]
    fn a_transmission_by_path_is_kept_and_a_deletion_forgets_it() {
        let mut images = Images::default();
        let cmd = |bytes: Vec<u8>| parse(&bytes).unwrap();
        assert!(images.apply(
            &cmd(transmit("a=T,U=1,f=100,t=f,q=2,i=7,c=4,r=2", "/tmp/a.png")),
            1
        ));
        let a = images.get(7).unwrap();
        assert_eq!(
            (a.path.as_path(), a.placed, a.cols, a.rows, a.serial),
            (
                std::path::Path::new("/tmp/a.png"),
                true,
                Some(4),
                Some(2),
                1
            )
        );
        // Sent first, placed after.
        assert!(images.apply(&cmd(transmit("a=t,f=100,t=f,i=8", "/tmp/b.png")), 2));
        assert!(!images.get(8).unwrap().placed);
        assert!(images.apply(&parse(b"\x1b_Ga=p,U=1,i=8,c=3,r=1\x1b\\").unwrap(), 3));
        assert_eq!(images.get(8).unwrap().cols, Some(3));
        // What isn't a PNG by an absolute path is passed over.
        for skipped in [
            transmit("a=T,U=1,f=32,t=f,i=9", "/tmp/c.rgba"),
            transmit("a=T,U=1,f=100,t=d,i=9", "/tmp/c.png"),
            transmit("a=T,U=1,f=100,t=f,i=9", "relative.png"),
            transmit("a=T,U=1,f=100,t=f,i=0", "/tmp/c.png"),
            transmit("a=T,U=1,f=100,t=f,m=1,i=9", "/tmp/c.png"),
        ] {
            assert!(!images.apply(&cmd(skipped.clone()), 4), "{skipped:?}");
        }
        assert_eq!(images.len(), 2);
        // A deletion of all placements leaves virtual ones, as kitty does;
        // by id or by range forgets.
        assert!(!images.apply(&parse(b"\x1b_Ga=d,d=A\x1b\\").unwrap(), 5));
        assert!(images.apply(&parse(b"\x1b_Ga=d,d=I,i=7,q=2\x1b\\").unwrap(), 5));
        assert!(images.get(7).is_none());
        assert!(images.apply(&parse(b"\x1b_Ga=d,d=r,x=1,y=10\x1b\\").unwrap(), 6));
        assert!(images.is_empty());
    }

    #[test]
    fn images_go_across_a_handover_in_the_order_given() {
        let mut images = Images::default();
        for (id, serial) in [(3u32, 5u64), (1, 9)] {
            images.apply(
                &parse(&transmit(
                    &format!("a=T,U=1,f=100,t=f,i={id},c=2,r=1"),
                    &format!("/tmp/{id}.png"),
                ))
                .unwrap(),
                serial,
            );
        }
        let handed = images.hand_over();
        assert_eq!(handed.iter().map(|h| h.id).collect::<Vec<_>>(), vec![3, 1]);
        let json = serde_json::to_string(&handed).unwrap();
        let back: Vec<Handed> = serde_json::from_str(&json).unwrap();
        let mut again = Images::default();
        assert_eq!(again.take_up(back, 100), 102);
        let one = again.get(1).unwrap();
        assert_eq!(
            (
                one.path.to_str(),
                one.placed,
                one.cols,
                one.serial,
                &one.look
            ),
            (Some("/tmp/1.png"), true, Some(2), 102, &Look::Unknown)
        );
        // An older build's handover, without what came later, reads too.
        let old: Handed = serde_json::from_str(r#"{"id":5,"path":"/tmp/5.png"}"#).unwrap();
        assert!(!old.placed && old.cols.is_none());
    }
}
