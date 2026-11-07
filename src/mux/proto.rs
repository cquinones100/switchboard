//! The messages between a client (an attached terminal, a command, a hook)
//! and the server, over the server's Unix socket. Each is a frame: a tag
//! byte, a big-endian u32 length, then the payload, which is the raw bytes
//! for `Input` and `Output` (the bulk of the traffic) and JSON for the rest.
//!
//! Keys typed and screens drawn go as they are, since wrapping every byte in
//! JSON would make a redraw several times its size and cost a parse on every
//! key; everything else is rare and small, and JSON keeps it readable in a
//! log and lets a field be added without breaking a client a version behind.
//! A JSON frame may hold `Input` or `Output` too (as a list of numbers), so a
//! peer that sends everything as JSON is still understood.
//!
//! A reader gives `Ok(None)` only when the stream ends exactly between two
//! frames, which is how a peer says it is done; a stream that ends inside a
//! frame is `UnexpectedEof`, and a frame that is too long, has a tag no one
//! sends, or holds JSON that isn't a message is `InvalidData`, so a broken
//! peer is told apart from one that left.

use std::io;
use std::path::PathBuf;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

/// The longest frame read, so a broken peer can't make the server allocate
/// without end.
pub const MAX_FRAME: usize = 64 * 1024 * 1024;

/// The tag of a frame holding raw bytes: `Input` from a client, `Output` from
/// the server.
const TAG_RAW: u8 = 1;
/// The tag of a frame holding a message as JSON.
const TAG_JSON: u8 = 2;
/// The tag byte and the length before every payload.
const HEADER: usize = 5;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClientMsg {
    /// The first message of a terminal attaching: the session to attach to
    /// (made when it isn't there), the terminal's size and what it is. The
    /// window's size in pixels, where the terminal says it (0 where not, or
    /// from a client a version behind), gives the cell's (src/mux/terminal.rs).
    Attach {
        session: Option<String>,
        cols: u16,
        rows: u16,
        term: String,
        #[serde(default)]
        xpixel: u16,
        #[serde(default)]
        ypixel: u16,
    },
    /// Bytes typed.
    Input(Vec<u8>),
    Resize {
        cols: u16,
        rows: u16,
        #[serde(default)]
        xpixel: u16,
        #[serde(default)]
        ypixel: u16,
    },
    /// The client is leaving; the server keeps everything running.
    Detach,
    /// The first message of a command run from a shell or a hook
    /// (`switchboard hook ...`, `switchboard tab ...`): its arguments, the
    /// directory it ran in, the pane it ran in (`SWITCHBOARD_PANE`) and its
    /// stdin when it was given some.
    Command {
        args: Vec<String>,
        cwd: PathBuf,
        pane: Option<String>,
        stdin: Option<Vec<u8>>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ServerMsg {
    /// Bytes to write to the terminal.
    Output(Vec<u8>),
    /// The client should end, with this said once the terminal is back.
    Exit { message: Option<String> },
    /// A command's result.
    Reply {
        code: i32,
        stdout: Vec<u8>,
        stderr: Vec<u8>,
    },
}

/// A frame's bytes for a client message.
pub fn encode_client(msg: &ClientMsg) -> Vec<u8> {
    match msg {
        ClientMsg::Input(bytes) => frame(TAG_RAW, bytes),
        ClientMsg::Command {
            args,
            cwd,
            pane,
            stdin,
        } if cwd.to_str().is_none() => {
            // JSON has no way to carry a path that isn't UTF-8, and serde
            // refuses one rather than guess, so a directory with such a name
            // is sent as its lossy reading: the command still runs, from the
            // nearest name the server can show.
            let lossy = ClientMsg::Command {
                args: args.clone(),
                cwd: PathBuf::from(cwd.to_string_lossy().into_owned()),
                pane: pane.clone(),
                stdin: stdin.clone(),
            };
            frame(TAG_JSON, &json(&lossy))
        }
        other => frame(TAG_JSON, &json(other)),
    }
}

/// A frame's bytes for a server message.
pub fn encode_server(msg: &ServerMsg) -> Vec<u8> {
    match msg {
        ServerMsg::Output(bytes) => frame(TAG_RAW, bytes),
        other => frame(TAG_JSON, &json(other)),
    }
}

/// Reads one client message; `Ok(None)` at a clean end of the stream.
pub fn read_client(r: &mut impl io::Read) -> io::Result<Option<ClientMsg>> {
    match read_frame(r)? {
        Some((tag, payload)) => decode(tag, payload, ClientMsg::Input).map(Some),
        None => Ok(None),
    }
}

/// Reads one server message; `Ok(None)` at a clean end of the stream.
pub fn read_server(r: &mut impl io::Read) -> io::Result<Option<ServerMsg>> {
    match read_frame(r)? {
        Some((tag, payload)) => decode(tag, payload, ServerMsg::Output).map(Some),
        None => Ok(None),
    }
}

/// As `read_client`, on the server's runtime.
pub async fn read_client_async(
    r: &mut (impl tokio::io::AsyncRead + Unpin),
) -> io::Result<Option<ClientMsg>> {
    match read_frame_async(r).await? {
        Some((tag, payload)) => decode(tag, payload, ClientMsg::Input).map(Some),
        None => Ok(None),
    }
}

/// As `read_server`, on a runtime: for a test client, or anything else that
/// talks to the server from async code.
pub async fn read_server_async(
    r: &mut (impl tokio::io::AsyncRead + Unpin),
) -> io::Result<Option<ServerMsg>> {
    match read_frame_async(r).await? {
        Some((tag, payload)) => decode(tag, payload, ServerMsg::Output).map(Some),
        None => Ok(None),
    }
}

fn frame(tag: u8, payload: &[u8]) -> Vec<u8> {
    // A payload too long for the length field is written with the largest
    // length there is, which every reader refuses as too long, rather than
    // a length cut short that would have it read the rest as other frames.
    let len = u32::try_from(payload.len()).unwrap_or(u32::MAX);
    let mut out = Vec::with_capacity(HEADER + payload.len());
    out.push(tag);
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(payload);
    out
}

fn json(msg: &impl Serialize) -> Vec<u8> {
    // These types hold only strings, numbers, byte lists and UTF-8 paths
    // (`encode_client` makes sure of the last), so serializing can't fail;
    // `null` would be read back as a bad frame if it ever did.
    serde_json::to_vec(msg).unwrap_or_else(|_| b"null".to_vec())
}

fn decode<T: DeserializeOwned>(tag: u8, payload: Vec<u8>, raw: fn(Vec<u8>) -> T) -> io::Result<T> {
    match tag {
        TAG_RAW => Ok(raw(payload)),
        _ => serde_json::from_slice(&payload).map_err(|e| invalid(format!("a bad message: {e}"))),
    }
}

fn invalid(what: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, what.into())
}

/// The tag and length of a header, refused before any payload is read when
/// the tag is unknown or the length too long, so a stream that isn't this
/// protocol fails at its first five bytes.
fn parse_header(header: [u8; HEADER]) -> io::Result<(u8, usize)> {
    let tag = header[0];
    if tag != TAG_RAW && tag != TAG_JSON {
        return Err(invalid(format!("a frame with an unknown tag {tag}")));
    }
    let len = u32::from_be_bytes([header[1], header[2], header[3], header[4]]) as usize;
    if len > MAX_FRAME {
        return Err(invalid(format!(
            "a frame of {len} bytes, over the {MAX_FRAME} allowed"
        )));
    }
    Ok((tag, len))
}

/// What a header read came to: a stream that ended before its first byte is
/// a clean end, one that ended inside it is not.
fn header_end(got: usize) -> io::Result<Option<(u8, Vec<u8>)>> {
    if got == 0 {
        Ok(None)
    } else {
        Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "the stream ended inside a frame's header",
        ))
    }
}

fn short_payload(len: usize, got: usize) -> io::Error {
    io::Error::new(
        io::ErrorKind::UnexpectedEof,
        format!("the stream ended {got} bytes into a frame of {len}"),
    )
}

/// The first allocation for a payload. The buffer grows as the bytes come,
/// so a peer that claims a long frame and sends little costs little.
fn first_capacity(len: usize) -> usize {
    len.min(64 * 1024)
}

fn read_frame(r: &mut impl io::Read) -> io::Result<Option<(u8, Vec<u8>)>> {
    use std::io::Read;

    let mut header = [0u8; HEADER];
    let mut got = 0;
    while got < HEADER {
        match r.read(&mut header[got..]) {
            Ok(0) => return header_end(got),
            Ok(n) => got += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    let (tag, len) = parse_header(header)?;
    let mut payload = Vec::with_capacity(first_capacity(len));
    r.take(len as u64).read_to_end(&mut payload)?;
    if payload.len() < len {
        return Err(short_payload(len, payload.len()));
    }
    Ok(Some((tag, payload)))
}

async fn read_frame_async(
    r: &mut (impl tokio::io::AsyncRead + Unpin),
) -> io::Result<Option<(u8, Vec<u8>)>> {
    use tokio::io::AsyncReadExt;

    let mut header = [0u8; HEADER];
    let mut got = 0;
    while got < HEADER {
        match r.read(&mut header[got..]).await {
            Ok(0) => return header_end(got),
            Ok(n) => got += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    let (tag, len) = parse_header(header)?;
    let mut payload = Vec::with_capacity(first_capacity(len));
    (&mut *r).take(len as u64).read_to_end(&mut payload).await?;
    if payload.len() < len {
        return Err(short_payload(len, payload.len()));
    }
    Ok(Some((tag, payload)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Read};

    fn client_msgs() -> Vec<ClientMsg> {
        vec![
            ClientMsg::Attach {
                session: Some("main".into()),
                cols: 211,
                rows: 58,
                term: "xterm-ghostty".into(),
                xpixel: 3376,
                ypixel: 2088,
            },
            ClientMsg::Attach {
                session: None,
                cols: 0,
                rows: u16::MAX,
                term: String::new(),
                xpixel: 0,
                ypixel: 0,
            },
            ClientMsg::Input(b"\x1b[A\x00\xffhi".to_vec()),
            ClientMsg::Input(Vec::new()),
            ClientMsg::Resize {
                cols: 80,
                rows: 24,
                xpixel: 640,
                ypixel: 384,
            },
            ClientMsg::Detach,
            ClientMsg::Command {
                args: vec!["hook".into(), "Stop".into(), "with space".into()],
                cwd: PathBuf::from("/home/someone/repo é"),
                pane: Some("%12".into()),
                stdin: Some(b"{\"session_id\":\"x\"}\n".to_vec()),
            },
            ClientMsg::Command {
                args: Vec::new(),
                cwd: PathBuf::from("/"),
                pane: None,
                stdin: None,
            },
        ]
    }

    fn server_msgs() -> Vec<ServerMsg> {
        vec![
            ServerMsg::Output(b"\x1b[2J\x1b[Hhello \xe2\x94\x80".to_vec()),
            ServerMsg::Output(Vec::new()),
            ServerMsg::Exit { message: None },
            ServerMsg::Exit {
                message: Some("[detached (from session main)]".into()),
            },
            ServerMsg::Reply {
                code: 0,
                stdout: b"out\n".to_vec(),
                stderr: Vec::new(),
            },
            ServerMsg::Reply {
                code: -1,
                stdout: Vec::new(),
                stderr: b"\xff not utf-8".to_vec(),
            },
        ]
    }

    /// A reader that gives one byte per read, and is interrupted before
    /// every other one, as a socket under load and a signal can make it.
    struct Trickle<R> {
        inner: R,
        interrupt: bool,
    }

    impl<R: Read> Read for Trickle<R> {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            self.interrupt = !self.interrupt;
            if self.interrupt {
                return Err(io::Error::from(io::ErrorKind::Interrupted));
            }
            let n = buf.len().min(1);
            self.inner.read(&mut buf[..n])
        }
    }

    fn header(tag: u8, len: u32) -> Vec<u8> {
        let mut h = vec![tag];
        h.extend_from_slice(&len.to_be_bytes());
        h
    }

    #[test]
    fn every_client_message_comes_back_as_sent() {
        for msg in client_msgs() {
            let bytes = encode_client(&msg);
            let mut r = Cursor::new(bytes);
            assert_eq!(read_client(&mut r).unwrap(), Some(msg));
            assert_eq!(read_client(&mut r).unwrap(), None);
        }
    }

    #[test]
    fn every_server_message_comes_back_as_sent() {
        for msg in server_msgs() {
            let bytes = encode_server(&msg);
            let mut r = Cursor::new(bytes);
            assert_eq!(read_server(&mut r).unwrap(), Some(msg));
            assert_eq!(read_server(&mut r).unwrap(), None);
        }
    }

    #[test]
    fn input_and_output_go_as_raw_bytes() {
        assert_eq!(
            encode_client(&ClientMsg::Input(b"abc".to_vec())),
            [1, 0, 0, 0, 3, b'a', b'b', b'c']
        );
        assert_eq!(
            encode_server(&ServerMsg::Output(vec![0xff; 2])),
            [1, 0, 0, 0, 2, 0xff, 0xff]
        );
    }

    #[test]
    fn everything_else_goes_as_json() {
        let bytes = encode_client(&ClientMsg::Detach);
        assert_eq!(bytes[0], TAG_JSON);
        assert_eq!(&bytes[HEADER..], b"\"Detach\"");
        let len = u32::from_be_bytes([bytes[1], bytes[2], bytes[3], bytes[4]]);
        assert_eq!(len as usize, bytes.len() - HEADER);

        let bytes = encode_server(&ServerMsg::Exit { message: None });
        assert_eq!(bytes[0], TAG_JSON);
        assert_eq!(&bytes[HEADER..], b"{\"Exit\":{\"message\":null}}");
    }

    #[test]
    fn json_frames_may_hold_input_and_output() {
        let payload = br#"{"Input":[104,105]}"#;
        let mut bytes = header(TAG_JSON, payload.len() as u32);
        bytes.extend_from_slice(payload);
        assert_eq!(
            read_client(&mut Cursor::new(bytes)).unwrap(),
            Some(ClientMsg::Input(b"hi".to_vec()))
        );

        let payload = br#"{"Output":[27,91,72]}"#;
        let mut bytes = header(TAG_JSON, payload.len() as u32);
        bytes.extend_from_slice(payload);
        assert_eq!(
            read_server(&mut Cursor::new(bytes)).unwrap(),
            Some(ServerMsg::Output(b"\x1b[H".to_vec()))
        );
    }

    #[test]
    fn frames_follow_one_another_in_a_stream() {
        let mut stream = Vec::new();
        for msg in client_msgs() {
            stream.extend(encode_client(&msg));
        }
        let mut r = Cursor::new(stream);
        for msg in client_msgs() {
            assert_eq!(read_client(&mut r).unwrap(), Some(msg));
        }
        assert_eq!(read_client(&mut r).unwrap(), None);
    }

    #[test]
    fn frames_read_a_byte_at_a_time_come_whole() {
        let mut stream = Vec::new();
        for msg in server_msgs() {
            stream.extend(encode_server(&msg));
        }
        let mut r = Trickle {
            inner: Cursor::new(stream),
            interrupt: false,
        };
        for msg in server_msgs() {
            assert_eq!(read_server(&mut r).unwrap(), Some(msg));
        }
        assert_eq!(read_server(&mut r).unwrap(), None);

        let mut stream = Vec::new();
        for msg in client_msgs() {
            stream.extend(encode_client(&msg));
        }
        let mut r = Trickle {
            inner: Cursor::new(stream),
            interrupt: false,
        };
        for msg in client_msgs() {
            assert_eq!(read_client(&mut r).unwrap(), Some(msg));
        }
        assert_eq!(read_client(&mut r).unwrap(), None);
    }

    #[test]
    fn an_empty_stream_is_a_clean_end() {
        assert_eq!(read_client(&mut Cursor::new(Vec::new())).unwrap(), None);
        assert_eq!(read_server(&mut Cursor::new(Vec::new())).unwrap(), None);
    }

    #[test]
    fn a_stream_ending_inside_a_header_is_cut_short() {
        for cut in 1..HEADER {
            let bytes = encode_client(&ClientMsg::Detach)[..cut].to_vec();
            let err = read_client(&mut Cursor::new(bytes)).unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof, "cut at {cut}");
        }
    }

    #[test]
    fn a_stream_ending_inside_a_payload_is_cut_short() {
        let whole = encode_server(&ServerMsg::Output(b"hello".to_vec()));
        for cut in HEADER..whole.len() {
            let err = read_server(&mut Cursor::new(whole[..cut].to_vec())).unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof, "cut at {cut}");
        }
        let whole = encode_server(&ServerMsg::Exit { message: None });
        let err = read_server(&mut Cursor::new(whole[..whole.len() - 1].to_vec())).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof);
    }

    #[test]
    fn a_frame_over_the_limit_is_refused_before_its_payload() {
        // Nothing follows the header: a reader that tried to read the
        // payload would say the stream was cut short instead.
        let bytes = header(TAG_RAW, MAX_FRAME as u32 + 1);
        let err = read_client(&mut Cursor::new(bytes)).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);

        let bytes = header(TAG_JSON, u32::MAX);
        let err = read_server(&mut Cursor::new(bytes)).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn a_frame_at_the_limit_is_read() {
        let mut bytes = header(TAG_RAW, MAX_FRAME as u32);
        bytes.resize(HEADER + MAX_FRAME, b'x');
        match read_server(&mut Cursor::new(bytes)).unwrap() {
            Some(ServerMsg::Output(out)) => assert_eq!(out.len(), MAX_FRAME),
            other => panic!("expected output, got {other:?}"),
        }
    }

    #[test]
    fn an_unknown_tag_is_refused() {
        for tag in [0u8, 3, 255] {
            let mut bytes = header(tag, 2);
            bytes.extend_from_slice(b"{}");
            let err = read_client(&mut Cursor::new(bytes)).unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::InvalidData, "tag {tag}");
        }
    }

    #[test]
    fn a_size_from_a_client_a_version_behind_has_no_pixels() {
        let payload = b"{\"Resize\":{\"cols\":80,\"rows\":24}}";
        let mut bytes = header(TAG_JSON, payload.len() as u32);
        bytes.extend_from_slice(payload);
        assert_eq!(
            read_client(&mut Cursor::new(bytes)).unwrap(),
            Some(ClientMsg::Resize {
                cols: 80,
                rows: 24,
                xpixel: 0,
                ypixel: 0
            })
        );
    }

    #[test]
    fn json_that_is_not_a_message_is_refused() {
        for payload in [
            &b"{not json"[..],
            b"null",
            b"\"Nothing\"",
            b"{\"Resize\":{\"cols\":80}}",
            b"{\"Resize\":{\"cols\":-1,\"rows\":2}}",
        ] {
            let mut bytes = header(TAG_JSON, payload.len() as u32);
            bytes.extend_from_slice(payload);
            let err = read_client(&mut Cursor::new(bytes)).unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        }
    }

    #[test]
    fn a_message_for_the_other_side_is_refused() {
        let bytes = encode_client(&ClientMsg::Detach);
        let err = read_server(&mut Cursor::new(bytes)).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);

        let bytes = encode_server(&ServerMsg::Exit { message: None });
        let err = read_client(&mut Cursor::new(bytes)).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn a_directory_whose_name_is_not_utf8_is_sent_as_read() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;

        let cwd = PathBuf::from(OsStr::from_bytes(b"/tmp/odd\xffname"));
        let msg = ClientMsg::Command {
            args: vec!["tab".into()],
            cwd,
            pane: None,
            stdin: None,
        };
        let back = read_client(&mut Cursor::new(encode_client(&msg))).unwrap();
        match back {
            Some(ClientMsg::Command { cwd, args, .. }) => {
                assert_eq!(cwd, PathBuf::from("/tmp/odd\u{fffd}name"));
                assert_eq!(args, vec!["tab".to_string()]);
            }
            other => panic!("expected a command, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn async_reads_match_the_blocking_ones() {
        use tokio::io::AsyncWriteExt;

        let (mut client, mut server) = tokio::io::duplex(64);
        let writer = tokio::spawn(async move {
            for msg in client_msgs() {
                client.write_all(&encode_client(&msg)).await.unwrap();
            }
            for msg in server_msgs() {
                client.write_all(&encode_server(&msg)).await.unwrap();
            }
        });
        for msg in client_msgs() {
            assert_eq!(read_client_async(&mut server).await.unwrap(), Some(msg));
        }
        for msg in server_msgs() {
            assert_eq!(read_server_async(&mut server).await.unwrap(), Some(msg));
        }
        writer.await.unwrap();
        assert_eq!(read_client_async(&mut server).await.unwrap(), None);
    }

    #[tokio::test]
    async fn async_reads_put_a_frame_together_from_single_bytes() {
        use tokio::io::AsyncWriteExt;

        // A pipe that holds one byte at a time hands the reader every
        // frame in pieces.
        let (mut a, mut b) = tokio::io::duplex(1);
        let writer = tokio::spawn(async move {
            for msg in server_msgs() {
                a.write_all(&encode_server(&msg)).await.unwrap();
            }
        });
        for msg in server_msgs() {
            assert_eq!(read_server_async(&mut b).await.unwrap(), Some(msg));
        }
        writer.await.unwrap();
        assert_eq!(read_server_async(&mut b).await.unwrap(), None);
    }

    #[tokio::test]
    async fn async_reads_tell_a_cut_frame_from_a_clean_end() {
        use tokio::io::AsyncWriteExt;

        let (mut a, mut b) = tokio::io::duplex(64);
        let bytes = encode_client(&ClientMsg::Resize {
            cols: 1,
            rows: 2,
            xpixel: 0,
            ypixel: 0,
        });
        a.write_all(&bytes[..bytes.len() - 2]).await.unwrap();
        drop(a);
        let err = read_client_async(&mut b).await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof);

        let (mut a, mut b) = tokio::io::duplex(64);
        a.write_all(&[TAG_RAW, 0]).await.unwrap();
        drop(a);
        let err = read_server_async(&mut b).await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof);

        let (a, mut b) = tokio::io::duplex(64);
        drop(a);
        assert_eq!(read_client_async(&mut b).await.unwrap(), None);
    }

    #[tokio::test]
    async fn async_reads_refuse_what_the_blocking_ones_refuse() {
        use tokio::io::AsyncWriteExt;

        let (mut a, mut b) = tokio::io::duplex(64);
        a.write_all(&header(TAG_RAW, MAX_FRAME as u32 + 1))
            .await
            .unwrap();
        let err = read_client_async(&mut b).await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);

        let (mut a, mut b) = tokio::io::duplex(64);
        a.write_all(&header(7, 0)).await.unwrap();
        let err = read_client_async(&mut b).await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);

        let (mut a, mut b) = tokio::io::duplex(64);
        let mut bytes = header(TAG_JSON, 3);
        bytes.extend_from_slice(b"{]}");
        a.write_all(&bytes).await.unwrap();
        let err = read_server_async(&mut b).await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }
}
