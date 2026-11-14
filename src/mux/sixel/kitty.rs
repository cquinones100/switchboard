//! What a pane's program has given the terminal through kitty's graphics
//! protocol, read from the commands `passthrough::Scanner` picks out of the
//! output of a pane allowed to send them: each image's id, the PNG file it
//! names, and the cells its virtual placement takes, so the server can
//! draw the picture itself as sixel where the program wrote its
//! placeholders (src/mux/sixel/mod.rs).
//!
//! What is read is what Notes, the image view and the Markdown views send
//! (and kitty's rules for the rest of it): a transmission of a PNG
//! (`f=100`) by its path (`t=f`, Notes and the image view) or with its
//! data in the command itself (`t=d`, the PR, ticket, link and file views,
//! through `graphics::ImageCache`), the data split over commands of 4096
//! bytes, each after the first carrying only `m` (more to come) and `q`,
//! as `a=t`, or as `a=T` with a virtual placement (`U=1`, its columns `c`
//! and rows `r`), a later `a=p` placing one sent before, and a deletion by
//! id (`d=i` or `d=I`) or a range of ids (`d=r`, `d=R`), the only ones
//! kitty applies to a virtual placement. Anything else (a placement at the
//! cursor, a query, a format other than PNG) is passed over: its
//! placeholders draw nothing, as they wouldn't in kitty without a virtual
//! placement. A picture's data is taken in only while sixel is chosen
//! (with kitty's protocol the commands go to the terminal and the server
//! draws nothing from them), and kept until its pane's program deletes it
//! or the pane goes, sixel turned off and on again included, since a view
//! gives each picture once; it is kept as it was sent, base64, so taking
//! it in is no more than keeping its parts, and decoded off the server's
//! task when the picture is drawn (src/mux/sixel/mod.rs). No more than `MOST_DATA` of a
//! picture is kept, and no more than `MOST_HELD` of a pane's together, the
//! one given longest ago going first.

use std::collections::HashMap;
use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt;
use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

/// The most images one pane keeps; past it, the one given longest ago goes.
const MOST: usize = 512;
/// The largest PNG given inline, as sent (base64); a larger one is passed
/// over.
pub const MOST_DATA: usize = 32 << 20;
/// The most a pane's PNGs given inline may take together, as sent; past
/// it, the one given longest ago goes (a megabyte in the unit tests, which
/// fill it).
const MOST_HELD: usize = if cfg!(test) { 1 << 20 } else { 128 << 20 };

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
    /// A later part of a transmission's data: its keys are only `m` and
    /// `q`, as kitty sends each part after the first.
    pub chunk: bool,
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
        chunk: false,
    };
    let (mut more_key, mut others) = (false, false);
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
            b"m" => more_key = true,
            b"q" => {}
            _ => others = true,
        }
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
    c.chunk = more_key && !others;
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

/// What a picture was given as: a PNG file by its path, or a PNG's data,
/// base64, as it was sent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Given {
    Path(PathBuf),
    Data(Arc<[u8]>),
}

impl Given {
    /// The bytes it holds in memory.
    fn held(&self) -> usize {
        match self {
            Given::Path(_) => 0,
            Given::Data(data) => data.len(),
        }
    }
}

/// One image a program gave the terminal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Image {
    pub given: Given,
    /// Placed for its placeholders, in this many columns and rows (kitty's
    /// `c` and `r`, either of which may be left to the picture's shape).
    pub placed: bool,
    pub cols: Option<u16>,
    pub rows: Option<u16>,
    /// Which transmission this is, the server's count: a file looked at is
    /// for the one that asked.
    pub serial: u64,
    /// What its file was when looked at; a PNG given inline needs no look.
    pub look: Look,
}

/// An image as a handover carries it to a new build (src/mux/upgrade.rs):
/// its file's path, or, for one given inline, its PNG base64, as it was
/// sent (`data`), the path then empty.
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<String>,
}

/// A transmission whose data is still coming: its first command, and its
/// data so far, base64.
#[derive(Clone, Debug)]
struct Pending {
    first: Command,
    data: Vec<u8>,
}

/// One pane's images, by id, and a transmission still coming.
#[derive(Clone, Debug, Default)]
pub struct Images {
    images: HashMap<u32, Image>,
    pending: Option<Pending>,
}

impl Images {
    pub fn get(&self, id: u32) -> Option<&Image> {
        self.images.get(&id)
    }

    pub fn get_mut(&mut self, id: u32) -> Option<&mut Image> {
        self.images.get_mut(&id)
    }

    pub fn len(&self) -> usize {
        self.images.len()
    }

    /// No image, and none coming.
    pub fn is_empty(&self) -> bool {
        self.images.is_empty() && self.pending.is_none()
    }

    /// Takes a command in, `serial` numbering a transmission, a PNG's data
    /// given inline kept only when `inline` (sixel chosen); gives whether
    /// anything shown may have changed.
    pub fn apply(&mut self, c: &Command, serial: u64, inline: bool) -> bool {
        if c.chunk {
            let Some(p) = &mut self.pending else {
                return false;
            };
            if p.data.len() + c.payload.len() > MOST_DATA {
                self.pending = None;
                return false;
            }
            p.data.extend_from_slice(&c.payload);
            if c.more {
                return false;
            }
            let Some(p) = self.pending.take() else {
                return false;
            };
            return self.transmit(&p.first, &p.data, serial);
        }
        // Any other command ends a transmission still coming, as kitty
        // takes one's parts only one after another.
        self.pending = None;
        match c.action {
            b't' | b'T' if c.more => {
                if inline && c.medium == b'd' && c.format == 100 && c.id.is_some_and(|id| id != 0) {
                    self.pending = Some(Pending {
                        first: Command {
                            payload: Vec::new(),
                            ..c.clone()
                        },
                        data: c.payload.clone(),
                    });
                }
                false
            }
            b't' | b'T' if c.medium == b'd' && !inline => false,
            b't' | b'T' => self.transmit(c, &c.payload, serial),
            b'p' if c.virtual_placement => match c.id.and_then(|id| self.images.get_mut(&id)) {
                Some(image) => {
                    image.placed = true;
                    image.cols = c.cols;
                    image.rows = c.rows;
                    true
                }
                None => false,
            },
            b'd' => match (c.delete, c.id, c.x, c.y) {
                (b'i' | b'I', Some(id), _, _) => self.images.remove(&id).is_some(),
                (b'r' | b'R', _, Some(from), Some(to)) => {
                    let before = self.images.len();
                    self.images.retain(|id, _| *id < from || *id > to);
                    self.images.len() != before
                }
                _ => false,
            },
            _ => false,
        }
    }

    /// A transmission whole, its keys `c` and its data `payload`: kept,
    /// when it is a PNG by an absolute path or its data.
    fn transmit(&mut self, c: &Command, payload: &[u8], serial: u64) -> bool {
        let Some(id) = c.id.filter(|id| *id != 0) else {
            return false;
        };
        if c.format != 100 {
            return false;
        }
        let given = match c.medium {
            b'f' => match crate::util::unbase64(payload)
                .map(|bytes| PathBuf::from(OsString::from_vec(bytes)))
                .filter(|p| p.is_absolute())
            {
                Some(path) => Given::Path(path),
                None => return false,
            },
            b'd' if !payload.is_empty() && payload.len() <= MOST_DATA => {
                Given::Data(payload.into())
            }
            _ => return false,
        };
        let placed = c.action == b'T' && c.virtual_placement;
        self.keep(
            id,
            Image {
                given,
                placed,
                cols: if placed { c.cols } else { None },
                rows: if placed { c.rows } else { None },
                serial,
                look: Look::Unknown,
            },
        );
        true
    }

    /// An image kept under `id`, the ones given longest ago going while
    /// there are more than `MOST`, or their data more than `MOST_HELD`.
    fn keep(&mut self, id: u32, image: Image) {
        self.images.insert(id, image);
        loop {
            let held: usize = self.images.values().map(|i| i.given.held()).sum();
            if self.images.len() <= MOST && held <= MOST_HELD {
                break;
            }
            let oldest = self
                .images
                .iter()
                .filter(|(_, i)| self.images.len() > MOST || i.given.held() > 0)
                .min_by_key(|(_, i)| i.serial)
                .map(|(k, _)| *k);
            match oldest {
                Some(oldest) => {
                    self.images.remove(&oldest);
                }
                None => break,
            }
        }
    }

    /// The pictures given inline: each one's id, transmission and size, as
    /// sent, for a handover to choose which it carries.
    pub fn inline(&self) -> impl Iterator<Item = (u32, u64, usize)> + '_ {
        self.images.iter().filter_map(|(id, i)| match &i.given {
            Given::Data(data) => Some((*id, i.serial, data.len())),
            Given::Path(_) => None,
        })
    }

    /// For a handover: each image whose path can be written as text, and
    /// each given inline that `carried` takes (by its id), in the order
    /// they were given.
    pub fn hand_over(&self, carried: impl Fn(u32) -> bool) -> Vec<Handed> {
        let mut out: Vec<(u64, Handed)> = Vec::new();
        for (id, i) in &self.images {
            let (path, data) = match &i.given {
                Given::Path(path) => match path.to_str() {
                    Some(path) => (path.to_string(), None),
                    None => continue,
                },
                Given::Data(data) => match std::str::from_utf8(data) {
                    Ok(data) if carried(*id) => (String::new(), Some(data.to_string())),
                    _ => continue,
                },
            };
            out.push((
                i.serial,
                Handed {
                    id: *id,
                    path,
                    placed: i.placed,
                    cols: i.cols,
                    rows: i.rows,
                    data,
                },
            ));
        }
        out.sort_by_key(|(serial, _)| *serial);
        out.into_iter().map(|(_, h)| h).collect()
    }

    /// The images a handover carried, numbered from `serial` on in the
    /// order they were given; gives the next number.
    pub fn take_up(&mut self, handed: Vec<Handed>, mut serial: u64) -> u64 {
        for h in handed {
            let given = match &h.data {
                Some(data) if !data.is_empty() && data.len() <= MOST_DATA => {
                    Given::Data(data.as_bytes().into())
                }
                Some(_) => continue,
                None => {
                    let path = PathBuf::from(&h.path);
                    if !path.is_absolute() {
                        continue;
                    }
                    Given::Path(path)
                }
            };
            if h.id == 0 {
                continue;
            }
            serial += 1;
            self.keep(
                h.id,
                Image {
                    given,
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

    impl Images {
        /// `apply` with sixel chosen.
        fn apply_in(&mut self, c: &Command, serial: u64) -> bool {
            self.apply(c, serial, true)
        }
    }

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
        assert!(images.apply_in(
            &cmd(transmit("a=T,U=1,f=100,t=f,q=2,i=7,c=4,r=2", "/tmp/a.png")),
            1
        ));
        let a = images.get(7).unwrap();
        assert_eq!(
            (&a.given, a.placed, a.cols, a.rows, a.serial),
            (
                &Given::Path(PathBuf::from("/tmp/a.png")),
                true,
                Some(4),
                Some(2),
                1
            )
        );
        // Sent first, placed after.
        assert!(images.apply_in(&cmd(transmit("a=t,f=100,t=f,i=8", "/tmp/b.png")), 2));
        assert!(!images.get(8).unwrap().placed);
        assert!(images.apply_in(&parse(b"\x1b_Ga=p,U=1,i=8,c=3,r=1\x1b\\").unwrap(), 3));
        assert_eq!(images.get(8).unwrap().cols, Some(3));
        // What isn't a PNG by an absolute path or its data is passed over.
        for skipped in [
            transmit("a=T,U=1,f=32,t=f,i=9", "/tmp/c.rgba"),
            transmit("a=T,U=1,f=32,t=d,i=9", "rgba"),
            transmit("a=T,U=1,f=100,t=s,i=9", "/shm-name"),
            transmit("a=T,U=1,f=100,t=f,i=9", "relative.png"),
            transmit("a=T,U=1,f=100,t=f,i=0", "/tmp/c.png"),
            transmit("a=T,U=1,f=100,t=f,m=1,i=9", "/tmp/c.png"),
        ] {
            assert!(!images.apply_in(&cmd(skipped.clone()), 4), "{skipped:?}");
        }
        assert_eq!(images.len(), 2);
        // A deletion of all placements leaves virtual ones, as kitty does;
        // by id or by range forgets.
        assert!(!images.apply_in(&parse(b"\x1b_Ga=d,d=A\x1b\\").unwrap(), 5));
        assert!(images.apply_in(&parse(b"\x1b_Ga=d,d=I,i=7,q=2\x1b\\").unwrap(), 5));
        assert!(images.get(7).is_none());
        assert!(images.apply_in(&parse(b"\x1b_Ga=d,d=r,x=1,y=10\x1b\\").unwrap(), 6));
        assert!(images.is_empty());
    }

    #[test]
    fn images_go_across_a_handover_in_the_order_given() {
        let mut images = Images::default();
        for (id, serial) in [(3u32, 5u64), (1, 9)] {
            images.apply_in(
                &parse(&transmit(
                    &format!("a=T,U=1,f=100,t=f,i={id},c=2,r=1"),
                    &format!("/tmp/{id}.png"),
                ))
                .unwrap(),
                serial,
            );
        }
        let handed = images.hand_over(|_| true);
        assert_eq!(handed.iter().map(|h| h.id).collect::<Vec<_>>(), vec![3, 1]);
        let json = serde_json::to_string(&handed).unwrap();
        let back: Vec<Handed> = serde_json::from_str(&json).unwrap();
        let mut again = Images::default();
        assert_eq!(again.take_up(back, 100), 102);
        let one = again.get(1).unwrap();
        assert_eq!(
            (&one.given, one.placed, one.cols, one.serial, &one.look),
            (
                &Given::Path(PathBuf::from("/tmp/1.png")),
                true,
                Some(2),
                102,
                &Look::Unknown
            )
        );
        // An older build's handover, without what came later, reads too.
        let old: Handed = serde_json::from_str(r#"{"id":5,"path":"/tmp/5.png"}"#).unwrap();
        assert!(!old.placed && old.cols.is_none() && old.data.is_none());
    }

    /// A PNG's data given inline as a view's `ImageCache` gives it: its
    /// keys and the first `chunk` bytes of its base64, then the rest in
    /// parts carrying only `m`.
    fn inline(keys: &str, data: &[u8], chunk: usize) -> Vec<Vec<u8>> {
        let encoded = util::base64(data);
        let parts: Vec<&[u8]> = encoded.as_bytes().chunks(chunk).collect();
        parts
            .iter()
            .enumerate()
            .map(|(i, part)| {
                let more = u8::from(i + 1 < parts.len());
                let keys = if i == 0 {
                    format!("{keys},m={more}")
                } else {
                    format!("m={more}")
                };
                let mut c = format!("\x1b_G{keys};").into_bytes();
                c.extend_from_slice(part);
                c.extend_from_slice(b"\x1b\\");
                c
            })
            .collect()
    }

    #[test]
    fn a_picture_given_inline_in_parts_is_kept_once_its_last_part_comes() {
        let png: Vec<u8> = (0..=255u8).cycle().take(3000).collect();
        let mut images = Images::default();
        let parts = inline("a=T,U=1,f=100,t=d,q=2,i=12,c=6,r=3", &png, 1000);
        assert_eq!(parts.len(), 4);
        for (n, part) in parts.iter().enumerate() {
            let c = parse(part).unwrap();
            assert_eq!(c.chunk, n > 0, "part {n}");
            let changed = images.apply_in(&c, 7);
            assert_eq!(changed, n + 1 == parts.len(), "part {n}");
        }
        let one = images.get(12).unwrap();
        assert_eq!((one.placed, one.cols, one.rows), (true, Some(6), Some(3)));
        match &one.given {
            Given::Data(data) => assert_eq!(util::unbase64(data).unwrap(), png),
            other => panic!("{other:?}"),
        }
        // With kitty's protocol nothing is kept of it, nor of its parts.
        let mut kitty = Images::default();
        for part in &parts {
            assert!(!kitty.apply(&parse(part).unwrap(), 7, false));
        }
        assert!(kitty.is_empty());
        // Sixel turned off after: what was given before is kept, for when
        // it is chosen again, since a view gives each picture once.
        assert!(!images.apply(&parse(&parts[0]).unwrap(), 8, false));
        assert!(images.get(12).is_some());
        // A part with no transmission under way is passed over, and any
        // other command ends one still coming.
        let mut lone = Images::default();
        assert!(!lone.apply_in(&parse(&parts[1]).unwrap(), 1));
        let mut cut = Images::default();
        cut.apply_in(&parse(&parts[0]).unwrap(), 1);
        assert!(!cut.is_empty(), "one coming");
        cut.apply_in(&parse(b"\x1b_Ga=d,d=I,i=99\x1b\\").unwrap(), 2);
        for part in &parts[1..] {
            cut.apply_in(&parse(part).unwrap(), 3);
        }
        assert!(cut.get(12).is_none() && cut.is_empty());
        // Given whole in one command, as a small picture is.
        let mut whole = Images::default();
        for part in inline("a=T,U=1,f=100,t=d,q=2,i=13,c=1,r=1", b"tiny", 4096) {
            assert!(whole.apply_in(&parse(&part).unwrap(), 1));
        }
        assert!(whole.get(13).is_some());
    }

    #[test]
    fn a_pane_s_inline_pictures_are_held_to_their_size_and_go_across_a_handover() {
        let mut images = Images::default();
        // Four pictures of a third of what a pane may hold, as sent: the
        // first goes.
        let big = vec![7u8; MOST_HELD / 3 / 4 * 3];
        for id in 1..=4u32 {
            let keys = format!("a=T,U=1,f=100,t=d,q=2,i={id},c=2,r=1");
            for part in inline(&keys, &big, 1 << 20) {
                images.apply_in(&parse(&part).unwrap(), id as u64);
            }
        }
        assert!(images.get(1).is_none(), "the one given longest ago");
        assert!((2..=4).all(|id| images.get(id).is_some()));
        // A picture by its path is kept beside them, taking nothing.
        images.apply_in(
            &parse(&transmit("a=T,U=1,f=100,t=f,i=5,c=1,r=1", "/tmp/5.png")).unwrap(),
            5,
        );
        assert_eq!(images.len(), 4);
        // A handover carries those it is told to, and a path whatever.
        let mut inline: Vec<(u32, u64, usize)> = images.inline().collect();
        inline.sort();
        assert_eq!(
            inline.iter().map(|i| i.0).collect::<Vec<_>>(),
            vec![2, 3, 4]
        );
        let handed = images.hand_over(|id| id != 2);
        assert_eq!(
            handed.iter().map(|h| h.id).collect::<Vec<_>>(),
            vec![3, 4, 5],
            "in the order given"
        );
        let json = serde_json::to_string(&handed).unwrap();
        let mut again = Images::default();
        again.take_up(serde_json::from_str(&json).unwrap(), 0);
        match &again.get(4).unwrap().given {
            Given::Data(data) => assert_eq!(util::unbase64(data).unwrap(), big),
            other => panic!("{other:?}"),
        }
        assert!(matches!(again.get(5).unwrap().given, Given::Path(_)));
    }
}
